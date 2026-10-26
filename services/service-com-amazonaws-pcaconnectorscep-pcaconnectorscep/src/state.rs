// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-pcaconnectorscep-pcaconnectorscep service.
//!
//! The service's resources are the SCEP **connector** and the **challenge** passwords that
//! hang off a general-purpose connector. Every implemented operation computes its answer
//! from the records below: nothing is canned, nothing is echoed straight back out of a
//! request without being stored first, and a delete is observable by the next reader.

use std::collections::BTreeMap;
use std::hash::{BuildHasher as _, Hasher as _};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ::aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeSerializer};
use ::aws_smithy_schema::{Schema, ShapeId, ShapeType, TraitMap};
use ::aws_smithy_schema_server::constraints::{validate, LengthTrait, PatternTrait};
use ::aws_smithy_schema_server::envelope;

use crate::ops::types::{
    Challenge, ChallengeMetadata, ChallengeMetadataSummary, Connector, ConnectorSummary,
    MobileDeviceManagement, OpenIdConfiguration,
};

/// The signing name, endpoint prefix and ARN namespace of this service, from the model's
/// `aws.auth#sigv4` / `aws.api#service` traits (all three are `pca-connector-scep`).
pub const ARN_SERVICE: &str = "pca-connector-scep";

/// Region used when a request carries no SigV4 credential scope to read one from (an
/// unsigned request still routes by endpoint host).
pub const DEFAULT_REGION: &str = "us-east-1";

/// `ConnectorStatus` value a freshly created connector carries. The full enum
/// (smithy_query: shape ConnectorStatus) is CREATING, ACTIVE, DELETING, FAILED.
pub const STATUS_CREATING: &str = "CREATING";
/// The ready state: a connector whose SCEP endpoint serves and that can mint challenges.
pub const STATUS_ACTIVE: &str = "ACTIVE";
/// `ConnectorStatus` value a connector carries while `DeleteConnector`'s teardown runs: the
/// delete answers 202 and only accepts the work, so the connector is reported in this status
/// until the teardown completes and the record (with its challenges) disappears.
pub const STATUS_DELETING: &str = "DELETING";

/// `ConnectorType` value of a connector created without `MobileDeviceManagement` — the
/// member's own doc: "If you don't supply a value, by default Connector for SCEP creates a
/// connector for general-purpose use."
pub const TYPE_GENERAL_PURPOSE: &str = "GENERAL_PURPOSE";
/// `ConnectorType` value of a connector created with an `IntuneConfiguration`.
pub const TYPE_INTUNE: &str = "INTUNE";

/// `ResourceId`/`ResourceType` vocabulary of this service's errors: the `ResourceType`
/// member's doc says it "can be either `Connector` or `Challenge`".
pub const RESOURCE_TYPE_CONNECTOR: &str = "Connector";
/// The other value `ResourceType` carries.
pub const RESOURCE_TYPE_CHALLENGE: &str = "Challenge";

/// `ValidationExceptionReason` for a call that names a connector whose status does not
/// allow it (smithy_query: shape ValidationExceptionReason).
pub const REASON_INVALID_STATE: &str = "INVALID_STATE";
/// `ValidationExceptionReason` for a call that does not apply to the connector's `Type`.
pub const REASON_INVALID_CONNECTOR_TYPE: &str = "INVALID_CONNECTOR_TYPE";

/// How long a connector stays in a non-terminal status before the clock settles it:
/// `CREATING` before it owes `ACTIVE`, and `DELETING` before the record disappears.
///
/// `CreateConnector` and `DeleteConnector` both answer HTTP 202 and the connector's own
/// `Status` enum carries `CREATING` and `DELETING`, so both are asynchronous: a resource the
/// service reports in a non-terminal status has a transition the clock owes. The span is the
/// emulator's own choice (no AWS-documented duration applies to connector provisioning or
/// teardown) and is deliberately non-zero, so a caller reading straight after the call still
/// observes the transient status — and the state errors that go with it — as it does against
/// the real service. The instant it comes due is stored on the record
/// ([`PendingTransition::due_at`]) and materialised by [`StoredConnector::settle`];
/// nothing sleeps.
pub const TRANSITION_SPAN: Duration = Duration::from_millis(500);

/// How long a `ClientToken` stays bound to the resource it created.
///
/// Documented on the member: "Client tokens for CreateChallenge time out after five
/// minutes. Therefore, if you call CreateChallenge multiple times with the same client
/// token within five minutes, Connector for SCEP recognizes that you are requesting only
/// one challenge and will only respond with one."
pub const CLIENT_TOKEN_TTL: Duration = Duration::from_secs(5 * 60);

/// `MaxResults` upper bound on both listings, from their `@range(1, 1000)`. Also the page
/// size used when a caller names none.
pub const MAX_RESULTS_LIMIT: i32 = 1000;

/// How many connectors this emulator will hold for one account in one Region.
///
/// The real service caps connectors per account per Region with a service quota, and
/// `CreateConnector` models `ServiceQuotaExceededException` for exceeding it. The number
/// here is NOT a fidelity claim about the published quota: it is the emulator's own
/// per-Region ceiling, set generously so no realistic workflow meets it.
pub const MAX_CONNECTORS: usize = 1_000;

/// How many connectors this emulator will hold AT ALL, across every Region.
///
/// The Region a connector is filed under comes from the request ([`request_region`]), so the
/// per-Region ceiling above bounds one Region's inventory but not the number of Regions a
/// caller addresses. This bounds the product, and with it the memory a caller looping on
/// create can make this process hold: it is the ceiling that keeps that growth finite, and
/// it is reached only by a caller inventing Regions, never by a workflow.
pub const MAX_CONNECTORS_TOTAL: usize = 4 * MAX_CONNECTORS;

/// How long a Region name this service will file a record under may be.
///
/// Neither ARN pattern bounds the length of its Region segment, so this is the emulator's
/// own bound on a value the request chooses, well above every published Region name.
pub const MAX_REGION_LENGTH: usize = 64;

/// The same ceiling for the challenge passwords of one connector, for the same reason.
pub const MAX_CHALLENGES_PER_CONNECTOR: usize = 1_000;

