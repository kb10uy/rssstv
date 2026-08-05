use std::{
    fs::{self, OpenOptions},
    io,
    path::{Path, PathBuf},
};

use directories::{BaseDirs, UserDirs};

const APP_DIRECTORY: &str = if cfg!(target_os = "linux") {
    "rssstv"
} else {
    "RSSSTV"
};
const DEFAULT_CONFIG: &str = "";

/// One of the directories the application keeps for the operator.
///
/// Every one of them is somewhere the operator is expected to work with a
/// file manager, so the interface only has to name them and point at them.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Folder {
    Received,
    Sent,
    Stocks,
    Templates,
    Assets,
    Config,
}

impl Folder {
    pub const ALL: [Self; 6] = [
        Self::Received,
        Self::Sent,
        Self::Stocks,
        Self::Templates,
        Self::Assets,
        Self::Config,
    ];

    pub const fn label_key(self) -> &'static str {
        match self {
            Self::Received => "menu-open-received",
            Self::Sent => "menu-open-sent",
            Self::Stocks => "menu-open-stocks",
            Self::Templates => "menu-open-templates",
            Self::Assets => "menu-open-assets",
            Self::Config => "menu-open-config",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppPaths {
    config_file: PathBuf,
    templates_dir: PathBuf,
    assets_dir: PathBuf,
    stocks_dir: PathBuf,
    sent_dir: PathBuf,
    received_dir: PathBuf,
    log_file: PathBuf,
}

impl AppPaths {
    pub fn discover() -> io::Result<Self> {
        let base_dirs = BaseDirs::new().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "could not determine the user directories",
            )
        })?;
        let pictures_dir = UserDirs::new()
            .and_then(|directories| directories.picture_dir().map(Path::to_path_buf))
            .unwrap_or_else(|| base_dirs.home_dir().join("Pictures"));

        // The log belongs to the machine it was written on, not to the
        // account: on Windows `data_dir` is the roaming profile, which would
        // synchronize a log describing hardware the other machine does not
        // have. Linux keeps a directory for exactly this; elsewhere the local
        // half of the data directory is the closest equivalent.
        let state_dir = base_dirs
            .state_dir()
            .unwrap_or_else(|| base_dirs.data_local_dir());

        Ok(Self::from_roots(
            base_dirs.config_dir().join(APP_DIRECTORY),
            base_dirs.data_dir().join(APP_DIRECTORY),
            pictures_dir.join("RSSSTV"),
            state_dir.join(APP_DIRECTORY),
        ))
    }

    pub(crate) fn from_roots(
        config_dir: PathBuf,
        data_dir: PathBuf,
        pictures_dir: PathBuf,
        state_dir: PathBuf,
    ) -> Self {
        Self {
            config_file: config_dir.join("config.toml"),
            templates_dir: data_dir.join("templates"),
            assets_dir: data_dir.join("assets"),
            stocks_dir: pictures_dir.join("Stocks"),
            sent_dir: pictures_dir.join("Sent"),
            received_dir: pictures_dir.join("Received"),
            log_file: state_dir.join("logs").join("rssstv.log"),
        }
    }

    pub fn initialize(&self) -> io::Result<()> {
        let config_dir = self.config_file.parent().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "the configuration file has no parent directory",
            )
        })?;

        let log_dir = self.log_file.parent().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "the log file has no parent directory",
            )
        })?;

        for directory in [
            config_dir,
            log_dir,
            &self.templates_dir,
            &self.assets_dir,
            &self.stocks_dir,
            &self.sent_dir,
            &self.received_dir,
        ] {
            fs::create_dir_all(directory)?;
        }

        create_default_config(&self.config_file)
    }

    pub fn config_file(&self) -> &Path {
        &self.config_file
    }

    /// Returns the directory the configuration file lives in.
    pub fn config_dir(&self) -> &Path {
        self.config_file.parent().unwrap_or(&self.config_file)
    }

    pub fn templates_dir(&self) -> &Path {
        &self.templates_dir
    }

    pub fn assets_dir(&self) -> &Path {
        &self.assets_dir
    }

    pub fn stocks_dir(&self) -> &Path {
        &self.stocks_dir
    }

    pub fn received_dir(&self) -> &Path {
        &self.received_dir
    }

    /// Returns the directory `folder` names.
    ///
    /// The configuration answers with the directory holding the file rather
    /// than the file itself: the application rewrites it as settings change,
    /// and a `.toml` has no dependable handler on every platform.
    pub fn folder(&self, folder: Folder) -> &Path {
        match folder {
            Folder::Received => &self.received_dir,
            Folder::Sent => &self.sent_dir,
            Folder::Stocks => &self.stocks_dir,
            Folder::Templates => &self.templates_dir,
            Folder::Assets => &self.assets_dir,
            Folder::Config => self.config_dir(),
        }
    }

    pub fn log_file(&self) -> &Path {
        &self.log_file
    }
}

fn create_default_config(path: &Path) -> io::Result<()> {
    match OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(mut file) => io::Write::write_all(&mut file, DEFAULT_CONFIG.as_bytes()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            if path.is_file() {
                Ok(())
            } else {
                Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!("configuration path is not a file: {}", path.display()),
                ))
            }
        }
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::TempDir;

    #[test]
    fn initialize_creates_application_directories_and_default_config() {
        let root = TempDir::new();
        let paths = AppPaths::from_roots(
            root.path().join("config"),
            root.path().join("data"),
            root.path().join("pictures"),
            root.path().join("state"),
        );

        paths.initialize().unwrap();

        assert_eq!(
            fs::read_to_string(&paths.config_file).unwrap(),
            DEFAULT_CONFIG
        );
        for directory in [
            &paths.templates_dir,
            &paths.assets_dir,
            &paths.stocks_dir,
            &paths.sent_dir,
            &paths.received_dir,
        ] {
            assert!(directory.is_dir());
        }
        assert!(
            paths.log_file.parent().unwrap().is_dir(),
            "the log has to have somewhere to be written before anything reports"
        );
    }

    /// The log describes one machine's hardware, so it must not be written
    /// where the account's roaming profile would synchronize it.
    #[test]
    fn the_log_is_kept_apart_from_the_roaming_data_directory() {
        let paths = AppPaths::from_roots(
            PathBuf::from("config"),
            PathBuf::from("data"),
            PathBuf::from("pictures"),
            PathBuf::from("state"),
        );

        assert!(paths.log_file.starts_with("state"));
        assert!(!paths.log_file.starts_with("data"));
    }

    #[test]
    fn initialize_does_not_replace_existing_config() {
        let root = TempDir::new();
        let paths = AppPaths::from_roots(
            root.path().join("config"),
            root.path().join("data"),
            root.path().join("pictures"),
            root.path().join("state"),
        );
        fs::create_dir_all(paths.config_file.parent().unwrap()).unwrap();
        fs::write(&paths.config_file, "callsign = \"JA1ABC\"\n").unwrap();

        paths.initialize().unwrap();

        assert_eq!(
            fs::read_to_string(&paths.config_file).unwrap(),
            "callsign = \"JA1ABC\"\n"
        );
    }
}
