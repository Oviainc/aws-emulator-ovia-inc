// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-applicationsignals-applicationsignals service.
//!
//! The service level objective (SLO) is this service's primary resource, so the store is
//! the set of SLOs this emulator holds plus the tags attached to each one by ARN. The
//! derivation helpers below live here, beside the store, because the SLO lifecycle
//! operations (`CreateServiceLevelObjective`, `GetServiceLevelObjective`,
//! `ListServiceLevelObjectives`, `UpdateServiceLevelObjective` and
//! `DeleteServiceLevelObjective`) must all agree on identity, on the ARN they mint and on
//! the server-derived members of a stored record — two copies of that logic would let a
//! Get disagree with the Create that wrote the record.

use std::collections::{BTreeMap, HashMap};
use std::ops::Bound;
use std::time::Duration;

use ::aws_smithy_types::DateTime;

use crate::ops::types::{
    ConflictException, DependencyConfig, ExclusionWindow, Goal, GroupingAttributeDefinition,
    GroupingConfiguration, Interval, MetricDataQuery, MetricSource,
    MonitoredRequestCountMetricDataQueries,
    RequestBasedServiceLevelIndicator, RequestBasedServiceLevelIndicatorConfig,
    RequestBasedServiceLevelIndicatorMetric, ResourceNotFoundException, RollingInterval,
    ServiceLevelIndicator, ServiceLevelIndicatorConfig, ServiceLevelIndicatorMetric,
    ServiceLevelObjective, ServiceLevelObjectiveSummary, Tag,
};
// The shapes the time-ranged topology reads project, on their own `use` so this set and the
// SLO lifecycle's above are edited on separate lines.
use crate::ops::types::{
    AttributeFilter, ChangeEvent, Dimension, MetricReference, Service, ServiceDependency,
    ServiceDependent, ServiceGroup, ServiceOperation, ServiceState, ServiceSummary,
};
// The dynamic instrumentation shapes, on their own `use` for the same reason.
use crate::ops::types::{
    InstrumentationConfiguration, InstrumentationConfigurationWithoutServiceEnv,
    InstrumentationStatusEvent, Location, LocationIdentifier,
};
// The shapes the audit read projects, on their own `use` for the same reason.
use crate::ops::types::{
    AuditFinding, AuditTarget, AuditTargetEntity, AuditorResult, MetricGraph, ServiceEntity,
};

/// This service's SigV4 signing name, which is also its ARN namespace
/// (`OPERATIONS.md`: `sigv4 signing name: application-signals`, `ARN namespace:
/// application-signals`). The modelled `ServiceLevelObjectiveArn` pattern
/// (`^arn:(aws|aws-us-gov):application-signals:[^:]*:[^:]*:slo/...$`) carries the same
/// namespace.
pub const SIGNING_NAME: &str = "application-signals";

/// The resource-type segment of an SLO ARN, from the modelled `ServiceLevelObjectiveArn`
/// pattern (`:slo/<name>`), which spells that segment in lower case.
pub const SLO_ARN_RESOURCE: &str = "slo";

/// The `ResourceType` an SLO failure reports, and the type token its message names: the
/// resource's TYPE NAME, which is capitalised — not the lower-case segment an ARN spells
/// (`SLO_ARN_RESOURCE`). The two are different strings for one resource kind, and a client
/// comparing `ResourceType` is comparing against the type name.
pub const SLO_RESOURCE_TYPE: &str = "Slo";

/// Region for a minted ARN when the request carries no SigV4 credential scope to read one
/// from; the emulator's documented default region (`AGENTS.md`).
pub const DEFAULT_REGION: &str = "us-east-1";

/// `EvaluationType` for an SLO configured with `SliConfig`
/// (`smithy_query(shape="EvaluationType")` -> `["PeriodBased", "RequestBased"]`).
pub const EVALUATION_TYPE_PERIOD_BASED: &str = "PeriodBased";

/// `EvaluationType` for an SLO configured with `RequestBasedSliConfig`.
pub const EVALUATION_TYPE_REQUEST_BASED: &str = "RequestBased";

/// The largest page `ListServiceLevelObjectives` returns when the caller names no
/// `MaxResults`: the modelled maximum of `ListServiceLevelObjectivesMaxResults`
/// (`range 1..50`), since the member declares no default.
pub const MAX_SLO_PAGE: i32 = 50;

/// The most SLOs this service holds, after which a create is refused with the modelled
/// `ServiceQuotaExceededException`.
///
/// The real service enforces a per-account, per-Region quota on service level objectives
/// (CloudWatch Application Signals, "Service level objectives" — a soft quota of 2,000),
/// and the create operation models `ServiceQuotaExceededException` precisely for it. The
/// cap is also what keeps the store bounded: the record count is otherwise chosen by the
/// caller, since nothing else in the create path evicts or expires a record.
pub const MAX_SLOS: usize = 2_000;

/// The largest page `ListAuditFindings` returns when the caller names no `MaxResults`.
/// Model doc for the member: "The maximum number of audit findings to return in one
/// operation. If you omit this parameter, the default of 10 is used."
pub const DEFAULT_AUDIT_FINDING_PAGE: i32 = 10;

/// `Goal.Interval` when omitted. Model doc: "If you omit this parameter, a rolling
/// interval of 7 days is used."
const DEFAULT_ROLLING_INTERVAL_DURATION: i32 = 7;

/// The `DurationUnit` of that default rolling interval
/// (`smithy_query(shape="DurationUnit")` -> `["MINUTE", "HOUR", "DAY", "MONTH"]`).
const DEFAULT_ROLLING_INTERVAL_UNIT: &str = "DAY";

/// `Goal.AttainmentGoal` when omitted. Model doc: "If you omit this parameter, 99 is used
/// to represent 99% as the attainment goal."
const DEFAULT_ATTAINMENT_GOAL: f64 = 99.0;

/// `Goal.WarningThreshold` when omitted. Model doc: "If you omit this parameter, the
/// default of 50.0 is used."
const DEFAULT_WARNING_THRESHOLD: f64 = 50.0;

/// `ServiceLevelIndicatorConfig.MetricThreshold`'s modelled `@default` of `0`.
const DEFAULT_SLI_METRIC_THRESHOLD: f64 = 0.0;

/// `ServiceLevelIndicatorConfig.ComparisonOperator`'s modelled `@default` of `"LessThan"`.
const DEFAULT_SLI_COMPARISON_OPERATOR: &str = "LessThan";

/// The marker every `NextToken` this service issues carries, so a well-formed token this
/// service did not issue is refused instead of being read as a cursor. The `/` cannot occur
/// in an SLO name (the modelled `ServiceLevelObjectiveName` pattern admits only
/// `[-._0-9A-Za-z ]`), so the prefix can never be mistaken for part of the cursor.
const NEXT_TOKEN_TAG: &str = "slos/1/";

/// The [`NEXT_TOKEN_TAG`] of each time-ranged topology read. One tag per operation, so a
/// token one of them issued is not read as a cursor by another: the offsets they count are
/// into different projections, and honouring a foreign one would silently skip entries.
pub const SERVICE_OPERATIONS_TOKEN_TAG: &str = "service-operations/1/";
/// [`SERVICE_OPERATIONS_TOKEN_TAG`], for `ListServiceDependencies`.
pub const SERVICE_DEPENDENCIES_TOKEN_TAG: &str = "service-dependencies/1/";
/// [`SERVICE_OPERATIONS_TOKEN_TAG`], for `ListServiceDependents`.
pub const SERVICE_DEPENDENTS_TOKEN_TAG: &str = "service-dependents/1/";
/// [`SERVICE_OPERATIONS_TOKEN_TAG`], for `ListAuditFindings`.
pub const AUDIT_FINDINGS_TOKEN_TAG: &str = "audit-findings/1/";
/// [`SERVICE_OPERATIONS_TOKEN_TAG`], for `ListServices`.
pub const SERVICES_TOKEN_TAG: &str = "services/1/";
/// [`SERVICE_OPERATIONS_TOKEN_TAG`], for `ListEntityEvents`.
pub const ENTITY_EVENTS_TOKEN_TAG: &str = "entity-events/1/";
/// [`SERVICE_OPERATIONS_TOKEN_TAG`], for `ListServiceStates`.
pub const SERVICE_STATES_TOKEN_TAG: &str = "service-states/1/";

/// Per-service state for com-amazonaws-applicationsignals-applicationsignals. A handler reaches it through [`service_state`].
///
/// `slos` is keyed by SLO NAME: a name is unique within an account and region (the
/// `ConflictException` a repeated create answers is exactly that uniqueness), and the
/// ordered map gives `ListServiceLevelObjectives` a deterministic order to page over.
///
/// The key is the name ALONE, not `(region, name)`, because this store is one account's and
/// one Region's: a process holds a single engine state, every ARN it mints carries
/// `engine::emulator_account_id()`, and `ListServiceLevelObjectives` reports nothing for any
/// other account. The region in a minted ARN is therefore a rendering of the caller's SigV4
/// scope rather than part of an SLO's identity, which is also why [`slo_name_of`] resolves
/// an ARN by its name segment and ignores the rest.
#[derive(Debug, Default)]
pub struct ComAmazonawsApplicationsignalsApplicationsignalsState {
    /// Every SLO created in this emulator, keyed by `ServiceLevelObjective::name`. Bounded
    /// at [`MAX_SLOS`] entries by the create path, which is the only writer.
    pub slos: BTreeMap<String, ServiceLevelObjective>,
    /// Tags per resource ARN, the key the service's tagging operations address a resource
    /// by (`TagResource`/`UntagResource`/`ListTagsForResource` take a `ResourceArn`). One
    /// entry per tagged resource of either kind this service holds, so
    /// [`MAX_SLOS`] + [`MAX_INSTRUMENTATION_CONFIGURATIONS`] bounds this map too; the entry
    /// is dropped once the resource holds no tags (see [`Self::remove_tags`]) or the resource
    /// itself is gone.
    pub tags: BTreeMap<String, BTreeMap<String, String>>,
    /// Every dynamic instrumentation configuration this emulator holds, grouped by the
    /// [`InstrumentationScope`] a `ListInstrumentationConfigurations` call names.
    ///
    /// The grouping is the one the API itself imposes: every instrumentation operation takes
    /// `Service`, `Environment` and `InstrumentationType` together (the type "determines
    /// which backing store to query"), and a list reports exactly one scope's set. Within a
    /// scope the records are keyed by [`instrumentation_key`] — signal type and location
    /// hash — which is the rest of the identity a create conflicts on, and the ordered map
    /// gives the list a deterministic order to page over.
    pub instrumentation_configurations: BTreeMap<InstrumentationScope, InstrumentationConfigurationSet>,
    /// The next instant [`ComAmazonawsApplicationsignalsApplicationsignalsState::expire_instrumentation_configurations`]
    /// has anything to do, or `None` while it owes nothing — so the pass every create and
    /// list makes walks the store only when the clock has actually made a lapse or a scope
    /// drop due.
    instrumentation_due_at: Option<DateTime>,
    /// The exclusion windows configured on each SLO, keyed by the same SLO NAME `slos` is
    /// keyed by, so the two maps name one resource and a window set cannot outlive the SLO
    /// it belongs to (see [`Self::remove`]).
    ///
    /// The windows of one SLO are an ORDERED SET identified by configuration: a removal
    /// names a window by its configuration (`RemoveExclusionWindows`: "The window
    /// configuration must match an existing exclusion window"), so two windows with the same
    /// configuration would be one window named twice. The order is the order the windows
    /// were added in, which is what makes a `ListServiceLevelObjectiveExclusionWindows` page
    /// cursor a stable boundary. Bounded at [`MAX_EXCLUSION_WINDOWS_PER_SLO`] per SLO by the
    /// only writer, and the entry is dropped once an SLO holds no windows — holding none and
    /// holding an empty set are the same state, exactly as for `tags`.
    pub exclusion_windows: BTreeMap<String, Vec<ExclusionWindow>>,
    /// This account's grouping configuration, or `None` while none is configured.
    ///
    /// At most ONE configuration exists: `PutGroupingConfiguration` "creates or updates the
    /// grouping configuration for this account", so a second put REPLACES the first rather
    /// than adding a record, and `DeleteGroupingConfiguration` "deletes the grouping
    /// configuration for this account", which returns the store to `None`. The single slot
    /// is also why this is an `Option` rather than a map: like `slos`, this store is one
    /// account's and one Region's.
    pub grouping_configuration: Option<GroupingConfiguration>,
    /// This account's Application Signals enablement, or `None` while discovery has never
    /// been started.
    ///
    /// At most ONE record exists, for the same reason `grouping_configuration` holds one:
    /// this store is one account's. `StartDiscovery` "enables this Amazon Web Services
    /// account to be able to use CloudWatch Application Signals by creating the
    /// AWSServiceRoleForCloudWatchApplicationSignals service-linked role", and an account
    /// that already holds that role needs no second one — so the record is written by the
    /// FIRST call and left exactly as it is by every later one
    /// ([`Self::start_discovery`]).
    discovery: Option<Discovery>,
}

impl ComAmazonawsApplicationsignalsApplicationsignalsState {
    /// The stored SLO an `Id` names, accepting either form the modelled
    /// `ServiceLevelObjectiveId` pattern allows: a bare name or a full `slo/<name>` ARN.
    pub fn resolve(&self, id: &str) -> Option<&ServiceLevelObjective> {
        self.slos.get(slo_name_of(id))
    }

    /// [`Self::resolve`] for a mutating caller.
    pub fn resolve_mut(&mut self, id: &str) -> Option<&mut ServiceLevelObjective> {
        self.slos.get_mut(slo_name_of(id))
    }

    /// Remove the SLO an `Id` names, along with its tags, and hand back the removed
    /// record. `None` when no SLO is stored under that name.
    pub fn remove(&mut self, id: &str) -> Option<ServiceLevelObjective> {
        let removed = self.slos.remove(slo_name_of(id));
        if let Some(slo) = removed.as_ref() {
            self.tags.remove(&slo.arn);
            // The window set belongs to the SLO, so it goes with it: a later SLO created
            // under the same name is a NEW resource and starts with no exclusion windows.
            self.exclusion_windows.remove(&slo.name);
        }
        removed
    }

    /// The ARN this resource's tag set is stored under, for a tagging operation's
    /// `ResourceArn`: the ARN of the STORED resource the request names, read off the record
    /// rather than taken from the request. `None` when the ARN names no resource this
    /// service holds, which is what the tagging operations answer
    /// `ResourceNotFoundException` for.
    ///
    /// `ResourceArn` is an ARN (`AmazonResourceName`), so only the ARN form resolves: a
    /// bare SLO name is not a resource ARN and names no resource here. Which ARNs name an
    /// SLO of this service and account is [`tagged_slo_name_in_arn`]'s rule.
    ///
    /// An instrumentation configuration is tagged too (`CreateInstrumentationConfiguration`
    /// takes a `TagList`), and its ARN carries the service and environment names, which may
    /// themselves contain `/` — so it is resolved by comparing against the ARN each stored
    /// record was minted with rather than by taking the identity apart again. The scan is
    /// bounded by [`MAX_INSTRUMENTATION_CONFIGURATIONS`].
    pub fn tagged_resource_arn(&self, resource_arn: &str) -> Option<&str> {
        if let Some(arn) = tagged_slo_name_in_arn(resource_arn)
            .and_then(|name| self.slos.get(name))
            .map(|slo| slo.arn.as_str())
        {
            return Some(arn);
        }
        self.instrumentation_configurations
            .values()
            .flat_map(|set| set.configurations.values())
            .find(|configuration| configuration.arn == resource_arn)
            .map(|configuration| configuration.arn.as_str())
    }

    /// The tags attached to the resource stored under `arn`, as the modelled `TagList`.
    ///
    /// Key order: the store is ordered by tag key, so two reads of one resource report the
    /// tags in the same order. A resource with no tags has no entry in the map and reports
    /// the empty list — holding no tags and holding an empty tag set are the same state.
    pub fn tag_list(&self, arn: &str) -> Vec<Tag> {
        self.tags
            .get(arn)
            .into_iter()
            .flatten()
            .map(|(key, value)| Tag {
                key: key.clone(),
                value: value.clone(),
            })
            .collect()
    }

    /// Detach `keys` from the tag set of the resource stored under `arn`, leaving every
    /// other tag of that resource in place.
    ///
    /// A key the resource does not carry removes nothing: removal is per key, so the result
    /// of a call is "none of these keys is attached any more" whether or not each one was
    /// attached to begin with, and repeating a call changes nothing further. The resource's
    /// entry is dropped once its last tag is gone, so an untagged resource is stored exactly
    /// as one that was never tagged.
    pub fn remove_tags<K: AsRef<str>>(&mut self, arn: &str, keys: &[K]) {
        let Some(tags) = self.tags.get_mut(arn) else {
            return;
        };
        for key in keys {
            tags.remove(key.as_ref());
        }
        if tags.is_empty() {
            self.tags.remove(arn);
        }
    }

    /// Replace this account's grouping configuration with `definitions`, stamping
    /// `updated_at` as the moment it was last written, and hand back the record that was
    /// stored. The stored record — never the request — is what the writer answers with and
    /// what a later read reports, so the two can never disagree.
    pub fn put_grouping_configuration(
        &mut self,
        definitions: Vec<GroupingAttributeDefinition>,
        updated_at: ::aws_smithy_types::DateTime,
    ) -> GroupingConfiguration {
        let configuration = GroupingConfiguration {
            grouping_attribute_definitions: definitions,
            updated_at,
        };
        self.grouping_configuration = Some(configuration.clone());
        configuration
    }

    /// Remove this account's grouping configuration, answering whether one was configured.
    ///
    /// Idempotent: clearing an account that has no configuration leaves the store `None`
    /// and is not an error, which is what the operation's `@idempotent` trait and its empty
    /// modelled error set (no not-found error) require.
    pub fn clear_grouping_configuration(&mut self) -> bool {
        self.grouping_configuration.take().is_some()
    }

    /// Enable Application Signals for this account, and hand back the record the account's
    /// enablement is held under.
    ///
    /// The enablement IS the service-linked role the account gains
    /// ([`discovery_service_linked_role_arn`]), stamped with the instant it was created, so
    /// `started_at` comes from the clock at the write rather than from a request.
    ///
    /// Idempotent: a role is created only while the account holds none, so an account that
    /// is already enabled keeps the record of the call that enabled it — neither the role
    /// nor the instant it was created moves — and every later call is the same success.
    pub fn start_discovery(&mut self, started_at: ::aws_smithy_types::DateTime) -> Discovery {
        self.discovery
            .get_or_insert_with(|| Discovery {
                service_linked_role_arn: discovery_service_linked_role_arn(),
                started_at,
            })
            .clone()
    }

    /// This account's Application Signals enablement, or `None` while discovery has never
    /// been started for it.
    pub fn discovery(&self) -> Option<&Discovery> {
        self.discovery.as_ref()
    }

    /// Whether another SLO would put this service over its [`MAX_SLOS`] quota.
    pub fn at_slo_quota(&self) -> bool {
        self.slos.len() >= MAX_SLOS
    }

    /// One page of the SLOs that pass `matches`, in name order, starting strictly after
    /// `cursor`, together with the cursor for the next page (`None` once the page is the
    /// last one).
    ///
    /// At most `page_size + 1` records are COLLECTED, so the memory the page holds and the
    /// response it produces are bounded by the page rather than by the number of stored
    /// records. The range scan itself still steps over any record the filter rejects, so a
    /// selector that matches few SLOs walks the stored set once.
    pub fn page<F>(
        &self,
        cursor: Option<&str>,
        page_size: usize,
        matches: F,
    ) -> (Vec<ServiceLevelObjectiveSummary>, Option<String>)
    where
        F: Fn(&ServiceLevelObjective) -> bool,
    {
        let lower = match cursor {
            Some(cursor) => Bound::Excluded(cursor),
            None => Bound::Unbounded,
        };
        let mut window: Vec<&ServiceLevelObjective> = self
            .slos
            .range::<str, _>((lower, Bound::Unbounded))
            .map(|(_, slo)| slo)
            .filter(|slo| matches(slo))
            .take(page_size.saturating_add(1))
            .collect();

        let next_cursor = if window.len() > page_size {
            window.truncate(page_size);
            window.last().map(|slo| encode_next_token(&slo.name))
        } else {
            None
        };
        (window.into_iter().map(summary_of).collect(), next_cursor)
    }
}

/// What this account's Application Signals enablement is: the service-linked role the
/// account gained when discovery was started, and the instant it gained it.
///
/// Held beside the account's other singleton records rather than derived, because the
/// instant is the clock reading of the call that enabled the account and nothing else in
/// the store carries it.
#[derive(Debug, Clone)]
pub struct Discovery {
    /// The ARN of the service-linked role the account holds, as
    /// [`discovery_service_linked_role_arn`] renders it.
    pub service_linked_role_arn: String,
    /// The instant the account was enabled: the clock at the first `StartDiscovery` call,
    /// which is when the role was created.
    pub started_at: DateTime,
}

/// The service principal the CloudWatch Application Signals service-linked role is linked
/// to — the service that assumes it, and the segment IAM files the role under.
///
/// It is this service's own endpoint prefix under `amazonaws.com` (`OPERATIONS.md`:
/// `Endpoint prefix: application-signals`), which is also the principal the emulated IAM
/// service's published catalog of service-linked role names lists this role under, so the
/// ARN this service renders and the one IAM reports for that role are the same string.
pub const SERVICE_PRINCIPAL: &str = "application-signals.amazonaws.com";

