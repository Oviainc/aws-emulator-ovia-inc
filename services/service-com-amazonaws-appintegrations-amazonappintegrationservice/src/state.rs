// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-appintegrations-amazonappintegrationservice service.
//!
//! Besides the state struct itself this module holds the few helpers the
//! event-integration and data-integration handlers share — ARN construction, the
//! request's region, the not-found wording, the record-to-modelled-shape conversions
//! and the list cursor codec — so the operation files agree on one implementation of
//! each instead of carrying a copy apiece that can drift apart.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::SystemTime;

use crate::ops::types::{
    DataIntegrationSummary, EventFilter, EventIntegration, FileConfiguration,
    ScheduleConfiguration,
};

/// The region an emulated request is treated as having been made in when its
/// SigV4 credential scope does not name one (an unsigned or anonymous request).
pub const DEFAULT_REGION: &str = "us-east-1";

/// The `arnNamespace` / `endpointPrefix` this service's ARNs carry, from the
/// model's `aws.api#service` trait (`smithy_query(query="service_info")`:
/// `arnNamespace: app-integrations`).
pub const ARN_NAMESPACE: &str = "app-integrations";

/// The page size a paginated `List` uses when the caller sends no `MaxResults`: the
/// largest page the model allows (`MaxResults` range 1..50 on both
/// `ListEventIntegrations` and `ListDataIntegrations`).
pub const DEFAULT_MAX_RESULTS: usize = 50;

/// The most event integrations this emulator stores at once.
///
/// AWS caps EventIntegrations with a service quota and reports an exceeded cap as
/// `ResourceQuotaExceededException` ("the allowed quota for the resource has been
/// exceeded"), which `CreateEventIntegration` models. The NUMBER here is this
/// emulator's own bound, not an AWS quota: it keeps an in-memory store that only a
/// caller's delete ever shrinks from growing without end, and sits far above anything a
/// test or a client script creates.
pub const MAX_STORED_EVENT_INTEGRATIONS: usize = 1_000;

/// The most data integrations this emulator stores at once.
///
/// AWS caps DataIntegrations with a service quota and reports an exceeded cap as
/// `ResourceQuotaExceededException` ("the allowed quota for the resource has been
/// exceeded"), which `CreateDataIntegration` models. The NUMBER here is this emulator's
/// own bound, not an AWS quota: it keeps an in-memory store that only a caller's delete
/// ever shrinks from growing without end, and sits far above anything a test or a
/// client script creates.
pub const MAX_STORED_DATA_INTEGRATIONS: usize = 1_000;

/// The most `ClientToken` -> data integration entries this emulator remembers at once.
///
/// The create writes one per stored integration and the delete drops the one that
/// minted the integration it removes, so this index can never hold more entries than
/// there are stored integrations: the ceiling is stated as the same number so the two
/// cannot drift apart. [`ComAmazonawsAppintegrationsAmazonappintegrationserviceState::remember_client_token`]
/// enforces it without refusing anything — it first drops entries whose integration is
/// gone, and past the ceiling simply does not remember the token (a forgotten token
/// makes a replay create a fresh resource, which the integration ceiling already
/// bounds).
pub const MAX_REMEMBERED_CLIENT_TOKENS: usize = MAX_STORED_DATA_INTEGRATIONS;

/// The most `ClientToken` -> event integration entries this emulator remembers at
/// once, the [`MAX_REMEMBERED_CLIENT_TOKENS`] of the event-integration index and
/// enforced the same way, by
/// [`ComAmazonawsAppintegrationsAmazonappintegrationserviceState::remember_event_client_token`].
pub const MAX_REMEMBERED_EVENT_CLIENT_TOKENS: usize = MAX_STORED_EVENT_INTEGRATIONS;

/// The resource type name AppIntegrations calls an event integration in the text of a
/// `ResourceNotFoundException` — the Smithy shape name of the resource
/// (`smithy_query(query="operation", operation="GetEventIntegration")`: the resource is
/// addressed by its `Name`).
pub const EVENT_INTEGRATION_RESOURCE: &str = "EventIntegration";

/// The same, for a data integration, which is addressed by an opaque identifier rather
/// than by a name (`GetDataIntegration`'s `Identifier` label).
pub const DATA_INTEGRATION_RESOURCE: &str = "DataIntegration";

/// What a resource addressed by its `Name` member is called in a not-found message.
pub const ADDRESSED_BY_NAME: &str = "name";

