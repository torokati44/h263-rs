//! Pure-rust H.263 decoder

#![feature(portable_simd)]

#[macro_use]
extern crate bitflags;

#[macro_use]
extern crate lazy_static;

mod decoder;
mod error;
pub mod parser;
mod traits;
mod types;

pub use decoder::{DecoderOption, H263State};
pub use error::{Error, Result};
pub use types::PictureTypeCode;
