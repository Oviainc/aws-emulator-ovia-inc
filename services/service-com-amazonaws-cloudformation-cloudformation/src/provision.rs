// SPDX-License-Identifier: CC0-1.0

//! Creating a stack's declared resources in the services that own them lives in
//! `state/provisioning.rs`, beside the deployment that drives it, and the identifiers
//! each declared resource reports live in `state/template.rs`'s return-value catalogue.
//!
//! This module is deliberately empty, and is not declared: an earlier revision of the
//! container-platform fix (Amazon ECR, Amazon ECS, Elastic Load Balancing v2) carried its
//! own provisioner here before `state/provisioning.rs` landed, and folding the two into
//! one layer is what the provisioner is — one place that calls the owning service, not
//! two.