/// The service-linked role `StartDiscovery` creates in the account.
///
/// Model doc for the operation: "Enables this Amazon Web Services account to be able to use
/// CloudWatch Application Signals by creating the
/// AWSServiceRoleForCloudWatchApplicationSignals service-linked role."
pub const DISCOVERY_SERVICE_LINKED_ROLE_NAME: &str =
    "AWSServiceRoleForCloudWatchApplicationSignals";

/// The ARN of [`DISCOVERY_SERVICE_LINKED_ROLE_NAME`]:
/// `arn:aws:iam::<account>:role/aws-service-role/application-signals.amazonaws.com/AWSServiceRoleForCloudWatchApplicationSignals`
/// — a role under the path IAM gives a service-linked role.
///
/// The role is an IAM resource, so the ARN names IAM rather than this service, and IAM is
/// global: the ARN carries NO Region, whichever Region the caller signed for. One role is
/// created per account and reused, so every call reads the same ARN.
pub fn discovery_service_linked_role_arn() -> String {
    // `iam` is the IAM ARN namespace the role belongs to, not this service's.
    ::engine::emulator_arn(
        "iam",
        "",
        &format!("role/aws-service-role/{SERVICE_PRINCIPAL}/{DISCOVERY_SERVICE_LINKED_ROLE_NAME}"),
    )
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsApplicationsignalsApplicationsignalsState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsApplicationsignalsApplicationsignalsState>)
}

/// The SLO name an `Id` denotes. The modelled `ServiceLevelObjectiveId` pattern is an
/// alternation of the bare `ServiceLevelObjectiveName` and the full
/// `ServiceLevelObjectiveArn`, so both forms name one record and every operation that
/// takes an `Id` must resolve them identically.
pub fn slo_name_of(id: &str) -> &str {
    slo_name_in_arn(id).unwrap_or(id)
}

/// The SLO name an ARN names, or `None` when the string is not an SLO ARN at all.
///
/// The ARN form of the modelled `ServiceLevelObjectiveArn`: a well-formed resource ARN
/// whose resource field is `<SLO_ARN_RESOURCE>/<name>`. This is [`slo_name_of`] WITHOUT its
/// bare-name fallback, for a member the model types as an ARN rather than as a
/// `ServiceLevelObjectiveId`.
///
/// Read through `service_common::arn` — the workspace's ONE resource-ARN parse — so the name
/// comes out of the ARN's RESOURCE field, the last of the six, rather than out of the first
/// `:slo/` the string happens to contain: no earlier field (a region read off the caller's
/// credential scope, say) can be mistaken for the resource one, and this service agrees with
/// every other about what an ARN is.
pub fn slo_name_in_arn(arn: &str) -> Option<&str> {
    ::service_common::arn::resource_id_of_type(arn, SLO_ARN_RESOURCE)
}

/// The SLO name a tagging operation's `ResourceArn` names, or `None` when the ARN names no
/// SLO of this service and account.
///
/// Stricter than [`slo_name_in_arn`], because `ResourceArn` is a bare `AmazonResourceName`:
/// the model puts no pattern on it, so nothing before the handler rejects an ARN of another
/// service or another account, and an ARN that merely CARRIES an SLO-shaped resource field
/// (`arn:aws:s3:::slo/x`) names no resource here. The VENDOR field must be this service's
/// namespace (`SIGNING_NAME`, which the modelled `ServiceLevelObjectiveArn` pattern spells)
/// — which is what `service_common::arn::parse_of` narrows a parse to — and the account
/// field must be the account this store holds (`engine::emulator_account_id()`). A
/// `ServiceLevelObjectiveId` needs neither check: its modelled pattern already fixes the
/// namespace before a handler runs.
///
/// The REGION field is not compared, for the reason the store's own doc gives: this process
/// holds one account's and one Region's records, and the region in a minted ARN is a
/// rendering of the writer's SigV4 scope rather than part of an SLO's identity.
pub fn tagged_slo_name_in_arn(resource_arn: &str) -> Option<&str> {
    let arn = ::service_common::arn::parse_of(resource_arn, SIGNING_NAME)?;
    if arn.account != ::engine::emulator_account_id() {
        return None;
    }
    slo_name_in_arn(resource_arn)
}

/// The ARN of the SLO called `name` in `region`, in the shape the modelled
/// `ServiceLevelObjectiveArn` pattern requires.
pub fn slo_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(SIGNING_NAME, region, &format!("{SLO_ARN_RESOURCE}/{name}"))
}

/// The ARN of the SLO an `Id` denotes, for either form the modelled
/// `ServiceLevelObjectiveId` pattern admits: an `Id` already in ARN form IS that ARN, and a
/// bare name is rendered into the ARN it would carry in `region` (this account's, since the
/// store is one account's — see [`slo_name_of`]).
///
/// This is how an error identifies an SLO that is not stored: there is no record to read an
/// ARN off, so the ARN is derived from the identity the request named.
///
/// Which ids are already in ARN form is [`slo_name_in_arn`]'s rule — the one ARN reader in
/// this module, so the ARN this renders and the name [`slo_name_of`] resolves cannot
/// disagree about the same id.
pub fn slo_arn_of(region: &str, id: &str) -> String {
    if slo_name_in_arn(id).is_some() {
        id.to_string()
    } else {
        slo_arn(region, id)
    }
}

/// The region an ARN minted for this request names: the one in the caller's SigV4
/// credential scope, else the emulator's default. An empty region in the scope falls back
/// too, so a minted ARN never carries an empty region segment.
pub fn region_of(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The modelled `ResourceNotFoundException` for an `Id` that names no stored SLO.
///
/// The failure identifies the SLO by its ARN — the full resource identifier, which is what
/// `ResourceId`'s doc ("Can't find the resource id") names and what a client parses to
/// resolve the resource — and reports the capitalised type name in `ResourceType`. Both are
/// `@required` members a client branches on, so a bare name there would hand it an
/// un-namespaced id and a type token that no equality test against the type name matches.
/// The message states the same two facts, so a caller reading only the prose sees the ARN
/// too.
pub fn slo_not_found(region: &str, id: &str) -> ResourceNotFoundException {
    let arn = slo_arn_of(region, id);
    ResourceNotFoundException {
        message: format!("{SLO_RESOURCE_TYPE} with arn '{arn}' does not exist."),
        resource_type: SLO_RESOURCE_TYPE.to_string(),
        resource_id: arn,
    }
}

/// The modelled `ResourceNotFoundException` for a tagging operation's `ResourceArn` that
/// names no resource this service holds.
///
/// `ResourceId` is the ARN the request named, reported verbatim: the member is already a
/// full resource identifier, so there is nothing to mint from it and nothing of the
/// caller's identity to re-derive — unlike [`slo_not_found`], which renders an ARN for a
/// request that may have named a bare SLO name. `ResourceType` is the capitalised type name
/// of the resource KIND the ARN addresses ([`tagged_resource_type`]), so a client comparing
/// it sees the type it asked about rather than the type of this service's other resource.
pub fn tagged_resource_not_found(resource_arn: &str) -> ResourceNotFoundException {
    let resource_type = tagged_resource_type(resource_arn);
    ResourceNotFoundException {
        message: format!("{resource_type} with arn '{resource_arn}' does not exist."),
        resource_type: resource_type.to_string(),
        resource_id: resource_arn.to_string(),
    }
}

/// The capitalised type name of the resource kind a tagging operation's `ResourceArn`
/// addresses: this service tags two kinds, and which one an ARN names is the resource TYPE
/// segment of an ARN of THIS service — read through `service_common::arn::parse_of`, the
/// workspace's ONE resource-ARN parse narrowed to this service's namespace, so the segment
/// comes out of the ARN's resource field rather than out of a substring match, and an ARN of
/// another vendor cannot be reported as a resource type of this one.
///
/// Anything else is reported as the SLO type, which is also what an ARN of another service or
/// another account earns: the tagging operations' primary resource.
pub fn tagged_resource_type(resource_arn: &str) -> &'static str {
    let names_configuration = ::service_common::arn::parse_of(resource_arn, SIGNING_NAME)
        .is_some_and(|arn| arn.resource_type() == INSTRUMENTATION_CONFIG_ARN_RESOURCE);
    if names_configuration {
        INSTRUMENTATION_CONFIG_RESOURCE_TYPE
    } else {
        SLO_RESOURCE_TYPE
    }
}

/// The modelled `ConflictException` for a create that names an SLO already stored.
///
/// `ConflictException` carries only `Message`, so the message is the only place the
/// conflicting resource can be identified — and it identifies it the same way the not-found
/// failure does, by the ARN of the resource that already exists (read off the stored
/// record, since there IS one), not by the bare name the request happened to send.
pub fn slo_already_exists(arn: &str) -> ConflictException {
    ConflictException {
        message: format!("{SLO_RESOURCE_TYPE} with arn '{arn}' already exists."),
    }
}

/// One clause of a `ValidationException` message: the member whose value violated a
/// constraint, and the requirement it violated.
///
/// The ONE place this service words a constraint violation, reached both by the
/// framework-level refusal renderer (`crate::service`) and by a handler that enforces a
/// constraint itself, so the two cannot drift.
///
/// The clause names the member path and the requirement and NOTHING of the value that was
/// rejected: `ValidationException` carries only `message`, so this text is the whole
/// machine-readable detail a client gets, and the rejected value is caller input that would
/// otherwise be copied into every client log recording the failure.
pub fn constraint_clause(member_path: &str, requirement: &str) -> String {
    format!("Value at '{member_path}' failed to satisfy constraint: {requirement}")
}

/// [`constraint_clause`] for a member that was ABSENT, whose requirement is that it be
/// present.
///
/// Worded through the same builder as every other clause, with NO rendering of the absent
/// value: the recorded refusal of a call that omits a `@required` `@httpQuery` member reads
/// `1 validation error detected: Value at 'endTime' failed to satisfy constraint: Member
/// must not be null` — the clause states the member and the requirement, and says nothing
/// where a value would go. Reporting the absence as a `null` VALUE would describe a value
/// the request never carried.
pub fn absent_member_clause(member_path: &str) -> String {
    constraint_clause(member_path, "Member must not be null")
}

/// The `ValidationException` message over one or more clauses: the count of violated
/// constraints, then the clauses.
pub fn validation_envelope(clauses: &[String]) -> String {
    match clauses.len() {
        1 => format!("1 validation error detected: {}", clauses[0]),
        n => format!(
            "{n} validation errors detected: {}",
            clauses.join("; ")
        ),
    }
}

/// A `@pattern` expression without the anchors a model may spell it with: the requirement
/// stated is the expression a value must satisfy, not the anchoring the model uses to make
/// the match total. A `$` escaped by a backslash is a literal dollar sign inside the
/// expression, not the end anchor, so it stays.
///
/// Only the expression's OUTER anchors are stripped: a `@pattern` that anchors each branch
/// of an alternation separately (`^a$|^b$`, as `ServiceLevelObjectiveId` does) keeps the
/// inner ones, because they are part of the expression's own structure rather than a wrapper
/// around it — dropping them would change which strings the reported expression describes.
pub fn unanchored(pattern: &str) -> &str {
    let body = pattern.strip_prefix('^').unwrap_or(pattern);
    match body.strip_suffix('$') {
        Some(trimmed) => {
            let backslashes = trimmed.len() - trimmed.trim_end_matches('\\').len();
            if backslashes % 2 == 0 {
                trimmed
            } else {
                body
            }
        }
        None => body,
    }
}

/// The default `Goal.Interval`: a rolling interval of 7 days.
fn default_interval() -> Interval {
    Interval::RollingInterval(RollingInterval {
        duration_unit: DEFAULT_ROLLING_INTERVAL_UNIT.to_string(),
        duration: DEFAULT_ROLLING_INTERVAL_DURATION,
    })
}

/// The `Goal` stored for a request, with each member the model documents a default for
/// filled in when the request omitted it.
pub fn goal_with_defaults(requested: Option<&Goal>) -> Goal {
    Goal {
        interval: Some(
            requested
                .and_then(|goal| goal.interval.clone())
                .unwrap_or_else(default_interval),
        ),
        attainment_goal: Some(
            requested
                .and_then(|goal| goal.attainment_goal)
                .unwrap_or(DEFAULT_ATTAINMENT_GOAL),
        ),
        warning_threshold: Some(
            requested
                .and_then(|goal| goal.warning_threshold)
                .unwrap_or(DEFAULT_WARNING_THRESHOLD),
        ),
    }
}

/// The `MetricType` the indicator of a stored SLO reports, from the config that built it.
///
/// Both indicator shapes document the member identically
/// (`smithy_query(shape="ServiceLevelIndicatorMetric")`, member `MetricType`): "If the SLO
/// monitors either the `LATENCY` or `AVAILABILITY` metric that Application Signals
/// collects, this field displays which of those metrics is used." The member therefore
/// reports the standard metric Application Signals SELECTED for this SLO, not whatever the
/// request happened to carry, so it is reported only when the config actually made that
/// selection:
///
/// * the config must name the Application Signals entity whose standard metrics are
///   collected — its `KeyAttributes`, which the member's config-side doc requires for an
///   SLO "related to a metric collected by Application Signals"; and
/// * the config must not specify a metric of its OWN, since then that metric, not a
///   standard one, is what the SLO monitors: CloudWatch metric or metric-math queries
///   (`MetricDataQueries`, or a request-based config's request-count queries), a
///   `MetricName` which the model defines as "the name of the CloudWatch metric to use for
///   the SLO, when using a custom metric rather than Application Signals standard metrics",
///   or a `MetricSource`, "the metric source for SLOs on resources other than Application
///   Signals services".
///
/// Otherwise the member stays ABSENT: reporting it would tell a client the SLO tracks an
/// Application Signals standard metric when the SLO's metric is the one the request spelled
/// out, and would send that classification back on a Get-into-Update round trip.
fn reported_metric_type(
    requested: Option<&String>,
    key_attributes: Option<&HashMap<String, String>>,
    metric_name: Option<&str>,
    metric_source: Option<&MetricSource>,
    config_names_its_own_metric: bool,
) -> Option<String> {
    let selects_standard_metric = key_attributes.is_some()
        && metric_name.is_none()
        && metric_source.is_none()
        && !config_names_its_own_metric;
    requested.filter(|_| selects_standard_metric).cloned()
}

/// The `ServiceLevelIndicator` a period-based `SliConfig` describes.
///
/// Every member of the indicator is the corresponding member of the config, with the
/// config's own modelled defaults applied to `MetricThreshold` and `ComparisonOperator`,
/// except `MetricType`, which is reported only when the config selected an Application
/// Signals standard metric (see [`reported_metric_type`]).
/// `SliMetric.MetricDataQueries` is `@required` on the indicator but optional on the
/// config: when the request supplies no queries it stays EMPTY rather than being filled
/// with a synthesised CloudWatch query — the real service builds that query from its own
/// standard-metric catalogue (namespace, dimension set and query id), none of which this
/// emulator can compute, and inventing one would report a metric nothing selected.
pub fn sli_from_config(config: &ServiceLevelIndicatorConfig) -> ServiceLevelIndicator {
    let metric = &config.sli_metric_config;
    ServiceLevelIndicator {
        sli_metric: ServiceLevelIndicatorMetric {
            key_attributes: metric.key_attributes.clone(),
            operation_name: metric.operation_name.clone(),
            metric_type: reported_metric_type(
                metric.metric_type.as_ref(),
                metric.key_attributes.as_ref(),
                metric.metric_name.as_deref(),
                metric.metric_source.as_ref(),
                metric
                    .metric_data_queries
                    .as_ref()
                    .is_some_and(|queries| !queries.is_empty()),
            ),
            metric_data_queries: metric.metric_data_queries.clone().unwrap_or_default(),
            dependency_config: metric.dependency_config.clone(),
            metric_source: metric.metric_source.clone(),
            composite_sli_config: metric.composite_sli_config.clone(),
        },
        metric_threshold: config
            .metric_threshold
            .unwrap_or(DEFAULT_SLI_METRIC_THRESHOLD),
        comparison_operator: config
            .comparison_operator
            .clone()
            .unwrap_or_else(|| DEFAULT_SLI_COMPARISON_OPERATOR.to_string()),
    }
}

/// The `RequestBasedServiceLevelIndicator` a `RequestBasedSliConfig` describes.
///
/// `MetricThreshold`/`ComparisonOperator` carry no default on this config (the model
/// declares them only "required if this SLO is tracking the Latency metric"), so they are
/// stored exactly as supplied, and `MetricType` follows the same standard-metric rule as
/// the period-based indicator (see [`reported_metric_type`]).
/// `MonitoredRequestCountMetric` is `@required` on the
/// indicator but optional on the config; as with the period-based queries it is kept at an
/// empty good-count list rather than being filled with a synthesised query.
pub fn request_based_sli_from_config(
    config: &RequestBasedServiceLevelIndicatorConfig,
) -> RequestBasedServiceLevelIndicator {
    let metric = &config.request_based_sli_metric_config;
    RequestBasedServiceLevelIndicator {
        request_based_sli_metric: RequestBasedServiceLevelIndicatorMetric {
            key_attributes: metric.key_attributes.clone(),
            operation_name: metric.operation_name.clone(),
            metric_type: reported_metric_type(
                metric.metric_type.as_ref(),
                metric.key_attributes.as_ref(),
                metric.metric_name.as_deref(),
                metric.metric_source.as_ref(),
                metric
                    .total_request_count_metric
                    .as_ref()
                    .is_some_and(|queries| !queries.is_empty())
                    || metric.monitored_request_count_metric.is_some(),
            ),
            total_request_count_metric: metric
                .total_request_count_metric
                .clone()
                .unwrap_or_default(),
            monitored_request_count_metric: metric
                .monitored_request_count_metric
                .clone()
                .unwrap_or(MonitoredRequestCountMetricDataQueries::GoodCountMetric(
                    Vec::new(),
                )),
            dependency_config: metric.dependency_config.clone(),
            metric_source: metric.metric_source.clone(),
            composite_sli_config: metric.composite_sli_config.clone(),
        },
        metric_threshold: config.metric_threshold,
        comparison_operator: config.comparison_operator.clone(),
    }
}

/// The `EvaluationType` the SLI config a request supplies describes, or `None` when the
/// request supplies neither config.
///
/// The two configs are mutually exclusive, and which one a request carries is the ONLY
/// thing that decides the kind: `SliConfig` is documented as "if this SLO is a period-based
/// SLO, this structure defines..." and `RequestBasedSliConfig` as "if this SLO is a
/// request-based SLO, ...". Derived in one place so every operation that accepts the pair
/// classifies it identically.
pub fn requested_evaluation_type(
    sli_config: Option<&ServiceLevelIndicatorConfig>,
    request_based_sli_config: Option<&RequestBasedServiceLevelIndicatorConfig>,
) -> Option<&'static str> {
    if sli_config.is_some() {
        Some(EVALUATION_TYPE_PERIOD_BASED)
    } else if request_based_sli_config.is_some() {
        Some(EVALUATION_TYPE_REQUEST_BASED)
    } else {
        None
    }
}

/// The `EvaluationType` a STORED SLO is of, or `None` when the record carries no indicator
/// at all (a create that supplied neither config).
///
/// Judged from the record's own declared `EvaluationType`, falling back to whichever
/// indicator it holds, so the kind of a record is read off the record rather than recomputed
/// from the request that is trying to change it. Only the two values the modelled
/// `EvaluationType` enum names are ever written, so any other string is treated as no
/// declared kind and the indicator decides.
pub fn stored_evaluation_type(slo: &ServiceLevelObjective) -> Option<&'static str> {
    match slo.evaluation_type.as_deref() {
        Some(EVALUATION_TYPE_PERIOD_BASED) => Some(EVALUATION_TYPE_PERIOD_BASED),
        Some(EVALUATION_TYPE_REQUEST_BASED) => Some(EVALUATION_TYPE_REQUEST_BASED),
        _ if slo.sli.is_some() => Some(EVALUATION_TYPE_PERIOD_BASED),
        _ if slo.request_based_sli.is_some() => Some(EVALUATION_TYPE_REQUEST_BASED),
        _ => None,
    }
}

