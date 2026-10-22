// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-ssmquicksetup-quicksetup service.
//!
//! The service's primary resource is the **configuration manager**: a collection of
//! configuration definitions, each of which the service deploys as one
//! **configuration**. Both live here exactly once — the manager keyed by its ARN,
//! the configuration by its service-generated id — so every operation reads and
//! writes the one record, and a configuration is reclaimed with the manager that
//! owns it.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    ConfigurationDefinition, ConfigurationDefinitionSummary, ConfigurationManagerSummary,
    ConfigurationSummary, Filter, StatusSummary,
};

/// The signing name, endpoint prefix and ARN namespace of this service, all three
/// `ssm-quicksetup` (the model's `aws.auth#sigv4` / `aws.api#service` traits).
pub const ARN_SERVICE: &str = "ssm-quicksetup";

/// The resource portion a configuration-manager ARN carries before its uuid, from
/// the modelled `ManagerArn` pattern
/// `^arn:aws:ssm-quicksetup:([^:]+):(\d{12}):configuration-manager/<uuid>$`.
pub const MANAGER_ARN_RESOURCE_PREFIX: &str = "configuration-manager/";

/// Region used when a request carries no SigV4 credential scope to read one from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The `StatusType` of the summary a configuration manager's deployment carries,
/// from the modelled `StatusType` enum (`crate::ops::types::STATUS_TYPE_VALUES`).
pub const STATUS_TYPE_DEPLOYMENT: &str = "Deployment";

/// The status a just-created deployment is in, from the modelled `Status` enum
/// (`crate::ops::types::STATUS_VALUES`).
pub const STATUS_INITIALIZING: &str = "INITIALIZING";

/// The status a deployment settles into, from the same enum.
pub const STATUS_SUCCEEDED: &str = "SUCCEEDED";

/// How long a deployment stays in its transient status before the clock owes it the
/// settled one.
///
/// `CreateConfigurationManager` starts a deployment, so the manager and its
/// configurations are reported in a non-terminal `Status` first: that is a
/// transition the clock owes. The span is this emulator's own choice (AWS documents
/// no duration for a Quick Setup deployment) and is deliberately non-zero, so a
/// caller reading straight after the create still observes the transient status, as
/// it does against the real service. The instant it comes due is stored on the
/// summary ([`StatusSummaryRecord::settles`]) and materialised by
/// [`ComAmazonawsSsmquicksetupQuicksetupState::settle`]; nothing sleeps.
pub const TRANSITION_SPAN: Duration = Duration::from_millis(500);

/// Largest page either list operation will build, and the page size applied when the
/// request names no `MaxItems`: the maximum of that member's modelled `@range(1,
/// 100)`.
pub const MAX_ITEMS_LIMIT: i32 = 100;

/// How many configuration managers this emulator will hold.
///
/// Emulator resource guard, NOT a published AWS quota: a create that would take the
/// store past it is refused rather than growing this process without bound.
pub const MAX_CONFIGURATION_MANAGERS: usize = 500;

/// How many configuration definitions one create may carry.
///
/// `ConfigurationDefinitionsInputList` carries no modelled length, so the bound
/// lives here. Emulator resource guard, NOT a published AWS quota.
pub const MAX_CONFIGURATION_DEFINITIONS: usize = 50;

/// How many parameters one configuration definition may carry.
///
/// `ConfigurationParametersMap` bounds each key and value but not the map's size, so
/// the bound lives here. Emulator resource guard, NOT a published AWS quota.
pub const MAX_PARAMETERS: usize = 100;

/// How many tags one configuration manager may carry.
///
/// `TagsMap` bounds each key and value but not the map's size, so the bound lives
/// here. Emulator resource guard, NOT a published AWS quota.
pub const MAX_TAGS: usize = 50;

/// How many `Filters` entries one listing request may carry, and how many `Values`
/// each of them may carry.
///
/// `FiltersList` and `FilterValues` are caller-sized and the model publishes no length
/// for either, while the matching work one request does is records x filters x values.
/// Emulator resource guards, NOT published AWS quotas: a request past them is refused
/// before the store lock is taken.
pub const MAX_FILTERS: usize = 10;

