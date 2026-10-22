// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-pcaconnectorad-pcaconnectorad service.
//!
//! The persisted records every handler in `src/ops/` reads and writes, plus the
//! small helpers several of them share (ARN minting, the request's region, the
//! clock-due `CREATING` -> `ACTIVE` settle, pagination tokens). Keeping them here
//! means two operations never grow two copies of one resource.
//!
//! The store is keyed on ARN alone and is therefore region-agnostic: a connector
//! minted under one Region's credential scope is readable from a request signed
//! for another. The ARN a resource carries still records the Region it was
//! created in, which is what the wire shapes report.

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, SystemTime};

use crate::ops::types::{TemplateDefinition, TemplateRevision, VpcInformation};

/// This service's ARN namespace and SigV4 signing name (`OPERATIONS.md`, "Wire
/// protocol"). Every ARN this service mints carries it.
pub const ARN_NAMESPACE: &str = "pca-connector-ad";

/// The region an ARN carries when the request is unsigned, so no SigV4 credential
/// scope names one. Emulators default to `us-east-1`
/// (`emulator_server::emulator_arn`'s doc).
pub const DEFAULT_REGION: &str = "us-east-1";

/// `ConnectorStatus` values, spelled as `types::CONNECTOR_STATUS_VALUES` holds them.
pub const CONNECTOR_STATUS_CREATING: &str = "CREATING";
/// See [`CONNECTOR_STATUS_CREATING`].
pub const CONNECTOR_STATUS_ACTIVE: &str = "ACTIVE";

/// `TemplateStatus` values, spelled as `types::TEMPLATE_STATUS_VALUES` holds them.
/// The enum has only `ACTIVE` and `DELETING`, so a created template is `ACTIVE`.
pub const TEMPLATE_STATUS_ACTIVE: &str = "ACTIVE";

/// How long after `CreateConnector` the connector settles from `CREATING` to
/// `ACTIVE`.
///
/// A CHOSEN span, not a documented one: AWS publishes no duration for connector
/// activation. It sits at the top of the 250-750 ms window the emulator uses for a
/// clock-owed transition — the widest the window allows, so a caller (or a test)
/// that reads the connector straight after creating it reliably observes
/// `CREATING` — and it is never zero.
pub const CONNECTOR_ACTIVATION_DELAY: Duration = Duration::from_millis(750);

/// `MaxResults` for a list call that supplies none: the modelled maximum of the
/// `MaxResults` member (`range 1..1000`), so an un-paged call returns everything.
pub const MAX_LIST_RESULTS: i32 = 1000;

/// The most connectors this emulator stores AT ALL, and the most templates it
/// stores under one connector. Past either, the create is refused with the
/// modelled `ServiceQuotaExceededException` instead of growing the store, so no
/// number of requests can grow the process heap without limit.
///
/// [`MAX_CONNECTORS`] is a bound on this STORE as a whole, not a per-account or
/// per-Region quota: the store is keyed on ARN alone (see the module header), so
/// the count it compares against spans every credential scope. The quota code and
/// refusal message say so rather than naming a Region the count does not isolate.
///
/// Both are the EMULATOR's store bounds, not quotas quoted from AWS: the service
/// publishes adjustable quotas, and these caps sit well above the default ones so
/// no faithful workflow reaches them. Only the refusal itself — a 402
/// `ServiceQuotaExceededException` once a quota is exhausted — is the service's
/// documented behaviour.
pub const MAX_CONNECTORS: usize = 100;
/// See [`MAX_CONNECTORS`].
pub const MAX_TEMPLATES_PER_CONNECTOR: usize = 200;

/// How many `ClientToken` -> ARN records the store keeps. An idempotency record
/// is a cache, not a resource: AWS honours a token for a bounded window and
/// forgets it afterwards, so past this many the OLDEST record is dropped rather
/// than the request refused. That bounds the map against an unlimited number of
/// distinct caller-supplied tokens.
pub const MAX_IDEMPOTENCY_RECORDS: usize = 4096;

