// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `list_domain_names` operation.
//!
//! HAND-AUTHORED, not scaffolded: `ListDomainNames` takes Smithy's `Unit` input, which
//! the op-stub generator declines to type (see `src/ops/SKIPPED_STUBS.md`), so its two
//! shapes are declared here beside the generated ones. Each structure/union is declared
//! exactly once across this service's `types/` modules, and `types/mod.rs` re-exports
//! them all.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-cloudsearch-a9searchcloudconfigservice2013.json");

/// `ListDomainNames` has no input members: the model binds it to `smithy.api#Unit`,
/// which is a PRELUDE shape and so is not in this service's model file. The legacy
/// `structure = "..."` key is what declares the shape id without resolving it against
/// the model, and the macro's empty-struct path emits the schema and the
/// serialize/deserialize impls from the id alone — which is all an input with no
/// members needs.
#[smithy_macros::smithy(structure = "smithy.api#Unit")]
#[derive(Debug, Clone, Default)]
pub struct ListDomainNamesInput {}

#[smithy_macros::smithy(shape = "com.amazonaws.cloudsearch#ListDomainNamesResponse")]
#[derive(Debug, Clone, Default)]
pub struct ListDomainNamesOutput {
    pub domain_names: Option<std::collections::HashMap<String, String>>,
}
