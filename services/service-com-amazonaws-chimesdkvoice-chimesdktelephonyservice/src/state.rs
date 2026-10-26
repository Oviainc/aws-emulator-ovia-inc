// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-chimesdkvoice-chimesdktelephonyservice service.
//!
//! The Voice Connector is this service's primary resource: `CreateVoiceConnector`,
//! `GetVoiceConnector`, `ListVoiceConnectors`, `UpdateVoiceConnector` and
//! `DeleteVoiceConnector` all read and write the one [`VoiceConnectorRecord`] map held
//! here, so every one of them answers from persisted state rather than from its own
//! request. The helpers beside the map (id minting, the derived outbound host name and
//! ARN, the record -> wire-shape projection and the opaque pagination token) live here
//! for the same reason: five operation files share them, and a second copy of any of
//! them would let two operations disagree about one resource.
//!
//! One thing this service reads is NOT in the struct below, because the other Chime
//! services read and write it too and the engine keys state by type: the family's
//! ARN-keyed TAG directory ([`chime_tags`]), which holds the tags of every Voice
//! Connector — including the ones a create was given.

use std::collections::HashMap;

use ::aws_smithy_types::DateTime;

use crate::ops::types::{
    BadRequestException, NotFoundException, ResourceLimitExceededException,
    ServiceFailureException, VoiceConnector,
};

/// The Region a Voice Connector is created in when the request names none.
///
/// From the model: `VoiceConnector$AwsRegion` documents "Default: us-east-1", and
/// `VoiceConnectorAwsRegion` enumerates the ten Regions the member accepts.
pub const DEFAULT_AWS_REGION: &str = "us-east-1";

/// Suffix of the outbound host name derived for a Voice Connector:
/// `<VoiceConnectorId>.voiceconnector.chime.aws`.
pub const OUTBOUND_HOST_SUFFIX: &str = "voiceconnector.chime.aws";

/// The `arnNamespace` this service's ARNs carry (`OPERATIONS.md`, `## Wire protocol`).
pub const ARN_NAMESPACE: &str = "chime";

/// The resource TYPE of a Voice Connector's ARN: `arn:aws:chime:<region>:<account>:vc/<id>`,
/// as [`VoiceConnectorRecord::new`] derives it. It is the one resource type this service
/// mints, so it is the one an ARN has to carry for this store to say whether the resource
/// exists.
pub const VOICE_CONNECTOR_RESOURCE_TYPE: &str = "vc";

/// How many tags one resource of THIS service carries at once: the bound its own model
/// declares on `TagList` (`@length(min: 1, max: 50)`,
/// `smithy_query(query="operation",operation="TagResource")`), which is the family's
/// usual one — so the shared directory's default bound is this service's bound too.
pub const MAX_TAGS_PER_RESOURCE: usize = ::service_common::chime_tags::MAX_TAGS_PER_RESOURCE;

/// Page size `ListVoiceConnectors` uses when the request names no `MaxResults`: the
/// maximum the model allows that member (`ResultMax`, range 1..100).
pub const MAX_LIST_PAGE_SIZE: i32 = 100;

/// The most Voice Connectors this emulator holds at once.
///
/// `voice_connectors` grows by one record per `CreateVoiceConnector` request and shrinks
/// only on an explicit delete, so without a ceiling its entry COUNT — and with it the
/// per-request ordering `ListVoiceConnectors` computes — is set by how many requests a
/// caller makes. AWS bounds the same thing with a per-account Voice Connector service
/// quota and refuses a create past it; the quota's value is adjustable and is not in the
/// model, so this number is the EMULATOR's own bound rather than a claim about AWS's,
/// while the refusal it produces is the error the model declares for exactly this case:
/// `ResourceLimitExceededException` (HTTP 400, "The request exceeds the resource limit.").
pub const MAX_VOICE_CONNECTORS: usize = 500;

// The failures the Voice Connector operations share.
//
// Each of the five handlers raises these through its OWN operation's error enum (the
// variants a model declares differ per operation), but the payload inside is the same
// refusal, and a payload built inline in five places drifts in wording. These
// constructors are the one place each payload's `Code` — a value of the model's
// `ErrorCode` enum — and its message are chosen.

/// `BadRequestException`'s payload: the request broke a rule the service enforces.
pub fn bad_request(message: impl Into<String>) -> BadRequestException {
    BadRequestException {
        code: Some("BadRequest".to_string()),
        message: Some(message.into()),
    }
}

/// `NotFoundException`'s payload for a well-formed id that names no stored connector.
pub fn voice_connector_not_found(voice_connector_id: &str) -> NotFoundException {
    NotFoundException {
        code: Some("NotFound".to_string()),
        message: Some(format!("no Voice Connector with id {voice_connector_id}")),
    }
}