/// See [`MAX_FILTERS`].
pub const MAX_FILTER_VALUES: usize = 20;

/// How many stored records ONE listing request may examine.
///
/// The page is bounded by `MaxItems`, but a filter that admits little would otherwise
/// walk the whole store to fill it, holding the store lock for records x filters x
/// values work. A request that reaches this many examined records ends its page early
/// and hands back the token to resume from, so paging still visits every record exactly
/// once. Emulator resource guard, NOT a published AWS quota.
pub const MAX_SCAN_ENTRIES: usize = 1_000;

/// `Filter` keys `ListConfigurationManagers` accepts.
///
/// The model documents no key catalog, so the keys are the listed shape's own
/// string-valued members — `ConfigurationManagerSummary` — whose values the modelled
/// `FilterValues` pattern (`^[A-Za-z0-9+=@_\/\s-]*$`) can express. `ManagerArn` is
/// not among them: an ARN carries `:`, which that pattern refuses, so no filter value
/// can ever name one.
pub const MANAGER_FILTER_KEYS: &[&str] = &["Name", "Description"];

/// `Filter` keys `ListConfigurations` accepts: the string-valued members of
/// `ConfigurationSummary`, for the same reason as [`MANAGER_FILTER_KEYS`]. The
/// manager is named by this operation's own `ManagerArn` member instead.
pub const CONFIGURATION_FILTER_KEYS: &[&str] = &[
    "Id",
    "ConfigurationDefinitionId",
    "Type",
    "TypeVersion",
    "Account",
    "Region",
];

/// A status change the clock owes a status summary: where it lands, and when.
#[derive(Debug, Clone)]
pub struct PendingStatus {
    /// The `Status` the summary settles into.
    pub to: String,
    /// The instant the change comes due.
    pub due_at: SystemTime,
}

/// One stored `StatusSummary`: the state of one kind of work on a manager or a
/// configuration.
#[derive(Debug, Clone)]
pub struct StatusSummaryRecord {
    /// A `StatusType` value.
    pub status_type: String,
    /// A `Status` value.
    pub status: String,
    pub status_message: Option<String>,
    pub last_updated_at: SystemTime,
    pub status_details: BTreeMap<String, String>,
    /// The change the clock owes this summary, or `None` once it has settled.
    pub settles: Option<PendingStatus>,
}

impl StatusSummaryRecord {
    /// The deployment summary a configuration manager (or one of its configurations)
    /// carries from the moment it is created: `INITIALIZING` now, `SUCCEEDED` once
    /// [`TRANSITION_SPAN`] has passed.
    pub fn deployment(now: SystemTime) -> Self {
        Self {
            status_type: STATUS_TYPE_DEPLOYMENT.to_string(),
            status: STATUS_INITIALIZING.to_string(),
            status_message: None,
            last_updated_at: now,
            status_details: BTreeMap::new(),
            settles: Some(PendingStatus {
                to: STATUS_SUCCEEDED.to_string(),
                due_at: now + TRANSITION_SPAN,
            }),
        }
    }

    /// Materialise the change the clock already owes this summary.
    ///
    /// This is the ONLY write a reporting operation makes: the change was owed at
    /// this instant whether or not anybody called. Nothing is cloned on the way: a
    /// summary that is not yet due is left exactly as it was.
    fn settle(&mut self, now: SystemTime) {
        if self.settles.as_ref().is_none_or(|pending| now < pending.due_at) {
            return;
        }
        let Some(pending) = self.settles.take() else {
            return;
        };
        self.status = pending.to;
        // The change happened when the clock made it due, not when someone looked.
        self.last_updated_at = pending.due_at.max(self.last_updated_at);
    }

