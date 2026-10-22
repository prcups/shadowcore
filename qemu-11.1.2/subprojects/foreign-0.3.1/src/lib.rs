#![doc = include_str!("../README.md")]

mod foreign;
mod r#impl;
pub mod prelude;

pub use foreign::*;

#[cfg(test)]
#[macro_use]
mod c_str;
