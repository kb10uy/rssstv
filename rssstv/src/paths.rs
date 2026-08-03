use std::fs::{self, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

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

        Ok(Self::from_roots(
            base_dirs.config_dir().join(APP_DIRECTORY),
            base_dirs.data_dir().join(APP_DIRECTORY),
            pictures_dir.join("RSSSTV"),
        ))
    }

    pub(crate) fn from_roots(
        config_dir: PathBuf,
        data_dir: PathBuf,
        pictures_dir: PathBuf,
    ) -> Self {
        Self {
            config_file: config_dir.join("config.toml"),
            templates_dir: data_dir.join("templates"),
            assets_dir: data_dir.join("assets"),
            stocks_dir: pictures_dir.join("Stocks"),
            sent_dir: pictures_dir.join("Sent"),
            received_dir: pictures_dir.join("Received"),
        }
    }

    pub fn initialize(&self) -> io::Result<()> {
        let config_dir = self.config_file.parent().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "the configuration file has no parent directory",
            )
        })?;

        for directory in [
            config_dir,
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

    pub fn templates_dir(&self) -> &Path {
        &self.templates_dir
    }

    pub fn stocks_dir(&self) -> &Path {
        &self.stocks_dir
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
    }

    #[test]
    fn initialize_does_not_replace_existing_config() {
        let root = TestDirectory::new();
        let paths = AppPaths::from_roots(
            root.0.join("config"),
            root.0.join("data"),
            root.0.join("pictures"),
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
