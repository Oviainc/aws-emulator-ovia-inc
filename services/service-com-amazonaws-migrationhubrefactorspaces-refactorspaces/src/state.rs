// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-migrationhubrefactorspaces-refactorspaces service.
//!
//! ONE store behind the engine's per-service lock, holding the Refactor Spaces
//! control-plane tree: every environment, and the applications nested under it. Every
//! handler in `src/ops/` reads and writes it through [`service_state`], so a create is
//! observable by every later read and a delete is observable as absence.
//!
//! The identifier minting, pagination-token codec and the clock-owed state transition
//! live here too, beside the data they act on, because several operations share them:
//! `CreateEnvironment` and `CreateApplication` both mint ids, `ListEnvironments` and
//! `ListApplications` both cut pages, and every reporting operation materialises the
//! transition a create already owed.

use std::collections::BTreeMap;
use std::time::{Duration, Instant, SystemTime};

use ::aws_smithy_types::DateTime;
use sha2::{Digest, Sha256};

/// The ARN namespace of this service, from the model's `aws.api#service`
/// `arnNamespace` (`smithy_query(query="service_info")`). Every ARN this service
/// stores is built from it through [`::engine::emulator_arn`].
pub const ARN_SERVICE: &str = "refactor-spaces";

/// The region an ARN is built in when the request carries no SigV4 credential scope to
/// read one from — the emulator's conventional default.
pub const DEFAULT_REGION: &str = "us-east-1";

/// `EnvironmentState` / `ApplicationState` members of this service's model
/// (`smithy_query(query="shape",shape="EnvironmentState")`): the only state values
/// these handlers ever store.
pub const STATE_CREATING: &str = "CREATING";
/// See [`STATE_CREATING`].
pub const STATE_ACTIVE: &str = "ACTIVE";
/// See [`STATE_CREATING`].
pub const STATE_DELETING: &str = "DELETING";

/// `ErrorResourceType` members this slice reports on a `ResourceNotFoundException`
/// (`smithy_query(query="shape",shape="ErrorResourceType")`).
pub const RESOURCE_TYPE_ENVIRONMENT: &str = "ENVIRONMENT";
/// See [`RESOURCE_TYPE_ENVIRONMENT`].
pub const RESOURCE_TYPE_APPLICATION: &str = "APPLICATION";

/// `ApiGatewayProxyInput$EndpointType` documents "If no value is specified in the
/// request, the value is set to `REGIONAL` by default."
pub const DEFAULT_ENDPOINT_TYPE: &str = "REGIONAL";

/// `ApiGatewayProxyInput$StageName` documents "The name of the API Gateway stage. The
/// name defaults to `prod`."
pub const DEFAULT_STAGE_NAME: &str = "prod";

/// How long after the creating call a `CREATING` resource is owed its move to `ACTIVE`.
///
/// AWS documents no duration for it, so this span is CHOSEN: it sits inside the
/// emulator's 250-750 ms window, which is deliberately non-zero so a caller acting
/// straight after `CreateEnvironment` still observes the transient `CREATING` state the
/// real service reports.
pub const PROVISIONING_SPAN: Duration = Duration::from_millis(500);

/// The ceiling on how many environments this store holds, and on how many applications
/// one environment holds: the two caps that bound caller-driven growth.
///
/// A generated emulator keeps its resources in memory for the life of the process and a
/// create is the only thing that adds one, so the store needs a maximum entry count —
/// neither a TTL nor an eviction policy is available to it, because `GetEnvironment` must
/// still answer what `CreateEnvironment` created. The two operations that grow the store
/// refuse past these caps with the service's own modelled `ServiceQuotaExceededException`
/// (HTTP 402), which Refactor Spaces declares on exactly those two operations and on no
/// read. The caps also bound the per-request, lock-held scans in
/// [`ComAmazonawsMigrationhubrefactorspacesRefactorspacesState::materialise_due_states`]
/// and `id_in_use`.
///
/// The numbers are the emulator's OWN bound, not a published AWS default: AWS publishes
/// both Refactor Spaces quotas as adjustable, so no single value is the contract, and
/// these are set high enough that no ordinary workflow meets one.
pub const MAX_ENVIRONMENTS: usize = 2_000;

/// See [`MAX_ENVIRONMENTS`].
pub const MAX_APPLICATIONS_PER_ENVIRONMENT: usize = 500;