/// The `QuotaCode` a store-bound refusal reports. The member's only documented
/// rule is "The code associated with the service quota"; AWS's real codes are
/// opaque `L-xxxxxxxx` identifiers this emulator cannot derive, so these name the
/// quota in readable form and are EMULATOR-CHOSEN, not quoted from AWS.
pub const QUOTA_CODE_CONNECTORS_PER_STORE: &str = "ConnectorsPerStore";
/// See [`QUOTA_CODE_CONNECTORS_PER_STORE`].
pub const QUOTA_CODE_TEMPLATES_PER_CONNECTOR: &str = "TemplatesPerConnector";

/// The tag every pagination cursor this service issues carries, so a token the
/// service did not mint is refused instead of being honoured as a cursor. Four
/// characters, so a tagged cursor's total length stays a multiple of four (see
/// [`encode_page_token`]).
const PAGE_TOKEN_TAG: &str = "pcad";

/// How many hex characters a cursor's `seq` field occupies: a `u64` rendered
/// zero-padded, so every cursor this service issues is the same length and the
/// tag plus the field is a multiple of four characters.
const PAGE_TOKEN_SEQ_WIDTH: usize = 16;

/// The Microsoft Active Directory certificate-template OID arc that every AD
/// template's object identifier sits under.
const AD_TEMPLATE_OID_ARC: &str = "1.3.6.1.4.1.311.21.8";

/// One persisted connector. Mirrors the modelled `Connector`/`ConnectorSummary`
/// members, plus the bookkeeping the wire shapes do not carry (`seq` for stable
/// list ordering, `active_at` for the clock-due settle, `tags`).
#[derive(Debug, Clone)]
pub struct ConnectorRecord {
    pub arn: String,
    pub certificate_authority_arn: String,
    pub directory_id: String,
    pub vpc_information: VpcInformation,
    pub certificate_enrollment_policy_server_endpoint: String,
    pub status: String,
    pub status_reason: Option<String>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    /// The instant the clock owes this connector its `CREATING` -> `ACTIVE` move.
    pub active_at: SystemTime,
    pub tags: HashMap<String, String>,
    /// Monotonic creation sequence: the stable list order and paging cursor.
    pub seq: u64,
}

/// One persisted template, a child of exactly one connector.
#[derive(Debug, Clone)]
pub struct TemplateRecord {
    pub arn: String,
    pub connector_arn: String,
    pub name: String,
    pub definition: TemplateDefinition,
    pub object_identifier: String,
    pub policy_schema: i32,
    pub status: String,
    pub revision: TemplateRevision,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    pub tags: HashMap<String, String>,
    /// Monotonic creation sequence: the stable list order and paging cursor.
    pub seq: u64,
}

/// Per-service state for com-amazonaws-pcaconnectorad-pcaconnectorad. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsPcaconnectoradPcaconnectoradState {
    /// Connectors by their own ARN.
    pub connectors: HashMap<String, ConnectorRecord>,
    /// Templates by their own ARN; `TemplateRecord::connector_arn` is the parent link.
    pub templates: HashMap<String, TemplateRecord>,
    /// `<operation>:<ClientToken>` -> the ARN the first call with that token minted,
    /// so a retried create returns the original resource instead of a second one.
    /// Bounded at [`MAX_IDEMPOTENCY_RECORDS`]; write it through
    /// [`Self::record_idempotency`], never directly.
    pub idempotency: HashMap<String, String>,
    /// The keys of `idempotency` in the order they were recorded, so the oldest
    /// record is the one dropped when the map reaches its bound.
    idempotency_order: VecDeque<String>,
    /// Source of [`ConnectorRecord::seq`] / [`TemplateRecord::seq`].
    next_seq: u64,
}

impl ComAmazonawsPcaconnectoradPcaconnectoradState {
    /// The next creation sequence number. Monotonic for the life of the store, so a
    /// record's `seq` orders it against every other record ever created here.
    pub fn next_seq(&mut self) -> u64 {
        self.next_seq += 1;
        self.next_seq
    }

