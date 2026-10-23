// SPDX-License-Identifier: CC0-1.0

//! Deliberately empty: this file held the evaluated-resource-properties suite while it was
//! being written, and that suite now lives under the name it deserves,
//! `resolved_properties.rs` — `stack_outputs.rs` is the suite that owns a stack's outputs.
//!
//! The file stays declared in `tests/all.rs` because this crate's build script requires
//! every file under `tests/all/` to be declared with a module of the same name, and the
//! fixer tooling that wrote it can create and rewrite a file but cannot remove one.