/// The `serviceCode` a `ServiceQuotaExceededException` from this service reports, from the
/// model's `aws.api#service` `endpointPrefix` (`smithy_query(query="service_info")`).
pub const SERVICE_CODE: &str = "refactor-spaces";

/// The alphabet the `^env-[0-9A-Za-z]{10}$` / `^app-[0-9A-Za-z]{10}$` id patterns allow.
const ID_ALPHABET: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// The length of the minted part of an id: both patterns pin it at exactly 10.
const ID_BODY_LEN: usize = 10;

/// The alphabet and length of the `^tgw-[-a-f0-9]{17}$` transit-gateway id pattern.
const TGW_ALPHABET: &[u8] = b"0123456789abcdef";
/// See [`TGW_ALPHABET`].
const TGW_BODY_LEN: usize = 17;

/// How many candidates a mint tries before giving up.
///
/// A candidate collides only if the digest of a fresh sequence number lands on an id
/// already in the store, which the growth caps make vanishingly unlikely; bounding the
/// retry turns "pathological store" from an unbounded spin on the request thread into an
/// explicit `InternalServerException` the caller can see.
const MINT_ATTEMPTS: u32 = 64;

/// The region to build this request's ARNs in: the one the caller signed for, else
/// [`DEFAULT_REGION`].
///
/// Read once at create time and stored on the record, so the ARN a later read returns is
/// the stored one and never depends on who is reading.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The API Gateway proxy configuration an application was created with.
///
/// Only the two members `ApiGatewayProxyInput` carries are stored. The members that
/// describe resources the real service PROVISIONS (`ProxyUrl`, `ApiGatewayId`,
/// `VpcLinkId`, `NlbArn`, `NlbName`) are deliberately absent: this emulator creates no
/// API Gateway, VPC link or load balancer, and inventing their ids would report work
/// nothing did.
#[derive(Debug, Clone)]
pub struct ApiGatewayProxy {
    /// `REGIONAL` or `PRIVATE`; [`DEFAULT_ENDPOINT_TYPE`] when the request omitted it.
    pub endpoint_type: String,
    /// The API Gateway stage name; [`DEFAULT_STAGE_NAME`] when the request omitted it.
    pub stage_name: String,
}

/// A stored Refactor Spaces application, owned by exactly one [`Environment`].
#[derive(Debug, Clone)]
pub struct Application {
    pub application_id: String,
    pub environment_id: String,
    pub name: String,
    pub arn: String,
    pub vpc_id: String,
    pub proxy_type: String,
    pub api_gateway_proxy: Option<ApiGatewayProxy>,
    pub owner_account_id: String,
    pub created_by_account_id: String,
    pub state: String,
    pub tags: BTreeMap<String, String>,
    pub created_time: DateTime,
    pub last_updated_time: DateTime,
    /// The `@idempotencyToken` the creating request carried, if any.
    pub client_token: Option<String>,
    /// When the clock owes this record its `CREATING` -> `ACTIVE` move; `None` once the
    /// move has been materialised.
    pub active_at: Option<Instant>,
}

/// A stored Refactor Spaces environment, with the applications created inside it in
/// creation order.
#[derive(Debug, Clone)]
pub struct Environment {
    pub environment_id: String,
    pub name: String,
    pub arn: String,
    pub description: Option<String>,
    pub network_fabric_type: String,
    /// Set only for a `TRANSIT_GATEWAY` environment, which is the one the real service
    /// provisions a transit gateway for.
    pub transit_gateway_id: Option<String>,
    pub owner_account_id: String,
    pub state: String,
    pub tags: BTreeMap<String, String>,
    pub created_time: DateTime,
    pub last_updated_time: DateTime,
    /// The `@idempotencyToken` the creating request carried, if any.
    pub client_token: Option<String>,
    /// See [`Application::active_at`].
    pub active_at: Option<Instant>,
    /// Applications of this environment, in creation order, so a page boundary is stable
    /// across calls.
    pub applications: Vec<Application>,
}

impl Environment {
    /// The application with `application_id`, if this environment holds it.
    pub fn application(&self, application_id: &str) -> Option<&Application> {
        self.applications
            .iter()
            .find(|app| app.application_id == application_id)
    }

    /// The application created under `client_token`, if any — the idempotent replay of
    /// `CreateApplication`.
    pub fn application_by_client_token(&self, client_token: &str) -> Option<&Application> {
        self.applications
            .iter()
            .find(|app| app.client_token.as_deref() == Some(client_token))
    }
}

