// SPDX-License-Identifier: CC0-1.0

//! Empty: how a write to a table with a vector index is held to that index's shape lives
//! in `state/table.rs` (`Table::check_vector_attributes`), beside the index-key rules
//! every write path is held to, which is where every caller reaches it.
