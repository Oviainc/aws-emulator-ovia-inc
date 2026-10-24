// SPDX-License-Identifier: CC0-1.0

//! RETIRED, deliberately empty: the `UpdateAssetModel` rules this suite used to
//! pin now live in `tests/all/update_asset_model.rs`, which is this crate's one
//! suite for that operation.
//!
//! What was here duplicated that file — the definition being replaced in full,
//! retained properties keeping their ids, the conditional headers evaluated
//! before any write with `412 PreconditionFailedException` leaving the name and
//! the ETag untouched, and a stale-tag replay — so the only rows it carried that
//! `update_asset_model.rs` did not were the two that target a version which does
//! not exist yet (`If-Match` / `If-None-Match` on `ACTIVE` while the model's owed
//! transition is still due). Those were folded into that file as
//! `the_conditional_headers_target_a_version_that_may_not_exist_yet`, and the
//! lost-update race it also covered sequentially is pinned with real overlapping
//! calls in `tests/all/simultaneous_writes.rs`.
//!
//! The file is left in place, declared in `tests/all.rs` but carrying no test,
//! because the fix tooling can rewrite a file but cannot remove one; the next
//! change in this directory should delete it and its declaration together.
