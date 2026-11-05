// SPDX-License-Identifier: CC0-1.0

//! An EMPTY suite, deliberately: its case now lives in
//! `all/cloudformation_sam_http_api_definitions.rs`, which drives the same stack through the
//! production service set rather than through a router of its own.
//!
//! Declared in `tests/all.rs` because the integration-test layout check refuses an
//! undeclared file under `tests/all/`, and this fixer's tool set can write a file but not
//! remove one — the same reason
//! `services/service-com-amazonaws-cloudformation-cloudformation/tests/all/delete_canceled_export_in_use.rs`
//! is still there. The next change to touch this directory should delete both lines and this
//! file.
