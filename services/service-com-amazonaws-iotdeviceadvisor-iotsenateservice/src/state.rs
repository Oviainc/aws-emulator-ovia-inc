// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-iotdeviceadvisor-iotsenateservice service.
//!
//! IoT Core Device Advisor keeps two resources: a **suite definition** (a versioned test
//! suite configuration) and a **suite run** of one definition version. Both live here, in
//! the single per-service store behind the engine's state lock, so a value a create mints
//! is observable by every later read in the process.
//!
//! Besides the records this module owns the handful of rules every handler shares: the ARN
//! and endpoint shapes, the version-string sequence, and the clock-owed suite-run status
//! transitions ([`SuiteRun::materialize_due`]).

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};

use crate::ops::types::{SuiteDefinitionConfiguration, SuiteRunConfiguration};

/// The region an ARN or endpoint falls back to when the request carries no SigV4 scope.
pub const DEFAULT_REGION: &str = "us-east-1";

/// This service's ARN namespace, as the Smithy model's `aws.api#service` trait gives it
/// (`arnNamespace: iotdeviceadvisor`).
pub const ARN_NAMESPACE: &str = "iotdeviceadvisor";

/// How long a suite run spends in each non-terminal status before the clock owes it the
/// next one. A chosen span (AWS documents no duration for it): inside the 250–750 ms
/// window the emulator contract fixes, so a caller acting straight after `StartSuiteRun`
/// still observes `PENDING`, and never zero.
pub const STATUS_TRANSITION: Duration = Duration::from_millis(500);

/// Why a suite run cannot reach a test-result status in this emulator.
///
/// Device Advisor drives real MQTT test cases against a physical device over the IoT data
/// plane; this emulator has no device and no test runner, so a run is accepted, tracked,
/// and then ends in the modelled `ERROR` status carrying this reason — never in `PASS` /
/// `FAIL` / `PASS_WITH_WARNINGS`, which would report an outcome for tests nothing ran.
pub const NO_TEST_RUNNER_REASON: &str =
    "Device Advisor test execution is not available in this emulator: no device test runner \
     is attached, so the suite run could not be executed against the primary device.";

/// Suite-run statuses from which no further transition is owed.
///
/// The `SuiteRunStatus` values that are not `PENDING`, `RUNNING` or `STOPPING`; a run in
/// one of these keeps its status and its `endTime` forever. The single definition of
/// "terminal": [`SuiteRun::materialize_due`] consults it rather than listing the statuses
/// a second time.
pub const TERMINAL_STATUSES: &[&str] = &[
    "CANCELED",
    "ERROR",
    "FAIL",
    "PASS",
    "PASS_WITH_WARNINGS",
    "STOPPED",
];

/// True when `status` is one no clock-owed transition leaves.
pub fn is_terminal(status: &str) -> bool {
    TERMINAL_STATUSES.contains(&status)
}

/// How many suite definitions this account's store holds at once.
///
/// An OCCUPANCY BOUND this emulator chooses, not a claim about a published AWS quota: the
/// store is in memory and every `CreateSuiteDefinition` adds a record, so without a cap a
/// caller could grow it without limit. `CreateSuiteDefinition` refuses past it with the
/// modelled `ValidationException` (the only client-input error it declares), the way a
/// real service refuses a creation past its account quota.
pub const MAX_SUITE_DEFINITIONS: usize = 100;

/// How many configuration version LABELS the store retains per suite definition.
///
/// The same occupancy bound as [`MAX_RUNS_PER_DEFINITION`], and again a RETENTION policy
/// rather than a refusal: every `UpdateSuiteDefinition` hands out one more version label,
/// so without a bound a caller could grow one definition's record without limit.
/// [`SuiteDefinition::record_version`] drops the LOWEST-ordinal labels once a definition
/// would hold more than this, so the most recent versions — the ones a read or a run names
/// — stay addressable, and an aged-out version reads as the modelled "no such version"
/// exactly as a version that never existed does.
pub const MAX_VERSIONS_PER_DEFINITION: usize = 50;

/// How many suite runs the store retains per definition.
///
/// The same occupancy bound for runs, as a RETENTION policy rather than a refusal: a
/// definition can be run any number of times, so `StartSuiteRun` drops the oldest runs of
/// that definition (and their tag rows) that are no longer in progress once it would
/// exceed this count, exactly as a real service ages out old run history. A run still in
/// progress is never dropped.
pub const MAX_RUNS_PER_DEFINITION: usize = 50;

/// How many tags one resource carries at once.
///
/// `TagMap` is `len 0..50` (`smithy_query(query="shape",shape="TagMap")`), so a resource
/// cannot end up holding more than 50 tags: `TagResource` refuses a merge that would leave
/// more than this with the modelled client-input error, the way a real service refuses a
/// tagging call past the per-resource tag limit.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// One of this service's two taggable resources, as a `resourceArn` names it.
///
/// `ListTagsForResource`/`TagResource`/`UntagResource` all document the same thing about
/// their `resourceArn`: "This can be SuiteDefinition ARN or SuiteRun ARN".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaggedResource {
    /// `…:suitedefinition/<suiteDefinitionId>`, as `CreateSuiteDefinition` mints it.
    SuiteDefinition(String),
    /// `…:suiterun/<suiteDefinitionId>/<suiteRunId>`, as `StartSuiteRun` mints it.
    SuiteRun {
        suite_definition_id: String,
        suite_run_id: String,
    },
}

