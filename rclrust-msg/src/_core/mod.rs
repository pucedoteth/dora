#![warn(clippy::all, clippy::nursery)]
#![allow(clippy::missing_safety_doc)]

pub use widestring;

pub mod sequence;
pub mod string;
pub mod traits;

pub use sequence::{FFISeq, OwnedFFISeq, RefFFISeq};
pub use string::{FFIString, FFIWString, OwnedFFIString, OwnedFFIWString};
pub use traits::{ActionT, FFIFromRust, FFIToRust, InternalDefault, MessageT, ServiceT};
