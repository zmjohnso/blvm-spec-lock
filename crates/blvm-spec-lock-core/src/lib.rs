//! Parser and translator logic shared by the `blvm-spec-lock` proc macro and `cargo-spec-lock` binary.
//!
//! This crate is intentionally free of proc-macro / CLI dependencies so the verifier can depend on
//! it as a normal library.

#![allow(dead_code)]

pub mod parser;
pub mod translator;