/// Parse `arn` as the ARN of a taggable IoT Device Advisor resource.
///
/// The tagging operations address a resource ONLY by ARN, and the model constrains
/// `AmazonResourceName` by length alone, so the ARN's shape is checked here:
/// `arn:<partition>:iotdeviceadvisor:<region>:<account>:` followed by one of the two
/// resource forms this service mints. `Err` carries why, for the modelled client-input
/// error — a value that is not one of this service's ARNs is a bad request, never a
/// missing resource, so it is refused before anything is looked up.
pub fn parse_resource_arn(arn: &str) -> Result<TaggedResource, String> {
    // The value is request-supplied and goes into the refusal text: `{arn:?}` quotes and
    // escapes it, so no newline inside it can forge a second line of the message.
    let malformed = || {
        format!(
            "resourceArn {arn:?} is not an IoT Device Advisor suite definition or suite run ARN"
        )
    };
    // `splitn(6, ..)` so a `:` inside the resource portion stays in it.
    let mut fields = arn.splitn(6, ':');
    let (
        Some("arn"),
        Some(partition),
        Some(service),
        Some(_region),
        Some(account),
        Some(resource),
    ) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    )
    else {
        return Err(malformed());
    };
    if partition.is_empty() || account.is_empty() || service != ARN_NAMESPACE {
        return Err(malformed());
    }
    let mut parts = resource.split('/');
    match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some("suitedefinition"), Some(id), None, None) if !id.is_empty() => {
            Ok(TaggedResource::SuiteDefinition(id.to_string()))
        }
        (Some("suiterun"), Some(definition), Some(run), None)
            if !definition.is_empty() && !run.is_empty() =>
        {
            Ok(TaggedResource::SuiteRun {
                suite_definition_id: definition.to_string(),
                suite_run_id: run.to_string(),
            })
        }
        _ => Err(malformed()),
    }
}

/// The smallest step a write stamp advances by when the clock has not visibly moved.
///
/// Timestamps leave this service as epoch seconds in an f64, whose mantissa resolves an
/// epoch instant to about a microsecond: that is the finest difference a client can
/// observe, so it is the step [`SuiteDefinition::write_stamp`] uses to keep successive
/// write stamps strictly increasing.
const WRITE_STAMP_TICK: Duration = Duration::from_micros(1);

/// A stored suite definition: its identity, its ONE live configuration, the version labels
/// it has handed out, and its timestamps.
///
/// Device Advisor keeps exactly one live configuration per `suiteDefinitionId`: an update
/// hands out a new version label and REPLACES that configuration, so the versions are
/// pointers into the definition's history rather than snapshots of it. Hence a label SET
/// beside a single [`configuration`](Self::configuration), not a label -> configuration map.
#[derive(Debug, Clone)]
pub struct SuiteDefinition {
    /// The minted `suiteDefinitionId` (a UUID, so inside the modelled 12..36 length).
    pub id: String,
    /// `arn:aws:iotdeviceadvisor:<region>:<account>:suitedefinition/<id>`.
    pub arn: String,
    /// The version recorded last — the one an unqualified read echoes. Tracked explicitly
    /// rather than derived from [`versions`](Self::versions), whose members order
    /// lexicographically (`v10` before `v2`).
    pub latest_version: String,
    /// Every version label this definition has handed out and still accepts on a read: a
    /// pointer set, holding no configuration of its own.
    pub versions: BTreeSet<String>,
    /// The ONE live configuration, as the last create or update left it. Every read of
    /// this definition — version-qualified or not — answers with this one, since a
    /// definition keeps no per-version snapshots.
    pub configuration: SuiteDefinitionConfiguration,
    pub created_at: SystemTime,
    pub last_modified_at: SystemTime,
}

impl SuiteDefinition {
    /// True when `version` is a label this definition has handed out and still retains —
    /// the one check a version-qualified read or run makes before serving the live
    /// configuration. See [`MAX_VERSIONS_PER_DEFINITION`] for what "still retains" drops.
    pub fn holds_version(&self, version: &str) -> bool {
        self.versions.contains(version)
    }

    /// `suiteDefinitionName` as the STORED live configuration holds it.
    pub fn name(&self) -> &str {
        self.configuration.suite_definition_name.as_str()
    }

