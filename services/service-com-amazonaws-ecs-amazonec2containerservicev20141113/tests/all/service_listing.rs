// SPDX-License-Identifier: CC0-1.0

//! MOVED — this file is intentionally empty. It carries no test; it stays declared in
//! `tests/all.rs` only because the crate's layout check refuses an undeclared file under
//! `tests/all/`, and an empty module compiles to nothing.
//!
//! The rule it used to carry — `ListServices` reports the ARNs of the services STORED for
//! the cluster the request names, narrowed by every filter the request sets, in a stable
//! order, paged at `maxResults` with a `nextToken` that walks the set exactly once — now
//! lives in the sibling suite `all/list_services.rs`, whose rows span the same filters, the
//! documented page-size bounds, the default page size and a deleted service leaving the
//! listing, over its own `page`/`all_pages` harness. Keeping a second copy of it would let
//! the two drift.
//!
//! The content is gone rather than the file because this fixer's tool set can write a file
//! but not remove one; the next change to touch this directory should delete it.