/// How many tags one resource may carry.
///
/// `Tags` carries no `@length`, so the model states no bound; AWS's tagging limit of 50 tags
/// per resource is what this enforces, which is also what keeps a caller from making one
/// create call cost unbounded memory.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// How long a tag key may be. `Tags` carries no `@length` on its key, so the bound is the
/// AWS-wide tagging one: a key is 1 to 128 characters.
pub const MAX_TAG_KEY_LENGTH: usize = 128;

/// How long a tag value may be: 0 to 256 characters, the AWS-wide bound. An EMPTY value is
/// a tag a caller may set, which is why only the key has a lower bound.
pub const MAX_TAG_VALUE_LENGTH: usize = 256;

/// The tag-key namespace Amazon Web Services keeps for the tags it sets itself, which a
/// caller may therefore not write. Matched without regard to case, so no spelling of it
/// gets a caller-set tag into the namespace.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// `ServiceCode` carried by this service's `ServiceQuotaExceededException`: the originating
/// service, which is this service's own endpoint prefix.
pub const SERVICE_CODE: &str = ARN_SERVICE;

/// `QuotaCode` for [`MAX_CONNECTORS`]. AWS's own `L-`code for the limit is not in the
/// model, so the emulator names the limit it enforces rather than inventing a code.
pub const QUOTA_CODE_CONNECTORS: &str = "ConnectorsPerAccountPerRegion";

/// `QuotaCode` for [`MAX_CHALLENGES_PER_CONNECTOR`], named for the same reason.
pub const QUOTA_CODE_CHALLENGES: &str = "ChallengesPerConnector";

/// The separator between a connector ARN and the challenge segment of a challenge ARN, from
/// `ChallengeArn`'s own pattern (`…:connector/<uuid>/challenge/<uuid>$`).
const CHALLENGE_SEGMENT: &str = "/challenge/";

/// A status change the clock owes a connector: where it lands, and when.
#[derive(Debug, Clone)]
pub struct PendingTransition {
    /// What the clock owes this connector.
    pub outcome: PendingOutcome,
    /// The instant the transition comes due.
    pub due_at: SystemTime,
}

/// Where a pending transition leaves the connector.
///
/// A connector in a non-terminal `ConnectorStatus` either settles into another status
/// (`CREATING` -> `ACTIVE`) or ceases to exist (`DELETING` -> gone, taking its challenges
/// with it), so those are the only two outcomes the clock can owe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PendingOutcome {
    /// The connector settles into this `ConnectorStatus` value.
    Status(String),
    /// The teardown completes: the record, its challenges and its tags are gone.
    Removed,
}

/// A `ClientToken` and the resource the call carrying it created.
#[derive(Debug, Clone)]
pub struct IdempotencyRecord {
    /// ARN of the connector or challenge that token created.
    pub arn: String,
    /// When the token was first seen; it stops binding [`CLIENT_TOKEN_TTL`] later.
    pub created_at: SystemTime,
    /// A rendering of the resource-defining inputs the call carried, so a retry carrying
    /// the same token can be checked to be the SAME request: a token answers for the
    /// resource it created only when the inputs still match.
    pub request: String,
}

/// A stored challenge password: the modelled `Challenge`, as state.
#[derive(Debug, Clone)]
pub struct StoredChallenge {
    pub arn: String,
    pub connector_arn: String,
    /// The SCEP challenge password, in UUID format (the `Password` member's own doc). It is
    /// `@sensitive`, so only `CreateChallenge` and `GetChallengePassword` return it — never
    /// a metadata listing.
    pub password: String,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    /// The tags `CreateChallenge` was given. They live ON the record, so they are dropped
    /// with it and a deleted challenge can never leave a tag entry behind.
    pub tags: BTreeMap<String, String>,
}

impl StoredChallenge {
    /// The modelled `Challenge`, as `CreateChallenge` returns it — password included.
    pub fn to_challenge(&self) -> Challenge {
        Challenge {
            arn: Some(self.arn.clone()),
            connector_arn: Some(self.connector_arn.clone()),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            updated_at: Some(::aws_smithy_types::DateTime::from(self.updated_at)),
            password: Some(self.password.clone()),
        }
    }

    /// The modelled `ChallengeMetadataSummary`, as `ListChallengeMetadata` returns it.
    /// `ChallengeMetadataSummary` models no password member, so metadata never carries one.
    pub fn to_metadata_summary(&self) -> ChallengeMetadataSummary {
        ChallengeMetadataSummary {
            arn: Some(self.arn.clone()),
            connector_arn: Some(self.connector_arn.clone()),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            updated_at: Some(::aws_smithy_types::DateTime::from(self.updated_at)),
        }
    }

    /// The modelled `ChallengeMetadata`, as `GetChallengeMetadata` returns it. Its members
    /// are the same four `ChallengeMetadataSummary` carries, and neither shape models a
    /// password, so the single read and the listing report one record identically.
    pub fn to_metadata(&self) -> ChallengeMetadata {
        let summary = self.to_metadata_summary();
        ChallengeMetadata {
            arn: summary.arn,
            connector_arn: summary.connector_arn,
            created_at: summary.created_at,
            updated_at: summary.updated_at,
        }
    }
}

/// A stored connector: everything `CreateConnector` was given, plus what the service minted.
#[derive(Debug, Clone)]
pub struct StoredConnector {
    pub arn: String,
    pub certificate_authority_arn: String,
    /// A `ConnectorType` value: [`TYPE_GENERAL_PURPOSE`] or [`TYPE_INTUNE`].
    pub connector_type: String,
    /// Echo of the stored MDM settings; `None` for a general-purpose connector, whose
    /// `MobileDeviceManagement` the model documents as empty.
    pub mobile_device_management: Option<MobileDeviceManagement>,
    /// OIDC parameters, minted for an Intune connector only.
    pub open_id_configuration: Option<OpenIdConfiguration>,
    /// The VPC endpoint the connector is reachable through, when the create named one.
    pub vpc_endpoint_id: Option<String>,
    /// A `ConnectorStatus` value.
    pub status: String,
    /// A `ConnectorStatusReason` value — only set when `status` is `FAILED`, per the
    /// `StatusReason` member's doc.
    pub status_reason: Option<String>,
    /// The transition the clock owes, or `None` in a settled status.
    pub pending: Option<PendingTransition>,
    /// The connector's HTTPS SCEP URL — derived from the access path the create chose, so a
    /// connector restricted to one VPC endpoint reports a different host from a public one.
    pub endpoint: String,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    /// The tags `CreateConnector` was given. They live ON the record, so they are dropped
    /// with it and a deleted connector can never leave a tag entry behind.
    pub tags: BTreeMap<String, String>,
    /// This connector's challenge passwords, keyed by challenge ARN and ordered by it, so
    /// `ListChallengeMetadata` paginates deterministically.
    pub challenges: BTreeMap<String, StoredChallenge>,
}

