// SPDX-License-Identifier: CC0-1.0

//! Superseded, and not part of the build: no module declaration references this file (see
//! `src/service.rs`), so nothing here compiles.
//!
//! The two store lookups the `execute-api` data plane's authorizer needs — the session the
//! presented access key id names, and the identity-based policy documents IAM holds for the
//! principal behind it — live in [`crate::service::invoke::execute_api_authorization`],
//! which is a superset of what this file briefly held: it reads the same two stores through
//! the same accessors, and beside them the permissions boundary in force on the principal,
//! which caps what those documents grant. Every caller asks that module, so there is one
//! resolution of who an invoke speaks for in this crate.
//!
//! It is retained only until a later change in this crate removes it, as
//! `src/service/execute_api.rs` and `services/common/src/identity_policy.rs` are.