/// `ServiceFailureException`'s payload for a state store that could not be reached —
/// the one case a handler cannot serve from state at all.
pub fn store_unavailable() -> ServiceFailureException {
    ServiceFailureException {
        code: Some("ServiceFailure".to_string()),
        message: Some("the Voice Connector store is unavailable".to_string()),
    }
}

/// `ResourceLimitExceededException`'s payload for a store already holding
/// [`MAX_VOICE_CONNECTORS`] connectors.
pub fn voice_connector_limit_reached() -> ResourceLimitExceededException {
    ResourceLimitExceededException {
        code: Some("ResourceLimitExceeded".to_string()),
        message: Some(format!(
            "this account already holds the maximum of {MAX_VOICE_CONNECTORS} Voice Connectors"
        )),
    }
}

/// `NotFoundException`'s payload for a tagging call whose `ResourceARN` names no resource
/// this service holds. Read by all three generic tagging operations, so the three cannot
/// word one refusal three ways.
pub fn tagged_resource_not_found(resource_arn: &str) -> NotFoundException {
    NotFoundException {
        code: Some("NotFound".to_string()),
        message: Some(format!("resource {resource_arn} does not exist")),
    }
}

/// `ResourceLimitExceededException`'s payload for a tagging call that would take a
/// resource past [`MAX_TAGS_PER_RESOURCE`] tags; `requested` is how many tags the
/// resource would have ended up with.
pub fn tag_limit_reached(resource_arn: &str, requested: usize) -> ResourceLimitExceededException {
    ResourceLimitExceededException {
        code: Some("ResourceLimitExceeded".to_string()),
        message: Some(format!(
            "{requested} tags were requested for {resource_arn}, which is more than the \
             maximum of {MAX_TAGS_PER_RESOURCE} tags per resource"
        )),
    }
}

/// One persisted Voice Connector.
///
/// Mirrors the model's `VoiceConnector` structure, but with the members the service
/// always knows held as non-optional: a stored connector always has an id, a name, a
/// Region, an outbound host name, an encryption flag, an ARN and both timestamps.
/// `IntegrationType`/`NetworkType` stay optional because the create request may omit
/// them. The connector's TAGS are not here: they live in the family's shared ARN-keyed
/// directory ([`chime_tags`]), which is what the generic tagging routes read and write.
#[derive(Debug, Clone)]
pub struct VoiceConnectorRecord {
    pub voice_connector_id: String,
    pub name: String,
    pub aws_region: String,
    pub outbound_host_name: String,
    pub require_encryption: bool,
    pub voice_connector_arn: String,
    pub integration_type: Option<String>,
    pub network_type: Option<String>,
    pub created_timestamp: DateTime,
    pub updated_timestamp: DateTime,
}

impl VoiceConnectorRecord {
    /// Build the record a `CreateVoiceConnector` request asks for. `outbound_host_name`
    /// and `voice_connector_arn` are DERIVED from the minted id (and the Region) rather
    /// than taken from the request, so every operation that returns this resource
    /// reports the same two values.
    pub fn new(
        voice_connector_id: String,
        name: String,
        aws_region: String,
        require_encryption: bool,
        created: DateTime,
    ) -> Self {
        let outbound_host_name = format!("{voice_connector_id}.{OUTBOUND_HOST_SUFFIX}");
        let voice_connector_arn = ::engine::emulator_arn(
            ARN_NAMESPACE,
            &aws_region,
            &format!("vc/{voice_connector_id}"),
        );
        Self {
            voice_connector_id,
            name,
            aws_region,
            outbound_host_name,
            require_encryption,
            voice_connector_arn,
            integration_type: None,
            network_type: None,
            created_timestamp: created,
            updated_timestamp: created,
        }
    }

    /// Project the stored record onto the model's `VoiceConnector` shape — the one
    /// conversion every operation that returns a connector goes through, so a create,
    /// a get, a list and an update cannot report the same resource differently.
    pub fn to_shape(&self) -> VoiceConnector {
        VoiceConnector {
            voice_connector_id: Some(self.voice_connector_id.clone()),
            aws_region: Some(self.aws_region.clone()),
            name: Some(self.name.clone()),
            outbound_host_name: Some(self.outbound_host_name.clone()),
            require_encryption: Some(self.require_encryption),
            created_timestamp: Some(self.created_timestamp),
            updated_timestamp: Some(self.updated_timestamp),
            voice_connector_arn: Some(self.voice_connector_arn.clone()),
            integration_type: self.integration_type.clone(),
            network_type: self.network_type.clone(),
        }
    }
}

/// Per-service state for com-amazonaws-chimesdkvoice-chimesdktelephonyservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsChimesdkvoiceChimesdktelephonyserviceState {
    /// Every Voice Connector that currently exists, keyed by its `VoiceConnectorId`.
    pub voice_connectors: HashMap<String, VoiceConnectorRecord>,
    /// Counter the minted ids are derived from. Only ever used by
    /// [`Self::mint_voice_connector_id`]; it is NOT an id itself.
    pub next_connector_seq: u64,
}