/// The `MetricSourceType` an SLI describes.
///
/// `smithy_query(shape="MetricSourceType")` lists exactly six values — `AppMonitor`,
/// `Canary`, `CloudWatchMetric`, `Service`, `ServiceDependency`, `ServiceOperation` — and
/// the member's doc calls them "the SLI metric source type for this SLO". They name the
/// six kinds of source an SLI config can describe, so the value follows from which source
/// members the request supplied, most specific first: a `MetricSource` names a resource
/// outside Application Signals and carries its own `Type` key attribute; otherwise a
/// dependency, then an operation, then a service narrows the Application Signals entity;
/// and an SLI that names no entity at all is reading plain CloudWatch metrics.
///
/// A `MetricSource` whose `Type` names neither of the two specific resource kinds the enum
/// has values for falls back to `CloudWatchMetric` rather than to `None`: the modelled
/// `MetricSource` doc defines the member as "the metric source for SLOs on resources other
/// than Application Signals services", so such an SLO IS reading that resource's CloudWatch
/// metrics, and `CloudWatchMetric` is the enum's only value for a non-Application-Signals
/// source. Reporting `None` would drop a source kind the request did state, and reporting
/// `Service` would claim an Application Signals entity the request did not name.
fn metric_source_type(
    key_attributes: Option<&HashMap<String, String>>,
    operation_name: Option<&str>,
    dependency_config: Option<&DependencyConfig>,
    metric_source: Option<&MetricSource>,
    metric_data_queries: &[MetricDataQuery],
) -> Option<String> {
    if let Some(source) = metric_source {
        // A key-attribute `Type` is a CloudFormation-style name such as
        // `AWS::RUM::AppMonitor`; its last segment is the enum value.
        let kind = source
            .metric_source_key_attributes
            .get("Type")
            .and_then(|value| value.rsplit("::").next())
            .unwrap_or_default();
        return Some(
            match kind {
                "AppMonitor" => "AppMonitor",
                "Canary" => "Canary",
                _ => "CloudWatchMetric",
            }
            .to_string(),
        );
    }
    if dependency_config.is_some() {
        return Some("ServiceDependency".to_string());
    }
    if operation_name.is_some() {
        return Some("ServiceOperation".to_string());
    }
    if key_attributes.is_some() {
        return Some("Service".to_string());
    }
    if !metric_data_queries.is_empty() {
        return Some("CloudWatchMetric".to_string());
    }
    None
}

/// The `MetricSourceType` of a stored SLO, from whichever indicator it carries.
pub fn derive_metric_source_type(
    sli: Option<&ServiceLevelIndicator>,
    request_based_sli: Option<&RequestBasedServiceLevelIndicator>,
) -> Option<String> {
    if let Some(sli) = sli {
        let metric = &sli.sli_metric;
        return metric_source_type(
            metric.key_attributes.as_ref(),
            metric.operation_name.as_deref(),
            metric.dependency_config.as_ref(),
            metric.metric_source.as_ref(),
            &metric.metric_data_queries,
        );
    }
    if let Some(request_based_sli) = request_based_sli {
        let metric = &request_based_sli.request_based_sli_metric;
        return metric_source_type(
            metric.key_attributes.as_ref(),
            metric.operation_name.as_deref(),
            metric.dependency_config.as_ref(),
            metric.metric_source.as_ref(),
            &metric.total_request_count_metric,
        );
    }
    None
}

/// The entity members a stored SLO's indicator identifies: key attributes, operation name,
/// dependency, metric source and composite-SLI configuration. `ListServiceLevelObjectives`
/// both projects and filters on them, so they are read out of the record in ONE place.
pub struct SloEntity {
    pub key_attributes: Option<HashMap<String, String>>,
    pub operation_name: Option<String>,
    /// The kind of metric the indicator measures (`MetricType`,
    /// `smithy_query(shape="ServiceLevelIndicatorMetric")`), which is how a read that
    /// selects on the measured type — an audit target naming a service operation and its
    /// metric type — narrows to the objectives that measure it.
    pub metric_type: Option<String>,
    pub dependency_config: Option<DependencyConfig>,
    pub metric_source: Option<MetricSource>,
    pub composite_sli_config: Option<crate::ops::types::CompositeSliConfig>,
}

/// Read the entity an SLO's indicator identifies.
pub fn entity_of(slo: &ServiceLevelObjective) -> SloEntity {
    if let Some(sli) = slo.sli.as_ref() {
        let metric = &sli.sli_metric;
        return SloEntity {
            key_attributes: metric.key_attributes.clone(),
            operation_name: metric.operation_name.clone(),
            metric_type: metric.metric_type.clone(),
            dependency_config: metric.dependency_config.clone(),
            metric_source: metric.metric_source.clone(),
            composite_sli_config: metric.composite_sli_config.clone(),
        };
    }
    if let Some(request_based_sli) = slo.request_based_sli.as_ref() {
        let metric = &request_based_sli.request_based_sli_metric;
        return SloEntity {
            key_attributes: metric.key_attributes.clone(),
            operation_name: metric.operation_name.clone(),
            metric_type: metric.metric_type.clone(),
            dependency_config: metric.dependency_config.clone(),
            metric_source: metric.metric_source.clone(),
            composite_sli_config: metric.composite_sli_config.clone(),
        };
    }
    SloEntity {
        key_attributes: None,
        operation_name: None,
        metric_type: None,
        dependency_config: None,
        metric_source: None,
        composite_sli_config: None,
    }
}

/// The `ServiceLevelObjectiveSummary` projection of a stored SLO, as
/// `ListServiceLevelObjectives` returns it.
pub fn summary_of(slo: &ServiceLevelObjective) -> ServiceLevelObjectiveSummary {
    let entity = entity_of(slo);
    ServiceLevelObjectiveSummary {
        arn: slo.arn.clone(),
        name: slo.name.clone(),
        key_attributes: entity.key_attributes,
        operation_name: entity.operation_name,
        dependency_config: entity.dependency_config,
        created_time: Some(slo.created_time),
        evaluation_type: slo.evaluation_type.clone(),
        metric_source_type: slo.metric_source_type.clone(),
        metric_source: entity.metric_source,
        composite_sli_config: entity.composite_sli_config,
    }
}

/// The message a handler reports when [`service_state`] answers `None`.
///
/// `None` means this service was mounted on an engine type it does not know — an assembly
/// mistake no request can fix, and one a correctly built router never produces. None of the
/// SLO operations models a server-fault error, so each reports it as the modelled
/// `ThrottlingException`: of the errors they do declare, it is the only one that does not
/// mislead the caller about its own request.
pub const STATE_UNAVAILABLE: &str = "Application Signals state is unavailable";

/// `NextToken` is opaque to the caller: the hex encoding of [`NEXT_TOKEN_TAG`] followed by
/// the last SLO name the previous page returned, which the next call resolves against the
/// ordered store as an exclusive cursor.
pub fn encode_next_token(last_name: &str) -> String {
    encode_cursor(NEXT_TOKEN_TAG, last_name)
}

/// The cursor a `NextToken` carries, or `None` when the token is not one this service
/// issued — which covers a token that is not hex, is not UTF-8, or is well-formed but
/// carries no [`NEXT_TOKEN_TAG`], so a forged token is refused rather than honoured as a
/// page boundary.
pub fn decode_next_token(token: &str) -> Option<String> {
    decode_cursor(NEXT_TOKEN_TAG, token)
}

/// A `NextToken` carrying `cursor` under `tag`: hex, so the caller reads it as the opaque
/// value the model documents rather than as a store key it could edit.
pub fn encode_cursor(tag: &str, cursor: &str) -> String {
    ::hex::encode(format!("{tag}{cursor}"))
}

/// The cursor `token` carries under `tag`, or `None` when it is not a token this service
/// issued under that tag.
pub fn decode_cursor(tag: &str, token: &str) -> Option<String> {
    let decoded = String::from_utf8(::hex::decode(token).ok()?).ok()?;
    decoded.strip_prefix(tag).map(ToString::to_string)
}

/// One page of a derived projection, plus the token for the page after it.
///
/// The projections the time-ranged topology reads page over are DERIVED (they are
/// recomputed from the stored SLOs on every call) rather than stored, so a cursor into them
/// is an offset into their deterministic order, not a key of a stored record. Each read
/// passes its own [`SERVICE_OPERATIONS_TOKEN_TAG`]-style tag, so a token is only ever
/// resolved against the projection that issued it; `Err` carries the refusal message for a
/// token that is not one this service issued under that tag.
pub fn page_of<T>(
    items: Vec<T>,
    tag: &str,
    token: Option<&str>,
    page_size: usize,
) -> Result<(Vec<T>, Option<String>), String> {
    // A page of nothing could not advance its own cursor, so a caller following `NextToken`
    // would loop for ever. Every caller's `MaxResults` is modelled with a minimum of 1, so
    // this only ever clamps a bound no request can state.
    let page_size = page_size.max(1);
    let offset = match token {
        None => 0,
        Some(token) => decode_cursor(tag, token)
            .and_then(|cursor| cursor.parse::<usize>().ok())
            .ok_or_else(|| format!("NextToken `{token}` is not a token this service issued"))?,
    };
    let mut page: Vec<T> = items.into_iter().skip(offset).collect();
    let next_token = if page.len() > page_size {
        page.truncate(page_size);
        Some(encode_cursor(tag, &offset.saturating_add(page_size).to_string()))
    } else {
        None
    };
    Ok((page, next_token))
}

/// Whether `candidate` holds every entry of `filter` with the same value.
///
/// Attribute filters on this API name the entity to match, and an entity is identified by
/// as few as one of its up to four `Attributes` entries (`length 1..4`), so a filter
/// matches when it is a subset of the stored attributes — exact equality is the case where
/// the caller names them all.
pub fn attributes_contain(
    candidate: Option<&HashMap<String, String>>,
    filter: &HashMap<String, String>,
) -> bool {
    let Some(candidate) = candidate else {
        return filter.is_empty();
    };
    filter
        .iter()
        .all(|(key, value)| candidate.get(key) == Some(value))
}

/// The `Attributes` entry that says WHICH KIND of object a key-attribute map identifies:
/// "`Type` designates the type of object this is"
/// (`smithy_query(query="doc", shape="GetServiceInput", member="KeyAttributes")`).
pub const TYPE_ATTRIBUTE: &str = "Type";

/// The `Attributes` entry that NAMES the object: "`Name` specifies the name of the object"
/// (`smithy_query(query="doc", shape="GetServiceInput", member="KeyAttributes")`).
const NAME_ATTRIBUTE: &str = "Name";

/// The `Attributes` entry that says where the object runs: "`Environment` specifies the
/// location where this object is hosted" (same member doc as [`NAME_ATTRIBUTE`]).
const ENVIRONMENT_ATTRIBUTE: &str = "Environment";

/// The `Attributes` entry that says what KIND of resource an object is: "`ResourceType`
/// specifies the type of the resource" (same member doc as [`NAME_ATTRIBUTE`]).
const RESOURCE_TYPE_ATTRIBUTE: &str = "ResourceType";

/// The `Attributes` entry that picks out WHICH resource of that kind: "`Identifier`
/// identifies the resource objects of this resource" (same member doc as
/// [`NAME_ATTRIBUTE`]).
const IDENTIFIER_ATTRIBUTE: &str = "Identifier";

/// The entity types whose identity NAMES the object: `Name` "is used only if the value of
/// the `Type` field is `Service`, `RemoteService`, or `AWS::Service`"
/// (`smithy_query(query="doc", shape="GetServiceInput", member="KeyAttributes")`).
const NAMED_ENTITY_TYPES: [&str; 3] = [SERVICE_ENTITY_TYPE, "RemoteService", "AWS::Service"];

/// The entity types whose identity picks out a RESOURCE: `ResourceType` and `Identifier` are
/// used "only when the value of the `Type` field is `Resource` or `AWS::Resource`" (same
/// member doc as [`NAMED_ENTITY_TYPES`]).
const RESOURCE_ENTITY_TYPES: [&str; 2] = ["Resource", "AWS::Resource"];

/// The entity types `Type` designates, in sorted order: the two groups above together, so
/// the set of types and the attributes each type makes applicable cannot disagree.
///
/// The `KeyAttributes` doc names them as the values the rest of the map is interpreted
/// against ([`NAMED_ENTITY_TYPES`], [`RESOURCE_ENTITY_TYPES`]; the same text on
/// `ListServiceOperationsInput`, `ListServiceDependenciesInput` and
/// `ListServiceDependentsInput`). A value outside this set designates no kind of object, so
/// none of the other attributes has a meaning.
pub const ENTITY_TYPES: [&str; 5] = [
    RESOURCE_ENTITY_TYPES[1], // AWS::Resource
    NAMED_ENTITY_TYPES[2],    // AWS::Service
    NAMED_ENTITY_TYPES[1],    // RemoteService
    RESOURCE_ENTITY_TYPES[0], // Resource
    NAMED_ENTITY_TYPES[0],    // Service
];

/// The attributes BESIDE `Type` that an identity of `entity_type` is read by: the ones the
/// member's own per-`Type` list makes applicable to it.
///
/// A service is named and placed (`Name` and `Environment`); a resource is picked out by
/// what kind of resource it is and which one (`ResourceType` and `Identifier`), for which
/// `Name` has no meaning — which is why the pair depends on the `Type`. The resource pair is
/// the identity the member doc writes out for a resource: `{ "Type": "AWS::Resource",
/// "ResourceType": "AWS::DynamoDB::Table", "Identifier": "Customers" }`
/// (`smithy_query(query="doc", shape="ListEntityEventsInput", member="Entity")`), beside the
/// service identity the same doc writes as `{ "Type": "Service", "Name": "visits-service",
/// "Environment": "petclinic-test" }`.
fn identifying_attributes(entity_type: &str) -> [&'static str; 2] {
    if RESOURCE_ENTITY_TYPES.contains(&entity_type) {
        [RESOURCE_TYPE_ATTRIBUTE, IDENTIFIER_ATTRIBUTE]
    } else {
        [NAME_ATTRIBUTE, ENVIRONMENT_ATTRIBUTE]
    }
}

/// The refusal a key-attribute map that is supposed to IDENTIFY an entity earns, or `None`
/// when it identifies one.
///
/// The map must first designate the KIND of object it names, with `Type` "designating the
/// type of object this is" and carrying one of the documented values ([`ENTITY_TYPES`]): a
/// map with no `Type`, or with a `Type` outside that set, names no entity at all and is
/// refused with the modelled `ValidationException` rather than answered with the caller's own
/// map echoed back as a discovered entity.
///
/// It must then carry the MINIMUM IDENTITY the member documents — "You must specify at least
/// the `Type`, `Name`, and `Environment` attributes" — read against the member's own
/// per-`Type` list: the attributes that `Type` makes applicable
/// ([`identifying_attributes`]) are the ones saying WHICH object of that kind the read is
/// about, so a map missing any of them pins down no single entity, however many of the
/// others it carries.
///
/// Shared so the reads that take this member as the entity to report on — `GetService`,
/// `ListServiceOperations`, `ListServiceDependencies` and `ListServiceDependents` — cannot
/// disagree about which identities are serviceable. The member is NOT the same rule
/// everywhere it appears: `ListServiceLevelObjectives` takes an OPTIONAL filter whose doc
/// drops the "must specify" sentence, an SLO's own `KeyAttributes` records the object the SLO
/// relates to, and `ListEntityEvents` takes the entity to report on under a member whose doc
/// carries no "must specify" sentence either — so none of them is subject to the minimum,
/// and `ListEntityEvents` reads its entity by the type rule alone
/// ([`entity_type_refusal_at`]). A caller may still name as few attributes BEYOND the minimum
/// as pins the entity down ([`attributes_contain`]).
pub fn entity_identity_refusal(key_attributes: &HashMap<String, String>) -> Option<String> {
    let entity_type = match entity_type_clause(KEY_ATTRIBUTES_MEMBER_PATH, key_attributes) {
        Err(clause) => return Some(validation_envelope(&[clause])),
        Ok(entity_type) => entity_type,
    };
    let clauses: Vec<String> = identifying_attributes(entity_type)
        .into_iter()
        .filter(|attribute| !key_attributes.contains_key(*attribute))
        .map(|attribute| {
            constraint_clause(
                KEY_ATTRIBUTES_MEMBER_PATH,
                &format!("Member must contain the entity identity attribute '{attribute}'"),
            )
        })
        .collect();
    (!clauses.is_empty()).then(|| validation_envelope(&clauses))
}

/// The refusal a map that must designate the KIND of object it names earns, or `None` when it
/// does — the type half of [`entity_identity_refusal`] on its own, for an operation whose
/// member documents no minimum identity: `ListEntityEvents` takes the entity to report on as
/// `Entity` ([`ENTITY_MEMBER_PATH`]), whose doc lists the same attributes per `Type` without
/// the "must specify" sentence.
///
/// The NAME is a parameter because a refusal states the member the request carries, and
/// naming a member the request never sent would leave a client with nothing to fix.
pub fn entity_type_refusal_at(
    member_path: &str,
    attributes: &HashMap<String, String>,
) -> Option<String> {
    entity_type_clause(member_path, attributes)
        .err()
        .map(|clause| validation_envelope(&[clause]))
}

/// The entity type `attributes` designates, or the clause a map that designates none earns.
///
/// The ONE place the `Type` entry is read, so every operation that reads an entity by this
/// map agrees on which values designate a kind of object.
fn entity_type_clause<'attributes>(
    member_path: &str,
    attributes: &'attributes HashMap<String, String>,
) -> Result<&'attributes str, String> {
    match attributes.get(TYPE_ATTRIBUTE) {
        None => Err(constraint_clause(
            member_path,
            &format!("Member must contain the entity type attribute '{TYPE_ATTRIBUTE}'"),
        )),
        Some(entity_type) if !ENTITY_TYPES.contains(&entity_type.as_str()) => {
            Err(constraint_clause(
                &format!("{member_path}.{TYPE_ATTRIBUTE}"),
                &format!(
                    "Member must satisfy enum value set: [{}]",
                    ENTITY_TYPES.join(", ")
                ),
            ))
        }
        Some(entity_type) => Ok(entity_type.as_str()),
    }
}

/// The member path a refusal of the entity-identity map names: the member in the lowerCamel
/// casing this service's `ValidationException` envelope states a member with, as the
/// framework-level refusal of the same member's modelled bounds already does.
const KEY_ATTRIBUTES_MEMBER_PATH: &str = "keyAttributes";

/// [`KEY_ATTRIBUTES_MEMBER_PATH`], for the `Entity` member `ListEntityEvents` takes the
/// entity to report on under.
pub const ENTITY_MEMBER_PATH: &str = "entity";

/// Whether two `DependencyConfig`s name the same dependency. The shape carries no
/// `PartialEq`, so the comparison is written out over its two members.
pub fn same_dependency(left: Option<&DependencyConfig>, right: &DependencyConfig) -> bool {
    left.is_some_and(|left| {
        left.dependency_operation_name == right.dependency_operation_name
            && left.dependency_key_attributes == right.dependency_key_attributes
    })
}

// =============================================================================
// The exclusion windows configured on an SLO
// =============================================================================

/// The most exclusion windows one SLO holds.
///
/// The API states the limit per SLO rather than per request:
/// `BatchUpdateExclusionWindows.AddExclusionWindows` documents "You can add up to 10
/// exclusion windows per SLO", and the modelled `ExclusionWindows` list caps both that
/// member and the `ListServiceLevelObjectiveExclusionWindows` output at 10 entries \u2014 so a
/// set beyond this size could not be reported back at all.
pub const MAX_EXCLUSION_WINDOWS_PER_SLO: usize = 10;

/// The most per-SLO failures one `BatchUpdateExclusionWindows` response reports: the modelled
/// maximum of `BatchUpdateExclusionWindowsErrors` (`len ..10`), while `SloIds` admits up to
/// 50 ids.
///
/// Which SLOs a request WAS applied to is reported in full by the response's `SloIds`, so an
/// id missing from that list was not updated whether or not the capped `Errors` list also
/// names it.
pub const MAX_EXCLUSION_WINDOW_ERRORS: usize = 10;

/// The largest page `ListServiceLevelObjectiveExclusionWindows` returns when the caller
/// names no `MaxResults`.
///
/// The member's doc sentence says "the default of 50 is used", but its modelled
/// `@range` is `1..10` and the `ExclusionWindows` list it fills holds at most 10 entries, so
/// 50 is not a page this operation can ever return: the effective default is the largest page
/// the model admits, which is also the whole window set of one SLO
/// ([`MAX_EXCLUSION_WINDOWS_PER_SLO`]).
pub const DEFAULT_EXCLUSION_WINDOW_PAGE: i32 = MAX_EXCLUSION_WINDOWS_PER_SLO as i32;

/// [`SERVICE_OPERATIONS_TOKEN_TAG`], for `ListServiceLevelObjectiveExclusionWindows`.
pub const EXCLUSION_WINDOWS_TOKEN_TAG: &str = "exclusion-windows/1/";

/// Why one SLO named by a `BatchUpdateExclusionWindows` request was not updated.
///
/// The failure is per SLO because the request is: one set of windows is applied to each id
/// in `SloIds` independently, and the response reports the ids it was applied to beside an
/// `Errors` entry for each one it was not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExclusionWindowFailure {
    /// The id names no stored SLO.
    NoSuchSlo,
    /// A window in `RemoveExclusionWindows` matches no exclusion window this SLO holds.
    UnmatchedRemoval,
    /// The additions would take this SLO past [`MAX_EXCLUSION_WINDOWS_PER_SLO`] windows.
    TooManyWindows,
}

