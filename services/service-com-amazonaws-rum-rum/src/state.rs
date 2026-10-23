// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-rum-rum service.
//!
//! The app monitor is this service's primary resource: `CreateAppMonitor`,
//! `GetAppMonitor`, `ListAppMonitors`, `UpdateAppMonitor`, `DeleteAppMonitor`,
//! `ListTagsForResource` and the data-plane `PutRumEvents` all read and write the ONE
//! [`AppMonitorRecord`] map held here, so every one of them answers from persisted state
//! rather than from its own request. The helpers beside the map (id minting, the derived
//! ARN and CloudWatch Logs group name, the `ResourceArn` -> monitor resolution, the
//! documented defaults, the record -> wire-shape projections and the error builders) live
//! here for the same reason: every operation file shares them, and a second copy of any
//! of them would let two operations disagree about one resource.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::ops::types::{
    AccessDeniedException, AppMonitor, AppMonitorConfiguration, AppMonitorDetails,
    AppMonitorSummary, ConflictException, CustomEvents, CwLog, DataStorage,
    DeobfuscationConfiguration, InvalidPolicyRevisionIdException, JavaScriptSourceMaps,
    MetricDestinationSummary, PolicyNotFoundException, ResourceNotFoundException,
    METRIC_DESTINATION_VALUES,
};

/// Who a request speaks for, and whether IAM lets it do what it asks: the authorization
/// evaluation every operation in this service runs before it reads or writes.
pub mod authorization;

/// The CloudWatch Logs side of `DataStorage.CwLog`: the permission turning log delivery
/// on requires, and the vended log group RUM provisions when it is on.
pub mod cw_log_delivery;

/// This service's ARN namespace / sigv4 signing name (`OPERATIONS.md`,
/// `## Wire protocol`: both are `rum`).
pub const ARN_NAMESPACE: &str = "rum";

/// The `appmonitor` resource segment of an app-monitor ARN.
const APP_MONITOR_RESOURCE: &str = "appmonitor";

/// The region used when a request carries no SigV4 credential scope to read one from
/// (an anonymous, host-routed request). Every generated emulator defaults here.
pub const DEFAULT_REGION: &str = "us-east-1";

/// `State` of an app monitor this emulator creates — a member of
/// [`crate::ops::types::STATE_ENUM_VALUES`] (`CREATED`, `DELETING`, `ACTIVE`).
///
/// `CreateAppMonitor` is synchronous and the monitor it returns is immediately able to
/// accept `PutRumEvents`, so the resource is created in a terminal state with no
/// clock-owed transition pending. Recorded behaviour: the terminal state RUM reports for
/// a monitor that exists and is not being torn down is `CREATED` — on a freshly created
/// monitor and on one that has since been updated alike — so that is the single value the
/// detail and the summary projections both report.
pub const CREATED_STATE: &str = "CREATED";

/// The `AppMonitorPlatform` of an app monitor for a WEB application — the one platform
/// whose application has an internet domain at all, and so the one a monitor's domain
/// authority can describe.
///
/// From the model: `AppMonitorPlatform` admits `Web`, `Android` and `iOS`
/// ([`crate::ops::types::APP_MONITOR_PLATFORM_VALUES`]), documented "Web for web
/// applications, Android for Android applications, and iOS for IOS applications", while
/// `Domain` is "The top-level internet domain name for which your application has
/// administrative authority" — a property only a web application has.
pub const WEB_PLATFORM: &str = "Web";

/// `Platform` of an app monitor whose create request named none.
///
/// From the model: `CreateAppMonitorRequest$Platform` documents "If you omit this
/// parameter, the default is Web."
pub const DEFAULT_PLATFORM: &str = WEB_PLATFORM;

/// `JavaScriptSourceMaps$Status` of an app monitor whose create request named no
/// `DeobfuscationConfiguration`: deobfuscation is off until a request turns it on. A
/// member of [`crate::ops::types::DEOBFUSCATION_STATUS_VALUES`].
pub const DEFAULT_DEOBFUSCATION_STATUS: &str = "DISABLED";

/// `CustomEvents$Status` of an app monitor whose create request named no `CustomEvents`.
///
/// From the model: `CreateAppMonitorRequest$CustomEvents` documents "If you omit this
/// parameter, custom events are DISABLED." A member of
/// [`crate::ops::types::CUSTOM_EVENTS_STATUS_VALUES`].
pub const DEFAULT_CUSTOM_EVENTS_STATUS: &str = "DISABLED";

/// `SessionSampleRate` of an app monitor whose create request named no
/// `AppMonitorConfiguration`.
///
/// From the model: `CreateAppMonitorRequest$AppMonitorConfiguration` documents "If you
/// omit this argument, the sample rate used for RUM is set to 10% of the user sessions."
/// (The member's own `@default` is `0`, which applies when the request DOES carry a
/// configuration but leaves the rate out.)
pub const DEFAULT_SESSION_SAMPLE_RATE: f64 = 0.1;

/// Prefix of the CloudWatch Logs group an app monitor copies telemetry into when
/// `CwLogEnabled` is set: RUM is a vended-logs service, so the group lives under
/// `/aws/vendedlogs/RUMService_`. The suffix AWS appends is not in the model, so this
/// emulator derives it from the monitor's own (account-unique) name, which keeps
/// `DataStorage.CwLog.CwLogGroup` stable across reads and distinct per monitor.
pub const CW_LOG_GROUP_PREFIX: &str = "/aws/vendedlogs/RUMService_";

/// The `resourceType` this service's `ConflictException` reports for an app monitor.
///
/// Recorded behavior: the duplicate-name conflict `CreateAppMonitor` answers names the
/// resource's type as RUM's own product noun, not the model's shape or resource name.
/// It is the ONLY error of this service that reports a `resourceType` at all — the
/// not-found refusals leave the member off the wire (see
/// [`crate::ops::types::ResourceNotFoundException`]).
pub const APP_MONITOR_RESOURCE_TYPE: &str = "RUM application";

/// The most app monitors this emulator holds at once.
///
/// `app_monitors` grows by one record per `CreateAppMonitor` request and shrinks only on
/// an explicit delete, so without a ceiling its entry count — and with it the heap this
/// emulator holds — is set by how many requests a caller makes. AWS bounds the same
/// thing with an adjustable per-account app-monitor quota whose value is not in the
/// model, so this number is the EMULATOR's own bound, while the refusal it produces is
/// the error `CreateAppMonitor` models for exactly this case:
/// `ServiceQuotaExceededException` (HTTP 402).
pub const MAX_APP_MONITORS: usize = 1000;

/// The most RUM events this emulator keeps per app monitor.
///
/// `PutRumEvents` appends to a monitor's buffer on every call, so the buffer needs a
/// ceiling to keep the emulator's heap bounded; past it the OLDEST event is dropped,
/// which is the same direction as the 30-day retention AWS documents for RUM-held data
/// (`CreateAppMonitorRequest$CwLogEnabled`: "Data collected by RUM is kept by RUM for 30
/// days and then deleted"). The count is this emulator's own bound, not AWS's.
pub const MAX_EVENTS_PER_APP_MONITOR: usize = 10_000;

/// The `MetricDestination` value naming the account's own CloudWatch metrics as the
/// destination of an app monitor's extended metrics — a member of
/// [`crate::ops::types::METRIC_DESTINATION_VALUES`].
pub const CLOUD_WATCH_DESTINATION: &str = "CloudWatch";

/// The `MetricDestination` value naming a CloudWatch Evidently experiment as the
/// destination of an app monitor's extended metrics — a member of
/// [`crate::ops::types::METRIC_DESTINATION_VALUES`].
pub const EVIDENTLY_DESTINATION: &str = "Evidently";

