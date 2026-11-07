// SPDX-License-Identifier: CC0-1.0

//! Intentionally empty: this module is superseded.
//!
//! What an export reads out of its source and writes to Amazon S3 lives in
//! `state/export_access.rs`, `state/export_objects.rs` and `state/export_parquet.rs`,
//! which is where `StartExportTask` carries it. Nothing declares this module, so nothing
//! here is built; the file remains only because this change's tooling cannot remove a
//! file from the tree.
