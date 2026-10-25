// SPDX-License-Identifier: CC0-1.0

//! MOVED — this suite is intentionally empty.
//!
//! It pinned the rule that an account managing no device in the signed Region is
//! answered from state like any other caller (an empty page from the collection
//! operations, the modelled `ResourceNotFoundException` from the id-addressed ones)
//! and never with a 403. The recorded behaviour of the service contradicts that: such
//! a Region fails an account/region device precondition and every operation the model
//! declares `AccessDeniedException` on refuses there, ahead of resolving any id and
//! ahead of validating any field.
//!
//! What this service answers such a caller is now pinned in
//! `tests/all/device_region_gate.rs`; what it answers an id naming nothing in a
//! Region that DOES hold an inventory is pinned in `tests/all/unknown_resource_id.rs`.
//!
//! The file stays declared in `tests/all.rs` because the crate's layout check refuses
//! an undeclared file under `tests/all/`, and an empty module compiles to nothing.