    /// Materialise every `CREATING` -> `ACTIVE` move the clock already owes at `now`.
    ///
    /// A reporting operation calls this before it reads: the transition was due at
    /// `active_at` whether or not anybody called, so applying it is not a mutation
    /// caused by the read.
    pub fn settle(&mut self, now: SystemTime) {
        for connector in self.connectors.values_mut() {
            if connector.status == CONNECTOR_STATUS_CREATING && now >= connector.active_at {
                connector.status = CONNECTOR_STATUS_ACTIVE.to_string();
            }
        }
    }

    /// Every connector whose `seq` is past `after`, in creation order.
    pub fn connectors_after(&self, after: u64) -> Vec<&ConnectorRecord> {
        let mut found: Vec<&ConnectorRecord> = self
            .connectors
            .values()
            .filter(|connector| connector.seq > after)
            .collect();
        found.sort_by_key(|connector| connector.seq);
        found
    }

    /// The template called `name` on `connector_arn`, if there is one. Template
    /// names are unique per connector.
    pub fn template_by_name(&self, connector_arn: &str, name: &str) -> Option<&TemplateRecord> {
        self.templates
            .values()
            .find(|template| template.connector_arn == connector_arn && template.name == name)
    }

    /// How many templates `connector_arn` already carries, against
    /// [`MAX_TEMPLATES_PER_CONNECTOR`].
    pub fn template_count(&self, connector_arn: &str) -> usize {
        self.templates
            .values()
            .filter(|template| template.connector_arn == connector_arn)
            .count()
    }

    /// Record that `key` (`<operation>:<ClientToken>`) minted `arn`, dropping the
    /// oldest records once the store holds [`MAX_IDEMPOTENCY_RECORDS`] of them.
    /// Forgetting an old token is the honest outcome: a token the service no
    /// longer remembers mints a new resource, which is what AWS does once a
    /// token's window has passed.
    ///
    /// The key's scope is the caller's to choose, and it carries NO Region or
    /// credential scope: this store is region-agnostic (see the module header),
    /// so one `ClientToken` string reused for the same operation names the same
    /// resource whichever Region's scope the retry is signed for — which is the
    /// point of an idempotency token. A key whose resource has a parent (a
    /// template's connector) includes that parent, so a token reused against a
    /// different parent cannot answer with an ARN the caller did not address.
    pub fn record_idempotency(&mut self, key: String, arn: String) {
        if self.idempotency.insert(key.clone(), arn).is_none() {
            self.idempotency_order.push_back(key);
        }
        while self.idempotency_order.len() > MAX_IDEMPOTENCY_RECORDS {
            if let Some(oldest) = self.idempotency_order.pop_front() {
                self.idempotency.remove(&oldest);
            }
        }
    }

    /// Remove `connector_arn` and cascade-remove its templates, dropping any
    /// idempotency token that pointed at a now-gone ARN (so a later create reusing
    /// that token mints a new resource rather than naming a deleted one).
    ///
    /// Returns whether a connector was actually present.
    pub fn remove_connector(&mut self, connector_arn: &str) -> bool {
        let removed = self.connectors.remove(connector_arn).is_some();
        let orphaned: HashSet<String> = self
            .templates
            .values()
            .filter(|template| template.connector_arn == connector_arn)
            .map(|template| template.arn.clone())
            .collect();
        for arn in &orphaned {
            self.templates.remove(arn);
        }
        let idempotency = &mut self.idempotency;
        idempotency.retain(|_, arn| arn != connector_arn && !orphaned.contains(arn));
        self.idempotency_order
            .retain(|key| idempotency.contains_key(key));
        removed
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsPcaconnectoradPcaconnectoradState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsPcaconnectoradPcaconnectoradState>)
}

/// The region this request was signed for, or [`DEFAULT_REGION`] when it carries no
/// SigV4 credential scope.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// A fresh connector ARN of the modelled form
/// `arn:aws:pca-connector-ad:<region>:<account>:connector/<uuid>`.
pub fn connector_arn(region: &str, connector_id: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("connector/{connector_id}"))
}

