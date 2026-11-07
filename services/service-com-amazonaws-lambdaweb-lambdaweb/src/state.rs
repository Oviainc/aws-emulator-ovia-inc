// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-lambdaweb-lambdaweb service.
//!
//! Every web function a client creates lives here, with its revisions and its
//! endpoints nested under it, so `CreateWebFunction`, `CreateWebFunctionRevision`,
//! `GetWebFunction`, `GetWebFunctionRevision`, `GetWebFunctionEndpoint`,
//! `ListWebFunctions` and `DeleteWebFunction` all read and write ONE copy of each
//! resource. The helpers beside the state struct — ARN construction, the request's
//! region, the documented resource-name rule, the record-to-modelled-shape
//! projections, the clock-driven `Pending -> Active` transition and the opaque list
//! cursor codec — are shared by those handlers rather than duplicated per
//! operation. The endpoint slice lives in [`endpoints`], the resource-policy
//! slice in [`resource_policies`] and the tag slice in [`tags`].

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};

use crate::ops::types::{
    BuildConfig, FunctionRevisionSummary, FunctionSummary, RevisionError, ServiceConfig,
};

/// The account-settings slice: the quotas this emulator applies to web
/// functions, and the usage `GetWebAccountSettings` derives from the store.
mod account;
pub use account::*;

/// The endpoint slice: what an endpoint of a web function is, and the rules the
/// create paths validate it against.
mod endpoints;
pub use endpoints::*;

/// The resource-policy slice: the resource-based policy a web function carries,
/// the document rule the put holds it to and the revision rule both mutating
/// operations apply.
mod resource_policies;
pub use resource_policies::*;

/// The tag slice: the tags a web function carries, and the rules the tag
/// operations hold a key set to.
mod tags;
pub use tags::*;

/// The region an emulated request is treated as having been made in when its
/// SigV4 credential scope does not name one (an unsigned or anonymous request).
pub const DEFAULT_REGION: &str = "us-east-1";

/// The `arnNamespace` / `endpointPrefix` / sigv4 name this service's ARNs carry,
/// from the model's `aws.api#service` trait (`smithy_query(query="service_info")`:
/// `arnNamespace: lambda`).
pub const ARN_NAMESPACE: &str = "lambda";

/// How long after the call that created it a web function (or a revision) stays
/// `Pending` before the clock owes it `Active`.
///
/// CHOSEN, not cited: the model documents the states but no duration for the
/// transition. It is deliberately non-zero, so a caller that reads straight back
/// still observes the transient `Pending` state the real service reports.
pub const PROVISIONING_DELAY: Duration = Duration::from_millis(500);

/// The most web functions this emulator stores for the account before
/// `CreateWebFunction` answers its modelled `ServiceQuotaExceededException`.
///
/// The quota EXISTS per the model (`ServiceQuotaExceededException`: "the maximum
/// number of web functions in your account"), but its value is not published, so
/// this bound is CHOSEN: far above any plausible client's function count, while
/// keeping the store a caller can grow bounded.
pub const MAX_WEB_FUNCTIONS: usize = 500;

/// The most revisions this emulator stores per web function before a create
/// answers its modelled `ServiceQuotaExceededException` ("the maximum number of
/// revisions or endpoints per web function"). CHOSEN for the same reason as
/// [`MAX_WEB_FUNCTIONS`].
pub const MAX_REVISIONS_PER_FUNCTION: usize = 100;

/// The longest `functionName` the service accepts.
///
/// The model constrains `FunctionName` to 1..256, but the member's own
/// documentation states the narrower rule this emulator enforces: "The name can
/// contain letters, numbers, hyphens (-), and underscores (_), and can't begin or
/// end with a hyphen or an underscore. The name is limited to 64 characters in
/// length." (source: `smithy_query(query="doc", shape="CreateWebFunctionRequest",
/// member="functionName")`).
pub const FUNCTION_NAME_MAX_CHARS: usize = 64;

