// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//! What the suite's tests share: the models this build compiles.

use super::workspace_models::{model_files_under, signed_rest_json_operations, SweptOperation};
use std::path::PathBuf;
use std::sync::OnceLock;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Every restJson1 operation this build's models sign; none is an error where the build says so.
pub fn signed_operations() -> &'static [SweptOperation] {
    static SIGNED: OnceLock<Vec<SweptOperation>> = OnceLock::new();
    SIGNED.get_or_init(|| {
        let models = option_env!("EMULATOR_SECURITY_MODELS");
        let root = models.map_or_else(workspace_root, PathBuf::from);
        let signed = signed_rest_json_operations(&model_files_under(&root));
        let required = option_env!("EMULATOR_SECURITY_REQUIRE_OPERATIONS").is_some();
        let missing = required && signed.is_empty();
        assert!(!missing, "no signed operation under {root:?}");
        signed
    })
}

/// The operation to call when any signed one will do; `None`, said aloud, when there is none.
pub fn probe_operation() -> Option<&'static SweptOperation> {
    let probe = signed_operations().first();
    if probe.is_none() {
        eprintln!("nothing to check: no signed restJson1 operation in this build");
    }
    probe
}
