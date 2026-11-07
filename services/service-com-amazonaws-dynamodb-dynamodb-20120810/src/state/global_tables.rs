// SPDX-License-Identifier: CC0-1.0

//! The LEGACY global-table surface: what the 2017.11.29 operations answer, and why.
//!
//! `CreateGlobalTable`, `DescribeGlobalTable`, `UpdateGlobalTable`,
//! `DescribeGlobalTableSettings` and `UpdateGlobalTableSettings` are all documented the
//! same way — "This documentation is for version 2017.11.29 (Legacy) of global tables,
//! which should be avoided for new global tables. Customers should use Global Tables
//! version 2019.11.21 (Current) when possible" (the `CreateGlobalTable` doc) — so they are
//! one surface, addressing global tables OF THAT VERSION.
//!
//! That version is no longer offered: a global table is created by adding replicas to a
//! table ([`super::replicas`], which establishes a group under
//! [`super::GLOBAL_TABLE_VERSION`]), and `CreateGlobalTable` — the only operation that
//! could create one of the legacy version — refuses the call as the client error it is
//! ([`legacy_version_withdrawn`]). So no global table of the legacy version exists, and
//! every legacy operation that ADDRESSES one answers the modelled
//! `GlobalTableNotFoundException` ([`legacy_global_table_not_found`]) — the error a client
//! catches by name, rather than an unmodelled server-class answer it can neither retry nor
//! fix.
//!
//! `ListGlobalTables` is the one operation of the surface that addresses no single global
//! table: it "lists all global tables that have a replica in the specified Region", which
//! is a question about replication groups whatever version established them, so it reports
//! the groups this store holds.

use super::GLOBAL_TABLE_VERSION;

/// The global tables version these operations document themselves for, and which is no
/// longer offered.
pub const LEGACY_GLOBAL_TABLE_VERSION: &str = "2017.11.29";

/// The message a call that would CREATE a global table of the legacy version is refused
/// with.
///
/// A permanently invalid request rather than a gap or a conflict: the version it asks for
/// is not on offer, so no retry and no change of state can make the same call succeed —
/// the caller has to create the global table the current way instead. The sentence states
/// the version that was asked for and the one to use, both facts of this service rather
/// than of the request.
pub fn legacy_version_withdrawn() -> String {
    format!(
        "Global tables version {LEGACY_GLOBAL_TABLE_VERSION} (Legacy) is no longer supported. Use \
         global tables version {GLOBAL_TABLE_VERSION} (Current) instead."
    )
}

/// The message the modelled `GlobalTableNotFoundException` ("The specified global table
/// does not exist") carries for a legacy operation addressing `name`: the global table it
/// names, which is what the caller asked about.
pub fn legacy_global_table_not_found(name: &str) -> String {
    format!("Global table not found: {name}")
}
