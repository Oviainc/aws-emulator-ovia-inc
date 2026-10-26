// SPDX-License-Identifier: CC0-1.0

//! Intentionally empty, and not declared as a module anywhere.
//!
//! This file briefly held a reader for the signing name a request names in its SigV4
//! credential scope, for every flavour of the family — the region-less SigV4A form
//! included, which the framework's own five-field resolver cannot read. Nothing in this
//! crate calls it: the one service that needed a claim rule for such a request reads the
//! URI path space its own model publishes instead (`crate::claims_modeled_path`'s rule,
//! applied in that service's `can_handle`), which needs no reader of the `Authorization`
//! header at all. A copy of the scope reader still lives privately in
//! `services/service-com-amazonaws-sustainability-awssustainabilityapiservice/src/service.rs`;
//! the right home for it, if a second service ever needs it, is this crate.
//!
//! The file is left in place because the issue-fixer tooling can write a file but cannot
//! remove one; nothing compiles it (no module declares it), and it may be deleted outright
//! by anyone touching this crate. Same note as `service_claim.rs`, `route_claim.rs` and
//! `modeled_route_claim.rs`.