/// The most extended-metrics destinations this emulator keeps per app monitor.
///
/// `PutRumMetricsDestination` adds one record per distinct destination key, so without a
/// ceiling the heap an app monitor holds is set by how many distinct Evidently
/// experiment ARNs a caller names. AWS bounds the same thing with an adjustable
/// per-app-monitor quota whose value is not in the model, so this number is the
/// EMULATOR's own bound; `PutRumMetricsDestination` models no quota error, so the
/// refusal past it is the `ValidationException` it does model.
pub const MAX_METRIC_DESTINATIONS_PER_APP_MONITOR: usize = 100;

/// The most bytes a resource-policy document may carry.
///
/// From the model: `PutResourcePolicy` documents "The maximum size of the policy is 4
/// KB" and `PolicySizeLimitExceededException` documents "The policy document is too
/// large. The limit is 4 KB." — 4 KB is AWS's own documented limit, not a bound this
/// emulator chose, and a document past it is refused with that modelled error.
pub const MAX_POLICY_DOCUMENT_BYTES: usize = 4 * 1024;

/// The `Version` values an IAM JSON policy document may declare.
///
/// Source: the IAM JSON policy language reference — `Version` "specifies the language
/// syntax rules" and admits the current `2012-10-17` and the legacy `2008-10-17`; any
/// other value makes the document malformed.
pub const POLICY_DOCUMENT_VERSIONS: &[&str] = &["2008-10-17", "2012-10-17"];

/// The top-level elements an IAM JSON policy document may carry.
///
/// Source: the IAM JSON policy language reference — a policy document is a JSON object
/// whose elements are `Version`, `Id` and `Statement`.
pub const POLICY_DOCUMENT_ELEMENTS: &[&str] = &["Version", "Id", "Statement"];

/// The `Effect` values an IAM policy statement may carry.
///
/// Source: the IAM JSON policy language reference — `Effect` is required and is either
/// `Allow` or `Deny`, matched case-sensitively.
pub const POLICY_STATEMENT_EFFECTS: &[&str] = &["Allow", "Deny"];

/// The message for the one failure that is not about the request: this service was
/// mounted on an engine that holds no store for it.
pub const NO_STORE: &str = "the com-amazonaws-rum-rum store is not mounted on this engine";

/// One RUM event a `PutRumEvents` call appended to an app monitor's buffer.
///
/// The event itself is kept as the JSON document the wire carried (`event_json`), which
/// is the form `GetAppMonitorData` reports events in (`EventDataList` is a list of
/// `JsonValue`), while the per-batch context the event arrived with — the batch id, the
/// optional alias and the user session it came from — is kept beside it.
#[derive(Debug, Clone)]
pub struct StoredRumEvent {
    /// `PutRumEventsRequest$BatchId` of the call that delivered this event.
    pub batch_id: String,
    /// `PutRumEventsRequest$Alias`, when the call carried one.
    pub alias: Option<String>,
    /// `PutRumEventsRequest$AppMonitorDetails`: the sending web client's own view of the
    /// app monitor (name, id, version). Kept verbatim rather than re-derived from the
    /// stored record, because it is the CLIENT's report — `GetAppMonitorData` returns the
    /// events as they were received, and a client may name a version the server knows
    /// nothing about.
    pub app_monitor_details: AppMonitorDetails,
    /// `UserDetails$userId` of the session this event came from.
    pub user_id: Option<String>,
    /// `UserDetails$sessionId` of the session this event came from.
    pub session_id: Option<String>,
    /// The event as a JSON object: `id`, `timestamp`, `type`, `metadata`, `details`.
    pub event_json: String,
}

/// The identity of one extended-metrics destination of an app monitor: its
/// `MetricDestination` and, for an Evidently destination, the ARN of the experiment that
/// receives the metrics (the empty string for a `CloudWatch` destination, which has no
/// ARN).
///
/// This pair is what `PutRumMetricsDestination` creates-or-updates and what
/// `DeleteRumMetricsDestination` names — both take exactly `Destination` plus the
/// optional `DestinationArn` — so it is the key the destinations map is held under.
pub type MetricDestinationKey = (String, String);

/// The [`MetricDestinationKey`] for `destination` and `destination_arn`, with an absent
/// or empty ARN normalised to the same key, so one destination cannot be stored twice
/// under two spellings of "no ARN".
pub fn metric_destination_key(
    destination: &str,
    destination_arn: Option<&str>,
) -> MetricDestinationKey {
    (
        destination.to_string(),
        destination_arn.unwrap_or_default().to_string(),
    )
}

/// One persisted extended-metrics destination of an app monitor.
///
/// Mirrors the model's `MetricDestinationSummary`, which is the only shape any operation
/// reports a destination through, with `Destination` held non-optional: a stored
/// destination always has one (it is `@required` on the put).
#[derive(Debug, Clone)]
pub struct MetricDestinationRecord {
    /// `Destination`, a member of [`crate::ops::types::METRIC_DESTINATION_VALUES`].
    pub destination: String,
    /// `DestinationArn`: the Evidently experiment receiving the metrics, and `None` for
    /// a `CloudWatch` destination, which the model says must carry no ARN.
    pub destination_arn: Option<String>,
    /// `IamRoleArn`: the role RUM assumes to write to that experiment. `None` for a
    /// `CloudWatch` destination, which the model says must carry no role.
    pub iam_role_arn: Option<String>,
}

impl MetricDestinationRecord {
    /// This record's identity in the destinations map.
    pub fn key(&self) -> MetricDestinationKey {
        metric_destination_key(&self.destination, self.destination_arn.as_deref())
    }

    /// Project the stored record onto the model's `MetricDestinationSummary` — the one
    /// conversion `ListRumMetricsDestinations` goes through, so what a read reports is
    /// exactly what the put wrote.
    ///
    /// EVERY member of the summary is materialised, not only the ones this destination
    /// kind carries a value for: a destination that has no ARN and no role (a
    /// `CloudWatch` one) reports `DestinationArn` and `IamRoleArn` as the EMPTY STRING
    /// rather than leaving them absent, which is the value recorded behaviour reports and
    /// which the model admits — `DestinationArn` is `@length(min: 0, max: 2048)`, and
    /// both members are documented as describing the Evidently case only ("If the
    /// destination is Evidently, this specifies the ARN of the Evidently experiment…",
    /// "This field appears only when the destination is Evidently"). So a client reading
    /// either member to decide whether a destination is Evidently-backed finds a defined
    /// empty value instead of a missing key.
    pub fn to_summary(&self) -> MetricDestinationSummary {
        MetricDestinationSummary {
            destination: Some(self.destination.clone()),
            destination_arn: Some(self.destination_arn.clone().unwrap_or_default()),
            iam_role_arn: Some(self.iam_role_arn.clone().unwrap_or_default()),
        }
    }
}

/// The resource-based policy attached to one app monitor.
///
/// `PutResourcePolicy` documents "Each app monitor can have one resource-based policy",
/// so a monitor holds at most one of these, and it lives ON the monitor's record: a
/// policy cannot outlive the resource whose access it controls, so `DeleteAppMonitor`
/// takes it away with the monitor. It is also what the authorization evaluation consults
/// beside the caller's identity-based policies ([`authorization`]).
#[derive(Debug, Clone)]
pub struct ResourcePolicyRecord {
    /// `PolicyDocument` in the canonical form the service stores it in
    /// ([`canonical_policy_document`]): the submitted document's content, with the
    /// whitespace between its JSON tokens dropped. Everything a caller controls inside
    /// the document — its elements, their order, their values — is kept as written, so
    /// what a read reports is the policy that was written.
    pub document: String,
    /// This revision's number: 1 for the first policy revision an app monitor ever
    /// stores and one more than the previous revision for every later successful write,
    /// which is what makes a revision ORDERED — a client can tell which of two
    /// revisions it holds is the later one. Rendered on the wire by
    /// [`Self::revision_id`]; written only through
    /// [`AppMonitorRecord::store_resource_policy`], so no two revisions of one monitor's
    /// policy can share a number.
    pub revision: u64,
}

