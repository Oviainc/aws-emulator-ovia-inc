// SPDX-License-Identifier: CC0-1.0

//! SUPERSEDED, and left behind only because this agent's tools cannot delete a file:
//! nothing declares this module (see `lib.rs`), so no code here reaches a build.
//!
//! The one evaluation of an IAM policy document against a request now lives in
//! [`crate::iam_authorization`], which mainline landed while this branch was open and
//! which is a superset of what this module held (identity-based AND resource-based
//! documents, principal elements, condition clauses, and a document-size backstop). Every
//! caller — `service-com-amazonaws-bedrockagentruntime-amazonbedrockagentruntimeservice`'s
//! `state::authorize` among them — asks that module instead, so there is exactly one
//! reading of an IAM document in this crate. Delete this file.
