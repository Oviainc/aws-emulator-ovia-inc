// SPDX-License-Identifier: CC0-1.0

//! Intentionally empty: this file is NOT part of the build.
//!
//! It held a reading of a caller-supplied Amazon ECR image path and the question a
//! service would put to ECR before storing a resource that pulls that image. The refusal
//! that question was written for is one the recorded service does not make, so `lib.rs`
//! declares no module here; the file is left with no code rather than carrying a reading
//! nothing compiles and nothing calls.
