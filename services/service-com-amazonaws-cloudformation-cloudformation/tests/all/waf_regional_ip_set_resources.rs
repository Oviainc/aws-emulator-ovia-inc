// SPDX-License-Identifier: CC0-1.0

//! RETIRED, deliberately empty: the `AWS::WAFRegional::IPSet` provisioning this suite
//! used to drive is mainline's own (`src/state/provisioner.rs`), and it is already pinned
//! end to end by `core/router/tests/all/cloudformation_provisions_waf_regional_ip_sets.rs`
//! — which mounts CloudFormation and AWS WAF Regional on one engine exactly as
//! `core/router/src/lib.rs` does and checks the exported `Ref`, the read-back through
//! `GetIPSet` and the release on stack deletion.
//!
//! This file carried a second copy of that coverage plus its own awsJson1_1 dispatch
//! helper (the duplication the code review flagged), and it is the only reason this crate
//! needed a dev-dependency on the waf-regional service crate — an edge in `Cargo.lock`
//! that collided with every mainline landing and kept the squash merge from applying. The
//! rows it added beyond the router suite (both deploy paths, the generated name, the
//! account quota, the reconciling update and `DELETE_FAILED` on a referenced IPSet) belong
//! in `all/provisioned_resources.rs`, whose table already covers every type
//! `state::provisioning` provisions, and should be added there with that suite's own
//! harness.
//!
//! Declared in `tests/all.rs` because the layout check refuses an undeclared file under
//! `tests/all/`, and this fixer's tool set can write a file but not remove one; the next
//! change to touch this directory should delete both lines and the file.