/// `ServiceConfig$timeoutSeconds`' modelled `@default` (model: range 3..900,
/// default 30).
pub const DEFAULT_TIMEOUT_SECONDS: i32 = 30;

/// `ServiceConfig$maxConcurrencyPerEnvironment`' modelled `@default` (model:
/// range 1..128, default 64).
pub const DEFAULT_MAX_CONCURRENCY_PER_ENVIRONMENT: i32 = 64;

/// The page size `ListWebFunctions` uses when the caller sends no `maxResults`:
/// the member's modelled `@default` (and the largest page its `@range` allows).
pub const DEFAULT_MAX_RESULTS: usize = 50;

/// The largest page `ListWebFunctions` returns (`MaxResults` range 1..50).
pub const MAX_MAX_RESULTS: usize = 50;

/// The only filter name `ListWebFunctions` supports: "The only supported filter
/// name is `state`." (source: `smithy_query(query="doc",
/// shape="ListWebFunctionsRequest", member="filters")`).
pub const STATE_FILTER_NAME: &str = "state";

/// `stateReason` while a web function is still being created.
pub const FUNCTION_PENDING_REASON: &str = "The function is being created.";
/// `stateReason` once a web function is serving.
pub const FUNCTION_ACTIVE_REASON: &str = "The function is active.";
/// `stateReason` while a revision is still being built.
pub const REVISION_PENDING_REASON: &str = "The revision is being created.";
/// `stateReason` once a revision is built.
pub const REVISION_ACTIVE_REASON: &str = "The revision is active.";

/// The `FunctionState` of a stored web function.
///
/// The variants are exactly the model's enum values
/// (`FUNCTION_STATE_VALUES` in `ops::types`), and [`FunctionState::wire`] is the
/// only place a state becomes a wire string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FunctionState {
    Pending,
    Active,
    Failed,
    Deleting,
}

impl FunctionState {
    /// This state as the model's enum value.
    pub fn wire(self) -> &'static str {
        match self {
            Self::Pending => "Pending",
            Self::Active => "Active",
            Self::Failed => "Failed",
            Self::Deleting => "Deleting",
        }
    }

    /// The state a wire value names, or `None` when it names none — which is how
    /// a `state` filter value that is not a `FunctionState` is refused.
    pub fn from_wire(value: &str) -> Option<Self> {
        [Self::Pending, Self::Active, Self::Failed, Self::Deleting]
            .into_iter()
            .find(|state| state.wire() == value)
    }
}

/// The `RevisionState` of a stored revision: the model's enum values
/// (`REVISION_STATE_VALUES` in `ops::types`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevisionState {
    Pending,
    Active,
    Failed,
}

impl RevisionState {
    /// This state as the model's enum value.
    pub fn wire(self) -> &'static str {
        match self {
            Self::Pending => "Pending",
            Self::Active => "Active",
            Self::Failed => "Failed",
        }
    }
}

/// One stored revision of a web function.
///
/// `build_config` and `service_config` hold the modelled shapes VERBATIM —
/// including the `timeoutSeconds` / `maxConcurrencyPerEnvironment` defaults
/// [`adopt_service_config`] applied — so every later read reports what was stored
/// instead of re-deriving anything from a request.
#[derive(Debug, Clone)]
pub struct Revision {
    pub revision_id: String,
    pub revision_arn: String,
    pub description: Option<String>,
    pub kms_key_arn: Option<String>,
    pub build_config: BuildConfig,
    pub service_config: ServiceConfig,
    pub state: RevisionState,
    pub state_reason: String,
    pub errors: Vec<RevisionError>,
    pub created_at: SystemTime,
    /// The instant the clock owes this revision its `Pending -> Active`
    /// transition, or `None` once it has been materialised.
    pub activates_at: Option<SystemTime>,
}