/// Per-service state for com-amazonaws-migrationhubrefactorspaces-refactorspaces. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsMigrationhubrefactorspacesRefactorspacesState {
    /// Environments in creation order, so list pagination is stable across calls.
    pub environments: Vec<Environment>,
    /// Resource policies by resource ARN. Still empty: `PutResourcePolicy`,
    /// `GetResourcePolicy` and `DeleteResourcePolicy` are scaffolded (HTTP 501).
    pub resource_policies: BTreeMap<String, String>,
    /// Feeds [`Self::mint_id`]; monotonic for the life of the store.
    id_sequence: u64,
}

impl ComAmazonawsMigrationhubrefactorspacesRefactorspacesState {
    /// The environment with `environment_id`, if the store holds it.
    pub fn environment(&self, environment_id: &str) -> Option<&Environment> {
        self.environments
            .iter()
            .find(|env| env.environment_id == environment_id)
    }

    /// The environment with `environment_id`, mutably.
    pub fn environment_mut(&mut self, environment_id: &str) -> Option<&mut Environment> {
        self.environments
            .iter_mut()
            .find(|env| env.environment_id == environment_id)
    }

    /// The environment created under `client_token`, if any — the idempotent replay of
    /// `CreateEnvironment`.
    pub fn environment_by_client_token(&self, client_token: &str) -> Option<&Environment> {
        self.environments
            .iter()
            .find(|env| env.client_token.as_deref() == Some(client_token))
    }

    /// Remove and return the environment with `environment_id`, keeping the creation
    /// order of the rest.
    pub fn remove_environment(&mut self, environment_id: &str) -> Option<Environment> {
        let at = self
            .environments
            .iter()
            .position(|env| env.environment_id == environment_id)?;
        Some(self.environments.remove(at))
    }

    /// Mint a fresh identifier `<prefix>-<10 chars of [0-9A-Za-z]>`, unused by any record
    /// this store holds, or `None` when [`MINT_ATTEMPTS`] candidates were all taken.
    ///
    /// The body is a digest of the prefix, the store's monotonic sequence number and the
    /// wall clock: the sequence is what makes the id unique within one store, and the
    /// digest is only what spreads it over the pattern's alphabet so it reads like a real
    /// one. It is NOT drawn from a cryptographic source and is not claimed to be
    /// unguessable; the ids name in-memory fixtures, and the emulator's authorization is
    /// the request signature, never knowledge of an id.
    pub fn mint_id(&mut self, prefix: &str) -> Option<String> {
        (0..MINT_ATTEMPTS).find_map(|_| {
            self.id_sequence += 1;
            let candidate = format!(
                "{prefix}-{}",
                derive_body(prefix, self.id_sequence, ID_ALPHABET, ID_BODY_LEN)
            );
            (!self.id_in_use(&candidate)).then_some(candidate)
        })
    }

    /// Mint the `^tgw-[-a-f0-9]{17}$` id of the transit gateway a `TRANSIT_GATEWAY`
    /// environment provisions, unused by any environment this store holds, or `None` when
    /// [`MINT_ATTEMPTS`] candidates were all taken.
    pub fn mint_transit_gateway_id(&mut self) -> Option<String> {
        (0..MINT_ATTEMPTS).find_map(|_| {
            self.id_sequence += 1;
            let candidate = format!(
                "tgw-{}",
                derive_body("tgw", self.id_sequence, TGW_ALPHABET, TGW_BODY_LEN)
            );
            (!self.transit_gateway_id_in_use(&candidate)).then_some(candidate)
        })
    }

    /// Whether any environment already reports `id` as its transit gateway.
    fn transit_gateway_id_in_use(&self, id: &str) -> bool {
        self.environments
            .iter()
            .any(|env| env.transit_gateway_id.as_deref() == Some(id))
    }

    /// Whether any environment or application already carries `id`.
    fn id_in_use(&self, id: &str) -> bool {
        self.environments.iter().any(|env| {
            env.environment_id == id
                || env
                    .applications
                    .iter()
                    .any(|app| app.application_id == id)
        })
    }

