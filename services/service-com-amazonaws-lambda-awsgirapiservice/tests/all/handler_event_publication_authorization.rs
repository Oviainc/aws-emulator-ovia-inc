// SPDX-License-Identifier: CC0-1.0

//! Superseded, and deliberately holding no test: the RECORD this suite was written to pin — the
//! credential a function's own SDK calls are signed with being resolvable to its execution role
//! by the account's identity store as well as by the token service's registry — is pinned by
//! `core/router/tests/all/lambda_execution_role_session_identity.rs`, which is where it belongs
//! because three crates have to agree for it to hold (this service mints the credential, the
//! token service vends the session, IAM holds the record).
//!
//! It is not kept here for two reasons. The rule's own decision is already pinned in this
//! target by `execution_role_session_identity.rs`'s neighbours — `handler_eventbridge_publication.rs`
//! drives the TABLE of policy shapes with the credential this service mints, and the
//! EventBridge crate's `handler_execution_role_publication.rs` runs a `python3.x` handler
//! through it — so a suite here would be a second copy of that rule. And an invocation of its
//! own costs this test target a share of the RUN BUDGET every other invocation suite draws on:
//! a run's memory bound is sampled from the whole process, so a suite that runs handlers raises
//! what the suites beside it are measured against.
//!
//! The file stays because every suite under `tests/all/` is declared in `tests/all.rs`.