impl Revision {
    /// This revision as the `FunctionRevisionSummary` a create reports.
    pub fn summary(&self) -> FunctionRevisionSummary {
        FunctionRevisionSummary {
            revision_arn: self.revision_arn.clone(),
            revision_id: self.revision_id.clone(),
            description: self.description.clone(),
            state: self.state.wire().to_string(),
            state_reason: self.state_reason.clone(),
            created_at: ::aws_smithy_types::DateTime::from(self.created_at),
        }
    }

    /// The stored build errors as the modelled `RevisionErrors` list, or `None`
    /// when there are none: the model constrains the list to 1..50 entries, so a
    /// revision with no errors reports no member rather than an empty list.
    pub fn modeled_errors(&self) -> Option<Vec<RevisionError>> {
        if self.errors.is_empty() {
            return None;
        }
        Some(self.errors.clone())
    }

    /// Apply the `Pending -> Active` transition if the clock has already made it
    /// due. Owed at [`Self::activates_at`] whether or not anybody called, so
    /// materialising it on a read reports what was already true.
    fn materialise_due(&mut self, now: SystemTime) {
        if self.state == RevisionState::Pending && self.activates_at.is_some_and(|due| now >= due) {
            self.state = RevisionState::Active;
            self.state_reason = REVISION_ACTIVE_REASON.to_string();
            self.activates_at = None;
        }
    }
}

/// One stored web function: what `CreateWebFunction` was given, plus the ARN this
/// service minted, the timestamps it fixed and the revisions created under it.
#[derive(Debug, Clone)]
pub struct WebFunction {
    pub function_name: String,
    pub function_arn: String,
    pub state: FunctionState,
    pub state_reason: String,
    /// Fixed at creation and never moved again.
    pub created_at: SystemTime,
    /// Bumped by every mutation of this function, and by the clock-driven
    /// `Pending -> Active` transition.
    pub updated_at: SystemTime,
    /// The instant the clock owes this function its `Pending -> Active`
    /// transition, or `None` once it has been materialised.
    pub activates_at: Option<SystemTime>,
    pub tags: BTreeMap<String, String>,
    /// The revisions of this function, keyed by `revisionId`.
    pub revisions: BTreeMap<String, Revision>,
    pub latest_revision_id: Option<String>,
    /// The endpoints of this function, keyed by `endpointName` — the endpoint's
    /// identity in this API. Nested here, so a function's delete takes its
    /// endpoints with it and every endpoint reader sees ONE copy of each.
    pub endpoints: BTreeMap<String, Endpoint>,
    /// The resource-based policy attached to this function, or `None` while it
    /// carries none. A function has at most one, and it is nested here for the
    /// same reason the endpoints are: a function's delete takes its policy with
    /// it, and every resource-policy operation reads ONE copy of it.
    pub resource_policy: Option<ResourcePolicy>,
}

impl WebFunction {
    /// This function as the `FunctionSummary` `ListWebFunctions` returns: the
    /// stored record's own fields, never request input.
    pub fn summary(&self) -> FunctionSummary {
        FunctionSummary {
            function_name: self.function_name.clone(),
            function_arn: self.function_arn.clone(),
            state: self.state.wire().to_string(),
            state_reason: self.state_reason.clone(),
            created_at: ::aws_smithy_types::DateTime::from(self.created_at),
            updated_at: ::aws_smithy_types::DateTime::from(self.updated_at),
        }
    }

    /// The stored tags as the modelled `Tags` map, or `None` when the function
    /// carries none.
    pub fn modeled_tags(&self) -> Option<HashMap<String, String>> {
        if self.tags.is_empty() {
            return None;
        }
        Some(modeled_tag_map(&self.tags))
    }

    /// The revision this function's `latestRevisionId` names, if it still has one.
    pub fn latest_revision(&self) -> Option<&Revision> {
        self.latest_revision_id
            .as_ref()
            .and_then(|id| self.revisions.get(id))
    }