impl ExclusionWindowFailure {
    /// The `ErrorCode` this failure reports.
    ///
    /// `ExclusionWindowErrorCode` is an unconstrained string in the model
    /// (`smithy_query(shape="ExclusionWindowErrorCode")` \u2014 a `string` with no enum), so the
    /// code reported is the NAME of the modelled exception a single-SLO call would have been
    /// refused with, which is the same token a client already branches on when it reads an
    /// error envelope — and the exception a request whose every id failed is answered with,
    /// so the two paths cannot disagree about one failure.
    pub fn error_code(self) -> &'static str {
        match self {
            Self::NoSuchSlo => "ResourceNotFoundException",
            Self::UnmatchedRemoval | Self::TooManyWindows => "ValidationException",
        }
    }

    /// The `ErrorMessage` this failure reports for the SLO `id` named, in `region`.
    ///
    /// A not-found failure is worded by [`slo_not_found`] and a constraint failure by
    /// [`validation_envelope`] over [`constraint_clause`], so a per-item error reads exactly
    /// like the exception the same failure earns on its own, and neither echoes the window
    /// configuration the request carried.
    pub fn error_message(self, region: &str, id: &str) -> String {
        match self {
            Self::NoSuchSlo => slo_not_found(region, id).message,
            Self::UnmatchedRemoval => validation_envelope(&[constraint_clause(
                "removeExclusionWindows",
                "Each window configuration must match an existing exclusion window of the SLO",
            )]),
            Self::TooManyWindows => validation_envelope(&[constraint_clause(
                "addExclusionWindows",
                &format!(
                    "Member must not take the SLO past {MAX_EXCLUSION_WINDOWS_PER_SLO} \
                     exclusion windows"
                ),
            )]),
        }
    }
}

impl ComAmazonawsApplicationsignalsApplicationsignalsState {
    /// The exclusion windows configured on the SLO an `Id` names, in the order they were
    /// added. Empty for a stored SLO that holds none, and for an id that names no SLO \u2014 a
    /// caller tells the two apart from [`Self::resolve`], which is what the reads refuse an
    /// unknown id by.
    pub fn exclusion_windows_of(&self, id: &str) -> &[ExclusionWindow] {
        self.exclusion_windows
            .get(slo_name_of(id))
            .map_or(&[], Vec::as_slice)
    }

    /// Remove `remove` from the window set of the SLO an `Id` names and then add `add` to it,
    /// or answer the single [`ExclusionWindowFailure`] that left the SLO untouched.
    ///
    /// Removals are applied BEFORE additions, so one call can replace a window: the
    /// configuration being removed frees its slot before the additions are counted against
    /// [`MAX_EXCLUSION_WINDOWS_PER_SLO`].
    ///
    /// A window is named by its configuration, which is the identity a removal uses, so the
    /// set holds each configuration once: adding a configuration the SLO already holds
    /// changes nothing (and consumes no slot), and removing one it does not hold is the
    /// failure [`ExclusionWindowFailure::UnmatchedRemoval`] rather than a silent no-op, since
    /// `RemoveExclusionWindows` requires that "the window configuration must match an
    /// existing exclusion window".
    ///
    /// The whole update is applied to a COPY of the set and stored only once every window has
    /// been applied, so an SLO that earns a failure is left exactly as it was.
    pub fn update_exclusion_windows(
        &mut self,
        id: &str,
        add: &[ExclusionWindow],
        remove: &[ExclusionWindow],
    ) -> Result<(), ExclusionWindowFailure> {
        let name = slo_name_of(id);
        if !self.slos.contains_key(name) {
            return Err(ExclusionWindowFailure::NoSuchSlo);
        }

        let mut windows = self.exclusion_windows.get(name).cloned().unwrap_or_default();
        for window in remove {
            let Some(at) = windows
                .iter()
                .position(|stored| same_exclusion_window(stored, window))
            else {
                return Err(ExclusionWindowFailure::UnmatchedRemoval);
            };
            windows.remove(at);
        }
        for window in add {
            if windows
                .iter()
                .any(|stored| same_exclusion_window(stored, window))
            {
                continue;
            }
            if windows.len() >= MAX_EXCLUSION_WINDOWS_PER_SLO {
                return Err(ExclusionWindowFailure::TooManyWindows);
            }
            windows.push(window.clone());
        }

        if windows.is_empty() {
            self.exclusion_windows.remove(name);
        } else {
            self.exclusion_windows.insert(name.to_string(), windows);
        }
        Ok(())
    }
}

/// Whether two exclusion windows are the same window: the configuration a removal names one
/// by, member for member.
///
/// Every member counts, because every member is part of what the window excludes: its length
/// (`Window`), when it starts (`StartTime`), how it repeats (`RecurrenceRule`) and why
/// (`Reason`). An omitted optional member is part of the configuration too \u2014 it matches an
/// omission, not an arbitrary value \u2014 so the window a caller added is named by exactly the
/// configuration they sent, and the set a `ListServiceLevelObjectiveExclusionWindows` reports
/// is the set a `RemoveExclusionWindows` can address. `Window` and `RecurrenceRule` carry no
/// `PartialEq`, so those members are compared out, as [`same_dependency`] is; `StartTime` uses
/// `DateTime`'s own, which compares the seconds and the sub-second fields \u2014 exactly "the same
/// instant at the resolution a `Timestamp` carries", and never the float seconds, where two
/// different instants could round together.
pub fn same_exclusion_window(left: &ExclusionWindow, right: &ExclusionWindow) -> bool {
    left.window.duration_unit == right.window.duration_unit
        && left.window.duration == right.window.duration
        && left.start_time == right.start_time
        && left
            .recurrence_rule
            .as_ref()
            .map(|rule| rule.expression.as_str())
            == right
                .recurrence_rule
                .as_ref()
                .map(|rule| rule.expression.as_str())
        && left.reason == right.reason
}

// =============================================================================
// The time range every time-ranged read takes
// =============================================================================

/// The refusal a `StartTime`/`EndTime` pair earns, or `None` when the range is one this
/// service serves.
///
/// Every operation that takes the pair documents it as a period to report over
/// (`smithy_query(query="operation", operation="GetService")`: both members `@required`),
/// and a period runs forwards: the real service refuses an inverted one with
/// `ValidationException` and states the rule with both offending instants in it. The
/// instants are rendered the way the service states them — the `Date.toString` form, in
/// UTC — so the message names the times the request carried rather than a re-encoding the
/// caller never sent.
///
/// The comparison is STRICT, because that is the rule the message states: a window whose
/// start equals its end spans no time at all, so it is refused with the same wording as an
/// inverted one rather than reporting over an empty period. It is made on the same instant
/// key every window comparison in this module uses ([`instant_nanos`]), so a pair this
/// refuses and a pair a projection reports over are ordered by one rule.
pub fn time_range_refusal(start_time: &DateTime, end_time: &DateTime) -> Option<String> {
    if instant_nanos(start_time) < instant_nanos(end_time) {
        return None;
    }
    Some(format!(
        "Invalid start time {} and end time {} provided: start time must be before end time",
        stated_instant(start_time),
        stated_instant(end_time),
    ))
}

/// One instant as this service states it in a refusal: `EEE MMM dd HH:mm:ss UTC yyyy`
/// (`Sun Sep 06 02:00:00 UTC 2026`), the `java.util.Date` rendering of a UTC instant.
///
/// The calendar conversion is the LIBRARY's: `DateTime::fmt(Format::DateTime)` renders the
/// instant as RFC-3339 (`2026-09-06T02:00:00Z`) and this only reorders those already-correct
/// fields and names the month, so no leap-year or era arithmetic is re-derived here. The
/// weekday is not part of that rendering, and it needs no calendar either: day zero of the
/// epoch — 1970-01-01 — was a Thursday, so it is the day count modulo 7.
///
/// An instant the library cannot render at all is stated as its epoch-second value, so the
/// message still names the instant the request carried.
fn stated_instant(at: &DateTime) -> String {
    /// Indexed by the epoch day count modulo 7, which is 0 on Thursday 1970-01-01.
    const WEEKDAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
    /// The month abbreviations `Date.toString` uses, in month order.
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];

    let Ok(rendered) = at.fmt(::aws_smithy_types::date_time::Format::DateTime) else {
        return at.as_secs_f64().to_string();
    };
    // `<date>T<time>Z`, where the time may carry a fractional part the `Date` form omits.
    let Some((date, time)) = rendered.split_once('T') else {
        return rendered;
    };
    let mut fields = date.split('-');
    let (Some(year), Some(month), Some(day)) = (fields.next(), fields.next(), fields.next())
    else {
        return rendered;
    };
    let hour_minute_second = time.trim_end_matches('Z');
    let hour_minute_second = hour_minute_second
        .split_once('.')
        .map_or(hour_minute_second, |(whole, _)| whole);
    let Some(month) = month
        .parse::<usize>()
        .ok()
        .and_then(|month| MONTHS.get(month.wrapping_sub(1)))
    else {
        return rendered;
    };
    let weekday = WEEKDAYS[at.secs().div_euclid(86_400).rem_euclid(7) as usize];
    format!("{weekday} {month} {day} {hour_minute_second} UTC {year}")
}

// =============================================================================
// The service topology the stored SLOs are evidence of
// =============================================================================

/// Whether a stored SLO is evidence of the entity it monitors over the requested window.
///
/// An SLO monitors its entity from the moment it is created until it is deleted, so the
/// period it is evidence for is `[CreatedTime, now]`, and the two periods overlap exactly
/// when the window does not END before the SLO existed and does not BEGIN after now. So a
/// read over a past window reports an entity the account has only just started monitoring
/// as absent, and a window entirely in the future observes nothing at all.
fn monitored_in_window(
    slo: &ServiceLevelObjective,
    window_start: &DateTime,
    window_end: &DateTime,
) -> bool {
    slo.created_time.as_secs_f64() <= window_end.as_secs_f64() && window_has_begun(window_start)
}

/// Whether a window has begun: nothing has been observed in one that starts after now,
/// however far ahead it ends.
fn window_has_begun(window_start: &DateTime) -> bool {
    window_start.as_secs_f64() <= DateTime::from(::std::time::SystemTime::now()).as_secs_f64()
}

/// A dedup/order key for one `Attributes` map: its entries in key order. Used only to
/// order and deduplicate derived topology entries, never put on the wire.
fn attribute_key(attributes: &HashMap<String, String>) -> String {
    attributes
        .iter()
        .collect::<BTreeMap<_, _>>()
        .into_iter()
        .map(|(key, value)| format!("{key}\u{1f}{value}"))
        .collect::<Vec<_>>()
        .join("\u{1e}")
}

/// The CloudWatch namespace Application Signals publishes the metrics it collects for the
/// entities it discovers in.
///
/// Every topology projection reports those metrics in a `@required` `MetricReferences`
/// member documented as "one metric associated with this \<entity\> that was discovered by
/// Application Signals" (`smithy_query(shape="ServiceDependent")` /
/// `smithy_query(shape="ServiceOperation")`, and the same sentence without the clause on
/// `Service`/`ServiceSummary`/`ServiceDependency`), so an entity this emulator reports as
/// discovered carries the metrics the service collects for it.
///
/// The model states only the SHAPE of a reference (`smithy_query(shape="MetricReference")`:
/// `Namespace`, `MetricName` and `MetricType` all `@required`, `Dimensions` optional), so
/// the CATALOGUE — this namespace, the metric names in [`STANDARD_METRICS`] and the
/// dimension names in [`metric_dimensions`] — is the one the Amazon CloudWatch User Guide's
/// Application Signals chapter documents: Application Signals collects `Latency`, `Fault`
/// and `Error` in the `ApplicationSignals` namespace, dimensioned by the entity that emits
/// them (`Service`, `Environment`, `Operation`) and, for a call that entity makes, by the
/// far end of that call (the `Remote*` dimensions).
///
/// Every dimension VALUE is read out of the stored record the reference describes, so the
/// references of two entities differ exactly as their stored identities do.
const APPLICATION_SIGNALS_NAMESPACE: &str = "ApplicationSignals";

/// The metrics of that catalogue, as `(MetricName, MetricType)`, in the order they are
/// reported.
///
/// `MetricType` is "used to display the appropriate statistics in the CloudWatch console"
/// (`smithy_query(shape="MetricReference")`, member `MetricType`, `pattern
/// ^[A-Za-z0-9 -]+$`) and names the standard metric in the upper-case form the model itself
/// uses for the one of these names it spells
/// (`smithy_query(shape="ServiceLevelIndicatorMetricType")` -> `["LATENCY",
/// "AVAILABILITY"]`).
const STANDARD_METRICS: [(&str, &str); 3] = [
    ("Latency", "LATENCY"),
    ("Fault", "FAULT"),
    ("Error", "ERROR"),
];

/// The `Attributes` entries a dimension value is read from, and the dimension each becomes.
///
/// The attribute names are the member's own (`smithy_query(shape="ServiceDependent")`,
/// member `DependentKeyAttributes`): `Name` "specifies the name of the object ... used only
/// if the value of the Type field is Service, RemoteService, or AWS::Service",
/// `Environment` "specifies the location where this object is hosted", and `ResourceType`
/// and `Identifier` are used "only when the value of the Type field is Resource or
/// AWS::Resource". So the near end of an edge contributes the service it is and the
/// environment it runs in, and the far end contributes either the remote service it is or
/// the remote resource it identifies — which is why the two sides map the same attributes
/// to different dimensions.
const NEAR_END_DIMENSIONS: [(&str, &str); 2] = [
    (ENVIRONMENT_ATTRIBUTE, "Environment"),
    (NAME_ATTRIBUTE, "Service"),
];

/// [`NEAR_END_DIMENSIONS`], for the far end of a call.
const FAR_END_DIMENSIONS: [(&str, &str); 3] = [
    (NAME_ATTRIBUTE, "RemoteService"),
    (RESOURCE_TYPE_ATTRIBUTE, "RemoteResourceType"),
    (IDENTIFIER_ATTRIBUTE, "RemoteResourceIdentifier"),
];

/// The dimension carrying the operation of the entity the metrics were collected for.
const OPERATION_DIMENSION: &str = "Operation";

/// The dimension carrying the operation the far end of a call serves.
const REMOTE_OPERATION_DIMENSION: &str = "RemoteOperation";

/// The dimensions that identify the standard metrics of ONE discovered entity, or of one
/// edge out of it: the entity's own identity and operation, plus the far end's identity and
/// operation when the reference describes a call.
///
/// A dimension whose attribute the stored identity does not carry is LEFT OUT rather than
/// reported empty — `Dimension.Value` is `length 1..1024`
/// (`smithy_query(shape="Dimension")`), so an absent fact is an absent dimension, and the
/// dimension set states exactly what the record states.
fn metric_dimensions(
    key_attributes: &HashMap<String, String>,
    operation_name: Option<&str>,
    far_end: Option<(&HashMap<String, String>, &str)>,
) -> Vec<Dimension> {
    let mut dimensions = Vec::new();
    for (attribute, dimension) in NEAR_END_DIMENSIONS {
        if let Some(value) = key_attributes.get(attribute) {
            dimensions.push(Dimension {
                name: dimension.to_string(),
                value: value.clone(),
            });
        }
    }
    if let Some(operation_name) = operation_name {
        dimensions.push(Dimension {
            name: OPERATION_DIMENSION.to_string(),
            value: operation_name.to_string(),
        });
    }
    if let Some((far_attributes, far_operation_name)) = far_end {
        for (attribute, dimension) in FAR_END_DIMENSIONS {
            if let Some(value) = far_attributes.get(attribute) {
                dimensions.push(Dimension {
                    name: dimension.to_string(),
                    value: value.clone(),
                });
            }
        }
        dimensions.push(Dimension {
            name: REMOTE_OPERATION_DIMENSION.to_string(),
            value: far_operation_name.to_string(),
        });
    }
    dimensions
}

/// The references to the standard metrics collected under `dimensions`: one per metric in
/// [`STANDARD_METRICS`], each identified by the same dimension set, which is what says
/// WHICH entity's or edge's metrics these are.
///
/// `AccountId` stays absent: the member is "Amazon Web Services account ID"
/// (`smithy_query(shape="MetricReference")`) and this store is one account's, so the metric
/// is in the caller's own account and there is no source account to name.
///
/// Single-homed here so the five topology projections cannot differ on the catalogue, and
/// so this reasoning is stated once.
fn standard_metric_references(dimensions: Vec<Dimension>) -> Vec<MetricReference> {
    STANDARD_METRICS
        .iter()
        .map(|(metric_name, metric_type)| MetricReference {
            namespace: APPLICATION_SIGNALS_NAMESPACE.to_string(),
            metric_type: (*metric_type).to_string(),
            dimensions: Some(dimensions.clone()),
            metric_name: (*metric_name).to_string(),
            account_id: None,
        })
        .collect()
}

/// The `GroupSource` of a group whose value was DERIVED from the entity's own attributes
/// under one of the definition's source keys.
///
/// `GroupSource` is "the source of the grouping attribute, such as `TAG`, `OTEL`, or
/// `DEFAULT`" (`smithy_query(shape="ServiceGroup")`), and the source keys "derive the
/// grouping attribute value from telemetry data, Amazon Web Services tags, or other
/// sources" (`smithy_query(shape="GroupingAttributeDefinition")`, member
/// `GroupingSourceKeys`). The attributes an entity is reported with here are the identity
/// the instrumented application's telemetry carries, not a tag this emulator holds, so a
/// value read out of them comes from telemetry.
const GROUP_SOURCE_TELEMETRY: &str = "OTEL";

/// The `GroupSource` of a group whose value came from the definition's
/// `DefaultGroupingValue`, which is "the default value to use for this grouping attribute
/// when no value can be derived from the source keys".
const GROUP_SOURCE_DEFAULT: &str = "DEFAULT";

/// The longest value a reported group may carry: `GroupValue` is modelled `length 1..255`
/// (`smithy_query(shape="GroupValue")`), while an attribute value is `length 1..1024`
/// (`smithy_query(shape="Attributes")`), so a long attribute value is not a value this
/// member can state.
const MAX_GROUP_VALUE: usize = 255;

/// What separates the grouping name from the value in a minted `GroupIdentifier`.
///
/// The identifier is "a unique identifier for this grouping attribute value"
/// (`smithy_query(shape="ServiceGroup")`), so it must distinguish one grouping's value from
/// another's: the pair of name and value IS that identity. `GroupingString` (the grouping
/// name) is `pattern ^[a-zA-Z0-9\s+\-=\._:/@]*$`, which admits no `#`, so the first `#`
/// always ends the name and the pair can be read back out of the identifier.
const GROUP_IDENTIFIER_SEPARATOR: char = '#';

/// The value one grouping attribute definition gives the entity `key_attributes`
/// identifies, with the `GroupSource` that value came from — or `None` when the definition
/// gives the entity no value at all.
///
/// A source key names an attribute to read the value from, and the keys are tried in the
/// order the definition lists them, so the first key the entity carries a USABLE value under
/// wins — a key whose attribute holds a value the modelled `GroupValue` cannot state is
/// passed over and the next key is tried. The comparison ignores ASCII case: a source key is
/// written in the casing of the telemetry field it names (`["business_unit", "team"]`) while
/// an entity's attributes are the capitalised `Attributes` names, and the same field named in
/// either casing is the same field. Which attribute ONE source key selects does not depend on
/// iteration order: the attribute whose name matches exactly is preferred, and among
/// attributes that match only case-insensitively the smallest name by byte order is taken, so
/// two reads of one entity report the same value ([`attribute_matching`]).
///
/// When no source key yields a value the definition's `DefaultGroupingValue` does, since it
/// "ensures all services have a grouping value even if the source data is missing". A
/// definition that declares no default and matches no attribute leaves the entity out of
/// that grouping entirely rather than reporting an empty value, which `GroupValue`
/// (`length 1..255`) cannot carry.
fn grouping_value(
    definition: &GroupingAttributeDefinition,
    key_attributes: &HashMap<String, String>,
) -> Option<(String, &'static str)> {
    let derived = definition
        .grouping_source_keys
        .iter()
        .flatten()
        .filter_map(|source_key| attribute_matching(key_attributes, source_key))
        .find(|value| reportable_group_value(value))
        .map(|value| (value.clone(), GROUP_SOURCE_TELEMETRY));
    derived.or_else(|| {
        definition
            .default_grouping_value
            .as_deref()
            .filter(|value| reportable_group_value(value))
            .map(|value| (value.to_string(), GROUP_SOURCE_DEFAULT))
    })
}

/// Whether a derived value is one the modelled `GroupValue` can state: non-empty and no
/// longer than [`MAX_GROUP_VALUE`].
fn reportable_group_value(value: &str) -> bool {
    !value.is_empty() && value.chars().count() <= MAX_GROUP_VALUE
}

/// The value `key_attributes` holds under the attribute `name` denotes, comparing attribute
/// names without ASCII case.
///
/// The choice is deterministic rather than the first entry a map walk happens to reach: the
/// exactly-named attribute is preferred, and failing that the smallest matching name by byte
/// order. `Attributes` is a map and its iteration order is unspecified, so an entity carrying
/// two names that differ only in case would otherwise be reported differently from one read
/// to the next.
fn attribute_matching<'a>(
    key_attributes: &'a HashMap<String, String>,
    name: &str,
) -> Option<&'a String> {
    if let Some(value) = key_attributes.get(name) {
        return Some(value);
    }
    key_attributes
        .iter()
        .filter(|(attribute, _)| attribute.eq_ignore_ascii_case(name))
        .min_by(|(left, _), (right, _)| left.cmp(right))
        .map(|(_, value)| value)
}

