// SPDX-License-Identifier: CC0-1.0

//! Intentionally EMPTY, and declared only because the build requires every file under
//! `tests/all/` to be: the subject this suite was written for — `StartExportTask` and the
//! objects an export writes into Amazon S3 — is driven end to end by
//! [`crate::rds_s3_integration`], which landed on mainline while this change was in
//! review and supersedes it. The file remains because this change's tooling cannot remove
//! a file from the tree.
