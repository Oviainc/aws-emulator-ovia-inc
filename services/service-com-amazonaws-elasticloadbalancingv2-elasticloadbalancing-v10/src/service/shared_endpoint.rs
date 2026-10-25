// SPDX-License-Identifier: CC0-1.0

//! The API version an awsQuery envelope names to be served out of THIS model on the
//! endpoint it shares with the Classic Elastic Load Balancing model.
//!
//! Elastic Load Balancing publishes ONE endpoint for two APIs. Both models declare
//! `endpointPrefix` / `arnNamespace` / `aws.auth#sigv4` name `elasticloadbalancing`
//! (`smithy_query(query="service_info")` on either), differing only in `sdkId` and in the
//! API version they declare — and the router requires exactly one claimant per request,
//! so the shared routing key resolves to the ONE service whose own identity is that name,
//! the Classic model. An awsQuery request carries the other half of the routing key in
//! its form body: `Version`, the API version the named action is asked for. That front
//! door resolves the PAIR the way AWS does, serving an envelope naming [`API_VERSION`]
//! out of this model's own operations over the one shared engine
//! (`services/service-com-amazonaws-elasticloadbalancing-elasticloadbalancing-v7`'s
//! `src/service/api_version.rs`, whose `V2FrontDoor` is the ONE place that mechanism
//! lives: this module carries the VALUE it keys on, not a second copy of the mechanism).
//!
//! This module is this model's statement of that one value, so neither the front door nor
//! this crate's own suites restate the literal beside the model that declares it. It
//! lives beside [`crate::service`] rather than in `crate::state` because it is a fact
//! about this service's WIRE identity, not about anything it stores.

/// The API version THIS model declares, from `smithy_query(query="service_info")` on this
/// crate's model: `version: 2015-12-01`.
pub const API_VERSION: &str = "2015-12-01";