impl ResourcePolicyRecord {
    /// `PolicyRevisionId` as the wire carries it: the revision number in decimal.
    ///
    /// `PolicyRevisionId` is modelled as a 1..255-character string with no pattern, and
    /// the value is the SERVICE's — a caller only ever passes one back to make a
    /// conditional request. The decimal rendering of the revision number fits those
    /// bounds for every number a `u64` holds.
    pub fn revision_id(&self) -> String {
        self.revision.to_string()
    }
}

/// One persisted app monitor.
///
/// Mirrors the model's `AppMonitor` structure, but with the members the service always
/// knows held as non-optional: a stored monitor always has a name, an id, an ARN, both
/// timestamps, a state, a platform, a `CwLogEnabled` flag, a configuration and a custom
/// events setting. `Domain`/`DomainList` and `DeobfuscationConfiguration` stay optional
/// because a request may omit them.
#[derive(Debug, Clone)]
pub struct AppMonitorRecord {
    pub name: String,
    /// The minted `AppMonitorId`: a UUID, as `AppMonitorId`'s 36-character pattern asks.
    pub id: String,
    /// `arn:aws:rum:<region>:<account>:appmonitor/<name>`, derived once at create time.
    /// It is the identity the tagging operations name a resource by: `ListTagsForResource`
    /// and `UntagResource` resolve a `ResourceArn` against this ONE stored value (see
    /// [`ComAmazonawsRumRumState::app_monitor_by_arn`] /
    /// [`ComAmazonawsRumRumState::app_monitor_by_arn_mut`]) rather than minting their own,
    /// so a tag call and a read of the monitor cannot disagree about which resource is
    /// meant. (`TagResource` is still scaffolded.)
    pub arn: String,
    pub domain: Option<String>,
    pub domain_list: Option<Vec<String>>,
    /// `Created` as the modelled 19-character `ISOTimestampString`.
    pub created: String,
    /// `LastModified` as the modelled 19-character `ISOTimestampString`, or `None` while
    /// this monitor has only ever been created.
    ///
    /// The member is "the date and time of the most recent changes to this app monitor's
    /// configuration", so a monitor nothing has changed yet HAS no such instant and the
    /// member is absent from every projection of it; it is set by the operations that
    /// change the configuration. Its presence is therefore the answer to "has this
    /// monitor been modified since it was created?".
    pub last_modified: Option<String>,
    /// `State`, a member of [`crate::ops::types::STATE_ENUM_VALUES`].
    pub state: String,
    /// `Platform`, a member of [`crate::ops::types::APP_MONITOR_PLATFORM_VALUES`].
    pub platform: String,
    /// Whether RUM copies this monitor's telemetry into CloudWatch Logs; what
    /// `DataStorage.CwLog` reports.
    pub cw_log_enabled: bool,
    pub app_monitor_configuration: AppMonitorConfiguration,
    pub custom_events: CustomEvents,
    /// `DeobfuscationConfiguration`, materialised for every monitor: a create that named
    /// none stores [`default_deobfuscation_configuration`] rather than leaving the member
    /// unset, so a read always reports the structure and its status.
    pub deobfuscation_configuration: DeobfuscationConfiguration,
    /// This monitor's tags: the map `CreateAppMonitor` seeds and `UntagResource` removes
    /// keys from, what `GetAppMonitor` reports as `AppMonitor$Tags` and what
    /// `ListTagsForResource` reports as its own `Tags`. A `BTreeMap` so the stored record
    /// iterates in one stable key order.
    pub tags: BTreeMap<String, String>,
    /// Events delivered by `PutRumEvents`, oldest first. Bounded by
    /// [`MAX_EVENTS_PER_APP_MONITOR`]; append through [`Self::record_events`].
    pub events: VecDeque<StoredRumEvent>,
    /// The extended-metrics destinations `PutRumMetricsDestination` created or updated
    /// for this monitor, keyed by [`MetricDestinationKey`]. A `BTreeMap` so
    /// `ListRumMetricsDestinations` pages in a stable, resumable order. They live ON the
    /// monitor record because a destination belongs to one app monitor: deleting the
    /// monitor drops its destinations with it. Bounded by
    /// [`MAX_METRIC_DESTINATIONS_PER_APP_MONITOR`].
    pub metric_destinations: BTreeMap<MetricDestinationKey, MetricDestinationRecord>,
    /// This monitor's one resource-based policy, or `None` while it carries none — the
    /// state `GetResourcePolicy` and `DeleteResourcePolicy` answer
    /// `PolicyNotFoundException` for, and what the authorization evaluation reads beside
    /// the caller's identity-based policies.
    pub resource_policy: Option<ResourcePolicyRecord>,
    /// How many resource-policy revisions this monitor has ever stored: the source of
    /// the next revision number ([`Self::store_resource_policy`]). It counts WRITES, not
    /// the policy currently held, so it is not reset by `DeleteResourcePolicy` — a
    /// revision id a caller read before a delete must never pass the conditional check
    /// of a policy stored after it.
    pub policy_revisions: u64,
}

impl AppMonitorRecord {
    /// The name of the CloudWatch Logs group this monitor's data is copied into, or
    /// `None` when `CwLogEnabled` is false — a monitor that stores no copy has no group
    /// (`CwLog$CwLogGroup` is "The name of the log group where the copies are stored").
    pub fn cw_log_group(&self) -> Option<String> {
        self.cw_log_enabled.then(|| cw_log_group_name(&self.name))
    }

    /// The resource policy document this monitor carries, for the authorization
    /// evaluation to consult.
    pub fn resource_policy_document(&self) -> Option<String> {
        self.resource_policy
            .as_ref()
            .map(|policy| policy.document.clone())
    }

    /// This monitor's tags as the model's `TagMap` — the ONE projection every operation
    /// that reports tags (`GetAppMonitor` inside `AppMonitor$Tags`,
    /// `ListTagsForResource` as its own `Tags` member) goes through, so two readers
    /// cannot report one monitor's tags differently.
    pub fn tag_map(&self) -> HashMap<String, String> {
        self.tags
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect()
    }

    /// Project the stored record onto the model's `AppMonitor` shape — the one
    /// conversion `GetAppMonitor` goes through, so what a read reports is exactly what
    /// the create and the updates wrote.
    pub fn to_app_monitor(&self) -> AppMonitor {
        AppMonitor {
            name: Some(self.name.clone()),
            domain: self.domain.clone(),
            domain_list: self.domain_list.clone(),
            id: Some(self.id.clone()),
            created: Some(self.created.clone()),
            last_modified: self.last_modified.clone(),
            tags: Some(self.tag_map()),
            state: Some(self.state.clone()),
            app_monitor_configuration: Some(self.app_monitor_configuration.clone()),
            data_storage: Some(DataStorage {
                cw_log: Some(CwLog {
                    cw_log_enabled: Some(self.cw_log_enabled),
                    cw_log_group: self.cw_log_group(),
                }),
            }),
            custom_events: Some(self.custom_events.clone()),
            deobfuscation_configuration: Some(self.deobfuscation_configuration.clone()),
            platform: Some(self.platform.clone()),
        }
    }

    /// Project the stored record onto the model's `AppMonitorSummary` — the subset
    /// `ListAppMonitors` reports, read off the same record `GetAppMonitor` reads.
    pub fn to_summary(&self) -> AppMonitorSummary {
        AppMonitorSummary {
            name: Some(self.name.clone()),
            id: Some(self.id.clone()),
            created: Some(self.created.clone()),
            last_modified: self.last_modified.clone(),
            state: Some(self.state.clone()),
            platform: Some(self.platform.clone()),
        }
    }

