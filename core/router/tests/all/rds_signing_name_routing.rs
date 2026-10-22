// SPDX-License-Identifier: CC0-1.0

//! MOVED — this file is intentionally empty. It carries no test; it stays declared in
//! `tests/all.rs` only because the crate's layout check refuses an undeclared file under
//! `tests/all/`, and an empty module compiles to nothing.
//!
//! The rule it used to carry — for a request to the SHARED `rds` signing name, EXACTLY
//! ONE of Amazon RDS, Amazon DocumentDB and Amazon Neptune claims it, asked of each
//! service's `Service::can_handle` directly — now lives in the sibling suite
//! `all/rds_docdb_neptune_signing_name_routing.rs`, as
//! `exactly_one_service_claims_each_request_for_the_rds_key`. That suite already owns the
//! harness for this rule (the three services over one engine and the SigV4 credential
//! scope builder), and keeping a second copy of it would let the two drift. The API
//! version a Query envelope must name is covered end to end, through a router, by
//! `services/service-com-amazonaws-rds-amazonrdsv19/tests/all/query_catalog_reads.rs`.
//!
//! The content is gone rather than the file because this fixer's tool set can write a
//! file but not remove one; the next change to touch this directory should delete it.