    /// Materialise every `CREATING` -> `ACTIVE` move the clock has already made due.
    ///
    /// This is the ONE write a reporting operation performs: the move was owed at the
    /// same instant whether or not anybody called, so applying it on read changes
    /// nothing a later caller would not have seen anyway. Nothing sleeps — the due
    /// instant is stored by the creating call and compared here.
    pub fn materialise_due_states(&mut self) {
        let now = Instant::now();
        let updated = DateTime::from(SystemTime::now());
        for env in &mut self.environments {
            activate_if_due(&mut env.state, &mut env.active_at, &mut env.last_updated_time, now, updated);
            for app in &mut env.applications {
                activate_if_due(&mut app.state, &mut app.active_at, &mut app.last_updated_time, now, updated);
            }
        }
    }
}

/// Apply one record's owed `CREATING` -> `ACTIVE` move when `now` has reached its due
/// instant, stamping `LastUpdatedTime` because the record changed.
fn activate_if_due(
    state: &mut String,
    active_at: &mut Option<Instant>,
    last_updated_time: &mut DateTime,
    now: Instant,
    updated: DateTime,
) {
    if active_at.is_some_and(|due| now >= due) {
        *state = STATE_ACTIVE.to_string();
        *active_at = None;
        *last_updated_time = updated;
    }
}

/// The minted body of an id: `len` characters of `alphabet`, derived from a SHA-256 of
/// the prefix, the store's sequence number and the wall clock.
fn derive_body(prefix: &str, sequence: u64, alphabet: &[u8], len: usize) -> String {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    let mut hasher = Sha256::new();
    hasher.update(prefix.as_bytes());
    hasher.update(sequence.to_be_bytes());
    hasher.update(nanos.to_be_bytes());
    let digest = hasher.finalize();
    digest
        .iter()
        .take(len)
        .map(|byte| char::from(alphabet[usize::from(*byte) % alphabet.len()]))
        .collect()
}

/// One page of a list operation: the items to answer with, and the token that resumes
/// after the last of them when more remain.
#[derive(Debug)]
pub struct Page<'a, T> {
    pub items: &'a [T],
    pub next_token: Option<String>,
}

/// Why a page could not be cut. Each variant is refused explicitly rather than silently
/// rewritten into something the caller did not ask for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageError {
    /// The caller's `NextToken` resumes after an item this list no longer holds, so there
    /// is no page it names.
    UnknownToken,
    /// `MaxResults` was not a positive count. The modelled `range 1..100` means the
    /// boundary validator refuses this before any handler runs, so it is reachable only
    /// from a programmatic caller — which is told so rather than handed one item.
    MaxResultsNotPositive,
}

/// Encode the opaque `NextToken` that resumes a listing after `last_id`.
///
/// Hex keeps the token inside the modelled `^[a-zA-Z0-9/\+\=]{0,2048}$` pattern and makes
/// it opaque to a caller, who must hand it back rather than construct one.
pub fn encode_page_token(last_id: &str) -> String {
    ::hex::encode(last_id)
}

/// The id a `NextToken` resumes after, or `None` when it is not one this service minted.
pub fn decode_page_token(token: &str) -> Option<String> {
    String::from_utf8(::hex::decode(token).ok()?).ok()
}

/// Cut the page `next_token` and `max_results` name out of `items`, which is in a stable
/// creation order, so paging through a list yields each item exactly once.
///
/// `id_of` names each item's identifier, which is what the token carries.
pub fn page<'a, T>(
    items: &'a [T],
    id_of: impl Fn(&T) -> &str,
    next_token: Option<&str>,
    max_results: Option<i32>,
) -> Result<Page<'a, T>, PageError> {
    if max_results.is_some_and(|max| max < 1) {
        return Err(PageError::MaxResultsNotPositive);
    }
    let start = match next_token {
        None => 0,
        Some(token) => {
            let last = decode_page_token(token).ok_or(PageError::UnknownToken)?;
            items
                .iter()
                .position(|item| id_of(item) == last)
                .ok_or(PageError::UnknownToken)?
                + 1
        }
    };
    let total = items.len();
    let start = start.min(total);
    // Every `MaxResults` that reaches here is at least 1, either because the boundary
    // validator held it to the modelled `range 1..100` or because the guard above
    // refused it, so a page is never empty while items remain.
    let limit = max_results.map_or(total, |max| max as usize);
    let end = start.saturating_add(limit).min(total);
    let items = &items[start..end];
    let next_token = if end < total {
        items.last().map(|item| encode_page_token(id_of(item)))
    } else {
        None
    };
    Ok(Page { items, next_token })
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsMigrationhubrefactorspacesRefactorspacesState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsMigrationhubrefactorspacesRefactorspacesState>)
}
