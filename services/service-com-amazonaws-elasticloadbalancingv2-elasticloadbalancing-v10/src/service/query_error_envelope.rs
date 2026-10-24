// SPDX-License-Identifier: CC0-1.0

//! SUPERSEDED — this file is intentionally empty. It carries no code and is declared
//! nowhere, so it compiles to nothing.
//!
//! It held this service's own writer for the `<RequestId>` an awsQuery `<ErrorResponse>`
//! carries beside its `<Error>`. While it was in flight, that rule landed in the SHARED
//! home every awsQuery service in this tree now reaches —
//! `services/common/src/query_error_envelope.rs`, used from `src/service.rs` as
//! `service_common::query_error_envelope::{RequestIdEnvelopeDefinition,
//! RequestIdEnvelopeOperation}` — so this module had no user and would have stated one
//! rule twice.
//!
//! The content is gone rather than the file because this fixer's tool set can write a
//! file but not remove one; the next change to touch this directory should delete it.