impl ComAmazonawsChimesdkvoiceChimesdktelephonyserviceState {
    /// Mint a fresh `VoiceConnectorId` that no stored connector holds.
    ///
    /// The model constrains the id to `^([a-z0-9]{21,22}|<uuid>)$`, so the id is the
    /// first 21 lowercase-hex characters of a digest of the counter: inside the
    /// character class, the modelled length, and distinct for every counter value. The
    /// caller must insert the record under the returned id while still holding the same
    /// state lock, or the id it returned names nothing.
    pub fn mint_voice_connector_id(&mut self) -> String {
        loop {
            self.next_connector_seq += 1;
            let candidate = voice_connector_id_for(self.next_connector_seq);
            if !self.voice_connectors.contains_key(&candidate) {
                return candidate;
            }
        }
    }

    /// Every stored id in the one stable order `ListVoiceConnectors` pages over.
    ///
    /// Sorted by id, which is a total order over the keys themselves: it needs no second
    /// structure beside `voice_connectors` to stay in step with it, so a page boundary
    /// cannot drift from the map it indexes.
    pub fn voice_connector_ids_in_page_order(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.voice_connectors.keys().cloned().collect();
        ids.sort();
        ids
    }

    /// Whether a tagging call may address `arn`.
    ///
    /// The family's one rule for that question lives in
    /// [`::service_common::chime_tags::addresses_taggable_resource`]; this store answers
    /// the half of it only this service can — whether an ARN that names a resource THIS
    /// service mints (one of type [`VOICE_CONNECTOR_RESOURCE_TYPE`]) is one it still
    /// holds. An ARN of any other Chime resource type is another family member's, which
    /// this store cannot check.
    pub fn taggable(&self, arn: &str) -> bool {
        let own_resource_exists = ::service_common::arn::resource_id_of_type(
            arn,
            VOICE_CONNECTOR_RESOURCE_TYPE,
        )
        .map(|id| self.voice_connectors.contains_key(id));
        ::service_common::chime_tags::addresses_taggable_resource(arn, own_resource_exists)
    }
}

/// The id derived from one counter value: 21 lowercase hex characters, which satisfies
/// the modelled `^[a-z0-9]{21,22}$` alternative.
fn voice_connector_id_for(seq: u64) -> String {
    use sha2::{Digest as _, Sha256};

    let digest = Sha256::digest(format!("chime-sdk-voice/voice-connector/{seq}").as_bytes());
    let mut id = hex::encode(digest);
    id.truncate(21);
    id
}

/// The wall-clock instant to stamp on a record being created or updated.
pub fn current_timestamp() -> DateTime {
    let since_epoch = ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .unwrap_or_default();
    DateTime::from_secs_f64(since_epoch.as_secs_f64())
}

/// Prefix carried inside a pagination token, so a token from somewhere else is rejected
/// rather than read as an id.
const PAGE_TOKEN_PREFIX: &str = "voice-connector:";

/// Encode the opaque `NextToken` that resumes a listing after `last_id`.
///
/// Opaque to the caller (hex of a tagged byte string) because the model gives `NextToken`
/// no structure: a client must pass back what it was handed, not construct one.
pub fn encode_page_token(last_id: &str) -> String {
    hex::encode(format!("{PAGE_TOKEN_PREFIX}{last_id}"))
}

/// The id a page token resumes after, or `None` when the token was not one this service
/// issued.
pub fn decode_page_token(token: &str) -> Option<String> {
    let bytes = hex::decode(token).ok()?;
    let text = String::from_utf8(bytes).ok()?;
    text.strip_prefix(PAGE_TOKEN_PREFIX).map(str::to_string)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsChimesdkvoiceChimesdktelephonyserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsChimesdkvoiceChimesdktelephonyserviceState>)
}

/// The SHARED Chime TAG directory in the shared engine: call it with a handler's
/// `&*self.engine`.
///
/// A resource's tags are keyed by ARN and live in ONE directory
/// ([`::service_common::chime_tags::ChimeTagDirectory`]) rather than on the
/// [`VoiceConnectorRecord`]s of this state struct, because the family's generic tagging
/// routes are served by one member for every member's resource: a copy inside
/// [`ComAmazonawsChimesdkvoiceChimesdktelephonyserviceState`] would be invisible to the
/// sibling serving a tagging call, which would then report "no tags" for a connector that
/// has them.
///
/// LOCK ORDER: a writer takes [`service_state`] first and this directory second, and no
/// reader holds this one while taking another store, so the order cannot invert.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn chime_tags<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<::service_common::chime_tags::ChimeTagDirectory>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<::service_common::chime_tags::ChimeTagDirectory>)
}