impl StoredConnector {
    /// Materialise the transition the clock already owes this connector, answering whether
    /// the record survives it (`false` once a teardown has come due, so the caller drops it).
    ///
    /// This is the ONLY write a reporting operation makes: the change was owed at this
    /// instant whether or not anybody called.
    fn settle(&mut self, now: SystemTime) -> bool {
        let Some(pending) = self.pending.clone() else {
            return true;
        };
        if now < pending.due_at {
            return true;
        }
        match pending.outcome {
            PendingOutcome::Status(status) => {
                self.status = status;
                // The change happened when the clock made it due, not when someone looked.
                self.updated_at = pending.due_at.max(self.updated_at);
                self.status_reason = None;
                self.pending = None;
                true
            }
            // The teardown is complete, so there is no record left to report.
            PendingOutcome::Removed => false,
        }
    }

    /// Accept a delete: the connector moves into `DELETING` and the clock owes its removal.
    ///
    /// `DeleteConnector` answers 202 with an empty body — it only accepts the work — so this
    /// is correct whatever non-terminal status the connector is in: a pending `ACTIVE` is
    /// replaced by the teardown rather than being waited for. A connector already tearing
    /// down keeps the due instant it was given, so repeating the `@idempotent` delete can
    /// neither extend the teardown nor restart it.
    pub fn begin_deletion(&mut self, now: SystemTime) {
        if self.is_deleting() {
            return;
        }
        self.status = STATUS_DELETING.to_string();
        // `StatusReason` says why a connector FAILED; one being torn down carries none.
        self.status_reason = None;
        self.updated_at = now;
        self.pending = Some(PendingTransition {
            outcome: PendingOutcome::Removed,
            due_at: now + TRANSITION_SPAN,
        });
    }

    /// True while a transition is still owed — the connector is mid-flight.
    pub fn is_in_transition(&self) -> bool {
        self.pending.is_some()
    }

    /// True once the connector is in the one status that lets a caller use it.
    pub fn is_active(&self) -> bool {
        self.status == STATUS_ACTIVE
    }

    /// True while the connector's teardown is running: it is still reported, but nothing can
    /// be built on it and it is about to disappear.
    pub fn is_deleting(&self) -> bool {
        self.status == STATUS_DELETING
    }

    /// The modelled `Connector`, as `GetConnector` returns it.
    pub fn to_connector(&self) -> Connector {
        Connector {
            arn: Some(self.arn.clone()),
            certificate_authority_arn: Some(self.certificate_authority_arn.clone()),
            r#type: Some(self.connector_type.clone()),
            mobile_device_management: self.mobile_device_management.clone(),
            open_id_configuration: self.open_id_configuration.clone(),
            status: Some(self.status.clone()),
            status_reason: self.status_reason.clone(),
            endpoint: Some(self.endpoint.clone()),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            updated_at: Some(::aws_smithy_types::DateTime::from(self.updated_at)),
        }
    }

    /// The modelled `ConnectorSummary`, as `ListConnectors` returns it. It carries the same
    /// members as `Connector`, so a listing and a get cannot disagree.
    pub fn to_summary(&self) -> ConnectorSummary {
        ConnectorSummary {
            arn: Some(self.arn.clone()),
            certificate_authority_arn: Some(self.certificate_authority_arn.clone()),
            r#type: Some(self.connector_type.clone()),
            mobile_device_management: self.mobile_device_management.clone(),
            open_id_configuration: self.open_id_configuration.clone(),
            status: Some(self.status.clone()),
            status_reason: self.status_reason.clone(),
            endpoint: Some(self.endpoint.clone()),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            updated_at: Some(::aws_smithy_types::DateTime::from(self.updated_at)),
        }
    }
}

/// Per-service state for com-amazonaws-pcaconnectorscep-pcaconnectorscep. A handler reaches it through [`service_state`].
///
/// This service is regional: a resource exists in exactly one Region — the one named by its
/// own ARN — so every record below is reached through an accessor that takes the Region the
/// request was signed for ([`request_region`]) and answers only for records of that Region.
/// A handler never indexes the maps directly, which is what keeps one Region's endpoint from
/// reporting, resolving or deleting another Region's resources.
#[derive(Debug, Default)]
pub struct ComAmazonawsPcaconnectorscepPcaconnectorscepState {
    /// Every connector of every Region, keyed by its ARN and ordered by it, so a listing
    /// paginates deterministically. The ARN carries the Region the connector exists in, so a
    /// Region's own inventory is the entries whose ARN names it
    /// ([`ComAmazonawsPcaconnectorscepPcaconnectorscepState::connectors_in`]). A connector
    /// owns its challenges and both own their tags, so deleting a connector drops all three
    /// and leaves nothing behind.
    pub connectors: BTreeMap<String, StoredConnector>,
    /// `CreateConnector`'s (Region, `ClientToken`) -> the connector ARN it created. The
    /// Region is half the key because a token identifies a retry of a call made against ONE
    /// Region's endpoint: the same token sent to another Region asks for a connector there.
    pub connector_tokens: BTreeMap<(String, String), IdempotencyRecord>,
    /// `CreateChallenge`'s (Region, `ClientToken`) -> the challenge ARN it created.
    pub challenge_tokens: BTreeMap<(String, String), IdempotencyRecord>,
}

impl ComAmazonawsPcaconnectorscepPcaconnectorscepState {
    /// Materialise everything the clock already owes: each connector's status transition or
    /// completed teardown, and the expiry of every `ClientToken` past its five-minute window.
    ///
    /// Called at the top of each handler, reporting ones included: they may write only a
    /// change that was already due at this instant.
    pub fn settle(&mut self, now: SystemTime) {
        // A connector whose teardown has come due is gone, challenges and tags with it.
        self.connectors.retain(|_, connector| connector.settle(now));
        let live = |record: &IdempotencyRecord| record.created_at + CLIENT_TOKEN_TTL > now;
        self.connector_tokens.retain(|_, record| live(record));
        self.challenge_tokens.retain(|_, record| live(record));
    }

