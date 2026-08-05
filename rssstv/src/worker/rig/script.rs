use std::{
    cell::Cell,
    collections::BTreeMap,
    fs, io,
    path::Path,
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

use mlua::{Function, HookTriggers, Lua, Table, UserData, UserDataMethods, Value, VmState};
use rssstv_rig::{RigError, Rigctld};
use thiserror::Error;

use crate::storage::{
    bands::{BandDefinition, BandPlan, BandValue},
    config::PortSettings,
};

/// The file the operator's own script is read from, beside `config.toml`.
pub const SCRIPT_FILE: &str = "rigcontrol.lua";

/// What runs when no script has been written.
///
/// Compiled in rather than written out on the first run: a file written once
/// is a file that never gains a later fix, and most operators never need to
/// edit it. The menu writes it out for the ones who do.
pub const DEFAULT_SCRIPT: &str = include_str!("../../../assets/rigcontrol.lua");

/// How long one call may run before it is abandoned.
///
/// This bounds computation. Time spent waiting on a socket does not advance
/// the instruction count, so what bounds a script that only talks to the rig
/// is the transport's own timeout, once per command.
const CALL_DEADLINE: Duration = Duration::from_secs(5);

/// How often the deadline is checked, in Lua instructions.
const HOOK_INTERVAL: u32 = 10_000;

/// A function the application calls on the operator's script.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Entry {
    Open,
    Close,
    Transmit,
    Receive,
    PollFrequency,
    SetFrequency,
    ChangeBand,
}

impl Entry {
    /// The name the script exports this under.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Close => "close",
            Self::Transmit => "transmit",
            Self::Receive => "receive",
            Self::PollFrequency => "poll_frequency",
            Self::SetFrequency => "set_frequency",
            Self::ChangeBand => "change_band",
        }
    }
}

#[derive(Clone, Debug, Error)]
pub enum ScriptError {
    #[error("{path}: {detail}")]
    Read { path: String, detail: String },
    #[error("rigcontrol.lua: {0}")]
    Load(String),
    #[error("rigcontrol.lua has to return a table of functions")]
    NotAModule,
    #[error("rig port `{name}`: {error}")]
    Port { name: String, error: RigError },
    #[error("rigcontrol.lua {entry}: {detail}")]
    Call { entry: &'static str, detail: String },
}

/// The operator's script, and the transports it sends over.
///
/// Lives on the rig worker's thread and never leaves it: a Lua state is not
/// shared, and neither is a socket that answers one command at a time.
pub struct ScriptHost {
    lua: Lua,
    module: Table,
    ports: Table,
    /// The bands, so that a call knows which one the rig is on without the
    /// worker having to say.
    bands: Arc<BandPlan>,
    /// When the call currently running has to be abandoned by.
    deadline: Rc<Cell<Option<Instant>>>,
    /// Set once a transport fails in a way the next command would not survive.
    ///
    /// The script sees every failure as a Lua error and may swallow one with
    /// `pcall`, so whether the connection is still worth holding is recorded
    /// where the transport itself noticed rather than read back out of an
    /// error the script has already handled.
    broken: Rc<Cell<bool>>,
}

impl ScriptHost {
    /// Reads the script beside `config.toml`, falling back to the default.
    pub fn source(config_dir: &Path) -> Result<String, ScriptError> {
        let path = config_dir.join(SCRIPT_FILE);
        match fs::read_to_string(&path) {
            Ok(source) => Ok(source),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(DEFAULT_SCRIPT.to_owned()),
            Err(error) => Err(ScriptError::Read {
                path: path.display().to_string(),
                detail: error.to_string(),
            }),
        }
    }