impl ComAmazonawsApplicationsignalsApplicationsignalsState {
    /// The groups the entity `key_attributes` identifies belongs to, as every projection
    /// that carries `ServiceGroups` reports them: one group per stored grouping attribute
    /// definition that gives this entity a value ([`grouping_value`]), in the order the
    /// configuration lists its definitions.
    ///
    /// `ServiceGroups` is "an array of service groups that this service belongs to, BASED ON
    /// THE CONFIGURED GROUPING ATTRIBUTES" (`smithy_query(shape="Service")`), so the stored
    /// grouping configuration — the one `PutGroupingConfiguration` wrote and
    /// `DeleteGroupingConfiguration` clears — is what the groups are computed from. With no
    /// configuration stored, or with none of its definitions giving this entity a value,
    /// the member stays absent: the entity belongs to no group, and the member is optional.
    ///
    /// Single-homed here so `GetService` and `ListServices` cannot disagree about which
    /// groups one entity belongs to.
    fn service_groups(&self, key_attributes: &HashMap<String, String>) -> Option<Vec<ServiceGroup>> {
        let configuration = self.grouping_configuration.as_ref()?;
        let groups: Vec<ServiceGroup> = configuration
            .grouping_attribute_definitions
            .iter()
            .filter_map(|definition| {
                let (group_value, group_source) = grouping_value(definition, key_attributes)?;
                Some(ServiceGroup {
                    group_identifier: format!(
                        "{}{GROUP_IDENTIFIER_SEPARATOR}{group_value}",
                        definition.grouping_name
                    ),
                    group_name: definition.grouping_name.clone(),
                    group_value,
                    group_source: group_source.to_string(),
                })
            })
            .collect();
        (!groups.is_empty()).then_some(groups)
    }

    /// Every stored SLO that monitors the entity `key_attributes` identifies, over a window
    /// ending at `window_end`.
    ///
    /// The requested attributes identify the entity, and an entity is identified by as few
    /// as one of its up to four `Attributes` entries, so an SLO matches when its own entity
    /// attributes hold every entry the request named ([`attributes_contain`] — the same
    /// match `ListServiceLevelObjectives` already applies to the same member).
    fn monitoring<'a>(
        &'a self,
        key_attributes: &'a HashMap<String, String>,
        window_start: &'a DateTime,
        window_end: &'a DateTime,
    ) -> impl Iterator<Item = &'a ServiceLevelObjective> + 'a {
        self.slos.values().filter(move |slo| {
            monitored_in_window(slo, window_start, window_end)
                && attributes_contain(entity_of(slo).key_attributes.as_ref(), key_attributes)
        })
    }

    /// The entity `key_attributes` identifies, as `GetService` reports it over the window.
    ///
    /// The reported key attributes are the fullest set this emulator holds for the entity —
    /// the request's own when it holds none, since the attributes ARE the entity's identity
    /// and the caller named it. So an entity no stored SLO monitors is reported by its
    /// identity alone rather than refused: Application Signals identifies an entity by its
    /// key attributes rather than by a stored resource id, and this read reports what it
    /// observed for that identity.
    ///
    /// `MetricReferences` reports the standard metrics Application Signals collects for the
    /// entity ([`standard_metric_references`]), dimensioned by the identity this read
    /// reports for it — but only for an entity the store is evidence of over the window: an
    /// identity nothing monitored was not DISCOVERED, so there is nothing collecting metrics
    /// for it and the list is empty, exactly as it is for a window in which nothing was
    /// observed.
    ///
    /// `ServiceGroups` reports the groups the configured grouping attributes put this entity
    /// in ([`Self::service_groups`]) — again only for an entity the store is evidence of,
    /// since an identity nothing monitored is no service to group.
    ///
    /// `AttributeMaps` and `LogGroupReferences` stay absent: they are platform and
    /// log-group facts the real service learns from the telemetry an instrumented
    /// application sends it, which this emulator never receives.
    pub fn observed_service(
        &self,
        key_attributes: &HashMap<String, String>,
        window_start: &DateTime,
        window_end: &DateTime,
    ) -> Service {
        let mut reported = key_attributes.clone();
        let mut discovered = false;
        for slo in self.monitoring(key_attributes, window_start, window_end) {
            discovered = true;
            if let Some(observed) = entity_of(slo).key_attributes {
                if observed.len() > reported.len() {
                    reported = observed;
                }
            }
        }
        let (metric_references, service_groups) = if discovered {
            (
                standard_metric_references(metric_dimensions(&reported, None, None)),
                self.service_groups(&reported),
            )
        } else {
            (Vec::new(), None)
        };
        Service {
            key_attributes: reported,
            attribute_maps: None,
            service_groups,
            metric_references,
            log_group_references: None,
        }
    }

    /// Every service entity this emulator has observed over the window, as `ListServices`
    /// reports it: one summary per distinct discovered service, in a deterministic order.
    ///
    /// The stored SLOs are this emulator's only evidence of a discovered service — it
    /// receives no instrumentation telemetry — so the discovered set is the set of entities
    /// the stored SLOs monitor, each over the window it is evidence for
    /// ([`monitored_in_window`], the same window rule the other topology reads apply). A
    /// dependency an SLO names is NOT in that set: the edge is evidence that the monitored
    /// entity calls it, not that Application Signals discovered and instrumented the callee,
    /// and `ListServiceDependencies` is where the edge is reported.
    ///
    /// An identity that is a STRICT SUBSET of another observed identity is not reported
    /// separately: as few as one of an entity's up to four `Attributes` entries identifies
    /// it ([`attributes_contain`]), so the subset names the same entity as the fuller set,
    /// and `GetService` answers for it with exactly that fuller set
    /// ([`Self::observed_service`]). Reporting both would list one service twice and hand
    /// the caller a summary whose `KeyAttributes` no `GetService` echoes back.
    ///
    /// `MetricReferences` reports the standard metrics collected for each reported identity
    /// ([`standard_metric_references`]) and `ServiceGroups` the groups the configured
    /// grouping attributes put it in ([`Self::service_groups`]) — every summary here IS a
    /// discovered service, so each carries both, exactly as [`Self::observed_service`]
    /// reports them for the same entity. `AttributeMaps` stays absent, as it does there: it
    /// is a platform and telemetry fact the real service learns from an instrumented
    /// application, which this emulator never receives.
    pub fn observed_services(
        &self,
        window_start: &DateTime,
        window_end: &DateTime,
    ) -> Vec<ServiceSummary> {
        let mut by_identity: BTreeMap<String, HashMap<String, String>> = BTreeMap::new();
        for slo in self
            .slos
            .values()
            .filter(|slo| monitored_in_window(slo, window_start, window_end))
        {
            let Some(attributes) = entity_of(slo).key_attributes else {
                continue;
            };
            // `ServiceSummary.KeyAttributes` is `@required` with `length 1..4`: an SLO whose
            // indicator names no entity at all is evidence of no service.
            if attributes.is_empty() {
                continue;
            }
            by_identity.insert(attribute_key(&attributes), attributes);
        }

        let identities: Vec<HashMap<String, String>> = by_identity.into_values().collect();
        identities
            .iter()
            .filter(|candidate| {
                !identities.iter().any(|other| {
                    other.len() > candidate.len() && attributes_contain(Some(other), candidate)
                })
            })
            .map(|key_attributes| ServiceSummary {
                key_attributes: key_attributes.clone(),
                attribute_maps: None,
                service_groups: self.service_groups(key_attributes),
                metric_references: standard_metric_references(metric_dimensions(
                    key_attributes,
                    None,
                    None,
                )),
            })
            .collect()
    }

    /// The operations of the entity `key_attributes` identifies, in name order: one entry
    /// per distinct operation the stored SLOs name on it.
    ///
    /// Each entry's `MetricReferences` reports the standard metrics collected for THAT
    /// operation ([`standard_metric_references`]): the dimensions are the entity's identity
    /// plus the operation, so two operations of one service carry different references. The
    /// identity used is the fullest one the stored SLOs hold for the entity, as
    /// [`Self::observed_service`] reports it, so the dimensions cannot disagree with the
    /// `GetService` answer for the same entity.
    pub fn observed_operations(
        &self,
        key_attributes: &HashMap<String, String>,
        window_start: &DateTime,
        window_end: &DateTime,
    ) -> Vec<ServiceOperation> {
        let mut by_name: BTreeMap<String, HashMap<String, String>> = BTreeMap::new();
        for slo in self.monitoring(key_attributes, window_start, window_end) {
            let entity = entity_of(slo);
            let Some(name) = entity.operation_name else {
                continue;
            };
            let observed = entity
                .key_attributes
                .unwrap_or_else(|| key_attributes.clone());
            let identity = by_name.entry(name).or_default();
            if observed.len() > identity.len() {
                *identity = observed;
            }
        }
        by_name
            .into_iter()
            .map(|(name, identity)| ServiceOperation {
                metric_references: standard_metric_references(metric_dimensions(
                    &identity,
                    Some(&name),
                    None,
                )),
                name,
            })
            .collect()
    }

    /// The dependencies of the entity `key_attributes` identifies: one entry per distinct
    /// edge the stored SLOs record from one of its operations to an operation of another
    /// entity.
    ///
    /// An SLO that names a dependency but no operation of its own records no edge: the
    /// modelled `ServiceDependency` states the edge as "the operation in this service that
    /// calls the dependency" (`OperationName`, `@required`), so without it there is no edge
    /// to report.
    ///
    /// Each edge's `MetricReferences` reports the standard metrics collected for THAT call
    /// ([`standard_metric_references`]): the dimensions are the calling entity's identity
    /// and operation plus the far end's identity and operation, so two edges out of one
    /// service carry different references.
    pub fn observed_dependencies(
        &self,
        key_attributes: &HashMap<String, String>,
        window_start: &DateTime,
        window_end: &DateTime,
    ) -> Vec<ServiceDependency> {
        let mut by_edge: BTreeMap<String, ServiceDependency> = BTreeMap::new();
        for slo in self.monitoring(key_attributes, window_start, window_end) {
            let entity = entity_of(slo);
            let (Some(operation_name), Some(dependency)) =
                (entity.operation_name, entity.dependency_config)
            else {
                continue;
            };
            let identity = entity
                .key_attributes
                .unwrap_or_else(|| key_attributes.clone());
            let edge = format!(
                "{operation_name}\u{1d}{}\u{1d}{}",
                attribute_key(&dependency.dependency_key_attributes),
                dependency.dependency_operation_name,
            );
            let metric_references = standard_metric_references(metric_dimensions(
                &identity,
                Some(&operation_name),
                Some((
                    &dependency.dependency_key_attributes,
                    &dependency.dependency_operation_name,
                )),
            ));
            by_edge.entry(edge).or_insert_with(|| ServiceDependency {
                operation_name,
                dependency_key_attributes: dependency.dependency_key_attributes,
                dependency_operation_name: dependency.dependency_operation_name,
                metric_references,
            });
        }
        by_edge.into_values().collect()
    }

    /// The dependents of the entity `key_attributes` identifies: the inverse of
    /// [`Self::observed_dependencies`], one entry per distinct edge the stored SLOs record
    /// from another entity INTO one of this entity's operations.
    ///
    /// `OperationName` is the operation of the named entity that was invoked (the
    /// dependency operation the edge points at), and `DependentOperationName` the operation
    /// of the dependent that invoked it — which an SLO that names no operation of its own
    /// leaves absent, as the model allows.
    ///
    /// `MetricReferences` reports the standard metrics collected for THAT call
    /// ([`standard_metric_references`]). The metrics of a call are the INVOKING entity's, so
    /// the dimensions are the dependent's identity and operation on the near end and the
    /// invoked entity and operation on the far end — the same dimension set
    /// [`Self::observed_dependencies`] reports for the same edge seen from the caller, which
    /// is what keeps the two directions from disagreeing about one edge.
    pub fn observed_dependents(
        &self,
        key_attributes: &HashMap<String, String>,
        window_start: &DateTime,
        window_end: &DateTime,
    ) -> Vec<ServiceDependent> {
        let mut by_edge: BTreeMap<String, ServiceDependent> = BTreeMap::new();
        for slo in self
            .slos
            .values()
            .filter(|slo| monitored_in_window(slo, window_start, window_end))
        {
            let entity = entity_of(slo);
            let Some(dependency) = entity.dependency_config else {
                continue;
            };
            if !attributes_contain(Some(&dependency.dependency_key_attributes), key_attributes) {
                continue;
            }
            let Some(dependent_key_attributes) = entity.key_attributes else {
                continue;
            };
            let edge = format!(
                "{}\u{1d}{}\u{1d}{}",
                attribute_key(&dependent_key_attributes),
                entity.operation_name.clone().unwrap_or_default(),
                dependency.dependency_operation_name,
            );
            let metric_references = standard_metric_references(metric_dimensions(
                &dependent_key_attributes,
                entity.operation_name.as_deref(),
                Some((
                    &dependency.dependency_key_attributes,
                    &dependency.dependency_operation_name,
                )),
            ));
            by_edge.entry(edge).or_insert_with(|| ServiceDependent {
                operation_name: Some(dependency.dependency_operation_name),
                dependent_key_attributes,
                dependent_operation_name: entity.operation_name,
                metric_references,
            });
        }
        by_edge.into_values().collect()
    }

    /// The last instant inside the window at which this emulator recorded a change to the
    /// service `identity` names, or `None` when it recorded none there.
    ///
    /// A stored SLO is a configuration of what Application Signals watches on its entity, so
    /// the instants that record a change to a service are the instants its SLOs were written
    /// at: each SLO's `CreatedTime` and its `LastUpdatedTime`, read off the stored record
    /// rather than re-derived, so the instant reported here is one `GetServiceLevelObjective`
    /// reports for the same SLO. Only an instant INSIDE the window counts: the window is the
    /// period the caller asks about, so a change before it is not news in it and a change
    /// after it has not happened yet. The bounds are compared the way every window in this
    /// module is ([`instant_nanos`]), so this read and `ListEntityEvents` cannot disagree
    /// about whether a change falls inside one.
    fn last_changed_in_window(
        &self,
        identity: &HashMap<String, String>,
        window_start: &DateTime,
        window_end: &DateTime,
    ) -> Option<DateTime> {
        let (window_start, window_end) = (instant_nanos(window_start), instant_nanos(window_end));
        self.slos
            .values()
            .filter(|slo| names_same_service(entity_of(slo).key_attributes.as_ref(), identity))
            .flat_map(|slo| [slo.created_time, slo.last_updated_time])
            .filter(|at| {
                let at = instant_nanos(at);
                window_start <= at && at <= window_end
            })
            .max_by_key(instant_nanos)
    }

    /// The state of each service this emulator observed over the window, as
    /// `ListServiceStates` reports it.
    ///
    /// The services considered are the DISCOVERED ones ([`Self::observed_services`]), so this
    /// read cannot report a service `ListServices` does not, and `filters` then narrows that
    /// set ([`passes_attribute_filter`]). Each reported entry carries the one most recent
    /// change recorded for its service inside the window
    /// ([`Self::last_changed_in_window`]) — `ServiceState.LatestChangeEvents` is `@required`
    /// with `length 1..1`, so a service with no recorded change inside the window has no
    /// state to report rather than one with an empty event list.
    ///
    /// `AttributeFilters` echoes the filters that were applied, which is what the member
    /// states it carries; a request that named none has none applied, and holding no filters
    /// and holding an empty set are the same state.
    ///
    /// `ChangeEvent` states WHAT changed, WHEN and WHERE: the entity is the identity this
    /// read reports for the service, the instant is the recorded one, and the account and
    /// region are this store's account and the caller's own region, exactly as a minted ARN
    /// renders them. `UserName` and `EventName` stay absent: the store records that a
    /// configuration changed and when, not who asked for it or what the service would call
    /// the change.
    pub fn observed_service_states(
        &self,
        window_start: &DateTime,
        window_end: &DateTime,
        filters: &[AttributeFilter],
        region: &str,
    ) -> Vec<ServiceState> {
        let applied = (!filters.is_empty()).then(|| filters.to_vec());
        self.observed_services(window_start, window_end)
            .into_iter()
            .filter(|summary| {
                filters
                    .iter()
                    .all(|filter| passes_attribute_filter(&summary.key_attributes, filter))
            })
            .filter_map(|summary| {
                let changed_at =
                    self.last_changed_in_window(&summary.key_attributes, window_start, window_end)?;
                Some(ServiceState {
                    attribute_filters: applied.clone(),
                    latest_change_events: vec![change_event(
                        changed_at,
                        &summary.key_attributes,
                        region,
                        None,
                    )],
                    service: summary.key_attributes,
                })
            })
            .collect()
    }
}

/// The `ChangeEventType` of a change to what Application Signals is configured to watch on a
/// service (`smithy_query(shape="ChangeEventType")` -> `["DEPLOYMENT", "CONFIGURATION"]`).
///
/// Every change this emulator records is of this kind: a deployment is an event of the
/// customer's own release pipeline, which the real service learns about from the telemetry
/// and the trail of an instrumented account, and this emulator receives neither.
pub const CONFIGURATION_CHANGE_EVENT: &str = "CONFIGURATION";

/// `ChangeEvent.EventId` for an event this emulator did not read off a CloudTrail record,
/// which is the value the member documents for one: "For CloudTrail-based events, this is the
/// CloudTrail event id. For other events, this will be `Unknown`"
/// (`smithy_query(query="doc", shape="ChangeEvent", member="EventId")`).
pub const UNRECORDED_EVENT_ID: &str = "Unknown";

/// Whether two key-attribute maps name the same service.
///
/// As few as one of an entity's up to four `Attributes` entries identifies it
/// ([`attributes_contain`]), so one map names the same service as the other when either is a
/// subset of it — which is the same rule that lets
/// [`ComAmazonawsApplicationsignalsApplicationsignalsState::observed_services`] fold a subset
/// identity into the fuller one rather than reporting two services. A map that names no
/// attributes at all names no service.
fn names_same_service(left: Option<&HashMap<String, String>>, right: &HashMap<String, String>) -> bool {
    let Some(left) = left else {
        return false;
    };
    if left.is_empty() || right.is_empty() {
        return false;
    }
    attributes_contain(Some(left), right) || attributes_contain(Some(right), left)
}

/// Whether a service's attributes pass one `AttributeFilter`.
///
/// The filter names an attribute and the values it accepts for it, and "services that have
/// any of these values for the attribute will be included in the results"
/// (`smithy_query(shape="AttributeFilter")`), so one filter passes when the service carries
/// that attribute with one of those values — and a request's filters narrow together, each
/// one being a further condition on the same service.
///
/// The attributes a service HAS here are its key attributes: the platform, business and other
/// descriptive attributes a filter may also name (`AttributeFilterName`: "such as `Platform`,
/// `Environment`, or `BusinessUnit`") reach the real service in the telemetry an instrumented
/// application sends it, which this emulator never receives — so a filter on an attribute the
/// service's identity does not carry selects nothing rather than everything.
fn passes_attribute_filter(
    key_attributes: &HashMap<String, String>,
    filter: &AttributeFilter,
) -> bool {
    key_attributes
        .get(&filter.attribute_filter_name)
        .is_some_and(|value| {
            filter
                .attribute_filter_values
                .iter()
                .any(|accepted| accepted == value)
        })
}

// =============================================================================
// The audit findings the stored SLOs are evidence of
// =============================================================================

/// The auditor this emulator holds the input of: the one that assesses the SERVICE LEVEL
/// OBJECTIVES of the audited entity.
///
/// The name is the one the API configures that auditor by, and the only one of the
/// configurable auditors whose input is a resource this service STORES
/// (`smithy_query(query="doc", shape="ListAuditFindingsInput", member="Auditors")`: "`slo` -
/// SloAuditor: Identifies SLO violations and detects breached thresholds during the
/// Assessment phase"). Every other auditor that doc names reads telemetry — the request,
/// error and latency metrics, the traces, the EMF logs and the change events an instrumented
/// application sends Application Signals — which this emulator never receives, so each of
/// them produces no result and an `Auditors` filter naming only those reports nothing.
pub const SLO_AUDITOR: &str = "slo";

/// The `Severity` of an auditor result that reports NO issue
/// (`smithy_query(shape="Severity")` -> `["CRITICAL", "HIGH", "MEDIUM", "LOW", "NONE"]`).
const SEVERITY_NONE: &str = "NONE";

/// The `DetailLevel` that asks for the fullest per-finding context
/// (`smithy_query(shape="DetailLevel")` -> `["BRIEF", "DETAILED"]`).
pub const DETAIL_LEVEL_DETAILED: &str = "DETAILED";

/// The budget status of an objective whose attainment has no evaluated datapoint
/// (`smithy_query(shape="ServiceLevelObjectiveBudgetStatus")` -> `["OK", "WARNING",
/// "BREACHED", "INSUFFICIENT_DATA"]`). It is what the SLO auditor's assessment rests on
/// here: an objective whose attainment cannot be evaluated has no breached threshold to
/// report, which is why the result it carries is a [`SEVERITY_NONE`] one.
const BUDGET_STATUS_INSUFFICIENT_DATA: &str = "INSUFFICIENT_DATA";

/// The `AuditorResult.Data` entries the SLO auditor reports: "additional data"
/// (`smithy_query(shape="AuditorResult")`), each read off the assessed record, named after
/// the member of that record it carries.
const AUDITOR_DATA_SLO_NAME: &str = "SloName";
/// [`AUDITOR_DATA_SLO_NAME`], for the assessed objective's ARN.
const AUDITOR_DATA_SLO_ARN: &str = "SloArn";
/// [`AUDITOR_DATA_SLO_NAME`], for the attainment goal it is assessed against.
const AUDITOR_DATA_ATTAINMENT_GOAL: &str = "AttainmentGoal";
/// [`AUDITOR_DATA_SLO_NAME`], for the budget status the assessment reached.
const AUDITOR_DATA_BUDGET_STATUS: &str = "BudgetStatus";

