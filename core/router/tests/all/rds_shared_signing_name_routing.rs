// SPDX-License-Identifier: CC0-1.0

//! MOVED — this file is intentionally empty and is not declared in `tests/all.rs`, so
//! nothing here compiles or runs.
//!
//! The rows it used to carry (a `DescribeOptionGroups` for an option group nothing
//! holds under the shared `rds` key, the same action under each peer's own key, and an
//! option group created and read back through the router) now live in the sibling suite
//! `all/rds_docdb_neptune_signing_name_routing.rs`, which already owns the harness for
//! that rule: one router carrying Amazon RDS, Amazon DocumentDB and Amazon Neptune, the
//! SigV4 scope builder, the `<Code>` reader and the "never a router-level refusal"
//! assertions. Keeping a second copy of that harness would let the two drift.
//!
//! The content is gone rather than the file because this fixer's tool set can write a
//! file but not remove one; the next change to touch this directory should delete it.