    /// The modelled `StatusSummary` for this record.
    pub fn to_status_summary(&self) -> StatusSummary {
        StatusSummary {
            status_type: self.status_type.clone(),
            status: Some(self.status.clone()),
            status_message: self.status_message.clone(),
            last_updated_at: ::aws_smithy_types::DateTime::from(self.last_updated_at),
            status_details: (!self.status_details.is_empty())
                .then(|| to_hash_map(&self.status_details)),
        }
    }
}

/// One configuration definition of a stored manager, with the `Id` the service
/// assigned it.
#[derive(Debug, Clone)]
pub struct ConfigurationDefinitionRecord {
    /// Service-assigned, matching the modelled `^[a-z0-9-]{1,20}$` id pattern that
    /// `UpdateConfigurationDefinition` binds on its `{Id}` label.
    pub id: String,
    pub type_: String,
    pub type_version: Option<String>,
    pub parameters: BTreeMap<String, String>,
    pub local_deployment_execution_role_name: Option<String>,
    pub local_deployment_administration_role_arn: Option<String>,
}

impl ConfigurationDefinitionRecord {
    /// The modelled `ConfigurationDefinition` for this record.
    pub fn to_configuration_definition(&self) -> ConfigurationDefinition {
        ConfigurationDefinition {
            r#type: self.type_.clone(),
            parameters: to_hash_map(&self.parameters),
            type_version: self.type_version.clone(),
            local_deployment_execution_role_name: self
                .local_deployment_execution_role_name
                .clone(),
            local_deployment_administration_role_arn: self
                .local_deployment_administration_role_arn
                .clone(),
            id: Some(self.id.clone()),
        }
    }

    /// The modelled `ConfigurationDefinitionSummary` for this record.
    ///
    /// `FirstClassParameters` are the definition's own parameters: this emulator
    /// stores every parameter the create supplied and distinguishes no subset of
    /// them, so reporting them is what the record holds.
    pub fn to_summary(&self) -> ConfigurationDefinitionSummary {
        ConfigurationDefinitionSummary {
            id: Some(self.id.clone()),
            r#type: Some(self.type_.clone()),
            type_version: self.type_version.clone(),
            first_class_parameters: Some(to_hash_map(&self.parameters)),
        }
    }
}

/// One stored configuration manager: everything `CreateConfigurationManager` was
/// given, plus what the service minted for it.
#[derive(Debug, Clone)]
pub struct ConfigurationManagerRecord {
    pub manager_arn: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub created_at: SystemTime,
    pub last_modified_at: SystemTime,
    pub tags: BTreeMap<String, String>,
    pub definitions: Vec<ConfigurationDefinitionRecord>,
    pub status_summaries: Vec<StatusSummaryRecord>,
}

impl ConfigurationManagerRecord {
    /// The modelled `ConfigurationManagerSummary` for this record.
    pub fn to_summary(&self) -> ConfigurationManagerSummary {
        ConfigurationManagerSummary {
            manager_arn: self.manager_arn.clone(),
            description: self.description.clone(),
            name: self.name.clone(),
            status_summaries: Some(
                self.status_summaries
                    .iter()
                    .map(StatusSummaryRecord::to_status_summary)
                    .collect(),
            ),
            configuration_definition_summaries: Some(
                self.definitions
                    .iter()
                    .map(ConfigurationDefinitionRecord::to_summary)
                    .collect(),
            ),
        }
    }

    /// The value of the `ConfigurationManagerSummary` member a `Filter` key names,
    /// or `None` when this record carries none. Borrowed, so matching allocates
    /// nothing.
    pub fn filter_field(&self, key: &str) -> Option<&str> {
        match key {
            "Name" => self.name.as_deref(),
            "Description" => self.description.as_deref(),
            _ => None,
        }
    }
}

/// One stored configuration: the deployment of one configuration definition, created
/// with the manager that owns it and reclaimed when that manager goes.
#[derive(Debug, Clone)]
pub struct ConfigurationRecord {
    /// Service-generated, and this record's store key.
    pub id: String,
    pub manager_arn: String,
    pub configuration_definition_id: String,
    pub type_: String,
    pub type_version: Option<String>,
    pub account: String,
    pub region: String,
    pub created_at: SystemTime,
    pub last_modified_at: SystemTime,
    pub parameters: BTreeMap<String, String>,
    pub status_summaries: Vec<StatusSummaryRecord>,
}