    /// Opens the transports and loads `source` against them.
    pub fn load(
        source: &str,
        ports: &BTreeMap<String, PortSettings>,
        bands: Arc<BandPlan>,
        timeout: Duration,
    ) -> Result<Self, ScriptError> {
        let lua = Lua::new();
        let deadline = Rc::new(Cell::new(None));
        let broken = Rc::new(Cell::new(false));
        install_deadline(&lua, Rc::clone(&deadline));

        let table = lua.create_table().map_err(load_error)?;
        for (name, settings) in ports {
            let rig = Rigctld::connect(&settings.address, timeout).map_err(|error| {
                ScriptError::Port {
                    name: name.clone(),
                    error,
                }
            })?;
            let port = RigctldPort {
                rig,
                broken: Rc::clone(&broken),
            };
            table.set(name.as_str(), port).map_err(load_error)?;
        }

        let module: Table = lua
            .load(source)
            .set_name(SCRIPT_FILE)
            .eval()
            .map_err(|error| match error {
                mlua::Error::FromLuaConversionError { .. } => ScriptError::NotAModule,
                other => ScriptError::Load(other.to_string()),
            })?;

        Ok(Self {
            lua,
            module,
            ports: table,
            bands,
            deadline,
            broken,
        })
    }

    /// Whether a transport failed in a way the connection cannot continue past.
    pub fn transport_failed(&self) -> bool {
        self.broken.get()
    }

    /// Calls `entry`, doing nothing when the script does not export it.
    ///
    /// A module that omits one is not in error: a station keyed by VOX exports
    /// no `transmit`, and saying so by leaving it out is the whole of what it
    /// has to say.
    pub fn call(&self, entry: Entry, frequency_hz: Option<u64>) -> Result<(), ScriptError> {
        self.invoke(entry, frequency_hz, None)
    }

    /// Asks the script to tune the rig.
    pub fn set_frequency(
        &self,
        frequency_hz: Option<u64>,
        target_hz: u64,
    ) -> Result<(), ScriptError> {
        let target = Value::Integer(i64::try_from(target_hz).unwrap_or(i64::MAX));
        self.invoke(Entry::SetFrequency, frequency_hz, Some(target))
    }

    /// Asks the script to move the rig to `band`, which it is handed whole.
    pub fn change_band(
        &self,
        frequency_hz: Option<u64>,
        band: &BandDefinition,
    ) -> Result<(), ScriptError> {
        let entry = Entry::ChangeBand;
        let table = self
            .band_table(band)
            .map_err(|error| call_error(entry, &error))?;
        self.invoke(entry, frequency_hz, Some(Value::Table(table)))
    }

    /// Calls `poll_frequency`, returning what the rig is tuned to.
    ///
    /// A script that reports nothing is one that could not read the tuning,
    /// which is a state rather than a failure: keying does not depend on it.
    pub fn poll_frequency(
        &self,
        frequency_hz: Option<u64>,
    ) -> Result<Option<(u64, String)>, ScriptError> {
        let entry = Entry::PollFrequency;
        let Some(function) = self.function(entry) else {
            return Ok(None);
        };
        let context = self
            .context(frequency_hz)
            .map_err(|error| call_error(entry, &error))?;
        let (frequency_hz, mode) = self
            .guarded(|| function.call::<(Option<u64>, Option<String>)>(context))
            .map_err(|error| call_error(entry, &error))?;
        match (frequency_hz, mode) {
            (None, None) => Ok(None),
            (Some(frequency_hz), Some(mode)) if !mode.trim().is_empty() => {
                Ok(Some((frequency_hz, mode)))
            }
            _ => Err(ScriptError::Call {
                entry: entry.name(),
                detail: "must return both frequency and mode, or neither".to_owned(),
            }),
        }
    }

    fn invoke(
        &self,
        entry: Entry,
        frequency_hz: Option<u64>,
        argument: Option<Value>,
    ) -> Result<(), ScriptError> {
        let Some(function) = self.function(entry) else {
            return Ok(());
        };
        let context = self
            .context(frequency_hz)
            .map_err(|error| call_error(entry, &error))?;
        self.guarded(|| match argument {
            Some(argument) => function.call::<()>((context, argument)),
            None => function.call::<()>(context),
        })
        .map_err(|error| call_error(entry, &error))
    }

    fn function(&self, entry: Entry) -> Option<Function> {
        self.module.get::<Option<Function>>(entry.name()).ok()?
    }