/// What a resource addressed by an opaque `Identifier` is called in one.
pub const ADDRESSED_BY_IDENTIFIER: &str = "identifier";

/// AppIntegrations' wording for a resource the request named and this account does not
/// hold: the resource TYPE, how the request addressed it, and the value it sent —
/// `"Could not find EventIntegration with name: demo."`,
/// `"Could not find DataIntegration with identifier: <id>."`.
///
/// Request-derived and one template for every resource of this service, so each handler
/// words its 404 through this function rather than inventing a sentence of its own: a
/// client or operator that reads the not-found text sees one shape across the API.
pub fn not_found_message(resource_type: &str, addressed_by: &str, value: &str) -> String {
    format!("Could not find {resource_type} with {addressed_by}: {value}.")
}

/// [`not_found_message`] for the event integration `name` the request named.
pub fn event_integration_not_found(name: &str) -> String {
    not_found_message(EVENT_INTEGRATION_RESOURCE, ADDRESSED_BY_NAME, name)
}

/// [`not_found_message`] for the data integration `identifier` the request named.
pub fn data_integration_not_found(identifier: &str) -> String {
    not_found_message(DATA_INTEGRATION_RESOURCE, ADDRESSED_BY_IDENTIFIER, identifier)
}

/// The persisted tags as the modelled `TagMap`, or `None` when the resource carries
/// none: the model constrains `TagMap` to 1..200 entries, so an untagged resource
/// reports no member rather than an empty map.
pub fn modeled_tags(tags: &BTreeMap<String, String>) -> Option<HashMap<String, String>> {
    if tags.is_empty() {
        return None;
    }
    Some(tags.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
}

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

    /// This record's tags as the modelled `TagMap` (see [`modeled_tags`]).
    pub fn modeled_tags(&self) -> Option<HashMap<String, String>> {
        modeled_tags(&self.tags)
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

/// One stored data integration: everything `CreateDataIntegration` was given, plus the
/// id and ARN the service minted for it, so `Get`/`List` report the identical values
/// `Create` returned instead of recomputing or echoing them.
#[derive(Debug, Clone)]
pub struct DataIntegrationRecord {
    pub id: String,
    pub arn: String,
    pub name: String,
    pub description: Option<String>,
    pub kms_key: String,
    pub source_uri: Option<String>,
    pub schedule_configuration: Option<ScheduleConfiguration>,
    pub file_configuration: Option<FileConfiguration>,
    pub object_configuration: Option<HashMap<String, HashMap<String, Vec<String>>>>,
    pub tags: BTreeMap<String, String>,
    pub client_token: Option<String>,
    /// When this record was created, and when it was last changed. No `Create`/`Get`/
    /// `List` output models a timestamp, so neither is reported today; they are
    /// persisted because `UpdateDataIntegration` (a scaffold stub) is what moves the
    /// second one, and a resource's own times are part of the record rather than of
    /// one operation's response.
    pub created_time: SystemTime,
    pub last_modified_time: SystemTime,
}

impl DataIntegrationRecord {
    /// This record's tags as the modelled `TagMap` (see [`modeled_tags`]).
    pub fn modeled_tags(&self) -> Option<HashMap<String, String>> {
        modeled_tags(&self.tags)
    }

    /// This record as a `ListDataIntegrations` summary — the three members that
    /// shape carries (`smithy_query(query="shape", shape="DataIntegrationSummary")`:
    /// `Arn`, `Name`, `SourceURI`).
    pub fn to_summary(&self) -> DataIntegrationSummary {
        DataIntegrationSummary {
            arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            source_uri: self.source_uri.clone(),
        }
    }
}

/// Per-service state for com-amazonaws-appintegrations-amazonappintegrationservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsAppintegrationsAmazonappintegrationserviceState {
    /// Every stored event integration, keyed by its name — the resource's
    /// identity in this API. A `BTreeMap` so list order, and therefore
    /// pagination, is deterministic. Bounded at [`MAX_STORED_EVENT_INTEGRATIONS`]
    /// entries by the create that writes it
    /// ([`Self::has_room_for_another_event_integration`]), so a caller cannot grow it
    /// without end.
    pub event_integrations: BTreeMap<String, EventIntegrationRecord>,
    /// `ClientToken` -> integration name, so a retried `CreateEventIntegration`
    /// carrying the token of a completed one answers with the ARN it already
    /// minted instead of creating a second resource or failing as a duplicate. One
    /// entry per stored integration, which the delete drops with it, so the ceiling
    /// above bounds this too.
    pub idempotency: BTreeMap<String, String>,
    /// Every stored data integration, keyed by the id the service minted for it —
    /// this resource's identity, which `Get`/`Delete` address it by. A `BTreeMap`
    /// for the same reason as above, and bounded at [`MAX_STORED_DATA_INTEGRATIONS`]
    /// entries by the create that writes it ([`Self::has_room_for_another_data_integration`]),
    /// so a caller cannot grow it without end.
    pub data_integrations: BTreeMap<String, DataIntegrationRecord>,
    /// `ClientToken` -> data integration id, the `data_integrations` counterpart of
    /// [`Self::idempotency`]. A separate map because the two tokens address
    /// different resources: one token namespace per resource, so a token used on a
    /// `CreateEventIntegration` cannot resolve to a data integration. Bounded at
    /// [`MAX_REMEMBERED_CLIENT_TOKENS`] by [`Self::remember_client_token`].
    pub data_integration_idempotency: BTreeMap<String, String>,
    /// The association ids a data integration has, keyed by the integration's id —
    /// the association store `CreateDataIntegrationAssociation` writes into once it
    /// is implemented (it answers HTTP 501 today, so nothing can add an entry yet).
    /// `DeleteDataIntegration` reads it, and drops the entry with the integration, so
    /// it holds at most one entry per stored integration and is bounded by the same
    /// ceiling. AWS deletes only a DataIntegration that has no
    /// DataIntegrationAssociations.
    pub data_integration_associations: BTreeMap<String, BTreeSet<String>>,
}

impl ComAmazonawsAppintegrationsAmazonappintegrationserviceState {
    /// The id of the data integration `identifier` names, or `None` when this account
    /// holds none.
    ///
    /// `Identifier` is documented as "a unique identifier for the DataIntegration"
    /// (`smithy_query(query="doc", shape="GetDataIntegrationRequest",
    /// member="Identifier")`) and is constrained only to a non-blank string, so both
    /// forms the service hands out address the resource: the `Id` it minted and the
    /// `Arn` built from that id. Nothing else resolves — not a name, because two
    /// resources could not then be told apart from one, and not an ARN this service
    /// did not mint, which is matched against the stored one in full so that a
    /// foreign ARN ending in the same id (another account, Region, vendor or resource
    /// type) names no resource here.
    pub fn data_integration_id(&self, identifier: &str) -> Option<String> {
        if self.data_integrations.contains_key(identifier) {
            return Some(identifier.to_string());
        }
        self.data_integrations
            .values()
            .find(|record| record.arn == identifier)
            .map(|record| record.id.clone())
    }

    /// Whether the store can hold another event integration, which is what
    /// `CreateEventIntegration` checks BEFORE it mints one: past
    /// [`MAX_STORED_EVENT_INTEGRATIONS`] it refuses with the quota error that operation
    /// models, for the same reason as [`Self::has_room_for_another_data_integration`].
    pub fn has_room_for_another_event_integration(&self) -> bool {
        self.event_integrations.len() < MAX_STORED_EVENT_INTEGRATIONS
    }

    /// Whether the store can hold another data integration, which is what
    /// `CreateDataIntegration` checks BEFORE it mints one: past
    /// [`MAX_STORED_DATA_INTEGRATIONS`] it refuses with the quota error the operation
    /// models, so this in-memory store cannot be grown without end one request at a
    /// time. A caller's delete frees a slot again.
    pub fn has_room_for_another_data_integration(&self) -> bool {
        self.data_integrations.len() < MAX_STORED_DATA_INTEGRATIONS
    }

    /// Remember that the create carrying `token` minted the data integration `id`, so a
    /// retry under that token answers with the resource it already made.
    ///
    /// Bounded, and it refuses nothing: entries whose integration is no longer stored
    /// are dropped first (they can only resolve to nothing), and past
    /// [`MAX_REMEMBERED_CLIENT_TOKENS`] the token is simply not remembered — a
    /// forgotten token makes a replay create a fresh resource, which
    /// [`Self::has_room_for_another_data_integration`] already bounds.
    pub fn remember_client_token(&mut self, token: String, id: String) {
        if self.data_integration_idempotency.len() >= MAX_REMEMBERED_CLIENT_TOKENS {
            let Self {
                data_integrations,
                data_integration_idempotency,
                ..
            } = self;
            data_integration_idempotency
                .retain(|_, minted| data_integrations.contains_key(minted));
        }
        if self.data_integration_idempotency.len() < MAX_REMEMBERED_CLIENT_TOKENS {
            self.data_integration_idempotency.insert(token, id);
        }
    }

    /// Remember that the create carrying `token` minted the event integration `name`,
    /// so a retry under that token answers with the resource it already made.
    ///
    /// Bounded exactly as [`Self::remember_client_token`] is, and for the same reason:
    /// the key is caller-supplied (`ClientToken`, `len 1..2048` — the length the
    /// boundary already enforces), so the index it is written into cannot be allowed to
    /// grow one request at a time. Entries whose integration is no longer stored are
    /// dropped first, and past [`MAX_REMEMBERED_EVENT_CLIENT_TOKENS`] the token is
    /// simply not remembered rather than the create refused.
    pub fn remember_event_client_token(&mut self, token: String, name: String) {
        if self.idempotency.len() >= MAX_REMEMBERED_EVENT_CLIENT_TOKENS {
            let Self {
                event_integrations,
                idempotency,
                ..
            } = self;
            idempotency.retain(|_, minted| event_integrations.contains_key(minted));
        }
        if self.idempotency.len() < MAX_REMEMBERED_EVENT_CLIENT_TOKENS {
            self.idempotency.insert(token, name);
        }
    }
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

/// The ARN of a data integration in `region`:
/// `arn:aws:app-integrations:<region>:<account>:data-integration/<id>`.
///
/// Built from the minted `Id`, not from the caller's `Name`: `Id` is what this
/// resource's identity is, and the ARN is the other form of it.
pub fn data_integration_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("data-integration/{id}"))
}

