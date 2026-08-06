//! Reading the template and stock directories into the lists the operator
//! picks from.
//!
//! Only the reading lives here; which entry is selected, and when the lists
//! are read again, stays with the interface.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

/// One file a library list offers.
#[derive(Clone, Debug)]
pub struct Entry {
    pub name: String,
    /// The image dimensions a stock entry shows beside its name.
    pub geometry: String,
    pub(crate) path: PathBuf,
}

impl Entry {
    /// Builds an entry that names no real file, for tests.
    #[cfg(test)]
    pub(crate) fn sample(name: &str, geometry: &str) -> Self {
        Self {
            name: name.to_owned(),
            geometry: geometry.to_owned(),
            path: PathBuf::from(name),
        }
    }

    fn new(path: PathBuf, geometry: String) -> Self {
        let name = path
            .file_name()
            .unwrap_or(path.as_os_str())
            .to_string_lossy()
            .into_owned();
        Self {
            name,
            geometry,
            path,
        }
    }
}

/// What a library scan brought back, adopted on the frame it arrives.
pub(crate) struct LibraryScan {
    pub(crate) templates: io::Result<Vec<Entry>>,
    pub(crate) stocks: io::Result<Vec<Entry>>,
}

pub(crate) fn template_entries(directory: &Path) -> io::Result<Vec<Entry>> {
    directory_entries(directory, |path| {
        path.extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("kdl"))
            .then(|| Entry::new(path.to_owned(), String::new()))
    })
}

pub(crate) fn stock_entries(directory: &Path) -> io::Result<Vec<Entry>> {
    directory_entries(directory, |path| {
        image::image_dimensions(path)
            .ok()
            .map(|(width, height)| Entry::new(path.to_owned(), format!("{width}×{height}")))
    })
}

fn directory_entries(
    directory: &Path,
    load: impl Fn(&Path) -> Option<Entry>,
) -> io::Result<Vec<Entry>> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if path.is_file()
            && let Some(entry) = load(&path)
        {
            entries.push(entry);
        }
    }
    entries.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.name.cmp(&right.name))
    });
    Ok(entries)
}