    /// Make `configuration` this definition's live one under a NEW version label, and hand
    /// that label back.
    ///
    /// The label set grows; the configuration is REPLACED, because a definition has exactly
    /// one live configuration and no per-version snapshots. The next ordinal is one past the
    /// highest this definition holds — not `versions.len()`, which would collide with a
    /// surviving label once retention has dropped an older one. BOTH timestamps move to the
    /// write instant: for the one live record `createdAt` behaves as a last-write stamp,
    /// just as `lastModifiedAt` does.
    pub fn record_version(
        &mut self,
        configuration: SuiteDefinitionConfiguration,
        now: SystemTime,
    ) -> String {
        let highest = self
            .versions
            .iter()
            .filter_map(|label| version_ordinal(label))
            .max()
            .unwrap_or(0);
        let label = version_label(highest + 1);
        self.versions.insert(label.clone());
        self.latest_version = label.clone();
        self.configuration = configuration;
        let stamp = self.write_stamp(now);
        self.created_at = stamp;
        self.last_modified_at = stamp;
        self.prune_versions(MAX_VERSIONS_PER_DEFINITION);
        label
    }

    /// The instant to stamp a write with: `now`, never at or before the stamp this record
    /// already carries.
    ///
    /// A write stamp is what orders a definition's writes, so two writes must not read as
    /// one: when the clock has not visibly advanced since the last write (a coarse clock,
    /// or two calls inside the same tick) the stamp moves on by [`WRITE_STAMP_TICK`]
    /// instead of standing still.
    fn write_stamp(&self, now: SystemTime) -> SystemTime {
        let previous = self.last_modified_at.max(self.created_at);
        now.max(previous + WRITE_STAMP_TICK)
    }

    /// Retain at most `keep` version labels, dropping the lowest-ordinal (oldest) ones.
    ///
    /// See [`MAX_VERSIONS_PER_DEFINITION`]. The current latest version is never dropped:
    /// it carries the highest ordinal, and `keep` is at least one.
    fn prune_versions(&mut self, keep: usize) {
        let keep = keep.max(1);
        if self.versions.len() <= keep {
            return;
        }
        // Oldest first by ORDINAL, not by the labels' lexicographic order (`v10` < `v2`).
        let mut labels: Vec<(usize, String)> = self
            .versions
            .iter()
            .map(|label| (version_ordinal(label).unwrap_or(0), label.clone()))
            .collect();
        labels.sort();
        let drop_count = self.versions.len() - keep;
        for (_, label) in labels.into_iter().take(drop_count) {
            self.versions.remove(&label);
        }
    }
}

/// A stored suite run of one definition version.
#[derive(Debug, Clone)]
pub struct SuiteRun {
    /// The minted `suiteRunId` (a UUID).
    pub id: String,
    /// `arn:aws:iotdeviceadvisor:<region>:<account>:suiterun/<definition id>/<run id>`.
    pub arn: String,
    pub suite_definition_id: String,
    pub suite_definition_version: String,
    /// The `suiteRunConfiguration` the start call carried, returned verbatim by reads.
    pub configuration: SuiteRunConfiguration,
    /// One of `SuiteRunStatus`.
    pub status: String,
    pub created_at: SystemTime,
    /// Set when the run leaves `PENDING`; absent while it is still queued.
    pub started_at: Option<SystemTime>,
    /// Set only once the status is terminal.
    pub end_at: Option<SystemTime>,
    pub error_reason: Option<String>,
    /// The Device Advisor endpoint a device under test connects to for this run.
    pub endpoint: String,
    /// The instant the next status transition becomes due, while one is owed.
    pub next_transition_at: Option<SystemTime>,
}

impl SuiteRun {
    /// Apply every status transition the clock has ALREADY made due at `now`.
    ///
    /// The whole lifecycle is clock-driven, so each reader materialises it before
    /// answering and nothing has to sleep: `PENDING` -> `RUNNING` -> `ERROR` for a run the
    /// emulator cannot execute (see [`NO_TEST_RUNNER_REASON`]), and `STOPPING` -> `STOPPED`
    /// for one a caller stopped. A transition that is not due yet is left alone, and a
    /// terminal run is never touched again.
    pub fn materialize_due(&mut self, now: SystemTime) {
        while let Some(due) = self.next_transition_at {
            if is_terminal(&self.status) {
                // Nothing is owed from a terminal status: drop any stale due instant.
                self.next_transition_at = None;
                return;
            }
            if due > now {
                return;
            }
            match self.status.as_str() {
                "PENDING" => {
                    self.status = "RUNNING".to_string();
                    self.started_at = Some(due);
                    self.next_transition_at = Some(due + STATUS_TRANSITION);
                }
                "RUNNING" => {
                    self.status = "ERROR".to_string();
                    self.error_reason = Some(NO_TEST_RUNNER_REASON.to_string());
                    self.end_at = Some(due);
                    self.next_transition_at = None;
                }
                "STOPPING" => {
                    self.status = "STOPPED".to_string();
                    self.end_at = Some(due);
                    self.next_transition_at = None;
                }
                _ => {
                    // Unreachable: `TERMINAL_STATUSES` above plus these three arms cover
                    // every `SuiteRunStatus`. Clear the due instant rather than loop.
                    self.next_transition_at = None;
                    return;
                }
            }
        }
    }

    /// True while this run holds the definition's single run slot: any status that is not
    /// terminal, so `STOPPING` still counts — a run the caller asked to stop is in
    /// progress until the stop completes.
    ///
    /// This is what `StartSuiteRun` refuses a concurrent start on, and what retention
    /// never drops.
    pub fn is_in_progress(&self) -> bool {
        !is_terminal(&self.status)
    }