    /// The connectors that exist in `region`, in ARN order: the stored records whose ARN
    /// names that Region, which is the whole inventory a call signed for it can see.
    ///
    /// Every ARN this service mints starts with the same prefix for a given Region
    /// ([`connector_arn_prefix`]), and the map is keyed by ARN, so one Region's records are a
    /// contiguous range: this walks that range and stops at its end rather than scanning the
    /// other Regions' records.
    pub fn connectors_in<'a>(
        &'a self,
        region: &str,
    ) -> impl Iterator<Item = (&'a String, &'a StoredConnector)> {
        let prefix = connector_arn_prefix(region);
        self.connectors
            .range(prefix.clone()..)
            .take_while(move |(arn, _)| arn.starts_with(&prefix))
    }

    /// The connector `connector_arn` names, as a call signed for `region` sees it: `None`
    /// when no record is stored under that ARN, and equally when the ARN names a connector
    /// of another Region, because that connector does not exist here.
    pub fn connector(&self, region: &str, connector_arn: &str) -> Option<&StoredConnector> {
        self.connectors
            .get(connector_arn)
            .filter(|_| arn_region(connector_arn) == Some(region))
    }

    /// The mutable twin of [`Self::connector`], refusing a foreign-Region ARN the same way,
    /// so no call can change — or tear down — a resource of a Region it was not signed for.
    pub fn connector_mut(
        &mut self,
        region: &str,
        connector_arn: &str,
    ) -> Option<&mut StoredConnector> {
        if arn_region(connector_arn) != Some(region) {
            return None;
        }
        self.connectors.get_mut(connector_arn)
    }

    /// The challenge stored under `challenge_arn`, found through the connector its ARN names
    /// — so a challenge is reachable only in the Region that connector exists in.
    pub fn challenge(&self, region: &str, challenge_arn: &str) -> Option<&StoredChallenge> {
        let connector_arn = connector_arn_of_challenge(challenge_arn)?;
        self.connector(region, connector_arn)?
            .challenges
            .get(challenge_arn)
    }

    /// Remove the challenge `challenge_arn` names, answering whether it was there — in
    /// `region`, which is the only Region its owning connector exists in.
    pub fn remove_challenge(&mut self, region: &str, challenge_arn: &str) -> bool {
        let Some(connector_arn) = connector_arn_of_challenge(challenge_arn) else {
            return false;
        };
        self.connector_mut(region, connector_arn)
            .is_some_and(|connector| connector.challenges.remove(challenge_arn).is_some())
    }

    /// The tags of the connector or challenge `resource_arn` names, or `None` when this
    /// service holds no resource under that ARN.
    ///
    /// Which of the two kinds an ARN names follows from its shape ([`resource_type_of`]), and
    /// the tags come off that record — both kinds carry their own map — so a tag listing is
    /// computed from the resource itself and disappears with it.
    pub fn tags_of(
        &self,
        region: &str,
        resource_arn: &str,
    ) -> Option<&BTreeMap<String, String>> {
        if connector_arn_of_challenge(resource_arn).is_some() {
            self.challenge(region, resource_arn)
                .map(|challenge| &challenge.tags)
        } else {
            self.connector(region, resource_arn)
                .map(|connector| &connector.tags)
        }
    }

    /// The tags of the connector or challenge `resource_arn` names, to WRITE, or `None` when
    /// this service holds no resource under that ARN.
    ///
    /// The mutable twin of [`tags_of`], reaching the same single map on the same record, so a
    /// tag written through here is the one a listing reads back and is dropped with the
    /// resource rather than lingering in a second table.
    pub fn tags_of_mut(
        &mut self,
        region: &str,
        resource_arn: &str,
    ) -> Option<&mut BTreeMap<String, String>> {
        match connector_arn_of_challenge(resource_arn) {
            Some(connector_arn) => self
                .connector_mut(region, connector_arn)?
                .challenges
                .get_mut(resource_arn)
                .map(|challenge| &mut challenge.tags),
            None => self
                .connector_mut(region, resource_arn)
                .map(|connector| &mut connector.tags),
        }
    }

    /// The record a still-live `ClientToken` of `CreateConnector`, sent to `region`, is
    /// bound to.
    pub fn connector_for_token(
        &self,
        region: &str,
        client_token: Option<&str>,
    ) -> Option<IdempotencyRecord> {
        client_token
            .and_then(|token| self.connector_tokens.get(&token_key(region, token)))
            .cloned()
    }

    /// The record a still-live `ClientToken` of `CreateChallenge`, sent to `region`, is
    /// bound to.
    pub fn challenge_for_token(
        &self,
        region: &str,
        client_token: Option<&str>,
    ) -> Option<IdempotencyRecord> {
        client_token
            .and_then(|token| self.challenge_tokens.get(&token_key(region, token)))
            .cloned()
    }
}

/// The key a `ClientToken` seen in `region` is remembered under, so one token stands for one
/// call against one Region's endpoint.
pub fn token_key(region: &str, client_token: &str) -> (String, String) {
    (region.to_string(), client_token.to_string())
}

/// The Region segment of an ARN: field four of
/// `arn:<partition>:<service>:<region>:<account>:<resource>`, which is where both
/// `ConnectorArn`'s and `ChallengeArn`'s own patterns put it. `None` for a string that
/// carries no non-empty Region segment there.
pub fn arn_region(arn: &str) -> Option<&str> {
    arn.split(':').nth(3).filter(|region| !region.is_empty())
}

/// Which of this service's two resource kinds an ARN names, as the `ResourceType` member of
/// its errors spells it ("either `Connector` or `Challenge`").
///
/// Decided by the ARN's own shape: a challenge ARN is its connector's ARN plus the challenge
/// segment ([`connector_arn_of_challenge`]), so anything without that segment names a
/// connector.
pub fn resource_type_of(resource_arn: &str) -> &'static str {
    if connector_arn_of_challenge(resource_arn).is_some() {
        RESOURCE_TYPE_CHALLENGE
    } else {
        RESOURCE_TYPE_CONNECTOR
    }
}