    /// Remove the destination the `(Destination, DestinationArn)` pair
    /// `destination`/`destination_arn` names — the identity
    /// `DeleteRumMetricsDestination` addresses one by. `None` when this monitor holds no
    /// such destination, which the caller renders as `ResourceNotFoundException`. Keyed
    /// through [`metric_destination_key`], so the delete, the put and the listing cannot
    /// disagree about which destination a request means.
    pub fn remove_metric_destination(
        &mut self,
        destination: &str,
        destination_arn: Option<&str>,
    ) -> Option<MetricDestinationRecord> {
        self.metric_destinations
            .remove(&metric_destination_key(destination, destination_arn))
    }

    /// The revision id of this monitor's stored resource policy, or `None` while it
    /// carries no policy.
    pub fn policy_revision_id(&self) -> Option<String> {
        self.resource_policy
            .as_ref()
            .map(ResourcePolicyRecord::revision_id)
    }

    /// Store `document` as this monitor's resource policy, as the NEXT revision of it,
    /// and report the record just written.
    ///
    /// The ONE writer of [`Self::resource_policy`] and of [`Self::policy_revisions`], so
    /// every accepted `PutResourcePolicy` advances the revision by exactly one: the first
    /// policy a monitor stores is revision 1, the write that replaces it is revision 2,
    /// and so on. The document is stored in its canonical form
    /// ([`canonical_policy_document`]): its content, with the caller's insignificant
    /// whitespace dropped.
    pub fn store_resource_policy(&mut self, document: &str) -> &ResourcePolicyRecord {
        self.policy_revisions = self.policy_revisions.saturating_add(1);
        let stored = ResourcePolicyRecord {
            document: canonical_policy_document(document),
            revision: self.policy_revisions,
        };
        self.resource_policy.insert(stored)
    }

    /// Why a request carrying `provided` as its `PolicyRevisionId` cannot act on this
    /// monitor's policy, or `None` when it can.
    ///
    /// From the model (`PutResourcePolicyRequest$PolicyRevisionId`): "You can provide the
    /// revision ID of your existing policy to make mutating requests against that policy
    /// … later requests about that policy will be rejected with an
    /// InvalidPolicyRevisionIdException error if they don't provide the correct current
    /// revision ID." So a request that names a revision id is CONDITIONAL on it being
    /// the current one — including when there is no policy at all, where no id can be the
    /// current one — and a request that names none is unconditional. Shared by
    /// `PutResourcePolicy` and `DeleteResourcePolicy`, which the model gives the same
    /// member and the same error.
    pub fn policy_revision_refusal(
        &self,
        provided: Option<&str>,
    ) -> Option<InvalidPolicyRevisionIdException> {
        let provided = provided?;
        (self.policy_revision_id().as_deref() != Some(provided))
            .then(|| policy_revision_mismatch(provided))
    }

    /// Append events to this monitor's buffer, dropping the oldest once the buffer is at
    /// [`MAX_EVENTS_PER_APP_MONITOR`].
    pub fn record_events(&mut self, events: impl IntoIterator<Item = StoredRumEvent>) {
        for event in events {
            while self.events.len() >= MAX_EVENTS_PER_APP_MONITOR {
                self.events.pop_front();
            }
            self.events.push_back(event);
        }
    }
}

/// Per-service state for com-amazonaws-rum-rum. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsRumRumState {
    /// Every app monitor that currently exists, keyed by its `Name` (unique per
    /// account). A `BTreeMap` so `ListAppMonitors` pages in a stable, resumable order.
    /// Bounded by [`MAX_APP_MONITORS`]; write it through [`Self::insert_app_monitor`] /
    /// [`Self::remove_app_monitor`] so the id index below cannot drift from it.
    pub app_monitors: BTreeMap<String, AppMonitorRecord>,
    /// Secondary index `Id` -> `Name`, so the data-plane `PutRumEvents` — which names a
    /// monitor by its minted id, not its name — resolves one in O(1) without scanning.
    pub monitor_ids: HashMap<String, String>,
    /// Per-state seed the minted ids are derived from, set on the first mint from the
    /// nanosecond clock so two runs of the emulator do not hand out the same ids. Only
    /// ever read by [`Self::mint_app_monitor_id`]; it is NOT an id itself.
    id_seed: u64,
    /// Monotonic counter making each minted id unique within this state. Same note: not
    /// an id, and never reported.
    next_id_seq: u64,
}

impl ComAmazonawsRumRumState {
    /// Mint a fresh `AppMonitorId` that no stored monitor holds.
    ///
    /// The modelled shape is a UUID —
    /// `^[a-fA-F0-9]{8}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{12}$`,
    /// length 36..36 — so the id is 16 bytes of a SHA-256 digest of the clock-derived
    /// seed and the counter, laid out as a version-4, RFC 4122 variant UUID (the version
    /// and variant nibbles are forced, as a v4 generator does). The digest gives ids no
    /// caller can guess from a previous one, the seed keeps ids from different runs
    /// apart, the counter keeps ids within one run unique, and the loop re-draws rather
    /// than ever handing back an id already in use. The caller must store the record
    /// under the returned id while still holding the same state lock, or the id it
    /// returned names nothing.
    pub fn mint_app_monitor_id(&mut self) -> String {
        if self.id_seed == 0 {
            self.id_seed = nanos_since_epoch() as u64;
        }
        loop {
            self.next_id_seq = self.next_id_seq.wrapping_add(1);
            let candidate = uuid_v4_shaped(APP_MONITOR_ID_DOMAIN, self.id_seed, self.next_id_seq);
            if !self.monitor_ids.contains_key(&candidate) {
                return candidate;
            }
        }
    }

    /// Store `record`, keeping the `Id` -> `Name` index in step with the primary map.
    pub fn insert_app_monitor(&mut self, record: AppMonitorRecord) {
        self.monitor_ids
            .insert(record.id.clone(), record.name.clone());
        self.app_monitors.insert(record.name.clone(), record);
    }

    /// Remove the monitor named `name` — with its tags and its event buffer, which live
    /// on the record — and drop its id from the index. `None` when no monitor has that
    /// name, which every caller renders as `ResourceNotFoundException`.
    pub fn remove_app_monitor(&mut self, name: &str) -> Option<AppMonitorRecord> {
        let removed = self.app_monitors.remove(name)?;
        self.monitor_ids.remove(&removed.id);
        Some(removed)
    }

    /// The stored monitor whose minted id is `id`, for a caller that mutates it (the
    /// data-plane `PutRumEvents`). The id index and the primary map are kept in step by
    /// the two writers above, so this is the ONE place the two-step lookup lives.
    pub fn app_monitor_by_id_mut(&mut self, id: &str) -> Option<&mut AppMonitorRecord> {
        let name = self.monitor_ids.get(id)?.clone();
        self.app_monitors.get_mut(&name)
    }

    /// The same lookup for a caller that only READS the monitor — the authorization
    /// evaluation, which needs the monitor's ARN and its resource policy before any write
    /// is decided on.
    pub fn app_monitor_by_id(&self, id: &str) -> Option<&AppMonitorRecord> {
        let name = self.monitor_ids.get(id)?;
        self.app_monitors.get(name)
    }

    /// The stored monitor whose ARN is exactly `arn`, for a caller that mutates it (the
    /// tagging operations, which name a resource by `ResourceArn` and by nothing else).
    ///
    /// The comparison is against [`AppMonitorRecord::arn`] in full — partition, service,
    /// region, account and resource segment — because that whole string is the resource's
    /// identity: an ARN that differs in any component names a DIFFERENT resource, which
    /// this emulator does not hold, so the caller renders `None` as
    /// `ResourceNotFoundException` (see [`tagged_resource_not_found`]). App monitors are
    /// the only taggable RUM resource (`TagResource`: "Currently, the only resources that
    /// can be tagged app monitors"), so this one lookup covers every ARN a tag call can
    /// name.
    pub fn app_monitor_by_arn_mut(&mut self, arn: &str) -> Option<&mut AppMonitorRecord> {
        self.app_monitors
            .values_mut()
            .find(|record| record.arn == arn)
    }

