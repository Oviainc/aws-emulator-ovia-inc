// SPDX-License-Identifier: CC0-1.0

//! Empty, and not declared as a module anywhere: no claim rule lives in this file.
//!
//! The rule for a signing-name family whose members' URI path spaces OVERLAP is this
//! crate's root `claims_modeled_route` in `lib.rs`, which a service calls directly with
//! the keys its endpoint answers on (the Bedrock AgentCore pair). The six Amazon Chime
//! services use a rule of their own, [`crate::chime_family`], because that family's
//! endpoint host is what names one member and several of its models bind IDENTICAL routes.
//!
//! The file is left in place because the issue-fixer tooling can write a file but cannot
//! remove one; nothing compiles it (no module declares it), and it may be deleted outright
//! by anyone touching this crate. Same note as `service_claim.rs` and
//! `modeled_route_claim.rs`.