/// The ARN of the connector collection this store holds —
/// `arn:aws:pca-connector-ad:<region>:<account>:connector` — the resource a
/// connector-count refusal is about, since no connector was minted. The Region
/// segment is the one the request was signed for; the bound itself is
/// store-global, which the refusal's message and quota code state.
pub fn connector_collection_arn(region: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, "connector")
}

/// A template ARN: the modelled child form, the parent connector ARN plus
/// `/template/<uuid>`.
pub fn template_arn(connector_arn: &str, template_id: &str) -> String {
    format!("{connector_arn}/template/{template_id}")
}

/// The certificate-enrollment policy-server endpoint a connector reports: the
/// connector's own id as the leading host label, then this service's regional
/// endpoint host (`<service>.<region>.amazonaws.com`, the spelling every other
/// endpoint of this service uses), with the AD policy-provider CEP path.
pub fn enrollment_policy_server_endpoint(connector_id: &str, region: &str) -> String {
    format!(
        "https://{connector_id}.{ARN_NAMESPACE}.{region}.amazonaws.com/ADPolicyProvider_CEP_Kerberos/service.svc/CEP"
    )
}

/// A fresh resource id: a lowercase hyphenated v4 UUID, the form every ARN this
/// service mints embeds (`ConnectorArn` and `TemplateArn` both pattern-match a
/// UUID there).
///
/// Minted by the framework's own v4 generator, `emulator_telemetry`'s `random()`
/// id constructor ("A fresh random (v4) identifier"), which is backed by the
/// `uuid` crate and which this crate already depends on — so this service
/// declares no id generator and no RNG of its own.
pub fn new_resource_id() -> String {
    ::emulator_telemetry::SessionId::random().as_str().to_string()
}

/// A template's object identifier: the AD certificate-template OID arc plus seven
/// numeric components derived from the template's own id, so the OID is stable
/// for the resource, unique to it, and inside the modelled
/// `CustomObjectIdentifier` pattern. `template_id` is an id as
/// [`new_resource_id`] renders one.
pub fn template_object_identifier(template_id: &str) -> String {
    let digits: Vec<u8> = template_id.bytes().filter(u8::is_ascii_hexdigit).collect();
    let mut oid = String::from(AD_TEMPLATE_OID_ARC);
    for chunk in digits.chunks_exact(4).take(7) {
        let component = std::str::from_utf8(chunk)
            .ok()
            .and_then(|text| u16::from_str_radix(text, 16).ok())
            .unwrap_or_default();
        oid.push('.');
        oid.push_str(&component.to_string());
    }
    oid
}

/// Encode a pagination cursor: the `seq` the page just served ended at, rendered
/// as [`PAGE_TOKEN_SEQ_WIDTH`] zero-padded hex characters behind
/// [`PAGE_TOKEN_TAG`]. Hex digits and the tag are inside the modelled `NextToken`
/// character class and the cursor's length is a multiple of four, so the token
/// matches the modelled pattern
/// `^(?:[A-Za-z0-9_-]{4})*(?:[A-Za-z0-9_-]{2}==|[A-Za-z0-9_-]{3}=)?$`.
pub fn encode_page_token(after_seq: u64) -> String {
    format!("{PAGE_TOKEN_TAG}{after_seq:0width$x}", width = PAGE_TOKEN_SEQ_WIDTH)
}

/// Decode a cursor produced by [`encode_page_token`]. `None` for anything this
/// service did not mint — a value carrying no [`PAGE_TOKEN_TAG`], a cursor field
/// of the wrong width, or one that is not hex — so a forged token is refused
/// rather than honoured. The caller answers `None` with a `ValidationException`.
pub fn decode_page_token(token: &str) -> Option<u64> {
    let cursor = token.strip_prefix(PAGE_TOKEN_TAG)?;
    if cursor.len() != PAGE_TOKEN_SEQ_WIDTH {
        return None;
    }
    u64::from_str_radix(cursor, 16).ok()
}
