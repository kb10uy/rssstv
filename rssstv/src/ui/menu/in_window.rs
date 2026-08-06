use super::{Action, Menu};

/// A placeholder for the platforms muda cannot serve.
///
/// The menu is drawn by [`super::bar`] instead; this type exists so the
/// application does not need to know which path is in use.
pub struct MenuHost;

impl MenuHost {
    pub fn install(
        _cc: &eframe::CreationContext<'_>,
        _model: &[Menu],
    ) -> Result<Self, std::convert::Infallible> {
        Ok(Self)
    }

    pub fn sync(&mut self, _model: &[Menu]) {}

    pub fn poll(&self) -> Vec<Action> {
        Vec::new()
    }

    pub fn prepare_for_close(&self) {}
}
