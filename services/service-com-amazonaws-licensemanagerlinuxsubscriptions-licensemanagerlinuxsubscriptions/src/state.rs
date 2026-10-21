// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-licensemanagerlinuxsubscriptions-licensemanagerlinuxsubscriptions service.
//!
//! One store holds every resource this service's operations share: the registered
//! third-party subscription providers and the per-account Linux subscriptions service
//! settings. The lookup / list / pagination helpers live here too, so the operation
//! handlers in `ops/` all resolve a resource the same way instead of each growing its
//! own copy.

use std::collections::HashMap;

use crate::ops::types::{LinuxSubscriptionsDiscoverySettings, RegisteredSubscriptionProvider};

/// `SubscriptionProviderStatus` for a provider whose access token License Manager holds
/// and accepted (`SubscriptionProviderStatus`: ACTIVE | INVALID | PENDING).
pub const PROVIDER_STATUS_ACTIVE: &str = "ACTIVE";

/// The `LinuxSubscriptionsDiscovery` value an account that never called
/// `UpdateServiceSettings` reports (`LinuxSubscriptionsDiscovery`: Disabled | Enabled).
pub const DISCOVERY_DISABLED: &str = "Disabled";

/// The `Status` of service settings that are applied (`Status`: Completed | Failed |
/// InProgress | Successful).
pub const SETTINGS_STATUS_SUCCESSFUL: &str = "Successful";

/// Why an operation could not be served from state.
///
/// Each handler maps this onto its OWN modelled error enum (the generated enums are
/// per-operation types), so the lookup rules themselves live here exactly once.
#[derive(Debug, Clone)]
pub enum StateError {
    /// The addressed resource is not in the store — `ResourceNotFoundException`.
    NotFound(String),
    /// The request is malformed in a way the model cannot express — `ValidationException`.
    Validation(String),
}

/// A registered Linux subscription provider, as `RegisterSubscriptionProvider` stored it.
#[derive(Debug, Clone)]
pub struct SubscriptionProviderRecord {
    /// The minted `SubscriptionProviderArn`; the key of `providers`.
    pub arn: String,
    /// The `SubscriptionProviderSource` that was registered (the model allows `RedHat`).
    pub source: String,
    /// The Secrets Manager secret holding the provider's access token.
    pub secret_arn: String,
    /// `SubscriptionProviderStatus` for the stored access token.
    pub status: String,
    /// Set only when the service has something to report about the token; never fabricated.
    pub status_message: Option<String>,
    /// When License Manager last retrieved third-party subscription data for this
    /// account. The emulator makes no outbound call to a third-party provider, so no
    /// retrieval has ever succeeded and this stays absent rather than being invented.
    pub last_successful_data_retrieval_time: Option<String>,
    /// The metadata tags the registration carried.
    pub tags: HashMap<String, String>,
}

/// The per-account Linux subscriptions service settings, as `UpdateServiceSettings`
/// persisted them.
#[derive(Debug, Clone)]
pub struct ServiceSettingsRecord {
    /// Whether discovery of Linux subscriptions is enabled.
    pub linux_subscriptions_discovery: String,
    /// The discovery settings: the source Regions and the Organizations integration.
    pub discovery_settings: LinuxSubscriptionsDiscoverySettings,
    /// The `Status` of the settings being applied.
    pub status: String,
    /// Detail about that status. Empty when the service has nothing to report.
    pub status_message: HashMap<String, String>,
    /// The Regions in which License Manager displays the aggregated data, in the order
    /// they became home Regions.
    pub home_regions: Vec<String>,
}

/// Per-service state for com-amazonaws-licensemanagerlinuxsubscriptions-licensemanagerlinuxsubscriptions. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsLicensemanagerlinuxsubscriptionsLicensemanagerlinuxsubscriptionsState {
    /// Registered providers by `SubscriptionProviderArn`.
    pub providers: HashMap<String, SubscriptionProviderRecord>,
    /// Provider ARNs in registration order, so a list is stable and pageable.
    pub provider_order: Vec<String>,
    /// `(source, secret ARN) -> SubscriptionProviderArn`, so re-registering the same
    /// provider resolves the record already in state instead of adding a second one.
    pub provider_index: HashMap<(String, String), String>,
    /// The per-account settings singleton, once an update persisted one.
    pub service_settings: Option<ServiceSettingsRecord>,
    /// Source of the resource ids this service mints.
    pub next_id: u64,
}

