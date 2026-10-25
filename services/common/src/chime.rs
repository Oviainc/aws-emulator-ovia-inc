// SPDX-License-Identifier: CC0-1.0

//! SUPERSEDED — this file is intentionally empty and is not declared in `lib.rs`, so
//! nothing here compiles or runs.
//!
//! The claim rule it used to carry for the six services that sign as `chime` now lives in
//! the sibling module [`crate::chime_family`], which states the same split and keys it on
//! the one signal that names a single member — the endpoint the request addresses —
//! before falling back to each model's own path space. Keeping a second rule for one
//! family would let the two drift into a path with two claimants or none, which is the
//! refusal the rule exists to remove.
//!
//! The content is gone rather than the file because this fixer's tool set can write a
//! file but not remove one; the next change to touch this directory should delete it.