impl ConfigurationRecord {
    /// The modelled `ConfigurationSummary` for this record.
    pub fn to_summary(&self) -> ConfigurationSummary {
        ConfigurationSummary {
            id: Some(self.id.clone()),
            manager_arn: Some(self.manager_arn.clone()),
            configuration_definition_id: Some(self.configuration_definition_id.clone()),
            r#type: Some(self.type_.clone()),
            type_version: self.type_version.clone(),
            region: Some(self.region.clone()),
            account: Some(self.account.clone()),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            first_class_parameters: Some(to_hash_map(&self.parameters)),
            status_summaries: Some(
                self.status_summaries
                    .iter()
                    .map(StatusSummaryRecord::to_status_summary)
                    .collect(),
            ),
        }
    }

    /// The value of the `ConfigurationSummary` member a `Filter` key names. Borrowed,
    /// so matching allocates nothing.
    pub fn filter_field(&self, key: &str) -> Option<&str> {
        match key {
            "Id" => Some(&self.id),
            "ConfigurationDefinitionId" => Some(&self.configuration_definition_id),
            "Type" => Some(&self.type_),
            "TypeVersion" => self.type_version.as_deref(),
            "Account" => Some(&self.account),
            "Region" => Some(&self.region),
            _ => None,
        }
    }
}

/// Per-service state for com-amazonaws-ssmquicksetup-quicksetup. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsSsmquicksetupQuicksetupState {
    /// Every configuration manager, keyed by its ARN. A `BTreeMap` so a list walks a
    /// stable, total order and a page token can name the next ARN.
    pub managers: BTreeMap<String, ConfigurationManagerRecord>,
    /// Every configuration, keyed by its service-generated id, for the same reason.
    pub configurations: BTreeMap<String, ConfigurationRecord>,
    /// Monotonic counter backing configuration-definition id assignment.
    next_definition_id: u64,
    /// Monotonic counter backing uuid assignment (see [`Self::fresh_uuid`]).
    next_uuid: u64,
}

impl ComAmazonawsSsmquicksetupQuicksetupState {
    /// A fresh uuid for a resource this service mints.
    ///
    /// The modelled `ManagerArn` pattern ends in an `8-4-4-4-12` hex uuid and a
    /// configuration's `Id` takes the same shape, so that is what this builds: the
    /// version (`4`) and variant (`8`) nibbles are set, the first three groups come
    /// from the host clock mixed through SplitMix64 (the construction
    /// `service-com-amazonaws-ram-amazonresourcesharing` uses for the same job), and
    /// the last group is this store's own monotonic counter, so two ids one store
    /// hands out can never collide however the clock behaves.
    pub fn fresh_uuid(&mut self) -> String {
        self.next_uuid = self.next_uuid.saturating_add(1);
        let mut seed = now_nanos() ^ self.next_uuid.rotate_left(17);
        let groups: Vec<u64> = (0..3).map(|_| splitmix64(&mut seed)).collect();
        format!(
            "{:08x}-{:04x}-4{:03x}-8{:03x}-{:012x}",
            groups[0] as u32,
            groups[1] as u16,
            groups[2] & 0x0fff,
            (groups[2] >> 16) & 0x0fff,
            self.next_uuid & 0xffff_ffff_ffff,
        )
    }
    /// The next configuration-definition id: a decimal, which satisfies the modelled
    /// `^[a-z0-9-]{1,20}$` id pattern and is unique for every id this store can hand
    /// out (the counter saturates at `u64::MAX`, which no store this emulator can hold
    /// could ever reach — `MAX_CONFIGURATION_MANAGERS` x
    /// `MAX_CONFIGURATION_DEFINITIONS` is its whole capacity).
    pub fn fresh_definition_id(&mut self) -> String {
        self.next_definition_id = self.next_definition_id.saturating_add(1);
        self.next_definition_id.to_string()
    }