/// The form a `ResourceArn` naming one of this service's resources takes: `ConnectorArn`'s
/// own `@pattern` with `ChallengeArn`'s challenge segment appended OPTIONALLY.
///
/// One expression covers both kinds because the challenge pattern IS the connector pattern
/// plus that segment (smithy_query: shape ConnectorArn, ChallengeArn), so the two cannot
/// drift apart here. Written exactly as the model writes them, because it is also the
/// requirement a refusal quotes back.
pub const RESOURCE_ARN_PATTERN: &str = concat!(
    r"^arn:aws(-[a-z]+)*:pca-connector-scep:[a-z]+(-[a-z]+)+-[1-9]\d*:\d{12}",
    r":connector\/[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}",
    r"(\/challenge\/[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12})?$"
);

/// Shortest a resource ARN of this service may be, from the `@length(5, 200)` that both
/// `ConnectorArn` and `ChallengeArn` declare.
pub const MIN_RESOURCE_ARN_LENGTH: i64 = 5;
/// Longest a resource ARN of this service may be, from the same `@length`.
pub const MAX_RESOURCE_ARN_LENGTH: i64 = 200;

/// `ResourceArn`'s constraints: the `@length` above and [`RESOURCE_ARN_PATTERN`], as the
/// trait values the framework's own constraint walk reads.
static RESOURCE_ARN_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
    let mut traits = TraitMap::new();
    traits.insert(Box::new(LengthTrait::new(
        Some(MIN_RESOURCE_ARN_LENGTH),
        Some(MAX_RESOURCE_ARN_LENGTH),
    )));
    traits.insert(Box::new(PatternTrait::new(RESOURCE_ARN_PATTERN)));
    traits
});

/// The `resourceArn` member schema that walk is driven over: its name gives the violation
/// path, in the lowerCamel spelling every clause uses, and its trait map carries the
/// constraints.
static M_RESOURCE_ARN: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.pcaconnectorscep#TaggedResource$resourceArn",
        "com.amazonaws.pcaconnectorscep",
        "resourceArn",
    ),
    ShapeType::String,
    "resourceArn",
    0,
)
.with_traits(&RESOURCE_ARN_TRAITS);

/// The root of the shadow input the walk runs over. Its member list is empty on purpose: the
/// walk visits what [`TaggedResource`] WRITES, and a root struct schema carries no member
/// name, so it contributes no path segment.
static TAGGED_RESOURCE_SCHEMA: Schema = Schema::new_struct(
    ShapeId::from_static(
        "com.amazonaws.pcaconnectorscep#TaggedResource",
        "com.amazonaws.pcaconnectorscep",
        "TaggedResource",
    ),
    ShapeType::Structure,
    &[],
);

/// One request's `ResourceArn`, written for the validating walk the way the codec writes a
/// string member.
struct TaggedResource<'a> {
    resource_arn: &'a str,
}

impl SerializableStruct for TaggedResource<'_> {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        s.write_string(&M_RESOURCE_ARN, self.resource_arn)
    }
}

/// How many fields an ARN is built from: `arn:<partition>:<service>:<region>:<account>:<resource>`.
const ARN_FIELD_COUNT: usize = 6;

/// Whether `value` is a well-formed ARN naming a resource of some service OTHER than this
/// one.
///
/// An ARN is six colon-separated fields — `arn:<partition>:<service>:<region>:<account-id>:<resource>`
/// — of which the literal `arn`, the partition, the service and the resource are always
/// filled; a region and an account id are empty in the ARNs of global and account-less
/// resources, so neither is required here. The resource field keeps any further colons,
/// because a resource id may carry them.
///
/// Such a value DOES name a resource, so a call naming it is a lookup that finds nothing
/// rather than a request this service cannot parse.
fn names_another_services_resource(value: &str) -> bool {
    let fields: Vec<&str> = value.splitn(ARN_FIELD_COUNT, ':').collect();
    let [prefix, partition, service, _region, _account, resource] = fields.as_slice() else {
        return false;
    };
    *prefix == "arn"
        && !partition.is_empty()
        && !service.is_empty()
        && *service != ARN_SERVICE
        && !resource.is_empty()
}

/// Why `resource_arn` is not a resource ARN this service could hold anything under, stated
/// the way the request boundary states a path label outside its own constraints, or `None`
/// when it is a value to look a resource up by.
///
/// The tagging operations take `ResourceArn` as a bare `String` with no `@pattern` and no
/// `@length` (smithy_query: operation ListTagsForResource / TagResource / UntagResource), so
/// nothing is refused for them at the request boundary and this is the only place the check
/// can live — while the same ARNs reached as `ConnectorArn` or `ChallengeArn` labels are
/// refused there, against the very patterns [`RESOURCE_ARN_PATTERN`] is built from.
///
/// Two things are therefore separated. A value that could not name a resource at all, and an
/// ARN of THIS service that is not one of its published resource ARNs, are input the service
/// cannot parse. An ARN of another service is well formed and simply holds nothing here, so
/// it is left to the lookup to miss.
///
/// The check and the wording are both the FRAMEWORK's: the constraint walk
/// ([`aws_smithy_schema_server::constraints::validate`]) runs over the member schema above
/// and renders one clause per constraint broken, length before pattern, in its
/// `N validation error(s) detected` envelope with its own elision of the echoed value — so a
/// string refused here is refused and worded exactly as the boundary refuses the same string
/// on a patterned label.
pub fn resource_arn_rejection(resource_arn: &str) -> Option<String> {
    if names_another_services_resource(resource_arn) {
        return None;
    }
    let violations = validate(&TaggedResource { resource_arn }, &TAGGED_RESOURCE_SCHEMA);
    envelope(&violations)
}

/// Whether `character` is one a tag key or a tag value may carry.
///
/// AWS's tag alphabet is letters, numbers, spaces and the characters `_ . : / = + - @`, and
/// it is not restricted to ASCII: "letters" and "numbers" of any script are allowed, which
/// is what [`char::is_alphanumeric`] answers. "Spaces" are the Unicode separators — a tab or
/// a newline is a control character rather than a space, and [`char::is_whitespace`] answers
/// for those too, so they are excluded here.
fn is_tag_character(character: char) -> bool {
    character.is_alphanumeric()
        || (character.is_whitespace() && !character.is_control())
        || matches!(character, '_' | '.' | ':' | '/' | '=' | '+' | '-' | '@')
}