    /// The stored monitor whose ARN is exactly `arn`, for a caller that only REPORTS it
    /// (`ListTagsForResource`).
    ///
    /// The same identity comparison as [`Self::app_monitor_by_arn_mut`] — the two differ
    /// only in mutability, so a tag read and a tag write can never disagree about which
    /// resource one `ResourceArn` names.
    pub fn app_monitor_by_arn(&self, arn: &str) -> Option<&AppMonitorRecord> {
        self.app_monitors.values().find(|record| record.arn == arn)
    }
}

/// The current nanosecond clock, the seed [`ComAmazonawsRumRumState::mint_app_monitor_id`]
/// draws from once per state.
fn nanos_since_epoch() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

/// The id space [`ComAmazonawsRumRumState::mint_app_monitor_id`] draws from.
const APP_MONITOR_ID_DOMAIN: &str = "rum/appmonitor";

/// One `(domain, seed, sequence)` triple as a canonical, lowercase, version-4-shaped
/// UUID string.
///
/// The 16 bytes come from a SHA-256 digest of the triple, then byte 6's high nibble is
/// set to `4` (the version) and byte 8's two high bits to `10` (the RFC 4122 variant),
/// which is exactly the layout a v4 generator writes. Distinct triples give distinct
/// ids: the digest is injective in practice and the sequence is strictly increasing.
/// `domain` keeps one kind of id from ever equalling another drawn from the same seed
/// and counter.
fn uuid_v4_shaped(domain: &str, seed: u64, sequence: u64) -> String {
    use sha2::{Digest as _, Sha256};

    let digest = Sha256::digest(format!("{domain}/{seed}/{sequence}").as_bytes());
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// The ARN of the app monitor named `name` in `region`.
pub fn app_monitor_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("{APP_MONITOR_RESOURCE}/{name}"),
    )
}

/// The ARN of the account's whole app-monitor collection in `region`:
/// `arn:aws:rum:<region>:<account>:appmonitor/*`.
///
/// What an operation that acts on the collection rather than on one monitor
/// (`ListAppMonitors`) is authorized against, so a policy scoped to one monitor's ARN
/// does not also grant the listing.
pub fn app_monitor_collection_arn(region: &str) -> String {
    app_monitor_arn(region, "*")
}

