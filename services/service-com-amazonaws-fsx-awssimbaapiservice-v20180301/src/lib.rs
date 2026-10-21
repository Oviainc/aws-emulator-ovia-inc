// SPDX-License-Identifier: CC0-1.0

#![allow(clippy::doc_lazy_continuation)]
#![allow(clippy::default_trait_access, clippy::use_self, clippy::derivable_impls, clippy::default_constructed_unit_structs)]
#![allow(non_camel_case_types)]
#![forbid(unsafe_code)]
#![doc(hidden)]
//! Service crate for `com-amazonaws-fsx-awssimbaapiservice-v20180301`.

pub mod ops;
pub mod scaffolded;
pub mod service;
pub mod state;

pub use state::ComAmazonawsFsxAwssimbaapiserviceV20180301State;

pub use service::EmulatedService;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-fsx-awssimbaapiservice-v20180301.json");

include!(concat!(env!("OUT_DIR"), "/com_amazonaws_fsx_awssimbaapiservice_v20180301_service_decl.rs"));
