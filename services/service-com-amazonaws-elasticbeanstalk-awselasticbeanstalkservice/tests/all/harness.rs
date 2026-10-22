// SPDX-License-Identifier: CC0-1.0

//! The name this crate's suites import the shared harness by.
//!
//! The harness itself — the router, the awsQuery request builder and the XML
//! readers — lives in the `lifecycle` suite, which marks it `pub(crate)` for
//! exactly this purpose. Re-exporting it here gives a suite one stable import
//! path for it, so no suite keeps a second copy of the encoder and the reader
//! that could then drift from the one the rest of the target uses.
//!
//! Declared in `tests/all.rs` like any other file under `tests/all/`, as the
//! layout rule requires; it holds no tests of its own.

pub(crate) use crate::lifecycle::{Answer, Emulator, PAST_DUE};