/// The region this request was signed for, or [`DEFAULT_REGION`] when it carries no
/// SigV4 credential scope.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// An instant as the modelled `ISOTimestampString`: `YYYY-MM-DDTHH:MM:SS`, exactly the
/// 19 characters that shape's `@length(min: 19, max: 19)` admits — RFC 3339 truncated to
/// whole seconds and with no zone suffix, which is how RUM renders `Created` and
/// `LastModified`.
pub fn iso_timestamp(at: SystemTime) -> String {
    let seconds = at
        .duration_since(UNIX_EPOCH)
        .map(|since_epoch| i64::try_from(since_epoch.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or_default();
    let rendered = ::aws_smithy_types::DateTime::from_secs(seconds)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default();
    let trimmed = rendered.strip_suffix('Z').unwrap_or(rendered.as_str());
    if trimmed.len() == 19 {
        trimmed.to_string()
    } else {
        // Only reachable for an instant RFC 3339 cannot render (a clock outside year
        // 0001..9999). The shape admits 19 characters and nothing else, so answer the
        // epoch rather than a string no client can parse.
        "1970-01-01T00:00:00".to_string()
    }
}

/// Now, as the modelled `ISOTimestampString`.
pub fn current_iso_timestamp() -> String {
    iso_timestamp(SystemTime::now())
}

/// The configuration an app monitor gets when its create request carried none: every
/// member unset except the documented 10% session sample rate.
pub fn default_app_monitor_configuration() -> AppMonitorConfiguration {
    AppMonitorConfiguration {
        session_sample_rate: DEFAULT_SESSION_SAMPLE_RATE,
        ..AppMonitorConfiguration::default()
    }
}

/// The custom-events setting an app monitor gets when its create request carried none.
pub fn default_custom_events() -> CustomEvents {
    CustomEvents {
        status: Some(DEFAULT_CUSTOM_EVENTS_STATUS.to_string()),
    }
}

/// The deobfuscation configuration an app monitor gets when its create request carried
/// none: source-map deobfuscation off, and no bucket to read maps from.
///
/// `DeobfuscationConfiguration` is `{ JavaScriptSourceMaps }` and
/// `JavaScriptSourceMaps$Status` is `@required` (a member of
/// [`crate::ops::types::DEOBFUSCATION_STATUS_VALUES`]), so there is no such thing as a
/// configuration that leaves the status open: a monitor created without one is created
/// with deobfuscation DISABLED, and recorded behaviour reports exactly that structure
/// rather than omitting the member. Which is also why a read never has to decide: the
/// record always holds one ([`AppMonitorRecord::deobfuscation_configuration`]).
pub fn default_deobfuscation_configuration() -> DeobfuscationConfiguration {
    DeobfuscationConfiguration {
        java_script_source_maps: Some(JavaScriptSourceMaps {
            status: DEFAULT_DEOBFUSCATION_STATUS.to_string(),
            s3_uri: None,
        }),
    }
}

/// Why this request's `Domain`/`DomainList` pair cannot be applied, or `None` when it
/// can.
///
/// From the model: `CreateAppMonitorRequest$DomainList` documents "The CreateAppMonitor
/// requires either the domain or the domain list" and
/// `UpdateAppMonitorRequest$DomainList` documents "The UpdateAppMonitor allows either the
/// domain or the domain list" — a monitor's domain authority is held in exactly ONE of
/// the two forms. Neither constraint is expressible in the model, so every operation that
/// accepts the pair refuses it here with the `ValidationException` it declares.
/// `required` is true for the operation that must end up with one (a create of a monitor
/// whose platform HAS an internet domain — see [`platform_requires_domain`]), false for
/// one that may leave the stored choice alone (an update) or for a monitor that has no
/// domain to name.
pub fn domain_choice_refusal(
    domain: Option<&str>,
    domain_list: Option<&[String]>,
    required: bool,
) -> Option<String> {
    match (domain, domain_list) {
        (Some(_), Some(_)) => Some(
            "Domain and DomainList cannot both be specified; name either the domain or \
             the domain list."
                .to_string(),
        ),
        (None, None) if required => {
            Some("Either Domain or DomainList must be specified.".to_string())
        }
        _ => None,
    }
}

/// Whether an app monitor created for `platform` must name its domain authority.
///
/// From the model: `CreateAppMonitorRequest$DomainList` documents "The CreateAppMonitor
/// requires either the domain or the domain list", and `Domain` is "the top-level
/// internet domain name for which your application has administrative authority". An
/// application on a platform other than [`WEB_PLATFORM`] — an Android or an iOS
/// application — is not served from an internet domain, so there is no domain authority
/// for it to hold and the requirement applies to the web platform alone. Recorded
/// behaviour agrees: a monitor created for a mobile platform with neither member is
/// accepted and read back carrying neither.
///
/// The OTHER half of the rule is platform-independent: whichever platform a monitor is
/// for, its domain authority is held in at most one of the two forms, so naming both is
/// refused for every platform ([`domain_choice_refusal`]).
pub fn platform_requires_domain(platform: &str) -> bool {
    platform == WEB_PLATFORM
}

/// Why this `AppMonitorConfiguration` cannot be applied, or `None` when it can.
///
/// From the model: `AppMonitorConfiguration$IncludedPages` and `$ExcludedPages` both
/// document "You can't include both `ExcludedPages` and `IncludedPages` in the same
/// operation", a rule no modelled constraint expresses, so every operation that accepts
/// a configuration refuses it here with the `ValidationException` it declares.
pub fn app_monitor_configuration_refusal(config: &AppMonitorConfiguration) -> Option<String> {
    let includes = config.included_pages.as_ref().is_some_and(|p| !p.is_empty());
    let excludes = config.excluded_pages.as_ref().is_some_and(|p| !p.is_empty());
    (includes && excludes).then(|| {
        "AppMonitorConfiguration cannot carry both IncludedPages and ExcludedPages."
            .to_string()
    })
}

/// Why this `Destination`/`DestinationArn` pair cannot be applied, or `None` when it can.
///
/// From the model: `PutRumMetricsDestinationRequest$DestinationArn` documents "Use this
/// parameter only if Destination is Evidently", the operation itself documents "If you
/// specify Evidently, you must also specify the ARN of the CloudWatchEvidently experiment
/// that is to be the destination", and `DeleteRumMetricsDestinationRequest$DestinationArn`
/// states both halves outright: "This parameter is required if Destination is Evidently.
/// If Destination is CloudWatch, do not use this parameter." So an Evidently destination
/// is identified by an experiment ARN and a CloudWatch one carries none. Neither half is
/// expressible as a modelled constraint, so every operation that accepts the pair refuses
/// it here with the `ValidationException` it declares.
///
/// The `Destination` value itself is checked against
/// [`crate::ops::types::METRIC_DESTINATION_VALUES`] first, so an operation stays correct
/// on a protocol whose boundary does not enforce the enum.
pub fn metric_destination_choice_refusal(
    destination: &str,
    destination_arn: Option<&str>,
) -> Option<String> {
    if !METRIC_DESTINATION_VALUES.contains(&destination) {
        return Some(format!(
            "Destination {destination} is not valid; valid values are {}.",
            METRIC_DESTINATION_VALUES.join(" and ")
        ));
    }
    match (destination, destination_arn) {
        (EVIDENTLY_DESTINATION, None) => Some(format!(
            "DestinationArn is required when Destination is {EVIDENTLY_DESTINATION}."
        )),
        (EVIDENTLY_DESTINATION, Some(_)) => None,
        (_, Some(_)) => Some(format!(
            "DestinationArn cannot be used when Destination is {destination}."
        )),
        (_, None) => None,
    }
}

/// Why this `Destination`/`IamRoleArn` pair cannot be applied, or `None` when it can.
///
/// From the model: `PutRumMetricsDestinationRequest$IamRoleArn` documents "This parameter
/// is required if Destination is Evidently. If Destination is CloudWatch, don't use this
/// parameter." — RUM needs a role to assume only when it writes to someone else's
/// experiment. Same reasoning as [`metric_destination_choice_refusal`]: the rule is prose
/// in the model, so the handler enforces it.
pub fn metric_destination_role_refusal(
    destination: &str,
    iam_role_arn: Option<&str>,
) -> Option<String> {
    match (destination, iam_role_arn) {
        (EVIDENTLY_DESTINATION, None) => Some(format!(
            "IamRoleArn is required when Destination is {EVIDENTLY_DESTINATION}."
        )),
        (EVIDENTLY_DESTINATION, Some(_)) => None,
        (_, Some(_)) => Some(format!(
            "IamRoleArn cannot be used when Destination is {destination}."
        )),
        (_, None) => None,
    }
}

/// `ResourceNotFoundException` for a name or id that names no stored app monitor.
///
/// The message is built from the request: the literal prefix `Resource not found `
/// followed by the value the request identified the monitor by, verbatim and with no
/// trailing period. That construction is RUM's own, and it is the same for every
/// operation that resolves an app monitor — recorded across this service's app-monitor
/// reads and writes, its metrics-destination calls and its resource-policy calls. The
/// body carries NOTHING else: `resourceName`/`resourceType` are off the wire for this
/// shape (see [`crate::ops::types::ResourceNotFoundException`]).
pub fn app_monitor_not_found(name_or_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: format!("{NOT_FOUND_MESSAGE_PREFIX}{name_or_id}"),
    }
}

/// The literal the app-monitor not-found message starts with, ahead of the requested
/// name or id.
pub const NOT_FOUND_MESSAGE_PREFIX: &str = "Resource not found ";

/// `ResourceNotFoundException` for a destination the named app monitor does not have —
/// what `DeleteRumMetricsDestination` answers for a `(Destination, DestinationArn)` pair
/// the monitor holds no record under.
///
/// The message is built from the request: the destination kind it named, and the Evidently
/// experiment ARN only where there is one to name. Like every other not-found body of this
/// service it carries `message` ALONE — `resourceName`/`resourceType` are off the wire for
/// this shape (see [`crate::ops::types::ResourceNotFoundException`]).
pub fn metric_destination_not_found(
    app_monitor_name: &str,
    destination: &str,
    destination_arn: Option<&str>,
) -> ResourceNotFoundException {
    let message = match destination_arn {
        Some(arn) => format!(
            "App monitor {app_monitor_name} has no {destination} metrics destination {arn}."
        ),
        None => {
            format!("App monitor {app_monitor_name} has no {destination} metrics destination.")
        }
    };
    ResourceNotFoundException { message }
}

/// The name of the CloudWatch Logs group an app monitor's telemetry is copied into.
///
/// One definition, used by the record's own [`AppMonitorRecord::cw_log_group`] (what
/// `GetAppMonitor` reports) and by the handlers that PROVISION the group in CloudWatch
/// Logs when delivery is turned on — so the name RUM advertises and the group it creates
/// cannot differ.
pub fn cw_log_group_name(app_monitor_name: &str) -> String {
    format!("{CW_LOG_GROUP_PREFIX}{app_monitor_name}")
}

/// The ARN of that log group, which is the resource the caller's
/// `logs:PutResourcePolicy` permission is evaluated against.
///
/// A CloudWatch Logs log-group ARN ends in `:*`, the form CloudWatch Logs itself reports
/// (`service-com-amazonaws-cloudwatchlogs-logs-20140328`'s `log_group_arn`).
pub fn cw_log_group_arn(region: &str, log_group_name: &str) -> String {
    ::engine::emulator_arn("logs", region, &format!("log-group:{log_group_name}:*"))
}

/// `AccessDeniedException` for a request IAM does not allow.
///
/// The message is the refusal's own wording — the principal, the action and the resource
/// the decision was made about — so a client can tell "this caller may not do this" from
/// any other 4xx.
pub fn access_denied(denial: &authorization::Denied) -> AccessDeniedException {
    AccessDeniedException {
        message: denial.message(),
    }
}

/// The whole message RUM's ARN-addressed tagging operations report for a resource they
/// cannot find.
///
/// Unlike the app-monitor operations' not-found message, which is built from the request
/// (it names the `Name` asked for), this one carries NOTHING from the request: the
/// recorded corpus unit `rum.untag-resource-unknown-resource-rejected` pins the message
/// for an unknown ARN as this exact sentence, and it is the same sentence whatever ARN the
/// request carried. So it is a constant here rather than a format string.
pub const TAGGED_RESOURCE_NOT_FOUND_MESSAGE: &str = "Resource was not found";

/// The HTTP status RUM's ARN-addressed tag WRITES answer on success: 204 No Content.
///
/// `TagResource` and `UntagResource` are the two operations of this service whose
/// modelled output declares no member at all, and both are served by the tagging
/// frontend: there is nothing for the answer to carry, and the status the service sends
/// says so. The AWS CloudWatch RUM API reference gives `HTTP/1.1 204` as the response
/// syntax of both, and every recorded response of either in the corpus is a 204 with no
/// content (eleven steps across five fixtures). The model's own `@http.code` is 200 for
/// both — defaulted on `TagResource`, declared on `UntagResource`
/// (`smithy_query(query="operation", operation="TagResource,UntagResource")`) — but a
/// status is what the service PUTS ON THE WIRE, and a client that distinguishes 204 from
/// 200 (empty-body handling, a cache, an assertion on the success class) sees the
/// difference, so the recorded behaviour is what the two handlers answer with.
///
/// `ListTagsForResource`, the frontend's third operation, reports a document and keeps
/// the 200 its own output needs; every other operation of this service is addressed by
/// `Name` or by `Id`, reports a document, and keeps its modelled status.
pub const TAGGING_NO_CONTENT_STATUS: u16 = 204;

/// `ResourceNotFoundException` for a `ResourceArn` that names no resource this service
/// holds — the not-found refusal every tagging operation answers (HTTP 404).
///
/// `message` is the service's own constant wording ([`TAGGED_RESOURCE_NOT_FOUND_MESSAGE`])
/// and is the whole body: like every other not-found of this service, the refusal reports
/// no `resourceName` and no `resourceType` (see
/// [`crate::ops::types::ResourceNotFoundException`]). The ARN is therefore taken only to
/// decide that nothing matched it, which the caller already knows it sent.
pub fn tagged_resource_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: TAGGED_RESOURCE_NOT_FOUND_MESSAGE.to_string(),
    }
}