    /// Apply every transition the clock has already made due to this function and
    /// its revisions. The function's `updated_at` moves to the instant the
    /// transition was DUE, not to now, so the timestamp a caller reads is stable
    /// across reads.
    fn materialise_due(&mut self, now: SystemTime) {
        if self.state == FunctionState::Pending {
            if let Some(due) = self.activates_at.filter(|due| now >= *due) {
                self.state = FunctionState::Active;
                self.state_reason = FUNCTION_ACTIVE_REASON.to_string();
                self.updated_at = due;
                self.activates_at = None;
            }
        }
        for revision in self.revisions.values_mut() {
            revision.materialise_due(now);
        }
        for endpoint in self.endpoints.values_mut() {
            endpoint.materialise_due(now);
        }
    }
}

/// The configuration a new revision is built from, as the two create operations
/// supply it.
#[derive(Debug, Clone)]
pub struct NewRevision {
    pub description: Option<String>,
    pub kms_key_arn: Option<String>,
    pub build_config: BuildConfig,
    pub service_config: ServiceConfig,
}

/// Per-service state for com-amazonaws-lambdaweb-lambdaweb. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsLambdawebLambdawebState {
    /// Every stored web function, keyed by `functionName` — the resource's
    /// identity in this API. A `BTreeMap`, so `ListWebFunctions` walks one stable
    /// order and a cursor taken on one page resumes at the right place.
    pub functions: BTreeMap<String, WebFunction>,
    /// Monotonic counter behind every minted `revisionId`, so two revisions —
    /// of one function or of two — never share an id.
    pub next_revision_seq: u64,
    /// Monotonic counter behind every minted POLICY `revisionId`, so two
    /// generations of a resource-based policy — of one function or of two — never
    /// share an id.
    pub next_policy_revision_seq: u64,
}

impl ComAmazonawsLambdawebLambdawebState {
    /// Apply every state transition the clock has already made due, across every
    /// stored function and revision.
    ///
    /// Called at the start of every handler, including the reporting ones: the
    /// transition was owed at [`PROVISIONING_DELAY`] after the call that started
    /// it whether or not anybody read the resource, so materialising it here
    /// reports what was already true rather than changing what a later caller
    /// sees.
    pub fn materialise_due_transitions(&mut self, now: SystemTime) {
        for function in self.functions.values_mut() {
            function.materialise_due(now);
        }
    }

    /// Mint a fresh `revisionId`: the counter rendered in the 8-4-4-4-12 shape
    /// this service's other revision ids carry (`DeleteResourcePolicy`'s
    /// `revisionId` is modelled `^[0-9a-f-]{36}$`), which also keeps the id free
    /// of `/` so it rides in the `{revisionId}` URI label unescaped.
    pub fn mint_revision_id(&mut self) -> String {
        self.next_revision_seq = self.next_revision_seq.wrapping_add(1);
        hex_uuid_shape(0, self.next_revision_seq)
    }

    /// Mint and persist a revision of `function_name`, returning its `revisionId`,
    /// or `None` when no such function is stored.
    ///
    /// Adding a revision mutates the function, so its `updated_at` moves to `now`.
    pub fn add_revision(
        &mut self,
        function_name: &str,
        new: NewRevision,
        now: SystemTime,
    ) -> Option<String> {
        if !self.functions.contains_key(function_name) {
            return None;
        }
        let revision_id = self.mint_revision_id();
        let function = self.functions.get_mut(function_name)?;
        let revision = Revision {
            revision_arn: revision_arn(&function.function_arn, &revision_id),
            revision_id: revision_id.clone(),
            description: new.description,
            kms_key_arn: new.kms_key_arn,
            build_config: new.build_config,
            service_config: adopt_service_config(new.service_config),
            state: RevisionState::Pending,
            state_reason: REVISION_PENDING_REASON.to_string(),
            errors: Vec::new(),
            created_at: now,
            activates_at: Some(now + PROVISIONING_DELAY),
        };
        function.revisions.insert(revision_id.clone(), revision);
        function.latest_revision_id = Some(revision_id.clone());
        function.updated_at = now;
        Some(revision_id)
    }
}