impl ComAmazonawsLicensemanagerlinuxsubscriptionsLicensemanagerlinuxsubscriptionsState {
    /// Mint the resource-id portion of a `SubscriptionProviderArn`.
    ///
    /// AWS puts a UUID there, so the id keeps that lowercase-hex 8-4-4-4-12 shape, which
    /// also satisfies the modelled `SubscriptionProviderArn` pattern's
    /// `[a-z0-9-\.]{1,510}` resource segment. The value is the monotonic registration
    /// counter rendered into that shape: unique within a run and never derived from the
    /// request.
    pub fn mint_provider_id(&mut self) -> String {
        self.next_id += 1;
        let counter = self.next_id;
        let high = (counter >> 32) as u32;
        format!("{high:08x}-0000-4000-8000-{counter:012x}")
    }

    /// The registered providers in registration order.
    pub fn providers_in_order(&self) -> Vec<SubscriptionProviderRecord> {
        self.provider_order
            .iter()
            .filter_map(|arn| self.providers.get(arn).cloned())
            .collect()
    }

    /// Resolve a registration by its `SubscriptionProviderArn`.
    ///
    /// `ResourceNotFoundException` when no provider with that ARN is registered.
    pub fn resolve_provider(&self, arn: &str) -> Result<&SubscriptionProviderRecord, StateError> {
        self.providers.get(arn).ok_or_else(|| {
            StateError::NotFound(format!("subscription provider {arn} is not registered"))
        })
    }

    /// Remove a registration from the store and from every index that reaches it.
    pub fn remove_provider(&mut self, arn: &str) -> Option<SubscriptionProviderRecord> {
        let record = self.providers.remove(arn)?;
        self.provider_order.retain(|stored| stored != arn);
        self.provider_index.retain(|_, indexed| indexed != arn);
        Some(record)
    }
}

/// The `RegisteredSubscriptionProvider` the model describes, built entirely from the
/// stored record — every member is what registration persisted.
pub fn registered_subscription_provider(
    record: &SubscriptionProviderRecord,
) -> RegisteredSubscriptionProvider {
    RegisteredSubscriptionProvider {
        subscription_provider_arn: Some(record.arn.clone()),
        subscription_provider_source: Some(record.source.clone()),
        secret_arn: Some(record.secret_arn.clone()),
        subscription_provider_status: Some(record.status.clone()),
        subscription_provider_status_message: record.status_message.clone(),
        last_successful_data_retrieval_time: record.last_successful_data_retrieval_time.clone(),
    }
}

/// The ceiling on how many subscription providers this store holds: the cap that bounds
/// caller-driven growth of the provider store.
///
/// A generated emulator keeps its resources in memory for the life of the process and a
/// registration is the only thing that adds one, so the store needs a maximum entry count
/// — neither a TTL nor an eviction policy is available to it, because
/// `GetRegisteredSubscriptionProvider` must still answer what `RegisterSubscriptionProvider`
/// registered. `RegisterSubscriptionProvider` refuses past this cap with its own modelled
/// `ValidationException` (HTTP 400), the only client error that operation declares for a
/// request it cannot accept. The number is the emulator's own bound, not a published AWS
/// default, and is set high enough that no ordinary workflow meets it.
pub const MAX_PROVIDERS: usize = 5_000;

/// The largest number of home Regions the settings may aggregate data in: the modelled
/// `HomeRegions` list is `len 1..100`, so a 101st home Region could not be reported.
pub const MAX_HOME_REGIONS: usize = 100;

/// What `GetServiceSettings` and `UpdateServiceSettings` report for this account.
///
/// Both operations answer with the same five members, so the mapping from the stored
/// singleton onto them lives here once. A member the account has nothing for is `None`
/// rather than an invented value.
#[derive(Debug, Clone, Default)]
pub struct ReportedServiceSettings {
    /// `LinuxSubscriptionsDiscovery`.
    pub linux_subscriptions_discovery: Option<String>,
    /// `LinuxSubscriptionsDiscoverySettings`, absent until an update persisted one.
    pub discovery_settings: Option<LinuxSubscriptionsDiscoverySettings>,
    /// `Status`.
    pub status: Option<String>,
    /// `StatusMessage`, absent while the service has nothing to report.
    pub status_message: Option<HashMap<String, String>>,
    /// `HomeRegions`, absent until an update made a Region the home Region.
    pub home_regions: Option<Vec<String>>,
}

/// The settings to report for this account: the persisted singleton, or the default an
/// account that never called `UpdateServiceSettings` has — discovery `Disabled`, with no
/// discovery settings and no home Region, since nothing has been configured.
pub fn reported_service_settings(
    settings: Option<&ServiceSettingsRecord>,
) -> ReportedServiceSettings {
    let Some(settings) = settings else {
        return ReportedServiceSettings {
            linux_subscriptions_discovery: Some(DISCOVERY_DISABLED.to_string()),
            discovery_settings: None,
            status: Some(SETTINGS_STATUS_SUCCESSFUL.to_string()),
            status_message: None,
            home_regions: None,
        };
    };
    ReportedServiceSettings {
        linux_subscriptions_discovery: Some(settings.linux_subscriptions_discovery.clone()),
        discovery_settings: Some(settings.discovery_settings.clone()),
        status: Some(settings.status.clone()),
        status_message: Some(settings.status_message.clone())
            .filter(|message| !message.is_empty()),
        home_regions: Some(settings.home_regions.clone()).filter(|regions| !regions.is_empty()),
    }
}