    /// True when a stop would move this run: it has not been asked to stop yet and has
    /// not finished. `STOPPING` and every terminal status answer false, so stopping twice
    /// neither restarts the stop clock nor resurrects a finished run.
    pub fn is_stoppable(&self) -> bool {
        matches!(self.status.as_str(), "PENDING" | "RUNNING")
    }
}

/// Per-service state for com-amazonaws-iotdeviceadvisor-iotsenateservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsIotdeviceadvisorIotsenateserviceState {
    /// suiteDefinitionId -> definition. A `BTreeMap` so every listing, and the paging
    /// token built from it, has one deterministic order.
    pub definitions: BTreeMap<String, SuiteDefinition>,
    /// suiteRunId -> run.
    pub runs: BTreeMap<String, SuiteRun>,
    /// resourceArn -> tags. The ONE place tags live: a definition's and a run's tags are
    /// both keyed by their ARN here, so the tagging operations and the reads that echo
    /// tags cannot disagree.
    pub tags: BTreeMap<String, BTreeMap<String, String>>,
    /// `clientToken` -> what that token already created, so a retried
    /// `CreateSuiteDefinition` returns the first definition instead of minting a second,
    /// and the same token offered for a DIFFERENT request is refused. One entry per
    /// stored definition at most — a token is recorded only alongside a new definition
    /// and dropped with it — so [`MAX_SUITE_DEFINITIONS`] bounds this map too.
    pub create_definition_tokens: BTreeMap<String, AcceptedCreateRequest>,
}

/// What one `clientToken` was accepted for: the definition it created, and a fingerprint
/// of the request that created it.
///
/// An `@idempotencyToken` identifies a RETRY of one request, not "whatever the caller
/// sends next under the same name": a token offered again with different parameters is a
/// client mistake, not a retry, so the fingerprint is kept to tell the two apart.
#[derive(Debug, Clone)]
pub struct AcceptedCreateRequest {
    pub suite_definition_id: String,
    /// See [`create_request_fingerprint`].
    pub fingerprint: String,
}

/// A fingerprint of a `CreateSuiteDefinition` request: everything the stored definition is
/// built from — the configuration, the tags, and the region the ARN is minted in.
///
/// Derived member by member from the already-normalised values (the tags as a sorted
/// map), each written as a length-prefixed field so no value can impersonate another
/// through a separator, and so the fingerprint is defined by the DATA rather than by a
/// formatter whose output is not a stability contract.
pub fn create_request_fingerprint(
    region: &str,
    configuration: &SuiteDefinitionConfiguration,
    tags: &BTreeMap<String, String>,
) -> String {
    /// One length-prefixed field: `<len>:<value>;`.
    fn field(value: &str, into: &mut String) {
        use ::std::fmt::Write;
        // `write!` into the buffer we already own: no per-field allocation.
        let _ = write!(into, "{}:{value};", value.len());
    }

    let mut out = String::new();
    field(region, &mut out);
    field(&configuration.suite_definition_name, &mut out);
    field(&configuration.root_group, &mut out);
    field(&configuration.device_permission_role_arn, &mut out);
    field(configuration.protocol.as_deref().unwrap_or(""), &mut out);
    field(
        match configuration.intended_for_qualification {
            Some(true) => "qualification=true",
            Some(false) => "qualification=false",
            None => "qualification=unset",
        },
        &mut out,
    );
    field(
        match configuration.is_long_duration_test {
            Some(true) => "longDuration=true",
            Some(false) => "longDuration=false",
            None => "longDuration=unset",
        },
        &mut out,
    );
    let devices = configuration.devices.as_deref().unwrap_or_default();
    field(&devices.len().to_string(), &mut out);
    for device in devices {
        field(device.thing_arn.as_deref().unwrap_or(""), &mut out);
        field(device.certificate_arn.as_deref().unwrap_or(""), &mut out);
        field(device.device_role_arn.as_deref().unwrap_or(""), &mut out);
    }
    field(&tags.len().to_string(), &mut out);
    for (key, value) in tags {
        field(key, &mut out);
        field(value, &mut out);
    }
    out
}

impl ComAmazonawsIotdeviceadvisorIotsenateserviceState {
    /// Materialise every clock-owed run transition, then hand back the run named by
    /// `(definition id, run id)`.
    ///
    /// A read goes through here: it writes only what the clock already made due at `now`
    /// — the same state a caller would have seen had nobody called at all — and only to
    /// the run the caller named. A run of ANOTHER definition is matched out first, so no
    /// operation ever touches a resource it was not asked about.
    pub fn run_mut(
        &mut self,
        suite_definition_id: &str,
        suite_run_id: &str,
        now: SystemTime,
    ) -> Option<&mut SuiteRun> {
        let run = self.runs.get_mut(suite_run_id)?;
        if run.suite_definition_id != suite_definition_id {
            return None;
        }
        run.materialize_due(now);
        Some(run)
    }

