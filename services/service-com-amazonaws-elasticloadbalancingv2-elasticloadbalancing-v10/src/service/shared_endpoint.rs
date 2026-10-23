// SPDX-License-Identifier: CC0-1.0

//! SUPERSEDED — this module carries no code and is declared nowhere, so it compiles to
//! nothing.
//!
//! It held this model's side of being served behind the one shared `elasticloadbalancing`
//! front door: an `ErasedOperation` that mounted this crate's `EmulatedService` over the
//! shared engine and ran the operation the request's `Action` named against it. While it
//! was in flight, the Classic crate landed the same mechanism on its own side, as
//! `V2FrontDoor` in
//! `services/service-com-amazonaws-elasticloadbalancing-elasticloadbalancing-v7/src/service/api_version.rs`
//! — which is the service that CLAIMS the shared routing key, and therefore the one place
//! the version half of AWS's `(Action, Version)` resolution can be applied. Two copies of
//! that mechanism would be two ways to serve one endpoint, so this one is gone rather than
//! kept beside it.
//!
//! The content is gone rather than the file because this fixer's tool set can write a file
//! but not remove one, exactly as `services/common/src/query_error_envelope.rs` records
//! for the same reason; the next change to touch this directory should delete it.
