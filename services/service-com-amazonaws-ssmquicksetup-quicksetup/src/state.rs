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
    ConfigurationSummary, Filter, ServiceSettings, StatusSummary,
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

/// How many regions' service settings this store will hold.
///
/// The settings record is keyed by the region the request was signed for, which is
/// caller-controlled text (bounded by [`request_region`] but not enumerated), so an
/// update naming an unseen region would otherwise grow this store without bound.
/// Emulator resource guard, NOT a published AWS quota: an update past it is refused
/// rather than stored. Far above the number of regions AWS publishes.
pub const MAX_SETTINGS_REGIONS: usize = 100;

/// One entry of the Quick Setup type catalog `ListQuickSetupTypes` publishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuickSetupTypeEntry {
    /// A `QuickSetupTypeOutput$Type`: the type a `ConfigurationDefinitionInput`
    /// names. Satisfies that member's modelled `^[a-zA-Z0-9_\-.:/]{3,200}$` pattern.
    pub type_: &'static str,
    /// A `QuickSetupTypeOutput$LatestVersion`: the newest configuration version
    /// this service offers for `type_`, in the form `TypeVersion` takes.
    pub latest_version: &'static str,
}

/// The configuration version this emulator's catalog publishes for every type it
/// offers.
///
/// AWS advances a type's `LatestVersion` as it ships new configuration versions of
/// that type; the model publishes no version catalog, so this emulator offers
/// exactly one version of each type — the first — and says so here rather than
/// inventing a different number per type. It satisfies the modelled
/// `ConfigurationDefinition$TypeVersion` `@length(1, 128)`, which is the only
/// constraint the model puts on a type version.
pub const CATALOG_LATEST_VERSION: &str = "1.0";

/// The Quick Setup configuration types this service publishes, in a fixed order so
/// two reads of the catalog report the same list.
///
/// Source: the configuration types this service's own model documents, from
/// `smithy_query(query="doc", shape="ConfigurationDefinitionInput",
/// member="Parameters")` — its parameter tables are titled
/// `<name> (Type: AWSQuickSetupType-<type>)`, one per supported type (the doc text
/// renders `AWS` as `Amazon Web Services`). `AWSQuickSetupType-SSM` is the Quick
/// Setup default configuration type, which that member's tables do not itemise; it
/// is published too because configuration definitions name it.
///
/// This is the DISCOVERY catalog, not an allow-list: `Type` is modelled as a
/// patterned string with no enum, so `CreateConfigurationManager` accepts any value
/// matching that pattern and this list does not narrow it.
pub const QUICK_SETUP_TYPE_CATALOG: &[QuickSetupTypeEntry] = &[
    // Conformance Packs
    QuickSetupTypeEntry { type_: "AWSQuickSetupType-CFGCPacks", latest_version: CATALOG_LATEST_VERSION },
    // Config Recording
    QuickSetupTypeEntry { type_: "AWSQuickSetupType-CFGRecording", latest_version: CATALOG_LATEST_VERSION },
    // Default Host Management Configuration
    QuickSetupTypeEntry { type_: "AWSQuickSetupType-DHMC", latest_version: CATALOG_LATEST_VERSION },
    // DevOps Guru
    QuickSetupTypeEntry { type_: "AWSQuickSetupType-DevOpsGuru", latest_version: CATALOG_LATEST_VERSION },
    // Distributor
    QuickSetupTypeEntry { type_: "AWSQuickSetupType-Distributor", latest_version: CATALOG_LATEST_VERSION },
    // Patch Policy
    QuickSetupTypeEntry { type_: "AWSQuickSetupType-PatchPolicy", latest_version: CATALOG_LATEST_VERSION },
    // Resource Explorer
    QuickSetupTypeEntry { type_: "AWSQuickSetupType-ResourceExplorer", latest_version: CATALOG_LATEST_VERSION },
    // Quick Setup default configuration
    QuickSetupTypeEntry { type_: "AWSQuickSetupType-SSM", latest_version: CATALOG_LATEST_VERSION },
    // Change Manager
    QuickSetupTypeEntry { type_: "AWSQuickSetupType-SSMChangeMgr", latest_version: CATALOG_LATEST_VERSION },
    // Host Management
    QuickSetupTypeEntry { type_: "AWSQuickSetupType-SSMHostMgmt", latest_version: CATALOG_LATEST_VERSION },
    // OpsCenter
    QuickSetupTypeEntry { type_: "AWSQuickSetupType-SSMOpsCenter", latest_version: CATALOG_LATEST_VERSION },
    // Resource Scheduler
    QuickSetupTypeEntry { type_: "AWSQuickSetupType-Scheduler", latest_version: CATALOG_LATEST_VERSION },
];

