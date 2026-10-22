// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-qbusiness-expertq service.
//!
//! The service's primary resource is the Amazon Q Business **application**, which owns
//! its child **indices**. Every implemented operation computes its answer from the
//! records below; nothing is canned and nothing is echoed straight back out of a
//! request.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    AttachmentsConfiguration, DocumentAttributeConfiguration, EncryptionConfiguration,
    IndexCapacityConfiguration, PersonalizationConfiguration, QAppsConfiguration,
    QuickSightConfiguration, Tag,
};

/// `com.amazonaws.qbusiness#ApplicationStatus` / `#IndexStatus` value a resource carries
/// while it is still being provisioned.
pub const STATUS_CREATING: &str = "CREATING";

/// `com.amazonaws.qbusiness#ApplicationStatus` / `#IndexStatus` value a provisioned
/// resource carries. The model documents it as the ready state: "The application is ready
/// to use when the status is `ACTIVE`."
pub const STATUS_ACTIVE: &str = "ACTIVE";

/// How long an application or index stays `CREATING` before the clock owes it `ACTIVE`.
///
/// A resource the service reports in a non-terminal status has a transition the clock
/// owes; the span is deliberately non-zero so a caller that reads straight after the
/// create still observes `CREATING`, as it does against the real service. The instant it
/// comes due is stored on the record (see [`StoredApplication::active_due_at`]) and
/// materialised by [`ComAmazonawsQbusinessExpertqState::settle`]; nothing ever sleeps.
pub const PROVISIONING_SPAN: Duration = Duration::from_millis(500);

/// The signing name (and ARN service namespace) of this service, from the model's
/// `aws.auth#sigv4` / `aws.api#service` traits.
pub const ARN_SERVICE: &str = "qbusiness";

/// Region used for ARNs when a request carries no SigV4 credential scope to read one
/// from (an unsigned request still routes by endpoint host).
pub const DEFAULT_REGION: &str = "us-east-1";

/// `maxResults` upper bound, from the `@range` on `MaxResultsIntegerForListApplications`
/// and `MaxResultsIntegerForListIndices` (both 1..100). Also the page size used when a
/// caller names none.
pub const MAX_RESULTS_LIMIT: i32 = 100;

/// How many applications this emulator will hold for the account.
///
/// The real service caps applications per account per Region with a service quota, and
/// `CreateApplication` models `ServiceQuotaExceededException` (HTTP 402) for exceeding it.
/// The number here is NOT a fidelity claim about the published quota (which is far
/// smaller and adjustable): it is the emulator's own ceiling, set generously so no
/// realistic workflow meets it, so that a caller looping on create cannot grow this
/// process's memory without bound.
pub const MAX_APPLICATIONS: usize = 1_000;

/// How many indices this emulator will hold under one application. Same reasoning, and
/// `CreateIndex` models the same `ServiceQuotaExceededException`.
pub const MAX_INDICES_PER_APPLICATION: usize = 1_000;

/// How many `clientToken` -> resource-id records are kept for idempotent replay.
///
/// Tokens are caller-supplied, so the map is caller-driven growth that holds no resource
/// of its own. Records naming a deleted resource are dropped with it
/// ([`ComAmazonawsQbusinessExpertqState::forget_client_tokens_for`]), and beyond this many
/// live records the oldest is evicted. An evicted token is indistinguishable from one
/// whose idempotency window has passed: the next create carrying it mints a new resource.
pub const MAX_REMEMBERED_CLIENT_TOKENS: usize = 10_000;

/// A stored Amazon Q Business application: everything `CreateApplication` was given plus
/// what the service itself minted.
#[derive(Debug, Clone)]
pub struct StoredApplication {
    pub application_id: String,
    pub application_arn: String,
    pub display_name: String,
    pub description: Option<String>,
    pub identity_type: Option<String>,
    pub iam_identity_provider_arn: Option<String>,
    pub identity_center_instance_arn: Option<String>,
    pub client_ids_for_oidc: Option<Vec<String>>,
    pub role_arn: Option<String>,
    pub encryption_configuration: Option<EncryptionConfiguration>,
    pub attachments_configuration: Option<AttachmentsConfiguration>,
    pub q_apps_configuration: Option<QAppsConfiguration>,
    pub personalization_configuration: Option<PersonalizationConfiguration>,
    pub quick_sight_configuration: Option<QuickSightConfiguration>,
    pub tags: Vec<Tag>,
    /// An `ApplicationStatus` value — [`STATUS_CREATING`] until the clock owes
    /// [`STATUS_ACTIVE`].
    pub status: String,
    /// When `CREATING` becomes `ACTIVE`.
    pub active_due_at: SystemTime,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    /// Creation order, so listings paginate deterministically.
    pub sequence: u64,
    pub indices: HashMap<String, StoredIndex>,
}