/// The kind of audit target a finding was produced for, as `AuditFinding.Type` reports it.
///
/// The tokens are the ones the target's own type member enumerates — "The type of entity
/// being audited, such as `service`, `SLO`, `service_operation`, or `canary`"
/// (`smithy_query(query="doc", shape="AuditTarget", member="Type")`) — each spelled exactly
/// as that doc spells it, so a client comparing against the documented set matches. The kind
/// is read off the modelled `AuditTargetEntity` UNION the request carried, not off the
/// target's free-form `Type` string, so the finding states the kind of entity that was
/// actually resolved.
fn audit_target_kind(target: &AuditTargetEntity) -> &'static str {
    match target {
        AuditTargetEntity::Service(_) => "service",
        AuditTargetEntity::Slo(_) => "SLO",
        AuditTargetEntity::ServiceOperation(_) => "service_operation",
        AuditTargetEntity::Canary(_) => "canary",
    }
}

/// The key attributes a `ServiceEntity` names, or `None` when it names no entity of this
/// store.
///
/// The shape's members ARE `Attributes` entries: `Type` "the type of the service entity",
/// `Name` "the name of the service" and `Environment` the location it is hosted in
/// (`smithy_query(shape="ServiceEntity")`), so a target's service entity becomes the same
/// kind of identity map `GetService` and `ListServiceOperations` take, and an absent member
/// simply names one fewer attribute ([`attributes_contain`] matches on a subset).
///
/// `None` for an entity that names NO attribute at all, since an empty map would match every
/// stored identity rather than one, and `None` for an entity in another account: the member
/// is "the Amazon Web Services account ID where the service is located. Provide this value
/// only for cross-account access", and this store holds one account's records (see the store
/// doc), so another account's entity is one it holds nothing for.
fn audited_entity_attributes(entity: &ServiceEntity) -> Option<HashMap<String, String>> {
    if entity
        .aws_account_id
        .as_deref()
        .is_some_and(|account| account != ::engine::emulator_account_id())
    {
        return None;
    }
    let mut attributes = HashMap::new();
    for (attribute, value) in [
        (TYPE_ATTRIBUTE, entity.r#type.as_deref()),
        (NAME_ATTRIBUTE, entity.name.as_deref()),
        (ENVIRONMENT_ATTRIBUTE, entity.environment.as_deref()),
    ] {
        if let Some(value) = value {
            attributes.insert(attribute.to_string(), value.to_string());
        }
    }
    (!attributes.is_empty()).then_some(attributes)
}

/// The SLO auditor's result for one assessed objective.
///
/// The auditor "identifies SLO violations and detects breached thresholds" (see
/// [`SLO_AUDITOR`]), and it is run here over the objectives the store holds. Its verdict is
/// the one the stored state supports: the attainment of an objective has no evaluated
/// datapoint in this emulator, so the assessment reaches
/// [`BUDGET_STATUS_INSUFFICIENT_DATA`], no threshold is observed breached, and the result's
/// severity — "the severity level of this audit finding, indicating the importance and
/// potential impact of the issue" (`smithy_query(shape="AuditorResult")`) — is the modelled
/// value for no issue, [`SEVERITY_NONE`]. Claiming any other severity would report a
/// violation nothing measured.
///
/// Every value in `Data` is read off the assessed record, so two objectives' results differ
/// exactly as their stored configuration does. The attainment goal is the one the objective
/// is evaluated against, which is the stored goal with the defaults the member documents
/// filled in ([`goal_with_defaults`]), so the result cannot disagree with what
/// `GetServiceLevelObjective` reports for the same objective.
fn slo_auditor_result(slo: &ServiceLevelObjective) -> AuditorResult {
    let mut data = HashMap::from([
        (AUDITOR_DATA_SLO_NAME.to_string(), slo.name.clone()),
        (AUDITOR_DATA_SLO_ARN.to_string(), slo.arn.clone()),
        (
            AUDITOR_DATA_BUDGET_STATUS.to_string(),
            BUDGET_STATUS_INSUFFICIENT_DATA.to_string(),
        ),
    ]);
    if let Some(attainment_goal) = goal_with_defaults(Some(&slo.goal)).attainment_goal {
        data.insert(
            AUDITOR_DATA_ATTAINMENT_GOAL.to_string(),
            attainment_goal.to_string(),
        );
    }
    AuditorResult {
        auditor: Some(SLO_AUDITOR.to_string()),
        description: Some(format!(
            "No attainment datapoint is recorded for service level objective {} in the \
             requested period, so no breached threshold is reported.",
            slo.name,
        )),
        data: Some(data),
        severity: Some(SEVERITY_NONE.to_string()),
    }
}

/// The metric graph a DETAILED finding carries for one assessed objective: "metric data
/// queries and time range information that provides context for audit findings through
/// relevant performance metrics" (`smithy_query(shape="MetricGraph")`).
///
/// The queries are the objective's OWN stored ones — the `MetricDataQueries` of a
/// period-based indicator, or the total and monitored request-count queries of a
/// request-based one — so the graph names the metrics the assessment is evaluated from
/// rather than a catalogue this read made up, and the range is the window the request asked
/// to be audited over.
fn assessed_metric_graph(
    slo: &ServiceLevelObjective,
    window_start: &DateTime,
    window_end: &DateTime,
) -> MetricGraph {
    let mut queries: Vec<MetricDataQuery> = Vec::new();
    if let Some(sli) = slo.sli.as_ref() {
        queries.extend(sli.sli_metric.metric_data_queries.iter().cloned());
    }
    if let Some(request_based_sli) = slo.request_based_sli.as_ref() {
        let metric = &request_based_sli.request_based_sli_metric;
        queries.extend(metric.total_request_count_metric.iter().cloned());
        let monitored = match &metric.monitored_request_count_metric {
            MonitoredRequestCountMetricDataQueries::GoodCountMetric(queries) => queries,
            MonitoredRequestCountMetricDataQueries::BadCountMetric(queries) => queries,
        };
        queries.extend(monitored.iter().cloned());
    }
    MetricGraph {
        // An absent fact is an absent member: an objective whose indicator holds no query
        // contributes no query rather than an empty list.
        metric_data_queries: (!queries.is_empty()).then_some(queries),
        start_time: Some(*window_start),
        end_time: Some(*window_end),
    }
}

/// `finding` with only the results of the named auditors left, or `None` once nothing is
/// left.
///
/// The filter is the one the member documents: "a list of auditor names to filter the
/// findings by. Only findings generated by the specified auditors will be returned" (see
/// [`SLO_AUDITOR`]) — so a finding none of whose results came from a named auditor was not
/// generated by one of them and is not reported. An ABSENT list names no filter and keeps
/// every finding; a list that is present but empty names no auditor, so it keeps none.
fn retaining_auditors(
    mut finding: AuditFinding,
    auditors: Option<&[String]>,
) -> Option<AuditFinding> {
    let Some(auditors) = auditors else {
        return Some(finding);
    };
    let results: Vec<AuditorResult> = finding
        .auditor_results
        .take()
        .unwrap_or_default()
        .into_iter()
        .filter(|result| {
            result
                .auditor
                .as_deref()
                .is_some_and(|auditor| auditors.iter().any(|named| named == auditor))
        })
        .collect();
    if results.is_empty() {
        return None;
    }
    finding.auditor_results = Some(results);
    Some(finding)
}

impl ComAmazonawsApplicationsignalsApplicationsignalsState {
    /// The audit findings the request's targets resolve to over the window, filtered by the
    /// auditors it named and carrying the per-finding context its detail level asked for.
    ///
    /// Every finding is an assessment of ONE stored service level objective: the audited
    /// entity is the entity that objective monitors (`AuditFinding.KeyAttributes` is
    /// `@required` with `length 1..4`, so an objective whose indicator identifies no entity
    /// is reported for no target), and the result it carries is the SLO auditor's
    /// ([`slo_auditor_result`]). Which objectives a target resolves to is [`Self::audited`];
    /// a target this store holds nothing for — an objective that was never created, an
    /// entity nothing monitors, a kind of resource this service does not hold — contributes
    /// no finding, exactly as the topology reads report an entity they observed nothing for
    /// as absent.
    ///
    /// An objective named by SEVERAL targets is assessed ONCE: the findings are the
    /// assessments of a set of objectives, not one per target, and the kind reported for a
    /// repeated one is that of the first target in the request that resolved to it. The
    /// order is the objective name's — the order `slos` is keyed in — which is what makes a
    /// page cursor over this projection a stable boundary.
    pub fn audited_findings(
        &self,
        targets: &[AuditTarget],
        auditors: Option<&[String]>,
        detailed: bool,
        window_start: &DateTime,
        window_end: &DateTime,
    ) -> Vec<AuditFinding> {
        let mut assessed: BTreeMap<String, &'static str> = BTreeMap::new();
        for target in targets {
            let kind = audit_target_kind(&target.data);
            for name in self.audited(&target.data, window_start, window_end) {
                assessed.entry(name).or_insert(kind);
            }
        }
        assessed
            .into_iter()
            .filter_map(|(name, kind)| {
                let slo = self.slos.get(&name)?;
                let entity = entity_of(slo);
                let key_attributes = entity
                    .key_attributes
                    .filter(|attributes| !attributes.is_empty())?;
                Some(AuditFinding {
                    key_attributes,
                    auditor_results: Some(vec![slo_auditor_result(slo)]),
                    operation: entity.operation_name,
                    // Only the SLO auditor produces a result here, and the context it
                    // reports is the metric graph of the objective it assessed. A
                    // `DependencyGraph` is the trace and critical-path auditors' artefact —
                    // built from the distributed traces this emulator never receives — so
                    // no finding here carries one.
                    metric_graph: detailed
                        .then(|| assessed_metric_graph(slo, window_start, window_end)),
                    dependency_graph: None,
                    r#type: Some(kind.to_string()),
                })
            })
            .filter_map(|finding| retaining_auditors(finding, auditors))
            .collect()
    }

    /// The names of the stored objectives one audit target resolves to over the window.
    ///
    /// Each kind of target names its entity the way the API identifies that kind:
    ///
    /// * an objective by name or ARN, resolved as every operation taking a
    ///   `ServiceLevelObjectiveId` resolves one ([`Self::resolve`]). A target that states
    ///   both must state them of ONE objective: two identities naming two records name no
    ///   single objective, and an identity that resolves to nothing resolves the whole
    ///   target to nothing.
    /// * a service by its key attributes ([`audited_entity_attributes`]), resolving to the
    ///   objectives that monitor it ([`Self::monitoring`] — the same subset match and the
    ///   same window rule the topology reads apply).
    /// * a service operation by its service AND its operation, since an operation is
    ///   identified by the service it belongs to; its optional metric type narrows further
    ///   to the objectives whose indicator measures that type.
    /// * a canary names a Synthetics resource, which this service does not hold, so it
    ///   resolves to nothing.
    fn audited(
        &self,
        target: &AuditTargetEntity,
        window_start: &DateTime,
        window_end: &DateTime,
    ) -> Vec<String> {
        match target {
            AuditTargetEntity::Slo(entity) => {
                let mut resolved: Option<&ServiceLevelObjective> = None;
                for id in [entity.slo_name.as_deref(), entity.slo_arn.as_deref()]
                    .into_iter()
                    .flatten()
                {
                    let Some(slo) = self.resolve(id) else {
                        return Vec::new();
                    };
                    if resolved.is_some_and(|already| already.name != slo.name) {
                        return Vec::new();
                    }
                    resolved = Some(slo);
                }
                resolved
                    .filter(|slo| monitored_in_window(slo, window_start, window_end))
                    .map(|slo| vec![slo.name.clone()])
                    .unwrap_or_default()
            }
            AuditTargetEntity::Service(entity) => {
                let Some(attributes) = audited_entity_attributes(entity) else {
                    return Vec::new();
                };
                self.monitoring(&attributes, window_start, window_end)
                    .map(|slo| slo.name.clone())
                    .collect()
            }
            AuditTargetEntity::ServiceOperation(entity) => {
                let (Some(service), Some(operation)) =
                    (entity.service.as_ref(), entity.operation.as_deref())
                else {
                    return Vec::new();
                };
                let Some(attributes) = audited_entity_attributes(service) else {
                    return Vec::new();
                };
                self.monitoring(&attributes, window_start, window_end)
                    .filter(|slo| {
                        let assessed = entity_of(slo);
                        assessed.operation_name.as_deref() == Some(operation)
                            && entity.metric_type.as_deref().is_none_or(|metric_type| {
                                assessed.metric_type.as_deref() == Some(metric_type)
                            })
                    })
                    .map(|slo| slo.name.clone())
                    .collect()
            }
            AuditTargetEntity::Canary(_) => Vec::new(),
        }
    }
}

// =============================================================================
// Dynamic instrumentation configurations
// =============================================================================

/// The resource-type segment of an instrumentation configuration ARN. The modelled
/// `InstrumentationConfigurationArn` doc spells the whole format:
/// `arn:${Partition}:application-signals:${Region}:${Account}:instrumentationConfig/${service}/${environment}/${signalType}/${locationHash}`.
pub const INSTRUMENTATION_CONFIG_ARN_RESOURCE: &str = "instrumentationConfig";

/// The `InstrumentationType` whose configurations are temporary
/// (`smithy_query(shape="InstrumentationType")`: "Temporary instrumentation that expires
/// automatically (default 24 hours)").
pub const INSTRUMENTATION_TYPE_BREAKPOINT: &str = "BREAKPOINT";

/// How many digest bytes a `LocationHash` renders: the member is modelled `length 16..16`
/// and the ARN pattern spells that suffix `[0-9a-f]{16}`, so 16 hex characters \u2014 8 bytes.
const LOCATION_HASH_BYTES: usize = 8;

/// How long a BREAKPOINT configuration lives when the request names no `ExpiresAt`.
/// `CreateInstrumentationConfiguration`'s doc: "For BREAKPOINT type configurations, they
/// expire after 24 hours unless a shorter expiration is provided", and the `ExpiresAt`
/// member's: "For BREAKPOINT: optional, defaults to 24 hours".
pub const BREAKPOINT_DEFAULT_EXPIRY: Duration = Duration::from_secs(24 * 60 * 60);

/// The shortest BREAKPOINT expiry a request may name. `ExpiresAt`'s doc: "must be between
/// 5 min and 24 hours".
pub const BREAKPOINT_MIN_EXPIRY: Duration = Duration::from_secs(5 * 60);

/// The longest BREAKPOINT expiry a request may name, from the same sentence.
pub const BREAKPOINT_MAX_EXPIRY: Duration = BREAKPOINT_DEFAULT_EXPIRY;

/// The `SyncInterval` a list reports. `InstrumentationConfigurationsPage.SyncInterval`'s
/// doc: "The suggested number of seconds to wait before the next sync request. This is at
/// least 60 seconds to prevent excessive polling", which is also the member's modelled
/// `range min 60`. This emulator holds one process's state and has no load to spread, so it
/// suggests exactly that floor.
pub const SYNC_INTERVAL_SECONDS: i32 = 60;

/// The page `ListInstrumentationConfigurations` returns when the caller names no
/// `MaxResults`: the member's modelled `@default` of 50 ("The default is 50 and the maximum
/// is 100").
pub const DEFAULT_INSTRUMENTATION_CONFIGURATION_PAGE: i32 = 50;

/// The [`NEXT_TOKEN_TAG`] of `ListInstrumentationConfigurations`, which every token it
/// issues carries together with a fingerprint of the scope that issued it (see
/// [`instrumentation_token_tag`]).
pub const INSTRUMENTATION_CONFIGURATIONS_TOKEN_TAG: &str = "instrumentation-configurations/1/";

/// The most instrumentation configurations this service holds, after which a create is
/// refused with the modelled `ServiceQuotaExceededException`.
///
/// AWS publishes no figure for this quota, so the bound is this emulator's own; what is not
/// chosen is that there IS one. `CreateInstrumentationConfiguration` is the only writer, and
/// only an explicit delete or a BREAKPOINT expiry removes a record, so without a cap the
/// store's size would be a property of the caller rather than of the service \u2014 and the
/// operation models `ServiceQuotaExceededException` for precisely that refusal.
pub const MAX_INSTRUMENTATION_CONFIGURATIONS: usize = 1_000;

/// The page `GetInstrumentationConfigurationStatus` returns when the caller names no
/// `MaxResults`: the member's modelled `@default` of 60 ("The maximum number of status events
/// to return in one call. The default is 60").
pub const DEFAULT_STATUS_EVENT_PAGE: i32 = 60;

/// The window a status read covers when the request names neither `StartTime` nor `EndTime`.
/// `GetInstrumentationConfigurationStatus`'s doc: "If no status or time window is provided,
/// the operation defaults to ACTIVE events from the last hour", and the members' own: "When
/// both are omitted, the time range defaults to the last hour".
pub const STATUS_EVENT_DEFAULT_WINDOW: Duration = Duration::from_secs(60 * 60);

/// The `InstrumentationConfigurationStatus` a status read reports when the request names
/// none. `GetInstrumentationConfigurationStatus.Status`'s doc: "If omitted, only ACTIVE
/// status events are returned", and the response member's: "If not specified in the request,
/// this is ACTIVE" (`smithy_query(query="shape",shape="InstrumentationConfigurationStatus")`
/// -> `["READY", "ERROR", "ACTIVE", "DISABLED"]`).
pub const DEFAULT_STATUS_EVENT_STATUS: &str = "ACTIVE";

/// The `UnprocessedStatusEventFailureReason` a reported status event earns when it names an
/// identity this service holds no configuration for
/// (`smithy_query(query="shape",shape="UnprocessedStatusEventFailureReason")` ->
/// `["THROTTLED", "INTERNAL_ERROR", "VALIDATION_ERROR"]`): the report is about a
/// configuration that does not exist, which is a property of the request.
pub const STATUS_EVENT_FAILED_VALIDATION: &str = "VALIDATION_ERROR";

/// The [`NEXT_TOKEN_TAG`] of `GetInstrumentationConfigurationStatus`, which every token it
/// issues carries together with a fingerprint of the query that issued it (see
/// [`instrumentation_status_token_tag`]).
pub const INSTRUMENTATION_STATUS_EVENTS_TOKEN_TAG: &str = "instrumentation-status-events/1/";

/// The most status events this service keeps for one instrumentation configuration, past
/// which the OLDEST event it holds is dropped.
///
/// AWS publishes no retention for reported status events, so the bound is this emulator's
/// own; what is not chosen is that there IS one.
/// `ReportInstrumentationConfigurationStatus` is the only writer and the caller chooses how
/// often it writes ("Report ACTIVE periodically (for example, every minute) while
/// instrumentation is running"), so without a cap the store's size would be a property of
/// the caller rather than of the service. Dropping the oldest event keeps the recent history
/// a read asks for: the cap holds more than ten pages at the modelled `MaxResults` maximum of
/// 100, and far more than the hour a read covers by default at the cadence the operation
/// suggests.
pub const MAX_STATUS_EVENTS_PER_CONFIGURATION: usize = 1_000;

/// How long a scope whose set has EMPTIED keeps its last-change instant before the record
/// itself is dropped.
///
/// A list tells every caller to come back after [`SYNC_INTERVAL_SECONDS`], but that member
/// is a FLOOR ("at least 60 seconds"), not a cadence: a conforming caller may poll a good
/// deal later. The horizon is therefore a generous multiple of the advertised interval, so a
/// caller polling slower than the floor still learns that its configuration lapsed, while
/// the record does not hold quota indefinitely after it stopped being news. Once the horizon
/// passes, a later sync is answered with `Changed: false` over an empty set, which is what a
/// scope this service holds nothing for means. Keeping the record for ever instead would let
/// a caller grow the store with scope keys of its own choosing \u2014 the count is also capped
/// (see
/// [`ComAmazonawsApplicationsignalsApplicationsignalsState::instrumentation_entry_count`]),
/// and this horizon is what keeps a retained record from holding that quota for ever.
pub const EMPTY_SCOPE_RETENTION: Duration =
    Duration::from_secs(EMPTY_SCOPE_RETENTION_INTERVALS * SYNC_INTERVAL_SECONDS as u64);

/// How many advertised sync intervals [`EMPTY_SCOPE_RETENTION`] spans.
const EMPTY_SCOPE_RETENTION_INTERVALS: u64 = 10;

/// The set of instrumentation configurations one list call reports over: the service, the
/// environment it runs in and the instrumentation type, which is every selector the
/// instrumentation operations take beside the location.
///
/// `InstrumentationType` is part of the scope rather than a filter over one store because
/// the model says so (`ListInstrumentationConfigurations.InstrumentationType`: "Required to
/// determine which backing store to query").
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct InstrumentationScope {
    pub instrumentation_type: String,
    pub service: String,
    pub environment: String,
}

impl InstrumentationScope {
    /// The scope a request's `InstrumentationType`, `Service` and `Environment` name.
    pub fn new(instrumentation_type: &str, service: &str, environment: &str) -> Self {
        Self {
            instrumentation_type: instrumentation_type.to_string(),
            service: service.to_string(),
            environment: environment.to_string(),
        }
    }
}

/// One [`InstrumentationScope`]'s configurations, plus when the set last changed.
#[derive(Debug, Default)]
pub struct InstrumentationConfigurationSet {
    /// The scope's configurations, keyed by [`instrumentation_key`].
    pub configurations: BTreeMap<String, InstrumentationConfiguration>,
    /// The instant this set last gained, lost or expired a configuration, or `None` while
    /// it has never held one.
    ///
    /// This is what `ListInstrumentationConfigurations` answers `Changed` from: the member
    /// is documented as "whether there are configuration changes since the provided
    /// SyncedAt timestamp", so the service has to remember when the set last changed rather
    /// than deduce it from the records it still holds \u2014 a deletion leaves no record to read
    /// a timestamp off.
    pub last_changed_at: Option<DateTime>,
    /// The status events reported for each configuration of this scope, keyed by the same
    /// [`instrumentation_key`] `configurations` is keyed by — so a configuration and the
    /// history reported against it are one record's two halves, and the history goes when
    /// the configuration goes.
    pub status_history: BTreeMap<String, InstrumentationStatusHistory>,
}

impl InstrumentationConfigurationSet {
    /// Record that this set changed at `at`, keeping the LATEST change instant: a change is
    /// news to every caller that synced before it, and an older change must not hide a
    /// newer one.
    fn record_change(&mut self, at: DateTime) {
        let keep = self
            .last_changed_at
            .is_none_or(|previous| previous.as_secs_f64() < at.as_secs_f64());
        if keep {
            self.last_changed_at = Some(at);
        }
    }
}

/// A status event's key within one configuration's history: the instant it was reported for,
/// then the status reported at that instant.
///
/// Time first, so the ordered map is a chronology a read walks backwards ("sorted with the
/// most recent first") and the oldest event is the one the cap drops. The status is part of
/// the key because one instant can carry one event per status, and because a repeated report
/// of the SAME status at the SAME instant is the same event: the operation is `@idempotent`,
/// so re-reporting one must not add a second event.
type StatusEventKey = (i64, u32, String);

/// The status events reported against ONE instrumentation configuration.
///
/// Bounded at [`MAX_STATUS_EVENTS_PER_CONFIGURATION`] events by [`Self::record`], the only
/// writer, so a caller reporting for ever does not grow the store for ever.
#[derive(Debug, Default)]
pub struct InstrumentationStatusHistory {
    /// Each event's optional `ErrorCause`, keyed by [`StatusEventKey`].
    events: BTreeMap<StatusEventKey, Option<String>>,
}

impl InstrumentationStatusHistory {
    /// Record that this configuration was in `status` at `at`, with the `error_cause` the
    /// report carried.
    ///
    /// Recording the same status at the same instant again REPLACES the event rather than
    /// adding one, which is what makes a repeated report of one event a no-op. Once the
    /// history is at its cap the oldest event it holds is dropped.
    fn record(&mut self, status: &str, at: &DateTime, error_cause: Option<&str>) {
        self.events.insert(
            status_event_key(status, at),
            error_cause.map(ToString::to_string),
        );
        while self.events.len() > MAX_STATUS_EVENTS_PER_CONFIGURATION {
            self.events.pop_first();
        }
    }

    /// How many events this history holds, over every status.
    pub fn len(&self) -> usize {
        self.events.len()
    }

    /// Whether this history holds no event at all.
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// One page of the events [`StatusEventQuery`] asks for, most recent first, together with
    /// the instant of the last event it reported when a further page remains (`None` once the
    /// page is the last one).
    ///
    /// The window is inclusive at both ends: an event is reported for an instant, and an
    /// instant the caller names as a bound is part of the range it asks about. At most
    /// `page_size + 1` matching events are collected, and the walk itself is over a history
    /// bounded by [`MAX_STATUS_EVENTS_PER_CONFIGURATION`].
    fn page(
        &self,
        query: &StatusEventQuery,
    ) -> (Vec<InstrumentationStatusEvent>, Option<StatusEventInstant>) {
        // A page of nothing could not advance its own cursor, so a caller following
        // `NextToken` would loop for ever. `MaxResults` is modelled `range 1..100`, so this
        // only ever clamps a bound no request can state.
        let page_size = query.page_size.max(1);
        let lower = status_event_instant(&query.start);
        let upper = status_event_instant(&query.end);
        let mut window: Vec<(&StatusEventKey, &Option<String>)> = self
            .events
            .iter()
            .rev()
            .filter(|(key, _)| {
                let at = (key.0, key.1);
                key.2 == query.status
                    && at >= lower
                    && at <= upper
                    && query.older_than.is_none_or(|bound| at < bound)
            })
            .take(page_size.saturating_add(1))
            .collect();

        let next = if window.len() > page_size {
            window.truncate(page_size);
            window.last().map(|(key, _)| (key.0, key.1))
        } else {
            None
        };

        let events = window
            .into_iter()
            .map(|(key, error_cause)| InstrumentationStatusEvent {
                time: DateTime::from_secs_and_nanos(key.0, key.1),
                error_cause: error_cause.clone(),
            })
            .collect();
        (events, next)
    }
}

/// What one status read asks of a configuration's history: the status whose chronology it
/// walks, the window it covers, where the previous page of the sequence stopped and how many
/// events a page holds.
#[derive(Debug, Clone)]
pub struct StatusEventQuery {
    pub status: String,
    pub start: DateTime,
    pub end: DateTime,
    pub older_than: Option<StatusEventInstant>,
    pub page_size: usize,
}

/// The [`StatusEventKey`] of `status` at `at`.
fn status_event_key(status: &str, at: &DateTime) -> StatusEventKey {
    (at.secs(), at.subsec_nanos(), status.to_string())
}

/// One instant of a status chronology, to the precision a `Timestamp` carries: whole seconds
/// and the nanoseconds within them.
pub type StatusEventInstant = (i64, u32);

/// How many nanoseconds make a second, which is the exclusive bound on the subsecond part of
/// an instant: a value at or above it belongs to the next second and is no subsecond part at
/// all.
const NANOS_PER_SECOND: u32 = 1_000_000_000;

/// The [`StatusEventInstant`] of `at`.
fn status_event_instant(at: &DateTime) -> StatusEventInstant {
    (at.secs(), at.subsec_nanos())
}

/// The boundary a status-event `NextToken` carries: the WINDOW the page sequence is being
/// taken over, and the instant of the last event the previous page reported, which the next
/// call resolves as an exclusive bound on a chronology read backwards.
///
/// The window travels in the token because the default window is RELATIVE ("the last hour"):
/// recomputing it on the next call would move both its bounds, so a page sequence that named
/// no window would drift across its own pages. Carrying it keeps every page of one sequence
/// over one window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatusEventCursor {
    pub start: StatusEventInstant,
    pub end: StatusEventInstant,
    pub older_than: StatusEventInstant,
}

