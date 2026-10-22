// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-licensemanagerlinuxsubscriptions-licensemanagerlinuxsubscriptions service.
//!
//! One store holds every resource this service's operations share: the registered
//! third-party subscription providers, the per-account Linux subscriptions service
//! settings and the instances Linux subscription discovery has found. The lookup / list
//! / filter / pagination helpers live here too, so the operation handlers in `ops/` all
//! resolve a resource the same way instead of each growing its own copy.

use std::collections::{BTreeMap, HashMap};

use crate::ops::types::{
    Filter, LinuxSubscriptionsDiscoverySettings, RegisteredSubscriptionProvider, OPERATOR_VALUES,
};

/// `SubscriptionProviderStatus` for a provider whose access token License Manager holds
/// and accepted (`SubscriptionProviderStatus`: ACTIVE | INVALID | PENDING).
pub const PROVIDER_STATUS_ACTIVE: &str = "ACTIVE";

/// The `LinuxSubscriptionsDiscovery` value an account that never called
/// `UpdateServiceSettings` reports (`LinuxSubscriptionsDiscovery`: Disabled | Enabled).
pub const DISCOVERY_DISABLED: &str = "Disabled";

/// The `Status` a settled change to the Linux subscriptions service settings reports
/// (`Status`: Completed | Failed | InProgress | Successful — `smithy_query(query="shape",
/// shape="Status")`).
///
/// `Completed` is the value the recorded service reports once an enable or a disable of
/// `LinuxSubscriptionsDiscovery` has been applied, and it is the value every client's
/// onboarding poll waits for. The settings singleton is written before the call that
/// changed it returns, so the change is already applied — the status is terminal on the
/// first read and no later transition is owed on it.
pub const SETTINGS_STATUS_COMPLETED: &str = "Completed";

/// The fixed text a refusal naming no registered subscription provider carries.
///
/// The recorded service states this refusal as a CONSTANT that names the resource type
/// and nothing from the request: the ARN the caller sent is not interpolated back into
/// it. The TAG APIs have their own taxonomy for the same condition — a different code
/// and a different constant, `crate::scaffolded::NOT_FOUND_MESSAGE`.
pub const PROVIDER_NOT_FOUND_MESSAGE: &str = "Registered Subscription Provider not found";

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

/// The `Status` of a discovered instance that is running, and so counts towards its
/// subscription's `InstanceCount` (`Subscription$InstanceCount`: "The total amount of
/// running instances using this subscription"). The member carries the EC2 instance
/// state name, which is `running` for an instance that is up.
pub const INSTANCE_STATUS_RUNNING: &str = "running";

/// One instance Linux subscription discovery has found, with the subscription it uses.
///
/// Discovery reads the account's EC2 inventory in the configured source Regions; nothing
/// in THIS service's API registers an instance, so the store below is written only by a
/// discovery. Both `ListLinuxSubscriptionInstances` (which reports these records) and
/// `ListLinuxSubscriptions` (which aggregates them into subscriptions) project it, so
/// the two lists can never disagree about what was discovered.
#[derive(Debug, Clone, Default)]
pub struct DiscoveredInstanceRecord {
    /// `Instance$InstanceID` — the instance id, and the key of `discovered_instances`.
    pub instance_id: String,
    /// `Instance$AccountID` — the account that owns the instance.
    pub account_id: String,
    /// `Instance$AmiId` — the AMI the instance was launched from.
    pub ami_id: String,
    /// `Instance$InstanceType`.
    pub instance_type: String,
    /// `Instance$Status` — the instance state; [`INSTANCE_STATUS_RUNNING`] is what makes
    /// the instance count towards its subscription.
    pub status: String,
    /// `Instance$Region` — the Region the instance runs in.
    pub region: String,
    /// `Instance$UsageOperation` — the billing usage operation the instance reports.
    pub usage_operation: String,
    /// `Instance$ProductCode` — the product codes on the instance's AMI.
    pub product_codes: Vec<String>,
    /// `Instance$LastUpdatedTime` — when discovery last refreshed this record.
    pub last_updated_time: Option<String>,
    /// `Instance$SubscriptionName` — the name of the subscription the instance uses, and
    /// half of the key `ListLinuxSubscriptions` groups on.
    pub subscription_name: String,
    /// The `Subscription$Type` of that subscription (subscription-included with Amazon
    /// EC2, BYOS, or from the Amazon Web Services Marketplace), the other half of that
    /// key. Discovery records it per instance, because the same subscription name can be
    /// licensed in more than one way.
    pub subscription_type: String,
    /// `Instance$OsVersion`.
    pub os_version: Option<String>,
    /// `Instance$SubscriptionProviderCreateTime`, set only for an instance whose
    /// subscription comes from a registered third-party provider.
    pub subscription_provider_create_time: Option<String>,
    /// `Instance$SubscriptionProviderUpdateTime`, set on the same condition.
    pub subscription_provider_update_time: Option<String>,
    /// `Instance$DualSubscription`.
    pub dual_subscription: Option<String>,
    /// `Instance$RegisteredWithSubscriptionProvider`.
    pub registered_with_subscription_provider: Option<String>,
}

