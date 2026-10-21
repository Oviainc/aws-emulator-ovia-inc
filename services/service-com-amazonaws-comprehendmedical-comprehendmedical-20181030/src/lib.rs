// SPDX-License-Identifier: CC0-1.0

#![allow(clippy::doc_lazy_continuation)]
#![allow(clippy::default_trait_access, clippy::use_self, clippy::derivable_impls, clippy::default_constructed_unit_structs)]
#![allow(non_camel_case_types)]
#![forbid(unsafe_code)]
#![doc(hidden)]
//! Service crate for `com-amazonaws-comprehendmedical-comprehendmedical-20181030`.

pub mod ops;
pub mod scaffolded;
pub mod service;
pub mod state;

pub use state::ComAmazonawsComprehendmedicalComprehendmedical20181030State;

pub use service::EmulatedService;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-comprehendmedical-comprehendmedical-20181030.json");

include!(concat!(env!("OUT_DIR"), "/com_amazonaws_comprehendmedical_comprehendmedical_20181030_service_decl.rs"));