/// The region this request was signed for, or the emulator's default when the request
/// carries no usable SigV4 credential scope.
///
/// Values outside the ARN grammar's region segment (`[a-z0-9-.]{1,63}`) are ignored, so
/// a minted ARN always satisfies the modelled `SubscriptionProviderArn` pattern.
pub fn request_region(headers: &::http::HeaderMap) -> String {
    ::emulator_server::sigv4_credential_scope(headers)
        .map(|scope| scope.region)
        .filter(|region| {
            !region.is_empty()
                && region.len() <= 63
                && region
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.')
        })
        .unwrap_or_else(|| "us-east-1".to_string())
}

/// An ARN for one of this service's resources:
/// `arn:aws:<signing name>:<region>:<account>:<kind>/<id>`.
pub fn resource_arn(signing_name: &str, region: &str, kind: &str, id: &str) -> String {
    ::engine::emulator_arn(signing_name, region, &format!("{kind}/{id}"))
}

/// One page of `items`, plus the token that reaches the next one.
///
/// `MaxResults` is the page size; a `NextToken` is this service's opaque cursor, minted
/// by the previous page of the SAME list (`kind` names that list: the operation and
/// whatever selected it) and rejected otherwise — a token is not a client-chosen value,
/// so an unparseable or foreign one is a `ValidationException` rather than a silent
/// restart from the beginning. A page size below 1 would make no progress, so it is
/// refused too.
pub fn paginate<T: Clone>(
    items: &[T],
    kind: &str,
    max_results: Option<i32>,
    next_token: Option<&str>,
) -> Result<(Vec<T>, Option<String>), StateError> {
    let offset = match next_token {
        Some(token) => decode_page_token(kind, token)?,
        None => 0,
    };
    if offset > items.len() {
        return Err(StateError::Validation(
            "NextToken does not refer to a position in this list".to_string(),
        ));
    }
    let page_size = match max_results {
        Some(max) if max < 1 => {
            return Err(StateError::Validation(format!(
                "MaxResults must be at least 1, got {max}"
            )))
        }
        Some(max) => max as usize,
        None => items.len(),
    };
    let end = offset.saturating_add(page_size).min(items.len());
    let page = items[offset..end].to_vec();
    let token = if end < items.len() {
        Some(encode_page_token(kind, end))
    } else {
        None
    };
    Ok((page, token))
}

/// This service's opaque pagination cursor: the list it belongs to and the offset it
/// resumes at.
///
/// Hex is an encoding, not a secret — a client can decode one. What makes a cursor
/// trustworthy here is the `kind` check in [`decode_page_token`]: a token is only
/// honoured by the list that minted it, so a forged or stale one is refused rather than
/// applied to a different list.
fn encode_page_token(kind: &str, offset: usize) -> String {
    ::hex::encode(format!("{kind}:{offset}"))
}

/// Decode a cursor minted by [`encode_page_token`] for the same list.
///
/// The offset is split off the END, so a list identity may itself contain `:`.
fn decode_page_token(kind: &str, token: &str) -> Result<usize, StateError> {
    let invalid = || StateError::Validation("NextToken is not a valid pagination token".to_string());
    let decoded = ::hex::decode(token).map_err(|_| invalid())?;
    let decoded = String::from_utf8(decoded).map_err(|_| invalid())?;
    let (token_kind, offset) = decoded.rsplit_once(':').ok_or_else(invalid)?;
    if token_kind != kind {
        return Err(invalid());
    }
    offset.parse::<usize>().map_err(|_| invalid())
}

/// A deterministic rendering of the `SubscriptionProviderSources` selection, for the
/// list identity a pagination cursor is minted against: a cursor is a position in ONE
/// list, so it must not be accepted for a list selected by a different filter.
///
/// Every component is LENGTH-PREFIXED, so no two different selections can render to the
/// same signature.
pub fn source_filter_signature(sources: &[String]) -> String {
    let mut signature = String::new();
    for source in sources {
        signature.push_str(&format!("{}.{source}", source.len()));
    }
    signature
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsLicensemanagerlinuxsubscriptionsLicensemanagerlinuxsubscriptionsState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsLicensemanagerlinuxsubscriptionsLicensemanagerlinuxsubscriptionsState>)
}