    /// Every run OF ONE DEFINITION, with its clock-owed transitions materialised.
    ///
    /// The definition filter runs before the materialisation, so a call about one
    /// definition never advances another definition's runs.
    pub fn runs_of_mut(
        &mut self,
        suite_definition_id: &str,
        now: SystemTime,
    ) -> impl Iterator<Item = &mut SuiteRun> {
        let suite_definition_id = suite_definition_id.to_string();
        self.runs.values_mut().filter_map(move |run| {
            if run.suite_definition_id != suite_definition_id {
                return None;
            }
            run.materialize_due(now);
            Some(run)
        })
    }

    /// Remove `keys` from the tags recorded for `arn` and report how many the call
    /// actually removed.
    ///
    /// The counterpart of [`merge_tags`](Self::merge_tags). A key the resource does not
    /// carry is simply absent afterwards too, so removing it is not an error and needs no
    /// refusal. An ARN left with no tags keeps NO row at all, exactly as `merge_tags`
    /// keeps none for an empty merge: [`tags_of`](Self::tags_of) must not be able to tell
    /// "untagged down to nothing" from "never tagged".
    pub fn remove_tags<'a>(&mut self, arn: &str, keys: impl IntoIterator<Item = &'a str>) -> usize {
        let Some(tags) = self.tags.get_mut(arn) else {
            return 0;
        };
        let removed = keys
            .into_iter()
            .filter(|key| tags.remove(*key).is_some())
            .count();
        if tags.is_empty() {
            self.tags.remove(arn);
        }
        removed
    }

    /// The tags recorded for `arn`, or `None` when it carries none — an absent `tags`
    /// member rather than an empty map.
    pub fn tags_of(&self, arn: &str) -> Option<std::collections::HashMap<String, String>> {
        let tags = self.tags.get(arn)?;
        (!tags.is_empty()).then(|| {
            tags.iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect::<std::collections::HashMap<_, _>>()
        })
    }

    /// True when this store holds the resource `resource` names, UNDER THE ARN it was
    /// named by.
    ///
    /// The ARN identifies the resource whole: an ARN carrying the right id but another
    /// partition, region or account names a resource of that other scope, which this
    /// store does not hold, so it is not found rather than silently treated as the local
    /// one. A pure predicate — a tag read never advances anything.
    pub fn holds_tagged_resource(&self, arn: &str, resource: &TaggedResource) -> bool {
        match resource {
            TaggedResource::SuiteDefinition(id) => self
                .definitions
                .get(id)
                .is_some_and(|definition| definition.arn == arn),
            TaggedResource::SuiteRun {
                suite_definition_id,
                suite_run_id,
            } => self.runs.get(suite_run_id).is_some_and(|run| {
                run.arn == arn && run.suite_definition_id == *suite_definition_id
            }),
        }
    }

    /// Add `tags` to `arn`'s row, overwriting the value of every key it already carries
    /// and leaving the rest in place.
    ///
    /// `Err` is how many tags the merge would have left, when that is past `limit`: the
    /// caller refuses with its own modelled error and the row is left untouched, so a
    /// refused tagging call changes nothing. An empty row is dropped rather than kept, so
    /// a resource with no tags has no row in either direction.
    pub fn merge_tags(
        &mut self,
        arn: &str,
        tags: BTreeMap<String, String>,
        limit: usize,
    ) -> Result<(), usize> {
        let existing = self.tags.get(arn);
        let would_hold = existing.map_or(tags.len(), |existing| {
            existing.len() + tags.keys().filter(|key| !existing.contains_key(*key)).count()
        });
        if would_hold > limit {
            return Err(would_hold);
        }
        if tags.is_empty() {
            return Ok(());
        }
        let row = self.tags.entry(arn.to_string()).or_default();
        row.extend(tags);
        Ok(())
    }

    /// Retain at most `keep` runs of `suite_definition_id`, dropping the OLDEST ones that
    /// have FINISHED, with their tag rows.
    ///
    /// The store's occupancy bound for runs (see [`MAX_RUNS_PER_DEFINITION`]). Liveness is
    /// judged on the CURRENT status, so the clock-owed transitions are materialised first:
    /// the lifecycle only advances when something touches the run, so a run nobody has
    /// read still carries the status it was stored with and would otherwise look in
    /// progress forever — leaving the bound with nothing it is allowed to drop.
    ///
    /// A run that is still in progress — including one that is `STOPPING` — is never
    /// dropped, so a caller can always read back the run it just started or stopped. That
    /// cannot defeat the bound: `StartSuiteRun` refuses a start while any run of the
    /// definition is in progress, so at most one run is ever skipped and the remaining
    /// candidates still take the count down to `keep`.
    pub fn prune_runs(&mut self, suite_definition_id: &str, keep: usize, now: SystemTime) {
        let mut total = 0usize;
        let mut droppable: Vec<(SystemTime, String, String)> = Vec::new();
        for run in self.runs.values_mut() {
            if run.suite_definition_id != suite_definition_id {
                continue;
            }
            run.materialize_due(now);
            total += 1;
            if !run.is_in_progress() {
                droppable.push((run.created_at, run.id.clone(), run.arn.clone()));
            }
        }
        if total <= keep {
            return;
        }
        // Oldest first: retention keeps the most recent runs.
        droppable.sort();
        for (_, id, arn) in droppable.into_iter().take(total - keep) {
            self.runs.remove(&id);
            self.tags.remove(&arn);
        }
    }
}