/// Why this one tag may not be stored, as the sentence to refuse the request with, or `None`
/// when the pair is one a caller may set.
///
/// The key is checked before the value, and each half's length before its characters, so the
/// text a message quotes back is always within the bound the quote came from.
pub fn tag_entry_rejection(key: &str, value: &str) -> Option<String> {
    let key_length = key.chars().count();
    if key_length == 0 {
        return Some(format!(
            "Tags carries an empty tag key; a tag key is 1 to {MAX_TAG_KEY_LENGTH} characters"
        ));
    }
    if key_length > MAX_TAG_KEY_LENGTH {
        return Some(format!(
            "Tags carries a tag key of {key_length} characters; a tag key is 1 to \
             {MAX_TAG_KEY_LENGTH} characters"
        ));
    }
    if let Some(character) = key.chars().find(|character| !is_tag_character(*character)) {
        return Some(format!(
            "the key of tag `{key}` carries {character:?}; a tag key may use only letters, \
             numbers, spaces and the characters _ . : / = + - @"
        ));
    }
    if key
        .get(..RESERVED_TAG_KEY_PREFIX.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(RESERVED_TAG_KEY_PREFIX))
    {
        return Some(format!(
            "tag key `{key}` uses the `{RESERVED_TAG_KEY_PREFIX}` prefix, which is reserved \
             for the tags Amazon Web Services sets itself"
        ));
    }
    let value_length = value.chars().count();
    if value_length > MAX_TAG_VALUE_LENGTH {
        return Some(format!(
            "the value of tag `{key}` is {value_length} characters; a tag value is at most \
             {MAX_TAG_VALUE_LENGTH} characters"
        ));
    }
    if let Some(character) = value.chars().find(|character| !is_tag_character(*character)) {
        return Some(format!(
            "the value of tag `{key}` carries {character:?}; a tag value may use only letters, \
             numbers, spaces and the characters _ . : / = + - @"
        ));
    }
    None
}

/// Why the `Tags` of a request may not be stored, or `None` when every entry is one a caller
/// may set.
///
/// `Tags` carries no modelled `@length` or `@pattern` on the map or on either half of an
/// entry, so nothing is refused at the request boundary and every tagging rule this service
/// holds a caller to lives here: the per-resource count ([`MAX_TAGS_PER_RESOURCE`]) and then
/// each entry ([`tag_entry_rejection`]).
///
/// The count is answered first, so an oversized map is refused for being oversized rather
/// than for whichever of its entries happened to be looked at; the entries are then walked
/// in key order, so the same map is always refused with the same sentence however the
/// request's own map iterates.
pub fn tags_rejection<'a>(tags: impl IntoIterator<Item = (&'a str, &'a str)>) -> Option<String> {
    let mut entries: Vec<(&str, &str)> = tags.into_iter().collect();
    if entries.len() > MAX_TAGS_PER_RESOURCE {
        return Some(format!(
            "Tags carries {} entries; a resource may have at most {MAX_TAGS_PER_RESOURCE}",
            entries.len()
        ));
    }
    entries.sort_unstable();
    entries
        .into_iter()
        .find_map(|(key, value)| tag_entry_rejection(key, value))
}

/// The region this request was signed for, read from its SigV4 credential scope.
///
/// The record store is partitioned by this value, so only a Region-shaped one is taken from
/// the request ([`is_region_shaped`]): a scope carrying anything else is served in
/// [`DEFAULT_REGION`], which keeps the Region a resource is filed under one that its ARN can
/// actually name.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .filter(|region| is_region_shaped(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Whether `value` has the shape every ARN of this service requires of its Region segment:
/// `[a-z]+(-[a-z]+)+-[1-9]\d*` (smithy_query shape=ConnectorArn), within
/// [`MAX_REGION_LENGTH`] — two or more lower-case words joined by `-`, then a number with no
/// leading zero.
pub fn is_region_shaped(value: &str) -> bool {
    if value.is_empty() || value.len() > MAX_REGION_LENGTH {
        return false;
    }
    let Some((words, number)) = value.rsplit_once('-') else {
        return false;
    };
    if !number
        .as_bytes()
        .first()
        .is_some_and(|byte| matches!(byte, b'1'..=b'9'))
        || !number.bytes().all(|byte| byte.is_ascii_digit())
    {
        return false;
    }
    let mut word_count = 0usize;
    for word in words.split('-') {
        if word.is_empty() || !word.bytes().all(|byte| byte.is_ascii_lowercase()) {
            return false;
        }
        word_count += 1;
    }
    word_count >= 2
}

/// A fresh identifier in the UUID form every id in this service's ARNs and its challenge
/// `Password` carry (`[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}`).
///
/// Unpredictable AND unique. Two independent contributions go into the digest: the
/// per-process root seed ([`id_seed`]) and a value hashed through a `RandomState` built for
/// THIS call, which `std` seeds from the operating system — it is what makes `HashMap`
/// resistant to collision attacks — so a caller holding one challenge password cannot derive
/// the next one. The counter and the clock also go in, so two ids minted in the same
/// nanosecond, in the same or different threads, still differ. The version and variant
/// nibbles are set as RFC 9562 §5.4 specifies for a random-bytes UUID, so the result is a
/// well-formed UUID and not merely 32 hex characters.
pub fn new_uuid() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_nanos())
        .unwrap_or_default();

    let mut hasher = ::std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write(&counter.to_le_bytes());
    hasher.write(&nanos.to_le_bytes());
    let per_call = hasher.finish();

    let digest = <::sha2::Sha256 as ::sha2::Digest>::digest(format!(
        "{}:{per_call}:{counter}:{nanos}",
        id_seed()
    ));

    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = ::hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// The per-process root seed [`new_uuid`] mixes into every identifier.
