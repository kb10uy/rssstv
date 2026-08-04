//! The interface: what the operator sees and what they can reach.
//!
//! Everything here draws or is drawn. The menu is the one part that is not
//! egui alone, because the platform draws it where the platform has one.

pub mod canvas;
pub mod menu;
pub mod raster;
pub mod view;