/// The stored form of a `PolicyDocument`: the submitted JSON text with every
/// insignificant whitespace character removed.
///
/// The service parses the document it is given (it refuses one that is not a well-formed
/// policy, see [`policy_document_refusal`]) and keeps the parse's canonical text, so what
/// a later read reports is the policy's content and not the caller's layout: whitespace
/// between JSON tokens carries no meaning. Nothing INSIDE the document is touched —
/// whitespace in a string literal is content, element order, numbers and escapes are kept
/// exactly as written — so the document still holds the same statements, in the same
/// order, as the caller wrote them.
pub fn canonical_policy_document(document: &str) -> String {
    let mut canonical = String::with_capacity(document.len());
    // Whether the scanner is inside a JSON string literal, and whether the previous
    // character opened an escape sequence inside one (so an escaped quote does not end
    // the literal).
    let mut in_string = false;
    let mut escaped = false;
    for character in document.chars() {
        if in_string {
            canonical.push(character);
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }
            continue;
        }
        if character == '"' {
            in_string = true;
            canonical.push(character);
        } else if !character.is_whitespace() {
            canonical.push(character);
        }
    }
    canonical
}

/// Why this `PolicyDocument` is not a resource policy this service can store, or `None`
/// when it is one.
///
/// `PolicyDocument` is modelled as a bare `String`, so NOTHING about its contents is
/// refused before a handler runs; `MalformedPolicyDocumentException` — "The policy
/// document that you specified is not formatted correctly" — is the modelled refusal for
/// a document that is not a well-formed IAM JSON policy. The grammar checked here is the
/// IAM JSON policy language's own: the document parses as JSON (by `serde_json`, this
/// crate's JSON parser) into an object whose only elements are
/// [`POLICY_DOCUMENT_ELEMENTS`], it carries at least one statement, and every statement
/// element has the type and the value set that language gives it. Semantics beyond the
/// grammar (whether an action exists, whether a principal resolves) are NOT checked:
/// they are not what "formatted correctly" means.
pub fn policy_document_refusal(document: &str) -> Option<String> {
    let parsed = match ::serde_json::from_str::<::serde_json::Value>(document) {
        Ok(parsed) => parsed,
        Err(error) => {
            return Some(format!(
                "The policy document is not valid JSON: {error}."
            ))
        }
    };
    let Some(elements) = parsed.as_object() else {
        return Some("The policy document must be a JSON object.".to_string());
    };
    if let Some(unknown) = elements
        .keys()
        .find(|key| !POLICY_DOCUMENT_ELEMENTS.contains(&key.as_str()))
    {
        return Some(format!(
            "The policy document carries the unknown element {unknown}; a policy \
             document's elements are {}.",
            POLICY_DOCUMENT_ELEMENTS.join(", ")
        ));
    }
    if let Some(version) = elements.get("Version") {
        let declared = version.as_str();
        if !declared.is_some_and(|value| POLICY_DOCUMENT_VERSIONS.contains(&value)) {
            return Some(format!(
                "The policy document's Version must be one of {}.",
                POLICY_DOCUMENT_VERSIONS.join(", ")
            ));
        }
    }
    if elements.get("Id").is_some_and(|id| !id.is_string()) {
        return Some("The policy document's Id must be a string.".to_string());
    }
    let statements = match elements.get("Statement") {
        None => {
            return Some("The policy document must carry a Statement element.".to_string())
        }
        // A single statement may be given as the object itself rather than as a
        // one-element list.
        Some(::serde_json::Value::Object(statement)) => {
            vec![::serde_json::Value::Object(statement.clone())]
        }
        Some(::serde_json::Value::Array(statements)) if !statements.is_empty() => {
            statements.clone()
        }
        Some(_) => {
            return Some(
                "The policy document's Statement must be a statement object or a \
                 non-empty list of them."
                    .to_string(),
            )
        }
    };
    statements.iter().find_map(policy_statement_refusal)
}

/// Why one element of a policy document's `Statement` is not a well-formed statement, or
/// `None` when it is one.
fn policy_statement_refusal(statement: &::serde_json::Value) -> Option<String> {
    let Some(members) = statement.as_object() else {
        return Some("Each policy statement must be a JSON object.".to_string());
    };
    match members.get("Effect").and_then(::serde_json::Value::as_str) {
        Some(effect) if POLICY_STATEMENT_EFFECTS.contains(&effect) => {}
        _ => {
            return Some(format!(
                "Each policy statement must carry an Effect of {}.",
                POLICY_STATEMENT_EFFECTS.join(" or ")
            ))
        }
    }
    // The IAM policy language takes the action either way round, but never both and
    // never neither: a statement with no action element applies to nothing.
    if members.contains_key("Action") == members.contains_key("NotAction") {
        return Some(
            "Each policy statement must carry exactly one of Action and NotAction.".to_string(),
        );
    }
    for element in ["Action", "NotAction", "Resource", "NotResource"] {
        if let Some(value) = members.get(element) {
            if !is_string_or_non_empty_string_list(value) {
                return Some(format!(
                    "A policy statement's {element} must be a string or a non-empty list \
                     of strings."
                ));
            }
        }
    }
    for element in ["Principal", "NotPrincipal"] {
        if let Some(value) = members.get(element) {
            if !is_policy_principal(value) {
                return Some(format!(
                    "A policy statement's {element} must be a string or a map of \
                     principal type to one or more principals."
                ));
            }
        }
    }
    if members.get("Sid").is_some_and(|sid| !sid.is_string()) {
        return Some("A policy statement's Sid must be a string.".to_string());
    }
    if members
        .get("Condition")
        .is_some_and(|condition| !is_policy_condition(condition))
    {
        return Some(
            "A policy statement's Condition must be a map of condition operator to a map \
             of condition key to its values."
                .to_string(),
        );
    }
    None
}

/// Whether `value` is how the IAM policy language writes one or more strings: the string
/// itself, or a non-empty list of strings.
fn is_string_or_non_empty_string_list(value: &::serde_json::Value) -> bool {
    match value {
        ::serde_json::Value::String(_) => true,
        ::serde_json::Value::Array(entries) => {
            !entries.is_empty() && entries.iter().all(::serde_json::Value::is_string)
        }
        _ => false,
    }
}

/// Whether `value` is a `Principal`/`NotPrincipal` element: the wildcard string, or a map
/// of principal type (`AWS`, `Service`, `Federated`, `CanonicalUser`) to one or more
/// principals.
fn is_policy_principal(value: &::serde_json::Value) -> bool {
    match value {
        ::serde_json::Value::String(_) => true,
        ::serde_json::Value::Object(by_type) => {
            !by_type.is_empty()
                && by_type
                    .values()
                    .all(is_string_or_non_empty_string_list)
        }
        _ => false,
    }
}

/// Whether `value` is a `Condition` element: a map of condition operator to a map of
/// condition key to the value(s) it is compared against.
fn is_policy_condition(value: &::serde_json::Value) -> bool {
    value.as_object().is_some_and(|by_operator| {
        by_operator.values().all(|by_key| {
            by_key
                .as_object()
                .is_some_and(|keys| keys.values().all(is_policy_condition_value))
        })
    })
}

