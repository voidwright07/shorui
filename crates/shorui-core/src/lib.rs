//! PDF operations behind Shorui.
//!
//! Everything here runs on the local machine. Nothing in this crate opens a network
//! connection; the few tools that need a helper program (Office conversion, HTML to PDF,
//! OCR on Linux) start a local process and say so in their error when it is missing.

pub mod ctx;
pub mod doc;
pub mod error;
pub mod fixtures;
pub mod helpers;
pub mod img;
pub mod range;
pub mod render;
pub mod text;
pub mod tools;

pub use ctx::{Ctx, Outcome};
pub use error::{Error, Result};
