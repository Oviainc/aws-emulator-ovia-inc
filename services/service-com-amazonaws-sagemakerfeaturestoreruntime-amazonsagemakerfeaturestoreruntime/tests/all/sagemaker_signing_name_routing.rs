// SPDX-License-Identifier: CC0-1.0

//! Superseded, and deliberately NOT declared in `tests/all.rs`.
//!
//! The nine-claimant `sagemaker` signing-name table this file used to hold now lives in
//! `core/router/tests/all/sagemaker_signing_name_routing.rs`, where it runs with EVERY
//! service mounted rather than with the nine as dev-dependencies of this leaf crate. The
//! FeatureStore Runtime rows it also covered — a malformed `FeatureGroupNameOrArn` on a
//! path-bound and a body-bound member, refused 400 `ValidationException` with the member
//! path and the modelled pattern — are asserted here by
//! `tests/all/feature_group_name_validation.rs` through the same real router.