impl StatusEventCursor {
    /// The cursor for the page after one taken over `[start, end]` whose last event was at
    /// `older_than`.
    pub fn new(start: &DateTime, end: &DateTime, older_than: StatusEventInstant) -> Self {
        Self {
            start: status_event_instant(start),
            end: status_event_instant(end),
            older_than,
        }
    }

    /// This cursor as the opaque payload of a `NextToken`.
    pub fn encode(&self) -> String {
        let parts = [self.start, self.end, self.older_than]
            .map(|(secs, nanos)| format!("{secs}.{nanos}"));
        parts.join("/")
    }

    /// The cursor a `NextToken` payload carries, or `None` when it is not one [`Self::encode`]
    /// produced.
    ///
    /// A `NextToken` is opaque but not authenticated, so every field it carries is a value a
    /// caller may have written: an instant is accepted only when its subsecond part is a real
    /// one (strictly under a second, [`NANOS_PER_SECOND`]), which is the range
    /// [`Self::window`]'s timestamps are defined over. A payload carrying anything else is not
    /// a cursor this service issued, and is refused exactly like a token that is not hex.
    pub fn decode(payload: &str) -> Option<Self> {
        let mut parts = payload.split('/');
        let mut instant = || -> Option<StatusEventInstant> {
            let (secs, nanos) = parts.next()?.split_once('.')?;
            let secs = secs.parse::<i64>().ok()?;
            let nanos = nanos.parse::<u32>().ok().filter(|nanos| *nanos < NANOS_PER_SECOND)?;
            Some((secs, nanos))
        };
        let start = instant()?;
        let end = instant()?;
        let older_than = instant()?;
        Some(Self {
            start,
            end,
            older_than,
        })
    }

    /// The window this cursor's page sequence is taken over.
    pub fn window(&self) -> (DateTime, DateTime) {
        (
            DateTime::from_secs_and_nanos(self.start.0, self.start.1),
            DateTime::from_secs_and_nanos(self.end.0, self.end.1),
        )
    }

    /// Whether this cursor's window is the one `[start, end]` names.
    pub fn covers(&self, start: &DateTime, end: &DateTime) -> bool {
        self.start == status_event_instant(start) && self.end == status_event_instant(end)
    }
}

/// The `NextToken` tag one status read issues its page tokens under: the operation's
/// [`INSTRUMENTATION_STATUS_EVENTS_TOKEN_TAG`] plus a fingerprint of the configuration and
/// the status whose chronology the page was taken over.
///
/// A cursor is a boundary within ONE chronology, so a token issued for another configuration
/// or another status would silently skip or repeat events if it were honoured. Binding the
/// chronology's identity into the tag makes such a token fail to decode, and it is refused
/// like any other token this service did not issue. The window is not part of the identity:
/// it travels inside the cursor (see [`StatusEventCursor`]).
pub fn instrumentation_status_token_tag(
    scope: &InstrumentationScope,
    key: &str,
    status: &str,
) -> String {
    let fingerprint = stable_hash(&[
        Some(scope.instrumentation_type.as_str()),
        Some(scope.service.as_str()),
        Some(scope.environment.as_str()),
        Some(key),
        Some(status),
    ]);
    format!("{INSTRUMENTATION_STATUS_EVENTS_TOKEN_TAG}{fingerprint}/")
}

/// The window a status read covers: the one the request names, or the documented default.
///
/// `StartTime` and `EndTime` "must both be provided together or both be omitted", and "when
/// both are omitted, the time range defaults to the last hour" — so a request naming exactly
/// one of them names no window, and neither does one whose window ends before it begins.
/// `Err` carries the refusal such a request earns.
pub fn status_event_window(
    start: Option<&DateTime>,
    end: Option<&DateTime>,
    now: &DateTime,
) -> Result<(DateTime, DateTime), String> {
    // Compared and derived on the exact instants a `Timestamp` carries — seconds and the
    // nanoseconds within them, the same pair the history is keyed by — so one precision model
    // governs the window and the events it selects.
    match (start, end) {
        (None, None) => Ok((
            DateTime::from_secs_and_nanos(
                now.secs()
                    .saturating_sub(STATUS_EVENT_DEFAULT_WINDOW.as_secs() as i64),
                now.subsec_nanos(),
            ),
            *now,
        )),
        (Some(start), Some(end))
            if status_event_instant(end) >= status_event_instant(start) =>
        {
            Ok((*start, *end))
        }
        (Some(_), Some(_)) => Err("EndTime must not be earlier than StartTime".to_string()),
        _ => Err(
            "StartTime and EndTime must both be provided together or both be omitted".to_string(),
        ),
    }
}

impl ComAmazonawsApplicationsignalsApplicationsignalsState {
    /// How many entries the instrumentation store holds against its quota: one per stored
    /// configuration, and at least one for a retained scope whose set has emptied.
    ///
    /// A retained scope costs quota exactly like a configuration, so
    /// [`MAX_INSTRUMENTATION_CONFIGURATIONS`] bounds the number of SCOPE KEYS the store
    /// holds as well as the number of records inside them: a caller cannot grow the store
    /// past the cap by creating configurations in fresh scopes and letting them lapse. Both
    /// halves of a scope key are modelled `length 1..255`, so a bounded count of keys is a
    /// bounded size too.
    pub fn instrumentation_entry_count(&self) -> usize {
        self.instrumentation_configurations
            .values()
            .map(|set| set.configurations.len().max(1))
            .sum()
    }

    /// Whether another instrumentation configuration would put this service over its
    /// [`MAX_INSTRUMENTATION_CONFIGURATIONS`] quota.
    pub fn at_instrumentation_configuration_quota(&self) -> bool {
        self.instrumentation_entry_count() >= MAX_INSTRUMENTATION_CONFIGURATIONS
    }

    /// The configuration stored in `scope` under `key` ([`instrumentation_key`]), or `None`
    /// when the scope holds no configuration at that location.
    pub fn instrumentation_configuration(
        &self,
        scope: &InstrumentationScope,
        key: &str,
    ) -> Option<&InstrumentationConfiguration> {
        self.instrumentation_configurations
            .get(scope)
            .and_then(|set| set.configurations.get(key))
    }

    /// Store `configuration` in `scope`, recording `at` as the instant the scope's set
    /// changed.
    pub fn put_instrumentation_configuration(
        &mut self,
        scope: InstrumentationScope,
        configuration: InstrumentationConfiguration,
        at: DateTime,
    ) {
        let key = instrumentation_key(&configuration.signal_type, &configuration.location_hash);
        let expires_at = configuration.expires_at;
        let set = self.instrumentation_configurations.entry(scope).or_default();
        set.configurations.insert(key, configuration);
        set.record_change(at);
        if let Some(expires_at) = expires_at {
            self.schedule_instrumentation_pass(expires_at);
        }
    }

    /// Note that [`Self::expire_instrumentation_configurations`] has something to do at `due`,
    /// keeping the EARLIEST such instant: the pass walks the store only once the first thing
    /// it owes has fallen due.
    fn schedule_instrumentation_pass(&mut self, due: DateTime) {
        let sooner = self
            .instrumentation_due_at
            .is_none_or(|known| due.as_secs_f64() < known.as_secs_f64());
        if sooner {
            self.instrumentation_due_at = Some(due);
        }
    }

    /// The next instant [`Self::expire_instrumentation_configurations`] has anything to do:
    /// the soonest `ExpiresAt` a stored configuration carries, or the soonest instant an
    /// emptied scope's record may be dropped, whichever comes first.
    fn next_instrumentation_pass(&self) -> Option<DateTime> {
        self.instrumentation_configurations
            .values()
            .flat_map(|set| {
                let expiries = set
                    .configurations
                    .values()
                    .filter_map(|stored| stored.expires_at);
                let droppable = if set.configurations.is_empty() {
                    set.last_changed_at.map(|changed| {
                        DateTime::from_secs_f64(
                            changed.as_secs_f64() + EMPTY_SCOPE_RETENTION.as_secs_f64(),
                        )
                    })
                } else {
                    None
                };
                expiries.chain(droppable)
            })
            .min_by(|left, right| left.as_secs_f64().total_cmp(&right.as_secs_f64()))
    }

    /// Drop every configuration whose `ExpiresAt` has fallen due by `now`, record the instant
    /// each lapse became due as a change to its scope's set, and drop a scope whose set has
    /// been empty for longer than [`EMPTY_SCOPE_RETENTION`].
    ///
    /// This is the one write a reporting operation may make: a BREAKPOINT configuration
    /// "expires after 24 hours unless a shorter expiration is provided", so its removal was
    /// owed by the CLOCK at `ExpiresAt` whether or not anybody called. The change is dated
    /// at the expiry instant rather than at `now` for the same reason: that is when the set
    /// changed, so a caller that synced after it is not told the set changed again. Dropping
    /// an emptied scope once its retention horizon has passed is owed by the clock in the
    /// same way.
    ///
    /// The store is walked only when the clock owes something ([`Self::next_instrumentation_pass`]),
    /// so the common call pays no scan at all — and the walk itself is over a store bounded
    /// by [`MAX_INSTRUMENTATION_CONFIGURATIONS`] entries, scopes included, so its cost cannot
    /// grow with the scopes a caller mints.
    pub fn expire_instrumentation_configurations(&mut self, now: &DateTime) {
        let threshold = now.as_secs_f64();
        if self
            .instrumentation_due_at
            .is_none_or(|due| due.as_secs_f64() > threshold)
        {
            return;
        }

        let lapsed: Vec<(InstrumentationScope, String, DateTime, String)> = self
            .instrumentation_configurations
            .iter()
            .flat_map(|(scope, set)| {
                set.configurations.iter().filter_map(move |(key, stored)| {
                    let expires_at = stored.expires_at?;
                    (expires_at.as_secs_f64() <= threshold).then(|| {
                        (
                            scope.clone(),
                            key.clone(),
                            expires_at,
                            stored.arn.clone(),
                        )
                    })
                })
            })
            .collect();

        for (scope, key, expires_at, arn) in lapsed {
            if let Some(set) = self.instrumentation_configurations.get_mut(&scope) {
                set.configurations.remove(&key);
                // The status history belongs to the configuration, so it goes with it: a
                // later configuration created at the same location is a NEW resource and
                // has reported nothing yet.
                set.status_history.remove(&key);
                set.record_change(expires_at);
            }
            // Tags are addressed by resource ARN, so a record that is gone takes its tags
            // with it rather than leaving them for the next resource minted at that ARN.
            self.tags.remove(&arn);
        }

        // An emptied scope keeps its last-change instant only while a caller could still be
        // syncing from before that change; past the horizon the record has nothing left to
        // tell anyone, and keeping it would let a caller grow the store with scope keys it
        // chooses.
        self.instrumentation_configurations.retain(|_, set| {
            !set.configurations.is_empty()
                || set.last_changed_at.is_none_or(|changed| {
                    changed.as_secs_f64() + EMPTY_SCOPE_RETENTION.as_secs_f64() > threshold
                })
        });

        self.instrumentation_due_at = self.next_instrumentation_pass();
    }

    /// Record that the configuration stored in `scope` under `key` ([`instrumentation_key`])
    /// was in `status` at `at`, with the `error_cause` the report carried, and answer whether
    /// the event was recorded.
    ///
    /// `false` — and nothing recorded — when the scope holds no configuration at that
    /// location: a status event is reported AGAINST a configuration, so there is nothing for
    /// one naming no configuration to be recorded against. Recording the same status at the
    /// same instant again replaces the event rather than adding one, so repeating a report
    /// changes nothing further.
    pub fn record_instrumentation_status(
        &mut self,
        scope: &InstrumentationScope,
        key: &str,
        status: &str,
        at: &DateTime,
        error_cause: Option<&str>,
    ) -> bool {
        let Some(set) = self.instrumentation_configurations.get_mut(scope) else {
            return false;
        };
        if !set.configurations.contains_key(key) {
            return false;
        }
        set.status_history
            .entry(key.to_string())
            .or_default()
            .record(status, at, error_cause);
        true
    }

    /// The status history of the configuration stored in `scope` under `key`
    /// ([`instrumentation_key`]), or `None` when the scope holds no history for that
    /// location — which is the state of a configuration nothing has reported for.
    pub fn instrumentation_status_history(
        &self,
        scope: &InstrumentationScope,
        key: &str,
    ) -> Option<&InstrumentationStatusHistory> {
        self.instrumentation_configurations
            .get(scope)
            .and_then(|set| set.status_history.get(key))
    }

    /// One page of the events [`StatusEventQuery`] asks for, of the configuration stored in
    /// `scope` under `key`, together with the cursor for the page after it.
    ///
    /// A configuration nothing has reported for has an empty history, which is not an error:
    /// the resource exists and its status events are the empty list.
    pub fn page_instrumentation_status_events(
        &self,
        scope: &InstrumentationScope,
        key: &str,
        query: &StatusEventQuery,
    ) -> (Vec<InstrumentationStatusEvent>, Option<StatusEventCursor>) {
        let (events, next) = match self.instrumentation_status_history(scope, key) {
            Some(history) => history.page(query),
            None => (Vec::new(), None),
        };
        (
            events,
            next.map(|older_than| {
                StatusEventCursor::new(&query.start, &query.end, older_than)
            }),
        )
    }

    /// Whether `scope`'s set has changed since `since`.
    ///
    /// Strictly after: a caller that synced at the instant of the last change already saw
    /// it, which is what makes an unchanged-since poll answer `Changed: false`. A scope that
    /// has never held a configuration has nothing new either.
    pub fn instrumentation_changed_since(
        &self,
        scope: &InstrumentationScope,
        since: &DateTime,
    ) -> bool {
        self.instrumentation_configurations
            .get(scope)
            .and_then(|set| set.last_changed_at)
            .is_some_and(|changed| changed.as_secs_f64() > since.as_secs_f64())
    }

    /// One page of `scope`'s configurations, in key order, starting strictly after `cursor`,
    /// together with the token for the page after it (`None` once the page is the last one).
    ///
    /// At most `page_size + 1` records are collected, so the response is bounded by the page
    /// rather than by the number of stored configurations. The cursor is a stored KEY rather
    /// than an offset, so a configuration created or expired between two pages cannot make
    /// the next page skip or repeat an entry.
    pub fn page_instrumentation_configurations(
        &self,
        scope: &InstrumentationScope,
        cursor: Option<&str>,
        page_size: usize,
    ) -> (
        Vec<InstrumentationConfigurationWithoutServiceEnv>,
        Option<String>,
    ) {
        let Some(set) = self.instrumentation_configurations.get(scope) else {
            return (Vec::new(), None);
        };
        // A page of nothing could not advance its own cursor, so a caller following
        // `NextToken` would loop for ever. `MaxResults` is modelled `range 1..100`, so this
        // only ever clamps a bound no request can state.
        let page_size = page_size.max(1);
        let lower = match cursor {
            Some(cursor) => Bound::Excluded(cursor),
            None => Bound::Unbounded,
        };
        let mut window: Vec<&InstrumentationConfiguration> = set
            .configurations
            .range::<str, _>((lower, Bound::Unbounded))
            .map(|(_, stored)| stored)
            .take(page_size.saturating_add(1))
            .collect();

        let next_token = if window.len() > page_size {
            window.truncate(page_size);
            window.last().map(|stored| {
                encode_cursor(
                    &instrumentation_token_tag(scope),
                    &instrumentation_key(&stored.signal_type, &stored.location_hash),
                )
            })
        } else {
            None
        };
        (
            window.into_iter().map(without_service_env).collect(),
            next_token,
        )
    }
}

