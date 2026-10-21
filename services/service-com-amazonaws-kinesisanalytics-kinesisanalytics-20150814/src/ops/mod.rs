// SPDX-License-Identifier: CC0-1.0

//! Operation handlers for com-amazonaws-kinesisanalytics-kinesisanalytics-20150814. One file per operation.
//!
//! This model names an OPERATION `DeleteApplicationOutput` and also gives the
//! `DeleteApplication` operation an output shape called `DeleteApplicationOutput`, so the
//! generated module's `pub use types::*` and `pub use delete_application_output::*` both
//! re-export that name. Both re-exports are wanted (the shape and the operation struct
//! are reached through this module), and the collision comes from the model, not from
//! this crate, so the ambiguity is allowed here rather than resolved by dropping one.
#![allow(ambiguous_glob_reexports)]
include!(concat!(env!("OUT_DIR"), "/com_amazonaws_kinesisanalytics_kinesisanalytics_20150814_ops_generated.rs"));
