// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `get_credential_report` operation.
//!
//! HAND-AUTHORED: the scaffold op-stub generator declined to emit a stub for
//! `GetCredentialReport` because its input is `smithy.api#Unit`, which it does not type yet
//! (see `src/ops/SKIPPED_STUBS.md`), so these shapes are written out here in the same shape
//! as a rendered sibling — exactly as `generate_credential_report_types.rs` was written for
//! the other half of the same feature.
//!
//! Each structure/union is declared exactly once across this service's `types/`
//! modules, and `types/mod.rs` re-exports them all.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-iam-awsidentitymanagementv20100508.json");

/// `GetCredentialReport` takes no input: the operation's modelled input is
/// `smithy.api#Unit`. The empty shape still carries a schema, a serializer and a
/// deserializer, because the framework's centralized request walk deserializes and
/// validates every input against `INPUT_SCHEMA` before `handle` runs.
#[smithy_macros::smithy(input = "smithy.api#Unit")]
#[derive(Debug, Clone, Default)]
pub struct GetCredentialReportInput;

#[smithy_macros::smithy(shape = "com.amazonaws.iam#GetCredentialReportResponse")]
#[derive(Debug, Clone, Default)]
pub struct GetCredentialReportOutput {
    pub content: Option<Vec<u8>>,
    pub report_format: Option<String>,
    pub generated_time: Option<::aws_smithy_types::DateTime>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.iam#CredentialReportNotPresentException")]
#[derive(Debug, Clone, Default)]
pub struct CredentialReportNotPresentException {
    pub message: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.iam#CredentialReportNotReadyException")]
#[derive(Debug, Clone, Default)]
pub struct CredentialReportNotReadyException {
    pub message: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.iam#CredentialReportExpiredException")]
#[derive(Debug, Clone, Default)]
pub struct CredentialReportExpiredException {
    pub message: Option<String>,
}

/// The values `ReportFormatType` accepts on the wire
/// (`smithy_query(query="shape", shape="ReportFormatType")`): the enum declares exactly
/// one, the MIME type a credential report is served as.
///
/// Declared here rather than in `shared_types` because `GetCredentialReportResponse` is the
/// only shape that reaches the enum.
pub const REPORT_FORMAT_TYPE_VALUES: &[&str] = &["text/csv"];
