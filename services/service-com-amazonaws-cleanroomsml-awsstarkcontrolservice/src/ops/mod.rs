// SPDX-License-Identifier: CC0-1.0

//! Operation handlers for com-amazonaws-cleanroomsml-awsstarkcontrolservice. One file per operation.
//!
//! This service models both `DeleteTrainedModel` and `DeleteTrainedModelOutput`,
//! so the operation struct the generated `pub use delete_trained_model_output::*;`
//! re-exports collides with the `DeleteTrainedModelOutput` type `pub use types::*;`
//! re-exports for `DeleteTrainedModel`. The collision is in generated code and
//! harmless (every use inside this crate is module-qualified), but the merge gate
//! builds with `-D warnings`, which makes `ambiguous_glob_reexports` an error, so
//! it is allowed at the one module the generated re-exports live in.
#![allow(ambiguous_glob_reexports)]
include!(concat!(env!("OUT_DIR"), "/com_amazonaws_cleanroomsml_awsstarkcontrolservice_ops_generated.rs"));
