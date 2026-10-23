// SPDX-License-Identifier: CC0-1.0

//! Deliberately empty: this file held the "what a stack claims about a resource nothing
//! created" suite while it was being written, and that suite now lives under the name it
//! deserves, `unprovisioned_resources.rs`.
//!
//! The file stays declared in `tests/all.rs` because this crate's build script requires
//! every file under `tests/all/` to be declared with a module of the same name, and the
//! fixer tooling that wrote it can create and rewrite a file but cannot remove one.
