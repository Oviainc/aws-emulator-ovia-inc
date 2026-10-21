// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-appintegrations-amazonappintegrationservice service.
//!
//! Besides the state struct itself this module holds the few helpers the
//! event-integration handlers share — ARN construction, the request's region, the
//! record-to-modelled-shape conversions and the list cursor codec — so the five
//! operation files agree on one implementation of each instead of carrying four
//! copies that can drift apart.

use std::collections::{BTreeMap, HashMap};
use std::time::SystemTime;

use crate::ops::types::{EventFilter, EventIntegration};

/// The region an emulated request is treated as having been made in when its
/// SigV4 credential scope does not name one (an unsigned or anonymous request).
pub const DEFAULT_REGION: &str = "us-east-1";

/// The `arnNamespace` / `endpointPrefix` this service's ARNs carry, from the
/// model's `aws.api#service` trait (`smithy_query(query="service_info")`:
/// `arnNamespace: app-integrations`).
pub const ARN_NAMESPACE: &str = "app-integrations";

/// The page size `ListEventIntegrations` uses when the caller sends no
/// `MaxResults`: the largest page the model allows (`MaxResults` range 1..50).
pub const DEFAULT_MAX_RESULTS: usize = 50;

/// The persisted `EventFilter` of an event integration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventFilterRecord {
    pub source: String,
}

/// One stored event integration: everything `CreateEventIntegration` was given
/// plus the ARN the service minted for it, so `Get`/`List` report the identical
/// values `Create` returned instead of recomputing or echoing them.
#[derive(Debug, Clone)]
pub struct EventIntegrationRecord {
    pub name: String,
    pub arn: String,
    pub description: Option<String>,
    pub event_bridge_bus: String,
    pub event_filter: EventFilterRecord,
    pub tags: BTreeMap<String, String>,
    pub client_token: Option<String>,
    pub created_time: SystemTime,
    pub last_modified_time: SystemTime,
}

impl EventIntegrationRecord {
    /// The persisted event filter as the modelled shape.
    pub fn modeled_event_filter(&self) -> EventFilter {
        EventFilter {
            source: self.event_filter.source.clone(),
        }
    }

    /// The persisted tags as the modelled `TagMap`, or `None` when the resource
    /// carries none: the model constrains `TagMap` to 1..200 entries, so an
    /// untagged resource reports no member rather than an empty map.
    pub fn modeled_tags(&self) -> Option<HashMap<String, String>> {
        if self.tags.is_empty() {
            return None;
        }
        Some(
            self.tags
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        )
    }

    /// This record as a `ListEventIntegrations` summary.
    pub fn to_event_integration(&self) -> EventIntegration {
        EventIntegration {
            event_integration_arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            description: self.description.clone(),
            event_filter: Some(self.modeled_event_filter()),
            event_bridge_bus: Some(self.event_bridge_bus.clone()),
            tags: self.modeled_tags(),
        }
    }
}

/// Per-service state for com-amazonaws-appintegrations-amazonappintegrationservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsAppintegrationsAmazonappintegrationserviceState {
    /// Every stored event integration, keyed by its name — the resource's
    /// identity in this API. A `BTreeMap` so list order, and therefore
    /// pagination, is deterministic.
    pub event_integrations: BTreeMap<String, EventIntegrationRecord>,
    /// `ClientToken` -> integration name, so a retried `CreateEventIntegration`
    /// carrying the token of a completed one answers with the ARN it already
    /// minted instead of creating a second resource or failing as a duplicate.
    pub idempotency: BTreeMap<String, String>,
}

/// The region of the request being served: the third field of its SigV4
/// credential scope, falling back to [`DEFAULT_REGION`] for a request that is
/// not signed.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The ARN of an event integration in `region`:
/// `arn:aws:app-integrations:<region>:<account>:event-integration/<name>`.
pub fn event_integration_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("event-integration/{name}"))
}

/// The pagination cursor for a page that starts at `name`.
///
/// Hex of the key rather than the key itself, so a caller cannot read structure
/// into the token and has nothing to build one from: a `NextToken` is opaque and
/// only ever travels back unchanged.
pub fn encode_cursor(name: &str) -> String {
    ::hex::encode(name.as_bytes())
}

/// The inclusive start key a cursor minted by [`encode_cursor`] carries, or
/// `None` when the token was not minted by this service.
pub fn decode_cursor(token: &str) -> Option<String> {
    String::from_utf8(::hex::decode(token).ok()?).ok()
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsAppintegrationsAmazonappintegrationserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsAppintegrationsAmazonappintegrationserviceState>)
}