/// The message a refusal carries when the resource the request NAMED does not exist.
///
/// Device Advisor builds this text from the request: the template is
/// `Unable to find resource with <member>: <value>`, where `<member>` is the input member
/// that identified the missing resource and `<value>` is the value the caller sent. One
/// definition for the whole service, so every lookup that cannot find what it was asked
/// for words it the same way, whichever error shape the operation models for it.
pub fn not_found_message(member: &str, value: &str) -> String {
    format!("Unable to find resource with {member}: {value}")
}

/// The message a `SuiteDefinitionConfiguration` with an empty `rootGroup` is refused with
/// when the suite is not intended for qualification.
///
/// Device Advisor builds it from the violated rule, with its `Invalid Request: ` prefix;
/// see [`root_group_violation`] for the rule itself.
pub const EMPTY_ROOT_GROUP_MESSAGE: &str =
    "Invalid Request: rootGroup cannot be empty for non-qualification test suite";

/// The upper bound the `MaxResults` shape's `range` trait puts on `maxResults`
/// (`range 1..50`), which is also the page size a caller who names none gets: one page
/// then holds everything unless the caller asks for fewer.
pub const MAX_RESULTS_MAX: i32 = 50;

/// True when `token` could be a pagination token one of this service's listings issued.
///
/// Every listing here resumes AT a resource id, so the token it hands out is one of the
/// ids this service mints — a hyphenated lowercase v4 UUID (see [`new_uuid`]). A token of
/// any other shape is one this service cannot have issued, which is what lets a listing
/// refuse a garbled or hand-written token with the modelled client-input error instead of
/// silently answering from the start of the map.
///
/// The shape is checked here rather than with `uuid::Uuid::parse_str`, which also accepts
/// upper-case, braced and urn forms this service never mints and would therefore let
/// through a token no listing of ours ever issued.
pub fn is_page_token_shape(token: &str) -> bool {
    /// A lowercase hex digit, the only byte a UUID group holds.
    fn is_lower_hex(byte: u8) -> bool {
        byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()
    }

    let groups: Vec<&str> = token.split('-').collect();
    groups.iter().map(|group| group.len()).eq([8, 4, 4, 4, 12])
        && groups.iter().all(|group| group.bytes().all(is_lower_hex))
}

/// The one rule the model states in prose for `SuiteDefinitionConfiguration$rootGroup`
/// and cannot express as a constraint: "if intendedForQualification is set to true,
/// rootGroup can be an empty string. If intendedForQualification is false, rootGroup
/// cannot be an empty string."
///
/// `Some(message)` names the violation for the modelled `ValidationException` with the
/// template AWS answers it with ([`EMPTY_ROOT_GROUP_MESSAGE`]); `None` means the
/// configuration satisfies the rule. The rule belongs to the SHAPE, so every operation
/// that accepts a `SuiteDefinitionConfiguration` asks here rather than restating it.
pub fn root_group_violation(configuration: &SuiteDefinitionConfiguration) -> Option<String> {
    (configuration.root_group.is_empty() && configuration.intended_for_qualification != Some(true))
        .then(|| EMPTY_ROOT_GROUP_MESSAGE.to_string())
}

/// The version string for the `n`-th configuration a definition holds (1-based).
///
/// `SuiteDefinitionVersion` is `len 2..255`, so a bare ordinal (`"1"`) is not a legal
/// value for the member; the sequence is `v1`, `v2`, … and the same strings are what
/// `GetSuiteDefinition` accepts back on its `suiteDefinitionVersion` query.
pub fn version_label(n: usize) -> String {
    format!("v{n}")
}

/// The ordinal inside a label [`version_label`] produced, or `None` for any other string.
///
/// The inverse of [`version_label`], and the only place the label's shape is decoded:
/// ordering versions by ordinal is what keeps `v10` after `v2`, which their keys'
/// lexicographic order does not.
pub fn version_ordinal(label: &str) -> Option<usize> {
    label.strip_prefix('v')?.parse().ok()
}

/// Record one state-changing call of this service in the emulator's audit trail.
///
/// Every mutating operation here emits exactly one event on the `emulator::audit` target:
/// who presented which access key id, in which region, which operation, which resource,
/// and how the call ended (the subscriber stamps the time). That is what makes a
/// destructive call — `DeleteSuiteDefinition` cascading to a definition's runs, or
/// `StopSuiteRun` moving a run — attributable after the fact.
///
/// An ATTEMPTED mutation is recorded too, with the outcome that refused it (an update of
/// an id the store does not hold, say): `resource` is then the ARN the request NAMED, not
/// a claim that such a resource is stored.
///
/// Request-derived values are written with `Debug`, so a newline inside one cannot forge a
/// second record. Nothing secret is written: the presented ACCESS KEY ID identifies the
/// caller and is not a secret, and no secret key, session token or emulator credential is
/// read here.
pub fn audit(
    operation: &str,
    parts: &::http::request::Parts,
    region: &str,
    resource: &str,
    outcome: &str,
) {
    ::tracing::info!(
        target: "emulator::audit",
        operation = %operation,
        access_key_id = ?::emulator_server::extract_akid(&parts.headers),
        region = ?region,
        resource = ?resource,
        outcome = %outcome,
        "state-changing iotdeviceadvisor request"
    );
}

