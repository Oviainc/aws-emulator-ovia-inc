// SPDX-License-Identifier: CC0-1.0

//! Where the module list of `tests/all.rs` is checked, and why not here.
//!
//! A suite module name may be bound only once in this crate's test-target root: a second
//! `mod <name>;` is `error[E0428]`, and the whole `all` target — every suite in it — then
//! fails to compile. That makes the rule uncheckable from inside the crate it protects,
//! because a suite here is only ever run once the target has already compiled.
//!
//! It is checked instead from the one target that links every service crate:
//! `core/router/tests/all/all_rs_declares_each_suite_once.rs` reads this crate's
//! `tests/all.rs`, along with every other crate's, and reports a name or a suite file
//! declared twice, or a declaration naming a file that does not exist. This file holds no
//! test of its own; it records where that check lives for whoever appends the next
//! declaration above.