/// The `ServiceConfig` to store: the supplied one with the model's declared
/// defaults applied to any member that arrived without a value.
///
/// `timeoutSeconds` is `@range(3..900) @default(30)` and
/// `maxConcurrencyPerEnvironment` is `@range(1..128) @default(64)`. Those ranges
/// are MODELLED, so the boundary validator has already refused any value outside
/// them before a handler runs — which is why a 0 reaching here can only mean the
/// member was absent, and why no range check belongs in this function.
pub fn adopt_service_config(mut config: ServiceConfig) -> ServiceConfig {
    if config.timeout_seconds == 0 {
        config.timeout_seconds = DEFAULT_TIMEOUT_SECONDS;
    }
    if config.max_concurrency_per_environment == 0 {
        config.max_concurrency_per_environment = DEFAULT_MAX_CONCURRENCY_PER_ENVIRONMENT;
    }
    config
}

/// The region of the request being served: the third field of its SigV4
/// credential scope, falling back to [`DEFAULT_REGION`] for a request that is not
/// signed.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The ARN of a web function in `region`:
/// `arn:aws:lambda:<region>:<account>:web-function/<functionName>` — the shape the
/// resource-policy operations' `resourceArn` pattern requires
/// (`^arn:[^:]+:lambda:[^:]+:\d{12}:web-function/[^/]+$`).
pub fn function_arn(region: &str, function_name: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("web-function/{function_name}"),
    )
}

/// The ARN of a revision, derived from its FUNCTION's stored ARN so the two always
/// name the same function, region and account:
/// `<functionArn>/revision/<revisionId>`. `RevisionArn` is modelled with a length
/// bound only (1..2048) and no pattern, so this layout mirrors the operation's own
/// URI (`/web-functions/{functionName}/revisions/{revisionId}`).
pub fn revision_arn(function_arn: &str, revision_id: &str) -> String {
    format!("{function_arn}/revision/{revision_id}")
}

/// The `functionName` a request names, resolved to the key the store holds.
///
/// The member's documentation says the caller "can specify either of the
/// following: The function name, for example my-function. The function ARN."
/// (source: `smithy_query(query="doc", shape="GetWebFunctionRequest",
/// member="functionName")`), so a value in the ARN form resolves to the name in
/// its `web-function/<name>` resource segment. Anything else is already a name.
pub fn resolve_function_name(value: &str) -> &str {
    if !value.starts_with("arn:") {
        return value;
    }
    value
        .split_once(":web-function/")
        .map_or(value, |(_, name)| name)
}

/// Why `name` is not a legal `functionName`, or `None` when it is.
///
/// The model bounds the member's length (1..256) only; this is the narrower rule
/// its documentation states — letters, numbers, hyphens and underscores, never
/// starting or ending with a hyphen or an underscore, at most
/// [`FUNCTION_NAME_MAX_CHARS`] characters — which the boundary validator cannot
/// enforce because the model carries no pattern for it.
pub fn function_name_violation(name: &str) -> Option<String> {
    resource_name_violation("functionName", name, FUNCTION_NAME_MAX_CHARS)
}

/// Why `name` is not a legal value for the `member` that names a resource of this
/// service, or `None` when it is.
///
/// One rule, stated identically by `functionName` and by `endpointName`: letters,
/// numbers, hyphens and underscores, never starting or ending with a hyphen or an
/// underscore, at most `max_chars` characters. The model gives those members a
/// length bound only, so the boundary validator cannot enforce it and every create
/// path checks it here.
pub fn resource_name_violation(member: &str, name: &str, max_chars: usize) -> Option<String> {
    if name.is_empty() {
        return Some(format!(
            "Member must have length greater than or equal to 1: {member}"
        ));
    }
    if name.chars().count() > max_chars {
        return Some(format!(
            "Member must have length less than or equal to {max_chars}: {member}"
        ));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Some(format!(
            "Member must contain only letters, numbers, hyphens (-) and underscores (_): {member}"
        ));
    }
    let first = name.chars().next();
    let last = name.chars().next_back();
    if matches!(first, Some('-' | '_')) || matches!(last, Some('-' | '_')) {
        return Some(format!(
            "Member must not begin or end with a hyphen (-) or an underscore (_): {member}"
        ));
    }
    None
}