    /// Builds the table a call is handed, fresh each time.
    fn context(&self, frequency_hz: Option<u64>) -> mlua::Result<Table> {
        let context = self.lua.create_table()?;
        context.set("ports", self.ports.clone())?;
        if let Some(frequency_hz) = frequency_hz {
            context.set("frequency", frequency_hz)?;
            if let Some(band) = self.bands.for_frequency(frequency_hz) {
                context.set("band", self.band_table(band)?)?;
            }
        }
        context.set(
            "log",
            self.lua.create_function(|_, message: String| {
                crate::storage::log::note(&format!("rig script: {message}"));
                Ok(())
            })?,
        )?;
        Ok(context)
    }

    /// Builds the table a band is handed as.
    ///
    /// Every setting the operator wrote comes through, because what to do on a
    /// band is the script's. Hyphens become underscores on the way, since a
    /// hyphen cannot appear in a Lua identifier and `band["receive-mode"]`
    /// would be a worse thing to have to write.
    fn band_table(&self, band: &BandDefinition) -> mlua::Result<Table> {
        let table = self.lua.create_table()?;
        table.set("name", band.name.as_str())?;
        table.set("low", band.low_hz)?;
        table.set("high", band.high_hz)?;
        for (key, value) in &band.settings {
            let key = key.replace('-', "_");
            match value {
                BandValue::Text(text) => table.set(key, text.as_str())?,
                BandValue::Integer(number) => table.set(key, *number)?,
                BandValue::Decimal(number) => table.set(key, *number)?,
                BandValue::Flag(flag) => table.set(key, *flag)?,
            }
        }
        Ok(table)
    }

    /// Runs `call` under a deadline, so a script that does not finish is
    /// abandoned rather than holding the worker.
    fn guarded<T>(&self, call: impl FnOnce() -> mlua::Result<T>) -> mlua::Result<T> {
        self.deadline.set(Some(Instant::now() + CALL_DEADLINE));
        let outcome = call();
        self.deadline.set(None);
        outcome
    }
}

impl core::fmt::Debug for ScriptHost {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("ScriptHost")
            .field("transport_failed", &self.transport_failed())
            .finish_non_exhaustive()
    }
}

/// Arranges for a call that outstays its deadline to be abandoned.
///
/// A hook that cannot be installed is reported by the first call that runs
/// long rather than here, because the only thing lost is the bound: a script
/// that behaves is unaffected either way, and refusing to connect over it
/// would take rig control away from a station whose script is fine.
fn install_deadline(lua: &Lua, deadline: Rc<Cell<Option<Instant>>>) {
    let installed = lua.set_hook(
        HookTriggers::new().every_nth_instruction(HOOK_INTERVAL),
        move |_, _| match deadline.get() {
            Some(deadline) if Instant::now() > deadline => {
                Err(mlua::Error::runtime("the script ran for too long"))
            }
            _ => Ok(VmState::Continue),
        },
    );
    if let Err(error) = installed {
        crate::storage::log::note(&format!("rig script is not bounded by a deadline: {error}"));
    }
}

fn load_error(error: mlua::Error) -> ScriptError {
    ScriptError::Load(error.to_string())
}

fn call_error(entry: Entry, error: &mlua::Error) -> ScriptError {
    ScriptError::Call {
        entry: entry.name(),
        detail: error.to_string(),
    }
}

/// A `rigctld` connection as the script reaches it.
struct RigctldPort {
    rig: Rigctld,
    broken: Rc<Cell<bool>>,
}

impl RigctldPort {
    /// Turns a transport failure into a Lua error, remembering the fatal ones.
    fn fail(&self, error: RigError) -> mlua::Error {
        if error.is_fatal() {
            self.broken.set(true);
        }
        mlua::Error::external(error)
    }
}

impl UserData for RigctldPort {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method_mut("send", |lua, this, command: String| {
            match this.rig.run(&command) {
                Ok(response) => lua.create_sequence_from(response.lines().to_vec()),
                Err(error) => Err(this.fail(error)),
            }
        });
        methods.add_method_mut("frequency", |_, this, ()| match this.rig.frequency_hz() {
            Ok(frequency_hz) => Ok(frequency_hz),
            Err(error) => Err(this.fail(error)),
        });
        methods.add_method_mut("mode", |_, this, ()| match this.rig.mode() {
            Ok(mode) => Ok(mode),
            Err(error) => Err(this.fail(error)),
        });
    }
}