/// One discovered Linux subscription: a `Subscription` as `ListLinuxSubscriptions`
/// reports it, aggregated from the discovered instances that use it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredSubscription {
    /// `Subscription$Name`.
    pub name: String,
    /// `Subscription$Type`.
    pub subscription_type: String,
    /// `Subscription$InstanceCount` — how many of the discovered instances using this
    /// subscription are running.
    pub instance_count: i64,
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
    /// The per-account settings singleton, once an update persisted one.
    pub service_settings: Option<ServiceSettingsRecord>,
    /// The instances Linux subscription discovery has found, by `Instance$InstanceID`.
    ///
    /// An instance id is unique, so the map both keeps a re-discovery of the same
    /// instance from adding a second record and gives the two list operations a stable,
    /// pageable order. Discovery is the only writer (see [`DiscoveredInstanceRecord`]):
    /// until one has run, the account has discovered nothing and both lists are empty,
    /// which is exactly what an account with no instances reports.
    pub discovered_instances: BTreeMap<String, DiscoveredInstanceRecord>,
}

impl ComAmazonawsLicensemanagerlinuxsubscriptionsLicensemanagerlinuxsubscriptionsState {
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
        self.providers
            .get(arn)
            .ok_or_else(|| StateError::NotFound(PROVIDER_NOT_FOUND_MESSAGE.to_string()))
    }

    /// The discovered instances in instance-id order.
    pub fn discovered_instances_in_order(&self) -> Vec<DiscoveredInstanceRecord> {
        self.discovered_instances.values().cloned().collect()
    }

    /// The discovered subscriptions: one `Subscription` per distinct (name, type) pair
    /// the discovered instances carry, each counting the instances using it that are
    /// running (`Subscription$InstanceCount`). A subscription that was discovered but has
    /// no running instance left still reports, with a count of zero — the two members say
    /// what was discovered and how much of it is running, which are different questions.
    ///
    /// Ordered by (name, type), a total order over the pairs, so a page is stable and a
    /// pagination cursor keeps its meaning between calls.
    pub fn discovered_subscriptions(&self) -> Vec<DiscoveredSubscription> {
        let mut counts: BTreeMap<(String, String), i64> = BTreeMap::new();
        for instance in self.discovered_instances.values() {
            let running = i64::from(instance.status == INSTANCE_STATUS_RUNNING);
            *counts
                .entry((
                    instance.subscription_name.clone(),
                    instance.subscription_type.clone(),
                ))
                .or_default() += running;
        }
        counts
            .into_iter()
            .map(|((name, subscription_type), instance_count)| DiscoveredSubscription {
                name,
                subscription_type,
                instance_count,
            })
            .collect()
    }

    /// The metadata tags of the resource `arn` addresses, or `None` when this account
    /// holds no such resource.
    ///
    /// The tag APIs accept a `SubscriptionProviderArn`, so a registered subscription
    /// provider is the only resource of this service they can address: every tag
    /// operation resolves its `resourceArn` through this one lookup (and
    /// [`resource_tags_mut`](Self::resource_tags_mut)) rather than reaching into
    /// `providers` itself, so what one of them writes is what the next one reports.
    pub fn resource_tags(&self, arn: &str) -> Option<&HashMap<String, String>> {
        self.providers.get(arn).map(|record| &record.tags)
    }

    /// The metadata tags of the resource `arn` addresses, for a tag write to modify in
    /// place; `None` when this account holds no such resource.
    pub fn resource_tags_mut(&mut self, arn: &str) -> Option<&mut HashMap<String, String>> {
        self.providers.get_mut(arn).map(|record| &mut record.tags)
    }

    /// Remove a registration from the store and from the order that reaches it.
    pub fn remove_provider(&mut self, arn: &str) -> Option<SubscriptionProviderRecord> {
        let record = self.providers.remove(arn)?;
        self.provider_order.retain(|stored| stored != arn);
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

/// The largest number of home Regions the settings may aggregate data in: the modelled
/// `HomeRegions` list is `len 1..100`, so a 101st home Region could not be reported.
pub const MAX_HOME_REGIONS: usize = 100;

/// The largest number of metadata tags one of this service's resources carries.
///
/// The modelled `Tags` map is `len 0..50` — both as `TagResource`'s input and as
/// `ListTagsForResource`'s output — so a resource holding a 51st tag could not be
/// reported by the operation that lists its tags. A tag write whose RESULTANT set would
/// pass this bound is therefore refused instead of being stored.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

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
            status: Some(SETTINGS_STATUS_COMPLETED.to_string()),
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

/// Whether `region` names an AWS Region.
///
/// The catalog is `emulator_telemetry::policy::KNOWN_REGIONS` ("every AWS region the
/// wire may carry, sorted for binary search", maintained with the backend contract and
/// already a dependency of this crate) rather than a list copied into this service: this
/// service models no Region enum — `SourceRegions` is a plain `StringList` — so the set
/// a value may name is exactly that shared catalog, and it must not drift from it. The
/// lookup is the binary search the catalog's ordering is maintained for.
pub fn is_known_region(region: &str) -> bool {
    ::emulator_telemetry::policy::KNOWN_REGIONS
        .binary_search(&region)
        .is_ok()
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
    length_prefixed(sources.iter().cloned())
}

/// The same rendering for a `FilterList` selection, so the `Filters`-driven lists get
/// cursors that are positions in the list THAT selection produced.
///
/// Each filter contributes its name, its operator and its values, every component
/// length-prefixed, closed by the number of values it carried — so neither a different
/// grouping of the same components nor a different operator can render the same
/// signature.
pub fn filter_signature(filters: &[Filter]) -> String {
    let mut signature = String::new();
    for filter in filters {
        signature.push_str(&length_prefixed(
            [
                filter.name.clone().unwrap_or_default(),
                filter.operator.clone().unwrap_or_default(),
            ]
            .into_iter()
            .chain(filter.values.clone().unwrap_or_default()),
        ));
        signature.push_str(&format!("/{}", filter.values.as_ref().map_or(0, Vec::len)));
    }
    signature
}

/// Render `components` so that no two different sequences render alike: each is prefixed
/// with its own length.
fn length_prefixed(components: impl IntoIterator<Item = String>) -> String {
    let mut rendered = String::new();
    for component in components {
        rendered.push_str(&format!("{}.{component}", component.len()));
    }
    rendered
}

/// The comparison an absent `Filter$Operator` means: `Filters` selects the results that
/// "match one or more sets of key-value pairs that you specify", so a filter with no
/// operator selects on equality (`Operator`: Contains | Equal | NotEqual).
pub const OPERATOR_EQUAL: &str = "Equal";

/// `Operator` for a filter that selects what does NOT equal any of its values.
pub const OPERATOR_NOT_EQUAL: &str = "NotEqual";

/// `Operator` for a filter that selects on a substring of the member's value.
pub const OPERATOR_CONTAINS: &str = "Contains";

/// Refuse a filter the list it was given to cannot apply.
///
/// `valid_names` is the set of `Filter$Name` values that list documents: a filter naming
/// anything else addresses no member, so it can select nothing and is a
/// `ValidationException` rather than a silently empty page. A filter carrying no value
/// has nothing to compare against (the modelled `Values` list is `len 1..100`), and an
/// operator outside the modelled `Operator` set names no comparison.
pub fn validate_filters(filters: &[Filter], valid_names: &[&str]) -> Result<(), StateError> {
    for filter in filters {
        let name = filter.name.as_deref().unwrap_or_default();
        if !valid_names.contains(&name) {
            return Err(StateError::Validation(format!(
                "Filters.Name must be one of {valid_names:?}, got {name:?}"
            )));
        }
        if filter.values.as_ref().is_none_or(Vec::is_empty) {
            return Err(StateError::Validation(format!(
                "Filters.Values must carry at least one value to filter {name:?} by"
            )));
        }
        if let Some(operator) = filter.operator.as_deref() {
            if !OPERATOR_VALUES.contains(&operator) {
                return Err(StateError::Validation(format!(
                    "Filters.Operator must be one of {OPERATOR_VALUES:?}, got {operator:?}"
                )));
            }
        }
    }
    Ok(())
}

/// Does one validated `filter` select a resource whose addressed member holds
/// `member_values`?
///
/// `member_values` is what the member the filter's `Name` addresses holds on that
/// resource: one value for a scalar member, several for a list member (`ProductCode`),
/// none when the resource leaves it unset. A list member satisfies the filter when any of
/// its values does, which is what makes `NotEqual` mean "no value of this member equals
/// any value I named".
pub fn filter_selects(filter: &Filter, member_values: &[String]) -> bool {
    let values = filter.values.as_deref().unwrap_or_default();
    let equals = member_values.iter().any(|held| values.contains(held));
    match filter.operator.as_deref().unwrap_or(OPERATOR_EQUAL) {
        OPERATOR_CONTAINS => member_values
            .iter()
            .any(|held| values.iter().any(|value| held.contains(value.as_str()))),
        OPERATOR_NOT_EQUAL => !equals,
        // `Equal`, and the equality an absent operator means.
        _ => equals,
    }
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