/// Whether `value` is a condition key's right-hand side: a string, a boolean, a number,
/// or a non-empty list of those.
fn is_policy_condition_value(value: &::serde_json::Value) -> bool {
    match value {
        ::serde_json::Value::String(_)
        | ::serde_json::Value::Bool(_)
        | ::serde_json::Value::Number(_) => true,
        ::serde_json::Value::Array(entries) => {
            !entries.is_empty() && entries.iter().all(is_policy_condition_value)
        }
        _ => false,
    }
}

/// The wording RUM answers `PolicyNotFoundException` with.
///
/// Recorded behaviour: the message is this CONSTANT, carrying no part of the request —
/// the monitor the caller asked about is already the one it addressed, so the error says
/// only that the resource policy is not there. (The shape models `message` and nothing
/// else, so the whole body is this one member.)
pub const POLICY_NOT_FOUND_MESSAGE: &str = "Resource policy does not exist";

/// `PolicyNotFoundException` for an app monitor that exists but carries no resource
/// policy.
pub fn policy_not_found() -> PolicyNotFoundException {
    PolicyNotFoundException {
        message: POLICY_NOT_FOUND_MESSAGE.to_string(),
    }
}

/// `InvalidPolicyRevisionIdException` for a conditional request whose `PolicyRevisionId`
/// is not the stored policy's current one.
///
/// From the model: "The policy revision ID that you provided doeesn't match the latest
/// policy revision ID." The message names the id the REQUEST carried; the current one is
/// deliberately not disclosed, since a caller that holds a stale id has to re-read the
/// policy to learn the new one.
pub fn policy_revision_mismatch(provided: &str) -> InvalidPolicyRevisionIdException {
    InvalidPolicyRevisionIdException {
        message: format!(
            "The policy revision ID {provided} does not match the latest policy revision ID."
        ),
    }
}

/// `ConflictException` for a create whose `Name` a stored app monitor already holds.
///
/// Like the not-found message, this one is built from the request: the literal `Resource `,
/// the requested name verbatim, and the literal ` already exists`, with no trailing
/// period. Unlike it, the body also reports the resource — `resourceName` is the name the
/// request asked for and `resourceType` is [`APP_MONITOR_RESOURCE_TYPE`] — because RUM
/// does serialize both members on this error.
pub fn app_monitor_already_exists(name: &str) -> ConflictException {
    ConflictException {
        message: format!("{CONFLICT_MESSAGE_PREFIX}{name}{CONFLICT_MESSAGE_SUFFIX}"),
        resource_name: name.to_string(),
        resource_type: Some(APP_MONITOR_RESOURCE_TYPE.to_string()),
    }
}

/// The literals the duplicate-name conflict message brackets the requested name with.
pub const CONFLICT_MESSAGE_PREFIX: &str = "Resource ";
/// See [`CONFLICT_MESSAGE_PREFIX`].
pub const CONFLICT_MESSAGE_SUFFIX: &str = " already exists";

/// The media type RUM announces for the JSON body of an answer to a request that carried
/// no payload, and so declared no media type of its own.
///
/// RECORDED BEHAVIOUR: the model declares `restJson1`, whose serializer stamps
/// [`JSON_CONTENT_TYPE`] on a JSON body, but a recorded real-RUM answer to a request with
/// no payload — `GetAppMonitor`, `ListAppMonitors`, `DeleteAppMonitor`, the resource-policy
/// reads, `ListRumMetricsDestinations`, their 404s included — carries
/// `application/x-amz-json-1.1`: the frontend answers in the media type the request's
/// payload declared, and a payload-less request declared none, so it falls back to RUM's
/// own JSON type. An answer to a request that DID carry a payload (`CreateAppMonitor`,
/// `UpdateAppMonitor`, `PutResourcePolicy`, `PutRumEvents`,
/// `BatchCreateRumMetricDefinitions`, their 4xxs included) is recorded in
/// [`JSON_CONTENT_TYPE`], the type that payload declared. The body is the same JSON
/// either way, so nothing but the announced type differs.
///
/// How the payload half is known: a replay of the recorded fixture
/// `rum-multi-platform-monitors` with `application/x-amz-json-1.1` announced on EVERY
/// answer reports `rum|CreateAppMonitor|2xx->2xx|$headers.content-type` as a divergence on
/// a step whose status and body both match, and the same replay passes end to end once a
/// payload-carrying request is answered in [`JSON_CONTENT_TYPE`]. Before the blanket
/// re-announce existed this service answered `application/json` everywhere and no sweep
/// ever filed a `CreateAppMonitor` content-type cluster, which agrees: the create's
/// recorded header was matching then.
///
/// Applied in [`crate::service::EmulatedService`], the one place every response this
/// service produces passes through.
pub const RESPONSE_CONTENT_TYPE: &str = "application/x-amz-json-1.1";

/// The media type the `restJson1` serializer stamps on a JSON body — the one value
/// [`RESPONSE_CONTENT_TYPE`] replaces, and the one a request's JSON payload declares. A
/// response whose body is NOT that JSON envelope (a `@httpPayload` blob's
/// `application/octet-stream`, a string payload's `text/plain`) announces the type its own
/// target shape maps to and is left alone.
pub const JSON_CONTENT_TYPE: &str = "application/json";

/// What separates the components of a continuation token's resume key: a control
/// character none of the components that precede the last one can contain (an
/// `AppMonitorName` matches `^(?!\.)[\.\-_#A-Za-z0-9]+$` and a `MetricDestination` is
/// one of two words). The LAST component needs no such guarantee, because
/// [`decode_pagination_token`] splits at most `components - 1` times and hands whatever
/// follows back whole — which matters for a `DestinationArn`, whose
/// `arn:[^:]*:[^:]*:[^:]*:[^:]*:.*` pattern ends in an unconstrained `.*`.
const PAGINATION_TOKEN_SEPARATOR: char = '\u{1f}';

/// An opaque continuation token for the last entry a page returned: `marker` followed by
/// the components of that entry's key.
///
/// ONE codec for every paginated operation of this service (`ListAppMonitors`,
/// `ListRumMetricsDestinations`), so their token formats cannot drift. Hex keeps the
/// token printable and opaque to a client — `NextToken` is an unconstrained `String`, so
/// a client must pass back what it was handed rather than construct one — while staying
/// decodable by the next call. `marker` is the issuing operation's own (see
/// [`decode_pagination_token`]).
pub fn encode_pagination_token(marker: &str, resume_after: &[&str]) -> String {
    let key = resume_after.join(&PAGINATION_TOKEN_SEPARATOR.to_string());
    hex::encode(format!("{marker}{key}"))
}

/// The `components` of the key a token issued by [`encode_pagination_token`] under
/// `marker` resumes after, or `None` for any value that operation never issued.
///
/// The marker is what makes a token the ISSUING operation's: without it any
/// hex-decodable string would be read as a continuation, and one list's token would
/// resume another's. A listing scoped to a parent resource puts that parent's name in
/// the key, so the caller can also check a token was issued for the resource it is
/// resuming.
pub fn decode_pagination_token(
    marker: &str,
    token: &str,
    components: usize,
) -> Option<Vec<String>> {
    let decoded = String::from_utf8(hex::decode(token).ok()?).ok()?;
    let resume_after = decoded.strip_prefix(marker)?;
    // `splitn` so only the components BEFORE the last one are delimited: the last
    // component keeps any separator it contains, rather than inflating the arity and
    // making this refuse a token the service itself issued.
    let key: Vec<String> = resume_after
        .splitn(components, PAGINATION_TOKEN_SEPARATOR)
        .map(str::to_string)
        .collect();
    // The first component identifies the entry to resume after, so a token carrying
    // none — or a key of the wrong arity for this operation — names no resume point.
    (key.len() == components && !key[0].is_empty()).then_some(key)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsRumRumState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsRumRumState>)
}