/// A freshly minted `UUID`-shaped id: a random v4 UUID, 36 characters, inside the
/// modelled 12..36 length.
///
/// The generator is the `uuid` crate's, reached through `emulator_telemetry`'s `random()`
/// constructor — `uuid::Uuid::new_v4().as_hyphenated()` verbatim
/// (`vendor/emulator-telemetry/src/types.rs`) — because `emulator-telemetry` is already
/// one of this crate's dependencies, so no new entry in the workspace lockfile is needed
/// for it. Only the generator is borrowed: the newtype's telemetry meaning is dropped on
/// the next line and nothing about the UUID's layout or entropy is relied on here. The
/// same borrowing is what `service-com-amazonaws-cloudwatchomni-cloudwatchomnifrontend`
/// does for its ids.
pub fn new_uuid() -> String {
    ::emulator_telemetry::BatchId::random().as_str().to_string()
}

/// The region to stamp into ARNs and endpoints: the SigV4 credential scope's region when
/// the request is signed and that region has the published shape, else [`DEFAULT_REGION`].
///
/// The scope region is caller-supplied, and it is interpolated into ARNs and into the
/// endpoint host, where `:` or an extra dotted label would produce a structurally
/// well-formed but fabricated value AWS never mints. AWS resolves the region from the
/// endpoint it served and only honours a region it publishes, so a region that does not
/// LOOK like a published one is replaced by the default instead of being reflected back.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| has_published_region_shape(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// True when `region` has the shape every AWS region identifier has:
/// `<2+ lowercase letters>(-<lowercase letters>)+-<digit>`, e.g. `us-east-1`,
/// `ap-southeast-4`, `il-central-1`, `us-gov-west-1`.
///
/// A shape test, not an allowlist of the regions that exist today: a new region must keep
/// working without an emulator change, while a value carrying ARN- or host-structural
/// characters (`:`, `.`, `/`, whitespace, upper case) is refused.
pub fn has_published_region_shape(region: &str) -> bool {
    let mut parts = region.split('-');
    let Some(first) = parts.next() else {
        return false;
    };
    if first.len() < 2 || !first.bytes().all(|b| b.is_ascii_lowercase()) {
        return false;
    }
    let rest: Vec<&str> = parts.collect();
    // At least one middle label plus the trailing number.
    let Some((number, middle)) = rest.split_last() else {
        return false;
    };
    if middle.is_empty() {
        return false;
    }
    middle
        .iter()
        .all(|label| !label.is_empty() && label.bytes().all(|b| b.is_ascii_lowercase()))
        && !number.is_empty()
        && number.bytes().all(|b| b.is_ascii_digit())
}

/// This service's ARN for `resource` in `region`.
pub fn arn(region: &str, resource: &str) -> String {
    ::emulator_server::emulator_arn(ARN_NAMESPACE, region, resource)
}

/// The modelled `Endpoint` length bounds (`smithy_query(query="shape",shape="Endpoint")`
/// gives `len 45..75`), so every host this module builds can be checked against them.
pub const MIN_ENDPOINT_LENGTH: usize = 45;
/// See [`MIN_ENDPOINT_LENGTH`].
pub const MAX_ENDPOINT_LENGTH: usize = 75;

/// Everything a Device Advisor host carries besides its label and its region.
const ENDPOINT_FIXED_LENGTH: usize = ".deviceadvisor.iot.".len() + ".amazonaws.com".len();

/// One Device Advisor host: a single label under this service's endpoint domain.
///
/// Both endpoints Device Advisor vends — the account-level one and a device-level one —
/// have this shape, so the LABEL is the only thing that tells them apart.
fn endpoint_host(label: &str, region: &str) -> String {
    format!("{label}.deviceadvisor.iot.{region}.amazonaws.com")
}

/// The region a host whose label is `label_length` characters long can carry while staying
/// inside [`MAX_ENDPOINT_LENGTH`]: the caller's region, or [`DEFAULT_REGION`] when the
/// caller's would push the host past it.
///
/// Every region AWS publishes fits; a longer one falls back rather than producing an
/// out-of-range `Endpoint` (the ARN still carries the caller's own region).
fn host_region(label_length: usize, region: &str) -> &str {
    if label_length + ENDPOINT_FIXED_LENGTH + region.len() > MAX_ENDPOINT_LENGTH {
        DEFAULT_REGION
    } else {
        region
    }
}

