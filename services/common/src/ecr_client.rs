// SPDX-License-Identifier: CC0-1.0

//! Intentionally empty: this file is NOT part of the build.
//!
//! It held the in-process Amazon ECR client a service would resolve a caller-supplied
//! container image path through. The refusal that reading was written for is one the
//! recorded service does not make, so `lib.rs` declares no module here and no crate
//! depends on the ECR client; the file is left with no code rather than carrying a
//! reading nothing compiles and nothing calls.
