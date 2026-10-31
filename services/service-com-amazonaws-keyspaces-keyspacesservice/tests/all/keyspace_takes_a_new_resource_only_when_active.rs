// SPDX-License-Identifier: CC0-1.0

//! Superseded, and declared nowhere: the suite that lives here moved to
//! `tests/all/a_keyspace_takes_a_new_resource_before_it_settles.rs`, which pins the rule
//! the recorded behaviour of Amazon Keyspaces actually shows — a keyspace takes a new
//! table, type or restored table as soon as it is stored, and only a keyspace whose
//! deletion is in flight refuses one.