/// The settings Quick Setup holds for ONE account and region: what the last
/// successful `UpdateServiceSettings` wrote there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceSettingsRecord {
    /// `ServiceSettings$ExplorerEnablingRoleArn`, or `None` while no update has
    /// named one.
    pub explorer_enabling_role_arn: Option<String>,
    /// When the record was last written. Not on the wire — the modelled
    /// `ServiceSettings` carries no timestamp — it is what distinguishes one write
    /// of the record from the next.
    pub last_modified_at: SystemTime,
}

impl ServiceSettingsRecord {
    /// The modelled `ServiceSettings` for this record.
    pub fn to_service_settings(&self) -> ServiceSettings {
        ServiceSettings {
            explorer_enabling_role_arn: self.explorer_enabling_role_arn.clone(),
        }
    }
}

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

/// Submit `summaries` for deployment again: the `Deployment` summary becomes a fresh
/// one in its transient status, due to settle [`TRANSITION_SPAN`] later, and every
/// other kind of summary is left as it was.
///
/// A mutation that changes what the service deploys (`UpdateConfigurationDefinition`)
/// starts a deployment exactly as `CreateConfigurationManager` does, so the same record
/// and the same clock-owed transition serve both; this is the one place that
/// construction lives.
pub fn restart_deployment(summaries: &mut Vec<StatusSummaryRecord>, now: SystemTime) {
    let fresh = StatusSummaryRecord::deployment(now);
    match summaries
        .iter_mut()
        .find(|summary| summary.status_type == STATUS_TYPE_DEPLOYMENT)
    {
        Some(existing) => *existing = fresh,
        None => summaries.push(fresh),
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
    /// The service settings record of each region one has been written for.
    ///
    /// Quick Setup's settings are per-account and per-Region, and this emulator
    /// serves one account, so the region the request was signed for
    /// ([`request_region`]) is the key. A region with no entry has never been
    /// configured, which `GetServiceSettings` reports by carrying no
    /// `ServiceSettings` at all. Bounded by [`MAX_SETTINGS_REGIONS`].
    pub service_settings: BTreeMap<String, ServiceSettingsRecord>,
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

/// Why the value of an `IAMRoleArn` member is unacceptable, or `None` when it is an
/// IAM role ARN.
///
/// `IAMRoleArn` is modelled as a plain string carrying
/// `@aws.api#arnReference(type: "AWS::IAM::Role")` and no `@pattern` or `@length`, so
/// the boundary validator admits any text; what the reference says the value IS, this
/// checks: the six-field ARN form `arn:<partition>:iam:<region>:<account>:role/<name>`
/// with the `iam` service and a `role` resource. The region and account fields are not
/// constrained here — an IAM ARN leaves the region empty and the account may be any
/// account the caller can name a role in — so only the parts the reference fixes are
/// required.
///
/// The one place the rule lives, so every `IAMRoleArn` member of this service refuses
/// the same values: `UpdateServiceSettings$ExplorerEnablingRoleArn` and
/// `ConfigurationDefinitionInput$LocalDeploymentAdministrationRoleArn`.
///
/// The message names the member, the requirement and the value, the way the boundary
/// validator's own refusals do. The echoed value is bounded for the reason
/// [`MAX_ECHOED_ARN`] gives.
pub fn iam_role_arn_rejection(member: &str, value: &str) -> Option<String> {
    let fields: Vec<&str> = value.split(':').collect();
    let well_formed = match fields.as_slice() {
        ["arn", partition, "iam", _region, _account, resource] => {
            !partition.is_empty()
                && resource
                    .strip_prefix("role/")
                    .is_some_and(|name| !name.is_empty())
        }
        _ => false,
    };
    (!well_formed).then(|| {
        format!(
            "{member} must be the ARN of an IAM role \
             (arn:<partition>:iam::<account>:role/<name>), got '{}'",
            echoed_arn(value)
        )
    })
}

/// Longest caller-supplied value echoed in a refusal message.
///
/// The value is caller-controlled, so an unelided echo would amplify one request into
/// an arbitrarily long response. The same bound the framework's own constraint walk
/// applies.
const MAX_ECHOED_ARN: usize = 256;

/// `value` as it may appear in a refusal message: itself when short, otherwise a
/// character-boundary-safe prefix plus the elision the framework's renderer appends.
fn echoed_arn(value: &str) -> String {
    if value.len() <= MAX_ECHOED_ARN {
        return value.to_string();
    }
    let mut end = MAX_ECHOED_ARN;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}... [{} bytes]", &value[..end], value.len())
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