    /// Materialise every status change the clock already owes, so a reporting
    /// operation answers with what is due at `now` rather than with what the last
    /// mutating call left behind.
    pub fn settle(&mut self, now: SystemTime) {
        for manager in self.managers.values_mut() {
            for summary in &mut manager.status_summaries {
                summary.settle(now);
            }
        }
        for configuration in self.configurations.values_mut() {
            for summary in &mut configuration.status_summaries {
                summary.settle(now);
            }
        }
    }

    /// Remove the manager `manager_arn` names together with every configuration it
    /// owns. `None` when no manager has that ARN.
    pub fn remove_manager(&mut self, manager_arn: &str) -> Option<ConfigurationManagerRecord> {
        let manager = self.managers.remove(manager_arn)?;
        self.configurations
            .retain(|_, configuration| configuration.manager_arn != manager_arn);
        Some(manager)
    }
}

/// Why a `Filters` list is unacceptable, or `None` when this operation can serve it.
///
/// Two rules, both applied BEFORE the store lock is taken, so a refused request never
/// occupies it:
///
/// * a filter key that names no field of the listed shape cannot be honoured, and
///   silently ignoring it would answer a narrowing request with unnarrowed results, so
///   it is refused — naming the keys that are accepted;
/// * the list and each filter's `Values` are caller-sized and the model publishes no
///   length for either, while the matching work one request does is records x filters
///   x values, so anything past [`MAX_FILTERS`] / [`MAX_FILTER_VALUES`] is refused
///   rather than evaluated.
pub fn filter_key_rejection(filters: Option<&[Filter]>, supported: &[&str]) -> Option<String> {
    let filters = filters?;
    if filters.len() > MAX_FILTERS {
        return Some(format!(
            "Filters carries {} entries, more than the {MAX_FILTERS} this operation accepts",
            filters.len()
        ));
    }
    if let Some(oversized) = filters
        .iter()
        .find(|filter| filter.values.len() > MAX_FILTER_VALUES)
    {
        return Some(format!(
            "Filters key '{}' carries {} values, more than the {MAX_FILTER_VALUES} this operation accepts",
            oversized.key,
            oversized.values.len()
        ));
    }
    let unsupported = filters
        .iter()
        .find(|filter| !supported.contains(&filter.key.as_str()))?;
    Some(format!(
        "Filters carries key '{}', which this operation does not filter on; accepted keys are [{}]",
        unsupported.key,
        supported.join(", ")
    ))
}

/// Whether a record whose fields `field` reports satisfies every supplied filter: a
/// filter holds when the record's value for its key is one of that filter's `Values`.
///
/// Both factors of the per-record cost are bounded before this runs: the filter list
/// and each `Values` list by [`filter_key_rejection`], and the records examined by
/// [`paginate`]. `field` borrows out of the record rather than cloning, so matching a
/// record allocates nothing.
pub fn matches_filters<'r>(
    filters: Option<&[Filter]>,
    field: impl Fn(&str) -> Option<&'r str>,
) -> bool {
    filters.is_none_or(|filters| {
        filters.iter().all(|filter| {
            field(&filter.key)
                .is_some_and(|value| filter.values.iter().any(|wanted| wanted.as_str() == value))
        })
    })
}

/// One page of a `BTreeMap`-ordered listing: the records `keep` admits, plus the key
/// the next page starts at when more remain.
///
/// `kind` names the listing the token belongs to, so a token one listing issued is not
/// accepted by the other (see [`encode_page_token`]).
///
/// Both bounds here are the server's own: `page_size` (see [`page_size`]) sizes the
/// page, so nothing a caller sends sizes an allocation, and [`MAX_SCAN_ENTRIES`]
/// bounds how many stored records ONE request examines — a narrow filter must not walk
/// the whole store under the store lock. Either way the token names where to resume,
/// so paging still visits every record exactly once.
pub fn paginate<'a, T>(
    kind: &str,
    entries: impl Iterator<Item = (&'a String, &'a T)>,
    page_size: usize,
    keep: impl Fn(&T) -> bool,
) -> (Vec<&'a T>, Option<String>) {
    let mut page = Vec::with_capacity(page_size);
    let mut next_token = None;
    for (examined, (key, record)) in entries.enumerate() {
        // The first record that does not fit becomes the next page's start, and only
        // then is a token emitted — the last page carries none.
        if page.len() == page_size || examined == MAX_SCAN_ENTRIES {
            next_token = Some(encode_page_token(kind, key));
            break;
        }
        if keep(record) {
            page.push(record);
        }
    }
    (page, next_token)
}