/// A stored index, a child of exactly one application.
#[derive(Debug, Clone)]
pub struct StoredIndex {
    pub index_id: String,
    pub index_arn: String,
    pub application_id: String,
    pub display_name: String,
    pub description: Option<String>,
    /// An `IndexType` value (`ENTERPRISE` | `STARTER`), absent when the caller named none.
    pub index_type: Option<String>,
    pub capacity_configuration: Option<IndexCapacityConfiguration>,
    pub document_attribute_configurations: Vec<DocumentAttributeConfiguration>,
    pub tags: Vec<Tag>,
    /// An `IndexStatus` value — [`STATUS_CREATING`] until the clock owes
    /// [`STATUS_ACTIVE`].
    pub status: String,
    /// When `CREATING` becomes `ACTIVE`.
    pub active_due_at: SystemTime,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    /// Creation order within the owning application.
    pub sequence: u64,
}

/// Apply the `CREATING` -> `ACTIVE` transition in place once `now` has reached `due`.
fn settle_status(
    status: &mut String,
    due: SystemTime,
    updated_at: &mut SystemTime,
    now: SystemTime,
) {
    if status == STATUS_CREATING && now >= due {
        *status = STATUS_ACTIVE.to_string();
        // `updated_at` moves to the instant the transition came due, not to "now": the
        // change happened when the clock made it due, whether or not anyone looked.
        *updated_at = due.max(*updated_at);
    }
}

impl StoredApplication {
    /// Materialise the transition the clock already owes this application and its
    /// indices.
    fn settle(&mut self, now: SystemTime) {
        settle_status(
            &mut self.status,
            self.active_due_at,
            &mut self.updated_at,
            now,
        );
        for index in self.indices.values_mut() {
            index.settle(now);
        }
    }

    /// The indices of this application in creation order.
    pub fn indices_in_order(&self) -> Vec<&StoredIndex> {
        let mut ordered: Vec<&StoredIndex> = self.indices.values().collect();
        ordered.sort_by_key(|index| index.sequence);
        ordered
    }
}

impl StoredIndex {
    fn settle(&mut self, now: SystemTime) {
        settle_status(
            &mut self.status,
            self.active_due_at,
            &mut self.updated_at,
            now,
        );
    }
}

/// Per-service state for com-amazonaws-qbusiness-expertq. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsQbusinessExpertqState {
    /// Every application, keyed by its minted `applicationId`.
    pub applications: HashMap<String, StoredApplication>,
    /// Monotonic creation counter. It orders list pages and backs the opaque `nextToken`,
    /// so a cursor keeps resuming correctly across deletes.
    next_sequence: u64,
    /// `<scope>/<clientToken>` -> the resource id the FIRST request carrying that token
    /// minted, so an `@idempotencyToken` replay returns that id instead of minting a
    /// second resource. The scope is the operation plus, for a child resource, the parent
    /// it is created under, so one token cannot be shared across two parents.
    client_tokens: HashMap<String, String>,
    /// The keys of [`Self::client_tokens`] in the order they were first recorded, so the
    /// oldest record is the one evicted once [`MAX_REMEMBERED_CLIENT_TOKENS`] is reached.
    client_token_order: VecDeque<String>,
}

impl ComAmazonawsQbusinessExpertqState {
    /// The next creation sequence number.
    pub fn next_sequence(&mut self) -> u64 {
        self.next_sequence += 1;
        self.next_sequence
    }

    /// Materialise every transition the clock already owes. Called at the top of each
    /// handler — including the reporting ones, which may only write a change that was
    /// already due at this instant.
    pub fn settle(&mut self, now: SystemTime) {
        for application in self.applications.values_mut() {
            application.settle(now);
        }
    }

    /// The id a previous request carrying this `clientToken` in this `scope` minted, if
    /// any.
    pub fn replayed_id(&self, scope: &str, client_token: Option<&str>) -> Option<String> {
        let token = client_token?;
        self.client_tokens.get(&token_key(scope, token)).cloned()
    }

    /// Record which resource a `clientToken` minted in this `scope`, evicting the oldest
    /// record once [`MAX_REMEMBERED_CLIENT_TOKENS`] are held.
    pub fn remember_client_token(&mut self, scope: &str, client_token: Option<&str>, id: &str) {
        let Some(token) = client_token else {
            return;
        };
        let key = token_key(scope, token);
        if self.client_tokens.insert(key.clone(), id.to_string()).is_none() {
            self.client_token_order.push_back(key);
        }
        while self.client_token_order.len() > MAX_REMEMBERED_CLIENT_TOKENS {
            if let Some(oldest) = self.client_token_order.pop_front() {
                self.client_tokens.remove(&oldest);
            }
        }
    }

