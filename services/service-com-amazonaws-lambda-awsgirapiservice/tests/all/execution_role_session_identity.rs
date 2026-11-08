// SPDX-License-Identifier: CC0-1.0

//! Superseded, and deliberately holding no test: what this suite pinned — the credential a
//! function's own SDK calls are signed with being resolvable to its execution role by the
//! account's identity store as well as by the token service's registry — now lives in
//! `core/router/tests/all/lambda_execution_role_session_identity.rs`, unchanged but for its
//! header. Three crates have to agree for that rule to hold (this service mints the credential,
//! the token service vends the session, IAM holds the record a service resolves a temporary
//! credential's principal through), which is what the cross-service target is for, and keeping
//! it there leaves this target's test schedule as it was: a suite added here changes how many
//! handler runs of the OTHER suites are in flight at once, and a run's memory bound is sampled
//! from the whole process.
//!
//! The file stays because every suite under `tests/all/` is declared in `tests/all.rs`.
