// SPDX-License-Identifier: CC0-1.0

//! Deliberately empty: the "every output of a deployed search-indexing pipeline carries the
//! value its service reports" suite moved to
//! `core/router/tests/all/cloudformation_search_indexing_pipeline_outputs.rs`.
//!
//! It belongs there because the estate it deploys needs FOUR services on one engine —
//! CloudFormation reaches Amazon OpenSearch Service, Amazon S3 and IAM through the in-process
//! callback to create the domain, the bucket and the execution role — and the cross-service
//! test crate already mounts every service, so the suite needs no dependency of this crate's
//! own.
//!
//! The file stays declared in `tests/all.rs` because this crate's build script requires every
//! file under `tests/all/` to be declared with a module of the same name, and the fixer
//! tooling that wrote it can create and rewrite a file but cannot remove one; the next change
//! to touch this directory should delete both lines and the file.