///
/// Taken once through an operating-system-seeded `RandomState`, so every identifier in the
/// process shares one unpredictable root, with no RNG crate and no entropy of this crate's
/// own invention.
fn id_seed() -> u64 {
    static SEED: OnceLock<u64> = OnceLock::new();

    *SEED.get_or_init(|| {
        let mut hasher = ::std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write(ARN_SERVICE.as_bytes());
        hasher.finish()
    })
}

/// The ARN of a connector: `arn:aws:pca-connector-scep:<region>:<account>:connector/<uuid>`,
/// which is what `ConnectorArn`'s own pattern requires.
pub fn connector_arn(region: &str, connector_id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("connector/{connector_id}"))
}

/// The prefix every connector ARN of `region` begins with — the same ARN
/// [`connector_arn`] builds, up to and including the resource segment, with no id. Because
/// the store is keyed by ARN, this is the lower bound of that Region's contiguous range of
/// records.
pub fn connector_arn_prefix(region: &str) -> String {
    connector_arn(region, "")
}

/// The ARN of a challenge: the owning connector's ARN plus `/challenge/<uuid>`, which is
/// what `ChallengeArn`'s own pattern requires.
pub fn challenge_arn(connector_arn: &str, challenge_id: &str) -> String {
    format!("{connector_arn}{CHALLENGE_SEGMENT}{challenge_id}")
}

/// The connector ARN embedded in a challenge ARN, or `None` when the ARN carries no
/// challenge segment.
pub fn connector_arn_of_challenge(challenge_arn: &str) -> Option<&str> {
    challenge_arn
        .split_once(CHALLENGE_SEGMENT)
        .map(|(connector, _)| connector)
}

/// DNS suffix of a host reachable over the public internet, which is where a connector
/// created without a VPC endpoint id serves.
const PUBLIC_DNS_SUFFIX: &str = "amazonaws.com";

/// DNS suffix of a host reachable only through an interface VPC endpoint — the `vpce.`
/// label AWS PrivateLink puts in front of [`PUBLIC_DNS_SUFFIX`] for an endpoint-specific
/// DNS name. A connector whose create named a VPC endpoint serves only there.
const PRIVATE_DNS_SUFFIX: &str = "vpce.amazonaws.com";

/// The connector's HTTPS SCEP URL.
///
/// The model states only that it is an HTTPS SCEP URL, so the host is derived from the
/// connector's own id and the Region the request was signed for: stable across reads,
/// distinct for two connectors, and carrying no clock or request id.
///
/// The access path the create chose is part of that derivation, because it is what makes the
/// URL reachable: with no VPC endpoint id the connector is "accessible over the public
/// internet" (the `VpcEndpointId` member's own doc) and the host sits under the public DNS
/// suffix; with one, the connector is "accessible only through that specific VPC endpoint",
/// so the host is qualified by that endpoint's id and sits under the PrivateLink suffix
/// instead. Two connectors that differ only in that choice therefore report different hosts.
pub fn connector_endpoint(
    region: &str,
    connector_id: &str,
    vpc_endpoint_id: Option<&str>,
) -> String {
    match vpc_endpoint_id {
        Some(vpc_endpoint_id) => format!(
            "https://{connector_id}.scep.{vpc_endpoint_id}.{ARN_SERVICE}.{region}.{PRIVATE_DNS_SUFFIX}"
        ),
        None => format!("https://{connector_id}.scep.{ARN_SERVICE}.{region}.{PUBLIC_DNS_SUFFIX}"),
    }
}

/// The audience an Intune connector publishes.
///
/// It is the single audience Microsoft Entra workload identity federation accepts in a
/// token presented for token exchange, so it is a property of the federation this
/// integration configures rather than of any one connector: every Intune connector
/// publishes it, and nothing a caller sends can change it.
pub const OIDC_AUDIENCE: &str = "api://AzureADTokenExchange";

/// Host label of the OIDC issuer this service publishes, in front of the service's own
/// regional domain — the identity plane an Entra federated credential trusts, which is a
/// different host from the connector's SCEP data plane ([`connector_endpoint`]).
const OIDC_HOST_LABEL: &str = "oidc";

/// The issuer URL an Intune connector publishes.
///
/// An OIDC issuer URL, so it is an HTTPS origin under this service's own regional domain
/// whose discovery document and keys a relying party fetches. It is reached by the Entra
/// tenant over the public internet, so it never carries the connector's VPC endpoint: the
/// access path chosen for the SCEP data plane says nothing about where the issuer serves.
/// The connector is named by the issuer's path, so two connectors publish two issuers,
/// each with its own key namespace, and the URL is stable for the life of the connector.
pub fn oidc_issuer(region: &str, connector_id: &str) -> String {
    format!(
        "https://{OIDC_HOST_LABEL}.{ARN_SERVICE}.{region}.{PUBLIC_DNS_SUFFIX}/{connector_id}"
    )
}

/// The subject an Intune connector publishes: the identifier this service mints for the
/// connector INSIDE its issuer's namespace, so it names a resource of the issuer rather
/// than repeating an ARN, and is distinct for two connectors.
pub fn oidc_subject(connector_id: &str) -> String {
    format!("connector/{connector_id}")
}

/// The OIDC parameters an Intune connector publishes for the caller to copy into their
/// Microsoft Entra app registration's federated credential.
///
/// All three are minted by the service: the issuer and the subject from the connector's own
/// identity and Region ([`oidc_issuer`], [`oidc_subject`]), the audience from the
/// federation protocol ([`OIDC_AUDIENCE`]). None of them is taken from the request — an
/// `IntuneConfiguration` carries the caller's Entra tenant id and primary domain, which
/// are what the caller already has and are neither an OIDC audience nor an issuer.
pub fn open_id_configuration(region: &str, connector_id: &str) -> OpenIdConfiguration {
    OpenIdConfiguration {
        issuer: Some(oidc_issuer(region, connector_id)),
        subject: Some(oidc_subject(connector_id)),
        audience: Some(OIDC_AUDIENCE.to_string()),
    }
}