/// A configuration's key within its [`InstrumentationScope`]: the rest of the identity a
/// create conflicts on ("If a configuration already exists for the same service,
/// environment, signal type, and location, this operation returns a conflict").
///
/// `/` separates the two parts unambiguously: `signal_type` is an enum value
/// (`DynamicInstrumentationSignalType`) and a location hash is 16 hex characters, so neither
/// can contain one.
pub fn instrumentation_key(signal_type: &str, location_hash: &str) -> String {
    format!("{signal_type}/{location_hash}")
}

/// The ARN of the configuration at `location_hash` in `scope`, in the format the modelled
/// `InstrumentationConfigurationArn` doc states:
/// `arn:${Partition}:application-signals:${Region}:${Account}:instrumentationConfig/${service}/${environment}/${signalType}/${locationHash}`.
pub fn instrumentation_configuration_arn(
    region: &str,
    scope: &InstrumentationScope,
    signal_type: &str,
    location_hash: &str,
) -> String {
    ::engine::emulator_arn(
        SIGNING_NAME,
        region,
        &format!(
            "{INSTRUMENTATION_CONFIG_ARN_RESOURCE}/{}/{}/{signal_type}/{location_hash}",
            scope.service, scope.environment
        ),
    )
}

/// The `NextToken` tag `scope` issues its page tokens under: the operation's
/// [`INSTRUMENTATION_CONFIGURATIONS_TOKEN_TAG`] plus a fingerprint of the scope.
///
/// A cursor is a key within ONE scope's ordered set, so a token issued for another scope
/// would silently skip entries if it were honoured. Binding the scope into the tag makes
/// such a token fail to decode, and it is refused like any other token this service did not
/// issue.
pub fn instrumentation_token_tag(scope: &InstrumentationScope) -> String {
    let fingerprint = stable_hash(&[
        Some(scope.instrumentation_type.as_str()),
        Some(scope.service.as_str()),
        Some(scope.environment.as_str()),
    ]);
    format!("{INSTRUMENTATION_CONFIGURATIONS_TOKEN_TAG}{fingerprint}/")
}

/// The `LocationHash` of a location: "A stable hash computed from the location that uniquely
/// identifies this instrumentation point within the service, environment, and signal type."
///
/// Stable (the same location always hashes the same, in this process and the next, because
/// nothing but the location's own members feeds it) and 16 lowercase hex characters, as the
/// member's `length 16..16` and the ARN pattern's `[0-9a-f]{16}` suffix require.
pub fn location_hash(location: &Location) -> String {
    match location {
        Location::CodeLocation(code) => {
            let line_number = code.line_number.map(|line| line.to_string());
            stable_hash(&[
                // The union member name, so two variants could never collide on one
                // rendering of their contents.
                Some("CodeLocation"),
                Some(code.language.as_str()),
                code.code_unit.as_deref(),
                code.class_name.as_deref(),
                code.method_name.as_deref(),
                Some(code.file_path.as_str()),
                line_number.as_deref(),
            ])
        }
    }
}

/// The `LocationHash` a `GetInstrumentationConfiguration` request's `LocationIdentifier`
/// denotes, for either form the model admits: "either full code location or a pre-computed
/// hash".
///
/// A full location is hashed by [`location_hash`] — the same function the create path keyed
/// the record by, so a request naming the location and one naming the hash resolve to one
/// record — and a hash the caller already holds IS the value, taken verbatim.
pub fn identified_location_hash(identifier: &LocationIdentifier) -> String {
    match identifier {
        LocationIdentifier::CodeLocation(code) => {
            location_hash(&Location::CodeLocation(code.clone()))
        }
        LocationIdentifier::LocationHash(hash) => hash.clone(),
    }
}

/// 16 lowercase hex characters of SHA-256 over `fields`.
///
/// The digest is the maintained `sha2` crate's, and the rendering is INJECTIVE: each field
/// contributes a present/absent marker, its byte length and its bytes, so no two different
/// field lists produce the same input \u2014 which is what makes two locations that differ only
/// in where a value sits (an absent `ClassName` against an empty one, say) hash differently.
fn stable_hash(fields: &[Option<&str>]) -> String {
    use ::sha2::Digest;

    let mut digest = ::sha2::Sha256::new();
    for field in fields {
        match field {
            Some(value) => {
                digest.update([1u8]);
                digest.update((value.len() as u64).to_le_bytes());
                digest.update(value.as_bytes());
            }
            None => digest.update([0u8]),
        }
    }
    ::hex::encode(&digest.finalize()[..LOCATION_HASH_BYTES])
}

/// A stored configuration as a list reports it: every member of the record except the
/// service and environment, "because they are provided in the request" and the page states
/// them once.
pub fn without_service_env(
    stored: &InstrumentationConfiguration,
) -> InstrumentationConfigurationWithoutServiceEnv {
    InstrumentationConfigurationWithoutServiceEnv {
        instrumentation_type: stored.instrumentation_type.clone(),
        signal_type: stored.signal_type.clone(),
        location: stored.location.clone(),
        location_hash: stored.location_hash.clone(),
        description: stored.description.clone(),
        expires_at: stored.expires_at,
        attribute_filters: stored.attribute_filters.clone(),
        capture_configuration: stored.capture_configuration.clone(),
        created_at: stored.created_at,
        arn: stored.arn.clone(),
    }
}

/// The `ExpiresAt` a create stores for an `InstrumentationType`, or `Err` with the refusal
/// the request earns.
///
/// The rule is the operation's own doc: "For BREAKPOINT type configurations, they expire
/// after 24 hours unless a shorter expiration is provided. For PROBE type configurations,
/// they persist until explicitly deleted; an expiration cannot be set for PROBE
/// configurations", and the member's: "For BREAKPOINT: optional, defaults to 24 hours, must
/// be between 5 min and 24 hours. For PROBE: not supported."
///
/// The window is measured from the instant the create is served, which is what makes the
/// stored expiry a lifetime rather than a wall-clock instant the caller may pick freely.
pub fn requested_expiry(
    instrumentation_type: &str,
    requested: Option<&DateTime>,
    now: &DateTime,
) -> Result<Option<DateTime>, String> {
    if instrumentation_type != INSTRUMENTATION_TYPE_BREAKPOINT {
        return match requested {
            Some(_) => Err(format!(
                "ExpiresAt is not supported for {instrumentation_type} instrumentation \
                 configurations, which persist until they are explicitly deleted"
            )),
            None => Ok(None),
        };
    }

    let Some(requested) = requested else {
        return Ok(Some(DateTime::from_secs_f64(
            now.as_secs_f64() + BREAKPOINT_DEFAULT_EXPIRY.as_secs_f64(),
        )));
    };

    let lifetime = requested.as_secs_f64() - now.as_secs_f64();
    if lifetime < BREAKPOINT_MIN_EXPIRY.as_secs_f64()
        || lifetime > BREAKPOINT_MAX_EXPIRY.as_secs_f64()
    {
        return Err(format!(
            "ExpiresAt must be between {} and {} seconds after the time of the request for \
             {INSTRUMENTATION_TYPE_BREAKPOINT} instrumentation configurations",
            BREAKPOINT_MIN_EXPIRY.as_secs(),
            BREAKPOINT_MAX_EXPIRY.as_secs(),
        ));
    }
    Ok(Some(*requested))
}

/// The modelled `ConflictException` for a create that names a configuration already stored.
///
/// `ConflictException` carries only `Message`, so the message is the only place the
/// conflicting resource can be identified \u2014 and it identifies it by the ARN read off the
/// STORED record, which is what a client resolves or adopts the existing configuration by.
pub fn instrumentation_configuration_already_exists(arn: &str) -> ConflictException {
    ConflictException {
        message: format!("Instrumentation configuration with arn '{arn}' already exists."),
    }
}

/// The only `DynamicInstrumentationDeletionStatus` the model declares
/// (`smithy_query(query="shape",shape="DynamicInstrumentationDeletionStatus")` ->
/// `["DELETED"]`), which `DeleteInstrumentationConfiguration` answers once the configuration
/// it named is no longer stored. Its member doc: "The value is DELETED when the
/// configuration has been removed."
pub const DELETION_STATUS_DELETED: &str = "DELETED";

/// The `ResourceType` an instrumentation configuration failure reports: the resource's TYPE
/// NAME (the modelled `InstrumentationConfiguration` shape), not the camel-case segment an
/// ARN spells ([`INSTRUMENTATION_CONFIG_ARN_RESOURCE`]) — the same distinction
/// [`SLO_RESOURCE_TYPE`] draws for an SLO, and what a client comparing `ResourceType`
/// compares against.
pub const INSTRUMENTATION_CONFIG_RESOURCE_TYPE: &str = "InstrumentationConfiguration";

/// The modelled `ResourceNotFoundException` for an identity that names no stored
/// configuration.
///
/// `ResourceType` and `ResourceId` are both `@required` members a client branches on, so the
/// failure reports the type name and the full resource identifier. There is no record to
/// read an ARN off, so the ARN is derived from the identity the REQUEST named — exactly as
/// [`slo_not_found`] does for an SLO — and the message states the same two facts.
pub fn instrumentation_configuration_not_found(
    region: &str,
    scope: &InstrumentationScope,
    signal_type: &str,
    location_hash: &str,
) -> ResourceNotFoundException {
    let arn = instrumentation_configuration_arn(region, scope, signal_type, location_hash);
    ResourceNotFoundException {
        message: format!(
            "{INSTRUMENTATION_CONFIG_RESOURCE_TYPE} with arn '{arn}' does not exist."
        ),
        resource_type: INSTRUMENTATION_CONFIG_RESOURCE_TYPE.to_string(),
        resource_id: arn,
    }
}

impl ComAmazonawsApplicationsignalsApplicationsignalsState {
    /// Remove the configuration stored in `scope` under `key` ([`instrumentation_key`]),
    /// along with its tags, and hand back the removed record. `None` — and nothing removed —
    /// when the scope holds no configuration at that location.
    ///
    /// `at` is recorded as the instant the scope's set changed, the mirror of
    /// [`Self::put_instrumentation_configuration`]: a deletion is news to every caller that
    /// synced before it, and it leaves no record for a list to read a timestamp off. A scope
    /// whose set this empties keeps that instant for [`EMPTY_SCOPE_RETENTION`] so a slower
    /// caller still learns of the change, and the expiry pass is scheduled for the end of
    /// that horizon so the emptied record is dropped once it has nothing left to tell.
    pub fn remove_instrumentation_configuration(
        &mut self,
        scope: &InstrumentationScope,
        key: &str,
        at: DateTime,
    ) -> Option<InstrumentationConfiguration> {
        let (removed, emptied) = {
            let set = self.instrumentation_configurations.get_mut(scope)?;
            let removed = set.configurations.remove(key)?;
            // The status history belongs to the configuration, so it goes with it: a later
            // configuration created at the same location is a NEW resource and has reported
            // nothing yet.
            set.status_history.remove(key);
            set.record_change(at);
            (removed, set.configurations.is_empty())
        };
        // Tags are addressed by resource ARN, so a record that is gone takes its tags with
        // it rather than leaving them for the next resource minted at that ARN.
        self.tags.remove(&removed.arn);
        if emptied {
            self.schedule_instrumentation_pass(DateTime::from_secs_f64(
                at.as_secs_f64() + EMPTY_SCOPE_RETENTION.as_secs_f64(),
            ));
        }
        Some(removed)
    }
}

// =============================================================================
// The change events this service holds a record of for an entity
// =============================================================================

/// The [`ENTITY_TYPES`] value designating a service entity — the kind of object an
/// instrumentation configuration's `Service`/`Environment` pair names, and the `Type` of the
/// service identity the member doc spells out: `{ "Type": "Service", "Name":
/// "visits-service", "Environment": "petclinic-test" }` (same member doc).
pub const SERVICE_ENTITY_TYPE: &str = "Service";

/// The `EventName` of the event a stored SLO's `CreatedTime` is the record of.
///
/// `EventName` is "The name or description of this change event"
/// (`smithy_query(query="doc", shape="ChangeEvent", member="EventName")`), so it says WHICH
/// change the event reports. Each dated member a record carries is written by exactly one
/// operation — `CreatedTime` by the create, `LastUpdatedTime` by the update — so naming that
/// operation describes the change without claiming anything the record does not hold.
const SLO_CREATED_EVENT_NAME: &str = "CreateServiceLevelObjective";

/// [`SLO_CREATED_EVENT_NAME`], for the `LastUpdatedTime` of an SLO that has been updated
/// since it was created.
const SLO_UPDATED_EVENT_NAME: &str = "UpdateServiceLevelObjective";

/// [`SLO_CREATED_EVENT_NAME`], for the `CreatedAt` of a stored instrumentation
/// configuration.
const INSTRUMENTATION_CREATED_EVENT_NAME: &str = "CreateInstrumentationConfiguration";

/// The service entity an instrumentation configuration's `Service` and `Environment` name,
/// as an `Attributes` identity.
///
/// A configuration is applied to the service it names in the environment it names, and that
/// pair IS a service entity's identity in the form the `Entity` member documents (`{ "Type":
/// "Service", "Name": …, "Environment": … }`), so the configuration's own record is what the
/// identity is read from.
pub fn service_entity_attributes(service: &str, environment: &str) -> HashMap<String, String> {
    HashMap::from([
        (TYPE_ATTRIBUTE.to_string(), SERVICE_ENTITY_TYPE.to_string()),
        (NAME_ATTRIBUTE.to_string(), service.to_string()),
        (ENVIRONMENT_ATTRIBUTE.to_string(), environment.to_string()),
    ])
}

/// One instant as a count of nanoseconds since the epoch, which is the precision a
/// [`DateTime`] holds.
///
/// Instants are compared through this rather than through `DateTime::as_secs_f64`, whose
/// resolution at present epoch values is around a hundred nanoseconds: the window bounds are
/// INCLUSIVE, so a comparison that cannot tell two instants apart would make the bound
/// approximate, and two distinct instants would order as equal.
///
/// The seconds and the nanoseconds within them are read through
/// [`status_event_instant`], the module's one reader of an instant's parts, so a comparison
/// here and the chronology a status-event token carries cannot disagree about what instant a
/// `Timestamp` denotes.
fn instant_nanos(at: &DateTime) -> i128 {
    let (secs, subsec_nanos) = status_event_instant(at);
    i128::from(secs) * i128::from(NANOS_PER_SECOND) + i128::from(subsec_nanos)
}

/// The Region a stored record was written in: the one its own ARN carries, read through
/// `service_common::arn` so it comes out of the ARN's REGION field rather than out of a
/// substring match.
///
/// `ChangeEvent.Region` is "The Amazon Web Services region where this change event
/// occurred", and the change this event reports is the write of that record, so the Region
/// is the record's own rather than the reader's. A record whose ARN carries no Region label
/// falls back to the emulator's default Region, exactly as [`region_of`] does for a minted
/// ARN.
fn record_region(arn: &str) -> &str {
    ::service_common::arn::region_of(arn, SIGNING_NAME).unwrap_or(DEFAULT_REGION)
}

/// One change event: the dated change `event_name` describes — nothing, where the record
/// says only THAT it changed — to the entity `entity` identifies, in `region`.
///
/// The ONE place this service builds a [`ChangeEvent`], so the two reads that report one —
/// `ListEntityEvents` through
/// [`ComAmazonawsApplicationsignalsApplicationsignalsState::entity_change_events`] and
/// `ListServiceStates` through
/// [`ComAmazonawsApplicationsignalsApplicationsignalsState::observed_service_states`] —
/// cannot differ on what an event of this emulator carries: the only kind of change it
/// records ([`CONFIGURATION_CHANGE_EVENT`]), the documented id of an event it did not read
/// off a CloudTrail record ([`UNRECORDED_EVENT_ID`]), and this store's account
/// (`engine::emulator_account_id()`, the account every ARN it mints carries), in which the
/// record the event is derived from is held. `UserName` ("the name of the user who initiated
/// this change event, if available") stays absent, because no record this service keeps holds
/// the identity that wrote it.
fn change_event(
    at: DateTime,
    entity: &HashMap<String, String>,
    region: &str,
    event_name: Option<&str>,
) -> ChangeEvent {
    ChangeEvent {
        timestamp: at,
        account_id: ::engine::emulator_account_id().to_string(),
        region: region.to_string(),
        entity: entity.clone(),
        change_event_type: CONFIGURATION_CHANGE_EVENT.to_string(),
        event_id: UNRECORDED_EVENT_ID.to_string(),
        user_name: None,
        event_name: event_name.map(ToString::to_string),
    }
}

impl ComAmazonawsApplicationsignalsApplicationsignalsState {
    /// The change events this service holds a record of for the entity `entity` identifies,
    /// as `ListEntityEvents` reports them: those whose instant falls in the requested period,
    /// most recent first.
    ///
    /// A change event is a DATED CHANGE this emulator has a record of, and the records it
    /// holds against an entity are the stored SLOs configured on it (dated by the
    /// `CreatedTime` of the create and the `LastUpdatedTime` of an update that followed it)
    /// and the instrumentation configurations applied to it (dated by their `CreatedAt`).
    /// Each event is read out of one such record, so an event is reported exactly when the
    /// store is evidence of the change it describes — and a record this service no longer
    /// holds leaves no event behind, since the store keeps no tombstone for a deletion.
    ///
    /// The entity is matched the way every other entity read matches it ([`attributes_contain`]):
    /// as few as one of an entity's up to four `Attributes` entries identifies it, so a
    /// record matches when its own identity holds every entry the request named. The
    /// REPORTED `Entity` is the record's identity rather than the request's, so a caller
    /// that named an entity partially is told the full identity the change was recorded
    /// against.
    ///
    /// The period is inclusive at both ends: an event at the instant the window begins or
    /// ends falls within it. Nothing else bounds the set — a window entirely in the future
    /// selects no event because no record is dated after now.
    ///
    /// The order is newest first, which is the order a history is read in, with the record's
    /// ARN and the event's name breaking a tie between two events at the same instant: the
    /// order has to be TOTAL for a page cursor into it to be a stable boundary.
    ///
    /// `ListServiceStates` reads the same SLO instants ([`Self::last_changed_in_window`]) but
    /// answers a different question — the ONE latest change of each DISCOVERED service,
    /// matching identities symmetrically ([`names_same_service`]) because the service it
    /// reports is one this emulator discovered rather than one a request named — so neither
    /// read can be expressed as the other; both build their event through [`change_event`],
    /// which is where what an event of this emulator carries is stated once.
    pub fn entity_change_events(
        &self,
        entity: &HashMap<String, String>,
        window_start: &DateTime,
        window_end: &DateTime,
    ) -> Vec<ChangeEvent> {
        // `(instant, tie-break key, event)`: the sort key is kept beside the event rather
        // than re-derived in the comparator, so the order cannot drift from the key.
        let mut dated: Vec<(i128, String, ChangeEvent)> = Vec::new();
        let (window_start, window_end) = (instant_nanos(window_start), instant_nanos(window_end));
        let mut collect = |at: DateTime,
                           attributes: &HashMap<String, String>,
                           arn: &str,
                           event_name: &str| {
            let at_nanos = instant_nanos(&at);
            if at_nanos < window_start || at_nanos > window_end {
                return;
            }
            dated.push((
                at_nanos,
                format!("{arn}\u{1d}{event_name}"),
                change_event(at, attributes, record_region(arn), Some(event_name)),
            ));
        };

        for slo in self.slos.values() {
            let Some(attributes) = entity_of(slo).key_attributes else {
                continue;
            };
            // `ChangeEvent.Entity` is `@required` with `length 1..4`, so a record whose
            // indicator names no entity at all is the record of a change to nothing and
            // could not be reported: this guards the OUTPUT constraint, not the match.
            if attributes.is_empty() || !attributes_contain(Some(&attributes), entity) {
                continue;
            }
            collect(
                slo.created_time,
                &attributes,
                &slo.arn,
                SLO_CREATED_EVENT_NAME,
            );
            // A record that has never been updated carries its create instant in both
            // members, which is ONE change, not two.
            if instant_nanos(&slo.last_updated_time) > instant_nanos(&slo.created_time) {
                collect(
                    slo.last_updated_time,
                    &attributes,
                    &slo.arn,
                    SLO_UPDATED_EVENT_NAME,
                );
            }
        }

        for (scope, set) in &self.instrumentation_configurations {
            let attributes = service_entity_attributes(&scope.service, &scope.environment);
            if !attributes_contain(Some(&attributes), entity) {
                continue;
            }
            for stored in set.configurations.values() {
                collect(
                    stored.created_at,
                    &attributes,
                    &stored.arn,
                    INSTRUMENTATION_CREATED_EVENT_NAME,
                );
            }
        }

        dated.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
        dated.into_iter().map(|(_, _, event)| event).collect()
    }
}
