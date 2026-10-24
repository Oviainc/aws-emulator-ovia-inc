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
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::ops::types::{
    Challenge, ChallengeMetadataSummary, Connector, ConnectorSummary, IntuneConfiguration,
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

/// How many connectors this emulator will hold.
///
/// The real service caps connectors per account per Region with a service quota, and
/// `CreateConnector` models `ServiceQuotaExceededException` for exceeding it. The number
/// here is NOT a fidelity claim about the published quota: it is the emulator's own
/// ceiling, set generously so no realistic workflow meets it, so that a caller looping on
/// create cannot grow this process's memory without bound.
pub const MAX_CONNECTORS: usize = 1_000;

/// The same ceiling for the challenge passwords of one connector, for the same reason.
pub const MAX_CHALLENGES_PER_CONNECTOR: usize = 1_000;

/// How many tags one resource may carry.
///
/// `Tags` carries no `@length`, so the model states no bound; AWS's tagging limit of 50 tags
/// per resource is what this enforces, which is also what keeps a caller from making one
/// create call cost unbounded memory.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

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
    /// The connector's HTTPS public SCEP URL.
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
#[derive(Debug, Default)]
pub struct ComAmazonawsPcaconnectorscepPcaconnectorscepState {
    /// Every connector, keyed by its ARN and ordered by it, so `ListConnectors` paginates
    /// deterministically. A connector owns its challenges and both own their tags, so
    /// deleting a connector drops all three and leaves nothing behind.
    pub connectors: BTreeMap<String, StoredConnector>,
    /// `CreateConnector`'s `ClientToken` -> the connector ARN it created.
    pub connector_tokens: BTreeMap<String, IdempotencyRecord>,
    /// `CreateChallenge`'s `ClientToken` -> the challenge ARN it created.
    pub challenge_tokens: BTreeMap<String, IdempotencyRecord>,
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

    /// The challenge stored under `challenge_arn`, found through the connector its ARN names.
    pub fn challenge(&self, challenge_arn: &str) -> Option<&StoredChallenge> {
        let connector_arn = connector_arn_of_challenge(challenge_arn)?;
        self.connectors
            .get(connector_arn)?
            .challenges
            .get(challenge_arn)
    }

    /// Remove the challenge `challenge_arn` names, answering whether it was there.
    pub fn remove_challenge(&mut self, challenge_arn: &str) -> bool {
        let Some(connector_arn) = connector_arn_of_challenge(challenge_arn) else {
            return false;
        };
        self.connectors
            .get_mut(connector_arn)
            .is_some_and(|connector| connector.challenges.remove(challenge_arn).is_some())
    }

    /// The record a still-live `ClientToken` of `CreateConnector` is bound to.
    pub fn connector_for_token(&self, client_token: Option<&str>) -> Option<IdempotencyRecord> {
        client_token
            .and_then(|token| self.connector_tokens.get(token))
            .cloned()
    }

    /// The record a still-live `ClientToken` of `CreateChallenge` is bound to.
    pub fn challenge_for_token(&self, client_token: Option<&str>) -> Option<IdempotencyRecord> {
        client_token
            .and_then(|token| self.challenge_tokens.get(token))
            .cloned()
    }
}

/// The region this request was signed for, read from its SigV4 credential scope.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
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

/// The connector's HTTPS public SCEP URL (the `Endpoint` member's whole doc).
///
/// The model states only that it is an HTTPS SCEP URL, so the host is derived from the
/// connector's own id and the Region the request was signed for: stable across reads,
/// distinct for two connectors, and carrying no clock or request id.
pub fn connector_endpoint(region: &str, connector_id: &str) -> String {
    format!("https://{connector_id}.scep.{ARN_SERVICE}.{region}.amazonaws.com")
}

/// The OIDC parameters an Intune connector publishes for the caller to copy into their
/// Microsoft Entra app registration.
///
/// The model documents what each member is FOR but not the strings AWS mints, so each is
/// derived from the connector record: the issuer is the connector's own SCEP URL, the
/// subject its ARN, and the audience the Entra application the token is for, as the
/// request's `IntuneConfiguration` named it.
pub fn open_id_configuration(
    connector_arn: &str,
    endpoint: &str,
    intune: &IntuneConfiguration,
) -> OpenIdConfiguration {
    OpenIdConfiguration {
        issuer: Some(endpoint.to_string()),
        subject: Some(connector_arn.to_string()),
        audience: Some(intune.azure_application_id.clone()),
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

/// Cursor scope of `ListConnectors`.
pub const CURSOR_SCOPE_CONNECTORS: &str = "connectors";

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
