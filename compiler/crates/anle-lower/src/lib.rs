//! AST → IR

pub mod build;
pub mod consts;
pub mod quota;

pub use build::{build, BuildError};