/// The ACCOUNT-LEVEL Device Advisor endpoint for this account in `region`.
///
/// Device Advisor vends one account-scoped host of the form
/// `<account>.deviceadvisor.iot.<region>.amazonaws.com`, which is what a suite run reports
/// and what a device under test connects to. It is derived here rather than stored, so
/// every operation that has to report it — `StartSuiteRun`, the suite-run reads, and
/// `GetEndpoint` — answers the same host for the same account and region.
pub fn endpoint_for_region(region: &str) -> String {
    let label = ::emulator_server::emulator_account_id();
    endpoint_host(label, host_region(label.len(), region))
}

/// The device a `GetEndpoint` request names, exactly as it names it.
///
/// Device Advisor lets a caller name the device under test by its thing, by its
/// certificate, or by the role it assumes; a request that names NONE of them is asking for
/// the account-level endpoint instead.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct DeviceIdentity<'a> {
    pub thing_arn: Option<&'a str>,
    pub certificate_arn: Option<&'a str>,
    pub device_role_arn: Option<&'a str>,
}

impl DeviceIdentity<'_> {
    /// True when the request named no device at all.
    pub fn names_no_device(&self) -> bool {
        self.thing_arn.is_none() && self.certificate_arn.is_none() && self.device_role_arn.is_none()
    }
}

/// Length of the single DNS label that identifies ONE device in a device-level endpoint.
///
/// The exact length is this emulator's choice — what a caller depends on is that the label
/// is one stable, lowercase-alphanumeric DNS label, and that the host stays inside the
/// modelled `Endpoint` bounds for every region AWS publishes.
pub const DEVICE_LABEL_LENGTH: usize = 14;

/// The alphabet a device label's FIRST character is drawn from: a hostname label is read
/// case-insensitively and AWS writes these prefixes in lowercase, and starting with a
/// letter keeps the label in the preferred form of RFC 1035 §2.3.1.
const DEVICE_LABEL_LEADING_ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz";

/// The alphabet the rest of a device label is drawn from.
const DEVICE_LABEL_ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";

/// The DEVICE-LEVEL Device Advisor endpoint for the device `device` names, in `region`.
///
/// Device Advisor vends an endpoint that is unique per device beside the account-level one
/// (AWS IoT Core Device Advisor developer guide, "Device Advisor endpoints"), so the label
/// is DERIVED — from the account, the region the host carries, and the ARNs the request
/// names — rather than drawn at random: asking twice for one device answers the same host,
/// and two devices get two hosts, with nothing stored per device. Truncating a digest to a
/// single label makes a collision between two devices a probability rather than an
/// impossibility; nothing treats the label as a unique key. The same derivation the
/// `com-amazonaws-iot-awsiotservice` crate uses for an account's IoT endpoint prefix.
///
/// This service holds no device registry — only the devices a suite definition or a suite
/// run points at — so two ARNs naming one real device (a thing and that thing's
/// certificate) are two identities here, each with its own stable host, rather than one.
pub fn endpoint_for_device(region: &str, device: &DeviceIdentity<'_>) -> String {
    let region = host_region(DEVICE_LABEL_LENGTH, region);
    let mut hasher = Sha256::new();
    // Domain-separated, and every field length-prefixed, so no two different identities
    // can feed the digest the same bytes.
    hasher.update(b"iotdeviceadvisor/device-endpoint/");
    for field in [
        ::emulator_server::emulator_account_id(),
        region,
        device.thing_arn.unwrap_or(""),
        device.certificate_arn.unwrap_or(""),
        device.device_role_arn.unwrap_or(""),
    ] {
        hasher.update(field.len().to_string().as_bytes());
        hasher.update(b":");
        hasher.update(field.as_bytes());
        hasher.update(b";");
    }
    let digest = hasher.finalize();
    let label: String = digest
        .iter()
        .take(DEVICE_LABEL_LENGTH)
        .enumerate()
        .map(|(position, byte)| {
            let alphabet = if position == 0 {
                DEVICE_LABEL_LEADING_ALPHABET
            } else {
                DEVICE_LABEL_ALPHABET
            };
            char::from(alphabet[usize::from(*byte) % alphabet.len()])
        })
        .collect();
    endpoint_host(&label, region)
}

/// True when `value` is an Amazon Resource Name of `service`.
///
/// The form every AWS ARN has — `arn:<partition>:<service>:<region>:<account>:<resource>`
/// with a non-empty partition, service and resource — with the service field equal to
/// `service`. The region and account fields may be empty, as they are in a global
/// service's ARN (an IAM role's, say).
pub fn is_arn_of_service(value: &str, service: &str) -> bool {
    let mut fields = value.splitn(6, ':');
    fields.next() == Some("arn")
        && fields.next().is_some_and(|partition| !partition.is_empty())
        && fields.next() == Some(service)
        // The region and the account, either of which a global service's ARN leaves empty.
        && fields.next().is_some()
        && fields.next().is_some()
        && fields.next().is_some_and(|resource| !resource.is_empty())
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsIotdeviceadvisorIotsenateserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsIotdeviceadvisorIotsenateserviceState>)
}