/// The page size to serve for a requested `MaxItems`: clamped into
/// `1..=MAX_ITEMS_LIMIT` whatever the request asked for, and that limit when it asked
/// for nothing.
pub fn page_size(max_items: Option<i32>) -> usize {
    max_items.map_or(MAX_ITEMS_LIMIT, |requested| {
        requested.clamp(1, MAX_ITEMS_LIMIT)
    }) as usize
}

/// The listing a page token belongs to: `ListConfigurationManagers`.
pub const MANAGER_PAGE_TOKEN_KIND: &str = "configuration-manager";

/// The listing a page token belongs to: `ListConfigurations`.
pub const CONFIGURATION_PAGE_TOKEN_KIND: &str = "configuration";

/// A page token naming the record the NEXT page starts at, tagged with the listing
/// that issued it.
///
/// Hex-encoded rather than handed back in the clear, which makes the token opaque to
/// a caller rather than secret: it carries nothing but the listing's name and a store
/// key. The tag is what lets [`decode_page_token`] refuse a token the OTHER listing
/// issued (whose key names a record of a different kind) as well as caller-crafted
/// text. Hex is inside both `StartingToken` patterns
/// (`^[A-Za-z0-9+=@_\/\s-]*$` and the `ListConfigurations` variant) and well inside
/// their 1024-character bound — a tag and an ARN encode to about 250 characters.
pub fn encode_page_token(kind: &str, key: &str) -> String {
    ::hex::encode(format!("{kind}\u{1f}{key}"))
}

/// The store key a page token names, or `None` when the token is not one `kind`'s
/// listing issued.
pub fn decode_page_token(kind: &str, token: &str) -> Option<String> {
    let decoded = ::hex::decode(token).ok()?;
    let decoded = String::from_utf8(decoded).ok()?;
    decoded
        .strip_prefix(&format!("{kind}\u{1f}"))
        .map(str::to_string)
}

/// The ARN of a configuration manager with `uuid` in `region`.
pub fn manager_arn(region: &str, uuid: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{MANAGER_ARN_RESOURCE_PREFIX}{uuid}"),
    )
}

/// The region this request was signed for, from its SigV4 credential scope; the
/// emulator's default region when it carries none.
///
/// The scope is caller-controlled text and the value lands inside an ARN whose
/// modelled pattern allows no `:` in that field and whose length the store keys on,
/// so anything outside `[A-Za-z0-9-]` is refused here and the default used instead: a
/// generated ARN can never carry text the model does not admit.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| {
            !region.is_empty()
                && region.len() <= 64
                && region
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// A `BTreeMap` as the wire map shapes want it. The one place this conversion lives.
pub fn to_hash_map(map: &BTreeMap<String, String>) -> HashMap<String, String> {
    map.iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

/// Now, in nanoseconds since the Unix epoch — only a uuid-generator seed.
fn now_nanos() -> u64 {
    SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .map_or(0, |since_epoch| since_epoch.as_nanos() as u64)
}

/// SplitMix64: the uuid generator's mixing step, so consecutive seeds do not produce
/// near-identical ids.
fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// A wire map as the store holds it: ordered, so two reads of one record report its
/// entries in the same order.
pub fn to_btree_map(map: Option<HashMap<String, String>>) -> BTreeMap<String, String> {
    map.unwrap_or_default().into_iter().collect()
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSsmquicksetupQuicksetupState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSsmquicksetupQuicksetupState>)
}