    /// Drop the idempotency records naming any of `ids`.
    ///
    /// Called when a resource is deleted: the record can no longer replay anything, and
    /// nothing else would ever reclaim it.
    pub fn forget_client_tokens_for(&mut self, ids: &[String]) {
        if ids.is_empty() {
            return;
        }
        let gone: std::collections::HashSet<&str> = ids.iter().map(String::as_str).collect();
        let Self {
            client_tokens,
            client_token_order,
            ..
        } = self;
        client_tokens.retain(|_, id| !gone.contains(id.as_str()));
        client_token_order.retain(|key| client_tokens.contains_key(key));
    }

    /// Applications in creation order.
    pub fn applications_in_order(&self) -> Vec<&StoredApplication> {
        let mut ordered: Vec<&StoredApplication> = self.applications.values().collect();
        ordered.sort_by_key(|application| application.sequence);
        ordered
    }
}

fn token_key(scope: &str, client_token: &str) -> String {
    format!("{scope}/{client_token}")
}

/// Mint a server-generated resource id.
///
/// `ApplicationId`, `IndexId` and the service's other ids are `@length(36, 36)` strings
/// matching `^[a-zA-Z0-9][a-zA-Z0-9-]{35}$`; a hyphenated UUID is exactly that shape, so
/// the id is generated (never derived from the request) and satisfies the model's own
/// output constraint.
pub fn mint_resource_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// The region this request was signed for, read from its SigV4 credential scope.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The ARN of an application: `arn:aws:qbusiness:<region>:<account>:application/<id>`.
pub fn application_arn(region: &str, application_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("application/{application_id}"),
    )
}

/// The ARN of an index, which AWS nests under its application:
/// `arn:aws:qbusiness:<region>:<account>:application/<applicationId>/index/<indexId>`
/// (the resource shape the Amazon Q Business service-authorization reference publishes
/// for the `index` resource type).
pub fn index_arn(region: &str, application_id: &str, index_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("application/{application_id}/index/{index_id}"),
    )
}

/// One page of a `@paginated` listing: the items to return, plus the cursor that resumes
/// exactly where the page ended (`None` when the page is the last one).
#[derive(Debug, Clone)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_token: Option<String>,
}

/// A `nextToken` no listing of this service ever minted.
#[derive(Debug, Clone, Copy)]
pub struct MalformedCursor;

/// Encode the resume cursor that follows the item with sequence number `sequence`.
///
/// Opaque to the caller (hex of the internal cursor text) and well within `NextToken`'s
/// `@length(1, 800)`.
pub fn encode_cursor(sequence: u64) -> String {
    ::hex::encode(format!("after-sequence:{sequence}"))
}

/// Decode a cursor minted by [`encode_cursor`]; `None` starts at the beginning.
pub fn decode_cursor(next_token: Option<&str>) -> Result<u64, MalformedCursor> {
    let Some(token) = next_token else {
        return Ok(0);
    };
    let bytes = ::hex::decode(token).map_err(|_| MalformedCursor)?;
    let text = String::from_utf8(bytes).map_err(|_| MalformedCursor)?;
    text.strip_prefix("after-sequence:")
        .and_then(|sequence| sequence.parse::<u64>().ok())
        .ok_or(MalformedCursor)
}

/// Take the page of `ordered` (sorted by sequence number) that follows `after`, at most
/// `max_results` long.
///
/// `max_results` is `@range(1, 100)` on both listings, so the boundary validator has
/// already refused anything outside it; an absent one takes the full page.
pub fn page_of<T>(
    ordered: impl IntoIterator<Item = (u64, T)>,
    after: u64,
    max_results: Option<i32>,
) -> Page<T> {
    let limit = max_results
        .unwrap_or(MAX_RESULTS_LIMIT)
        .clamp(1, MAX_RESULTS_LIMIT) as usize;
    let mut items = Vec::new();
    let mut last_sequence = after;
    let mut more_follow = false;
    for (sequence, item) in ordered.into_iter().filter(|(sequence, _)| *sequence > after) {
        if items.len() == limit {
            // One item beyond the page proves a next page exists; the cursor names where
            // this page ended, so resuming never repeats or skips an item.
            more_follow = true;
            break;
        }
        last_sequence = sequence;
        items.push(item);
    }
    Page {
        items,
        next_token: more_follow.then(|| encode_cursor(last_sequence)),
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsQbusinessExpertqState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsQbusinessExpertqState>)
}