/// The resource-defining inputs of a `CreateConnector` call, rendered so two calls can be
/// compared.
///
/// `Tags` is deliberately NOT part of it: tags are mutable after the create (through
/// `TagResource`), so they do not define which connector a request asked for.
pub fn connector_request(
    certificate_authority_arn: &str,
    mobile_device_management: Option<&MobileDeviceManagement>,
    vpc_endpoint_id: Option<&str>,
) -> String {
    let mdm = match mobile_device_management {
        Some(MobileDeviceManagement::Intune(intune)) => format!(
            "Intune({},{})",
            intune.azure_application_id, intune.domain
        ),
        None => "none".to_string(),
    };
    format!(
        "ca={certificate_authority_arn}\nmdm={mdm}\nvpce={}",
        vpc_endpoint_id.unwrap_or("none")
    )
}

/// The resource-defining input of a `CreateChallenge` call: the connector it asked for.
pub fn challenge_request(connector_arn: &str) -> String {
    format!("connector={connector_arn}")
}

/// One page of a `@paginated` listing: the items to return, plus the cursor that resumes
/// exactly where the page ended (`None` when the page is the last one).
#[derive(Debug, Clone)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_token: Option<String>,
}

/// A `NextToken` no listing of this service ever minted.
#[derive(Debug, Clone, Copy)]
pub struct MalformedCursor;

/// Prefix the cursor plaintext carries, so a token this service never minted cannot decode
/// into a usable position.
const CURSOR_PREFIX: &str = "pca-connector-scep-after:";

/// Separates the cursor's scope from the key it names. A newline cannot occur in a scope or
/// in an ARN, so the split is unambiguous.
const CURSOR_SEPARATOR: char = '\n';

/// Padding appended when the hex cursor does not fill whole four-character groups, so the
/// token still matches `NextToken`'s pattern.
const CURSOR_PADDING: &str = "==";

/// Cursor-scope prefix of `ListConnectors`.
pub const CURSOR_SCOPE_CONNECTORS: &str = "connectors";

/// Cursor scope of `ListConnectors`, which lists the connectors of ONE Region: the scope
/// names that Region, so a token minted against another Region's endpoint — or by the
/// challenge listing — names no position here and is refused rather than resuming a walk it
/// was not taken from.
pub fn cursor_scope_connectors(region: &str) -> String {
    format!("{CURSOR_SCOPE_CONNECTORS}/{region}")
}

/// Cursor scope of `ListChallengeMetadata`, which lists ONE connector's challenges: the
/// scope names that connector, so a token minted for another connector — or for the
/// connector listing — names no position here and is refused rather than silently skipping
/// entries.
pub fn cursor_scope_challenges(connector_arn: &str) -> String {
    format!("challenges/{connector_arn}")
}

/// Encode the resume cursor that follows the record keyed `key` within `scope`.
///
/// Opaque to the caller and matching `NextToken`'s own pattern
/// (`^(?:[A-Za-z0-9_-]{4})*(?:[A-Za-z0-9_-]{2}==|[A-Za-z0-9_-]{3}=)?$`): hex characters are
/// all in that alphabet, and a hex string's length is always even, so it either fills whole
/// four-character groups or leaves two — which the pattern's `{2}==` tail accepts. The cursor
/// names a POSITION in the key ordering rather than an offset, so resuming never repeats or
/// skips a record when one is created or deleted between pages.
///
/// It is self-describing — scope included — so a listing validates a token by decoding it
/// and never has to record the tokens it issued, which matters because a listing is a
/// reporting operation: remembering an issued token would make a read mutate state.
pub fn encode_cursor(scope: &str, key: &str) -> String {
    let encoded = ::hex::encode(format!("{CURSOR_PREFIX}{scope}{CURSOR_SEPARATOR}{key}"));
    if encoded.len() % 4 == 0 {
        encoded
    } else {
        format!("{encoded}{CURSOR_PADDING}")
    }
}

/// Decode a cursor minted by [`encode_cursor`] for the SAME `scope`; `None` starts at the
/// beginning. A token of another scope is a [`MalformedCursor`], not a position.
pub fn decode_cursor(
    scope: &str,
    next_token: Option<&str>,
) -> Result<Option<String>, MalformedCursor> {
    let Some(token) = next_token else {
        return Ok(None);
    };
    // Hex never contains `=`, so stripping the padding cannot remove cursor content.
    let body = token.strip_suffix(CURSOR_PADDING).unwrap_or(token);
    let bytes = ::hex::decode(body).map_err(|_| MalformedCursor)?;
    let text = String::from_utf8(bytes).map_err(|_| MalformedCursor)?;
    let body = text.strip_prefix(CURSOR_PREFIX).ok_or(MalformedCursor)?;
    let (token_scope, key) = body.split_once(CURSOR_SEPARATOR).ok_or(MalformedCursor)?;
    if token_scope != scope {
        return Err(MalformedCursor);
    }
    Ok(Some(key.to_string()))
}

/// Take the page of `ordered` (sorted by key) that follows `after`, at most `max_results`
/// long. The cursor it mints carries `scope`, so it only resumes the listing that issued it.
///
/// `MaxResults` is `@range(1, 1000)`, so the boundary validator has already refused anything
/// outside it; an absent one takes a full page.
pub fn page_of<T>(
    scope: &str,
    ordered: impl IntoIterator<Item = (String, T)>,
    after: Option<&str>,
    max_results: Option<i32>,
) -> Page<T> {
    debug_assert!(
        max_results.is_none_or(|value| (1..=MAX_RESULTS_LIMIT).contains(&value)),
        "MaxResults is @range(1, {MAX_RESULTS_LIMIT}) and is refused at the boundary, so a \
         handler can only ever see a value inside it; got {max_results:?}"
    );
    let limit = max_results
        .unwrap_or(MAX_RESULTS_LIMIT)
        .clamp(1, MAX_RESULTS_LIMIT) as usize;
    let mut items = Vec::new();
    let mut last_key = String::new();
    let mut more_follow = false;
    for (key, item) in ordered
        .into_iter()
        .filter(|(key, _)| after.is_none_or(|after| key.as_str() > after))
    {
        if items.len() == limit {
            // One item beyond the page proves a next page exists; the cursor names where
            // this page ended, so resuming never repeats or skips an item.
            more_follow = true;
            break;
        }
        last_key = key;
        items.push(item);
    }
    Page {
        items,
        next_token: more_follow.then(|| encode_cursor(scope, &last_key)),
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsPcaconnectorscepPcaconnectorscepState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsPcaconnectorscepPcaconnectorscepState>)
}
