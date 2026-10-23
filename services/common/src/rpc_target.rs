// SPDX-License-Identifier: CC0-1.0

//! MOVED — this crate declares no `rpc_target` module, so this file compiles nowhere.
//!
//! The rule it held — on the awsJson protocols the operation an `X-Amz-Target` names is
//! its LAST dot-separated component and the service is the component before it, because
//! an AWS SDK's published target prefix is opaque and may itself contain dots — now
//! lives beside its one caller, at
//! `services/service-com-amazonaws-cloudtrail-cloudtrail-20131101/src/service/rpc_target.rs`.
//!
//! It lives there rather than here because CloudTrail is the only service in this
//! emulator whose published target prefix contains dots: for a single-identifier prefix
//! the framework's first-dot read (`emulator_server::service_from_target`, which
//! [`crate::claims_request`] uses) and the rule above agree, so no other service needs
//! it, and a service crate taking a path dependency on this one for a single reader
//! rewrites `Cargo.lock` for every service that already depends on it. Move the reader
//! back here, declared in `lib.rs`, the moment a second service needs it.