/// The pagination cursor for a page that starts at the store key `name` (an event
/// integration's name, a data integration's id).
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

#[cfg(test)]
mod tests {
    //! The bound a wire test cannot see: an idempotency index never grows past its
    //! ceiling, however many distinct `ClientToken`s a caller cycles through.

    use super::*;

    /// RULE: remembering a token is bounded. Every entry a caller can add is for a
    /// resource it created; once the index is full of entries whose resource is gone,
    /// those are dropped, and a token is never remembered past the ceiling — so a
    /// create/delete loop carrying a fresh token each time cannot grow the index
    /// without end. Both indexes, because both keys are caller-supplied.
    #[test]
    fn an_idempotency_index_never_grows_past_its_ceiling() {
        let cycles = MAX_REMEMBERED_CLIENT_TOKENS + 25;
        let mut state = ComAmazonawsAppintegrationsAmazonappintegrationserviceState::default();
        for cycle in 0..cycles {
            // A create/delete loop: the record is never there by the time the next
            // token arrives, which is the shape that would grow an unbounded index.
            state.remember_client_token(format!("data-token-{cycle}"), format!("id-{cycle}"));
            state.remember_event_client_token(
                format!("event-token-{cycle}"),
                format!("name-{cycle}"),
            );
            assert!(
                state.data_integration_idempotency.len() <= MAX_REMEMBERED_CLIENT_TOKENS,
                "case data cycle {cycle}: expected at most {MAX_REMEMBERED_CLIENT_TOKENS} \
                 remembered tokens, got {}",
                state.data_integration_idempotency.len()
            );
            assert!(
                state.idempotency.len() <= MAX_REMEMBERED_EVENT_CLIENT_TOKENS,
                "case event cycle {cycle}: expected at most \
                 {MAX_REMEMBERED_EVENT_CLIENT_TOKENS} remembered tokens, got {}",
                state.idempotency.len()
            );
        }
    }

    /// RULE: a token the index has room for is remembered, so bounding it does not stop
    /// idempotency working. Rows differ in the token's length, including the modelled
    /// maximum (`ClientToken`, `len 1..2048`).
    #[test]
    fn a_token_the_index_has_room_for_is_remembered() {
        let mut state = ComAmazonawsAppintegrationsAmazonappintegrationserviceState::default();
        for (case, token, id) in [
            ("a short token", "t1".to_string(), "id-1"),
            ("a longer token", "retry-token-0001".to_string(), "id-2"),
            (
                "a token at the modelled maximum length",
                "t".repeat(2048),
                "id-3",
            ),
        ] {
            state.remember_client_token(token.clone(), id.to_string());
            assert_eq!(
                state
                    .data_integration_idempotency
                    .get(&token)
                    .map(String::as_str),
                Some(id),
                "case {case:?}: expected the token to resolve to {id}, got {:?}",
                state.data_integration_idempotency.get(&token)
            );
        }
    }
}