/// 32 hex digits — `high` then `low`, 16 each — rendered in the 8-4-4-4-12 shape,
/// which satisfies this service's `^[a-f0-9-]{36}$` token and revision-id
/// patterns.
fn hex_uuid_shape(high: u64, low: u64) -> String {
    let digits = format!("{high:016x}{low:016x}");
    format!(
        "{}-{}-{}-{}-{}",
        &digits[0..8],
        &digits[8..12],
        &digits[12..16],
        &digits[16..20],
        &digits[20..32]
    )
}

/// Where a `ListWebFunctions` page resumes: the digest of the key the page starts
/// at, and that key's index in the listing the cursor was minted from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListCursor {
    /// The first 8 bytes of SHA-256 over the resume key, so a cursor still finds
    /// its key after the listing shifted.
    pub digest: u64,
    /// The resume key's index when the cursor was minted — the fallback when the
    /// key itself is gone.
    pub offset: usize,
}

/// The first 8 bytes of SHA-256 over `key`, as the cursor's validator.
fn key_digest(key: &str) -> u64 {
    let digest = Sha256::digest(key.as_bytes());
    let mut head = [0u8; 8];
    head.copy_from_slice(&digest[..8]);
    u64::from_be_bytes(head)
}

/// The opaque `nextToken` for a page that resumes at `key`, the `offset`-th item
/// of the listing it was minted from.
///
/// Self-describing rather than stored: nothing a client can read structure out of,
/// nothing the service has to keep, and — as the model's
/// `NextToken` pattern `^[a-f0-9-]{36}$` demands — 36 characters of lowercase hex
/// and hyphens.
pub fn encode_cursor(key: &str, offset: usize) -> String {
    hex_uuid_shape(key_digest(key), u64::try_from(offset).unwrap_or(u64::MAX))
}

/// The cursor a `nextToken` carries, or `None` when the token is not one this
/// service minted (the caller answers that with a `ValidationException`).
pub fn decode_cursor(token: &str) -> Option<ListCursor> {
    let groups: Vec<&str> = token.split('-').collect();
    if groups.iter().map(|group| group.len()).ne([8usize, 4, 4, 4, 12]) {
        return None;
    }
    let digits = groups.concat();
    // Lowercase only, as the model's `^[a-f0-9-]{36}$` pattern says and as
    // `encode_cursor` always mints: an upper-cased token is not one this service
    // handed out.
    if !digits.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')) {
        return None;
    }
    let digest = u64::from_str_radix(&digits[0..16], 16).ok()?;
    let offset = u64::from_str_radix(&digits[16..32], 16).ok()?;
    Some(ListCursor {
        digest,
        offset: usize::try_from(offset).ok()?,
    })
}

/// The index in `keys` that `cursor` resumes at.
///
/// The cursor's own offset when the key there still hashes to the cursor's
/// digest; else wherever that key moved to; else — the key was deleted between
/// pages — the offset it pointed at, clamped to the end of the listing.
pub fn resume_index(cursor: ListCursor, keys: &[&str]) -> usize {
    if keys
        .get(cursor.offset)
        .is_some_and(|key| key_digest(key) == cursor.digest)
    {
        return cursor.offset;
    }
    keys.iter()
        .position(|key| key_digest(key) == cursor.digest)
        .unwrap_or_else(|| cursor.offset.min(keys.len()))
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsLambdawebLambdawebState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsLambdawebLambdawebState>)
}
