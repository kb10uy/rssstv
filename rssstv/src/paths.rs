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
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    static NEXT_TEMP_DIRECTORY: AtomicUsize = AtomicUsize::new(0);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let index = NEXT_TEMP_DIRECTORY.fetch_add(1, Ordering::Relaxed);
            let path =
                std::env::temp_dir().join(format!("rssstv-paths-{}-{index}", std::process::id()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn initialize_creates_application_directories_and_default_config() {
        let root = TestDirectory::new();
        let paths = AppPaths::from_roots(
            root.0.join("config"),
            root.0.join("data"),
            root.0.join("pictures"),
            root.0.join("state"),
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
        let root = TestDirectory::new();
        let paths = AppPaths::from_roots(
            root.0.join("config"),
            root.0.join("data"),
            root.0.join("pictures"),
            root.0.join("state"),
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
