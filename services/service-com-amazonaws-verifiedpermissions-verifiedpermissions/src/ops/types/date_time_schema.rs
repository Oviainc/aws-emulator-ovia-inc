// SPDX-License-Identifier: CC0-1.0

//! SUPERSEDED — see [`super::date_time_wire`], which is the module `types/mod.rs`
//! pulls in with `#[macro_use]`.
//!
//! This file held an earlier, equivalent copy of those macros (the
//! `@timestampFormat("date-time")` member-schema workaround for the vendored
//! codegen). Both landed in the same release window; `date_time_wire` won, every
//! `*_types` module uses its `vp_schema_and_codec!`, and nothing declares this
//! module, so no line of it is compiled. It is left here only because the fixer
//! toolchain has no file-removal operation — delete the file outright in any
//! later change that touches this directory.
