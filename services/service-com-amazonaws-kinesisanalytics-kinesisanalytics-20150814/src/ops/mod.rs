// SPDX-License-Identifier: CC0-1.0

//! Operation handlers for com-amazonaws-kinesisanalytics-kinesisanalytics-20150814. One file per operation.
//!
//! Every name this module publishes binds to exactly ONE item, so `crate::ops::<Name>`
//! never depends on the order in which the generated `pub use types::*;` /
//! `pub use <snake_op>::*;` globs are emitted. This model is one that could collide: it
//! names an OPERATION `DeleteApplicationOutput`, which is also the `<Op>Output` form of
//! the `DeleteApplication` operation's output struct. It does not, because that output
//! struct carries the model's own output shape name, `DeleteApplicationResponse`, leaving
//! `DeleteApplicationOutput` to the operation alone.
//!
//! So this module needs no lint suppression, and must not carry one: the crate-level
//! `#![allow(ambiguous_glob_reexports)]` it used to have made `crate::ops::<Name>` mean
//! whichever glob the generator happened to emit first. Should a model ever make two
//! globs publish one name, settle it HERE with an explicit, non-glob
//! `pub use types::<Shape>;` — an explicit re-export shadows both globs, and
//! `crate::ops::<ShapeName>` must name the modelled shape because the generated service
//! declaration is expanded with `types = "crate::ops"` — never by allowing the lint.
//! An operation's handler struct is reached at `crate::ops::<snake_op>::<Op>`, which is
//! how the generated `impl Operation` and `EmulatedService::new` name it.
include!(concat!(env!("OUT_DIR"), "/com_amazonaws_kinesisanalytics_kinesisanalytics_20150814_ops_generated.rs"));
