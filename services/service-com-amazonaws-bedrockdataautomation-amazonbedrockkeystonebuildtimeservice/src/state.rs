// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-bedrockdataautomation-amazonbedrockkeystonebuildtimeservice service.
//!
//! This module owns the Blueprint record the control-plane operations
//! (`CreateBlueprint`, `GetBlueprint`, `ListBlueprints`, `UpdateBlueprint`,
//! `DeleteBlueprint`, `CreateBlueprintVersion`) share and the DataAutomationProject
//! record `CreateDataAutomationProject` and `GetDataAutomationProject` share, plus
//! the few helpers more than one of those handlers needs: minting a resource ARN,
//! resolving the request's region, and the opaque `nextToken` codec used by the
//! paginated list. Operation BEHAVIOUR stays in `src/ops/<operation>.rs`; only the
//! shared records and their projections live here.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    Blueprint, BlueprintSummary, CustomOutputConfiguration, DataAutomationLibraryConfiguration,
    DataAutomationProject, OverrideConfiguration, StandardOutputConfiguration, Tag,
};

/// The `blueprintStage` a blueprint takes when `CreateBlueprint` names none.
///
/// Source: `smithy_query(shape="BlueprintStage")` -> `["DEVELOPMENT", "LIVE"]`;
/// a newly created blueprint is a development-stage blueprint until it is
/// promoted (`CopyBlueprintStage`).
pub const DEFAULT_BLUEPRINT_STAGE: &str = "DEVELOPMENT";

/// The region an ARN carries when the request names none (unsigned/local calls).
pub const DEFAULT_REGION: &str = "us-east-1";

/// The ARN namespace Bedrock Data Automation resources live under
/// (`OPERATIONS.md`: "ARN namespace: bedrock").
pub const ARN_SERVICE: &str = "bedrock";

/// The largest page `ListBlueprints` serves, and the page size it uses when the
/// request names no `maxResults`: `ListBlueprintsRequest$maxResults` is modelled
/// `@range(min: 1, max: 1000)`, so no caller can ask for more than this and an
/// absent `maxResults` cannot serialise the whole store either.
pub const BLUEPRINT_PAGE_LIMIT: i32 = 1_000;

/// How many blueprints the store holds before `CreateBlueprint` refuses with the
/// modelled `ServiceQuotaExceededException` (HTTP 402).
///
/// Bedrock Data Automation publishes an adjustable quota on blueprints per
/// account per Region, and this emulator models neither Service Quotas nor an
/// account's adjustments — so this is the emulator's OWN bound on the store
/// rather than a claim about any account's limit. It is set far above any
/// workflow an emulator serves, and it is what stops the state growing without
/// end: every record it holds is released by `DeleteBlueprint`.
///
/// It also bounds `create_idempotency`: a create inserts exactly one token entry
/// for the record it created, and `remove_blueprint` drops every token that
/// resolves to a deleted ARN, so that map holds at most one entry per live
/// blueprint.
pub const BLUEPRINT_QUOTA: usize = 1_000;

/// How many schema snapshots one blueprint holds before
/// `CreateBlueprintVersion` refuses with the modelled
/// `ServiceQuotaExceededException` (HTTP 402). Same reasoning as
/// [`BLUEPRINT_QUOTA`]: a version cut copies the whole live schema (modelled up
/// to 100000 characters), so the per-blueprint snapshot count needs its own
/// bound. `DeleteBlueprint` with a `blueprintVersion` releases one snapshot and
/// without one releases them all.
///
/// Together with [`BLUEPRINT_QUOTA`] it bounds `version_idempotency` too: a cut
/// inserts one token entry per snapshot it takes, and every path that releases a
/// snapshot ([`ComAmazonawsBedrockdataautomationAmazonbedrockkeystonebuildtimeserviceState::forget_version_token`]
/// on a versioned delete,
/// [`ComAmazonawsBedrockdataautomationAmazonbedrockkeystonebuildtimeserviceState::remove_blueprint`]
/// on a whole-blueprint delete) releases its token with it, so that map holds at
/// most one entry per live snapshot.
pub const BLUEPRINT_VERSION_QUOTA: usize = 100;

/// The `projectStage` a DataAutomationProject takes when the create request names
/// none.
///
/// Source: `smithy_query(shape="DataAutomationProjectStage")` ->
/// `["DEVELOPMENT", "LIVE"]`, and the member carries NO `@default`, so this value
/// is the emulator's own choice rather than a modelled one. It is `LIVE` because
/// that is the stage a project has to be in to be usable without naming one: the
/// runtime service resolves a project through
/// `DataAutomationConfiguration$stage` (`smithy_query(shape=
/// "DataAutomationConfiguration")` on the Bedrock Data Automation Runtime model),
/// which is optional too, so a project created with no stage named and invoked
/// with no stage named must be the same project.
pub const DEFAULT_PROJECT_STAGE: &str = "LIVE";

/// The two non-failed statuses of a DataAutomationProject, as
/// `smithy_query(shape="DataAutomationProjectStatus")` lists them (`COMPLETED`,
/// `IN_PROGRESS`, `FAILED`): a create answers `IN_PROGRESS` and the project
/// settles into `COMPLETED`.
pub const PROJECT_IN_PROGRESS: &str = "IN_PROGRESS";

/// See [`PROJECT_IN_PROGRESS`].
pub const PROJECT_COMPLETED: &str = "COMPLETED";

/// How long after `CreateDataAutomationProject` the clock owes the created
/// project's `IN_PROGRESS` -> `COMPLETED` transition.
///
/// AWS documents no duration for it — `CreateDataAutomationProjectResponse$status`
/// is modelled as one of `IN_PROGRESS`/`COMPLETED`/`FAILED` precisely because the
/// project is not ready when the call returns — so this span is CHOSEN, not
/// modelled: 500 ms, inside the project's 250-750 ms band. It is deliberately
/// non-zero, so a caller reading the project straight after the create still
/// observes `IN_PROGRESS`. Nothing sleeps for it: the due instant is stored on the
/// record and compared against the clock.
pub const PROJECT_CREATION_SPAN: Duration = Duration::from_millis(500);

/// How many DataAutomationProjects the store holds before
/// `CreateDataAutomationProject` refuses with the modelled
/// `ServiceQuotaExceededException` (HTTP 402). Same reasoning as
/// [`BLUEPRINT_QUOTA`]: Bedrock Data Automation publishes an adjustable quota on
/// projects per account per Region and this emulator models neither Service Quotas
/// nor an account's adjustments, so this is the emulator's OWN bound on the store.
pub const PROJECT_QUOTA: usize = 1_000;

/// A stored DataAutomationProject: exactly the members the modeled
/// `DataAutomationProject` shape carries, plus the tag list and the instant the
/// clock owes this project's `IN_PROGRESS` -> `COMPLETED` transition.
#[derive(Debug, Clone)]
pub struct DataAutomationProjectRecord {
    pub project_arn: String,
    pub project_name: String,
    pub project_stage: String,
    pub project_type: String,
    pub project_description: Option<String>,
    pub standard_output_configuration: Option<StandardOutputConfiguration>,
    pub custom_output_configuration: Option<CustomOutputConfiguration>,
    pub override_configuration: Option<OverrideConfiguration>,
    pub data_automation_library_configuration: Option<DataAutomationLibraryConfiguration>,
    pub creation_time: ::aws_smithy_types::DateTime,
    pub last_modified_time: ::aws_smithy_types::DateTime,
    pub kms_key_id: Option<String>,
    pub kms_encryption_context: Option<HashMap<String, String>>,
    /// When the create this record came from stops being in progress. Set once, at
    /// creation; every reader derives [`Self::status_at`] from it rather than
    /// storing a status that a read would have to write.
    pub completes_at: SystemTime,
    pub tags: Vec<Tag>,
}

impl DataAutomationProjectRecord {
    /// This project's modeled `status` as of `now`: `IN_PROGRESS` until the
    /// transition the create owed falls due, `COMPLETED` from then on.
    ///
    /// A pure function of the stored due instant, so reporting it writes nothing and
    /// two readers at the same instant agree.
    pub fn status_at(&self, now: SystemTime) -> &'static str {
        if now >= self.completes_at {
            PROJECT_COMPLETED
        } else {
            PROJECT_IN_PROGRESS
        }
    }

    /// Project the record onto the modeled `DataAutomationProject` shape, with the
    /// status the clock has made due at `now`.
    ///
    /// INFALLIBLE: every member of the projection is a member of the record, so no
    /// request can make this fail and no handler needs an unwrap for it.
    pub fn to_project(&self, now: SystemTime) -> DataAutomationProject {
        DataAutomationProject {
            project_arn: self.project_arn.clone(),
            creation_time: self.creation_time,
            last_modified_time: self.last_modified_time,
            project_name: self.project_name.clone(),
            project_stage: Some(self.project_stage.clone()),
            project_type: Some(self.project_type.clone()),
            project_description: self.project_description.clone(),
            standard_output_configuration: self.standard_output_configuration.clone(),
            custom_output_configuration: self.custom_output_configuration.clone(),
            override_configuration: self.override_configuration.clone(),
            data_automation_library_configuration: self
                .data_automation_library_configuration
                .clone(),
            status: self.status_at(now).to_string(),
            kms_key_id: self.kms_key_id.clone(),
            kms_encryption_context: self.kms_encryption_context.clone(),
        }
    }
}

/// One snapshot of a blueprint's schema, taken by `CreateBlueprintVersion`.
#[derive(Debug, Clone)]
pub struct BlueprintVersionSnapshot {
    /// The schema as it stood when the version was cut.
    pub schema: String,
    /// When the version was cut.
    pub created_at: ::aws_smithy_types::DateTime,
}

/// A stored blueprint: exactly the members the modeled `Blueprint` shape carries
/// that this service owns, plus the version snapshots and the tag list.
#[derive(Debug, Clone)]
pub struct BlueprintRecord {
    pub blueprint_arn: String,
    pub blueprint_name: String,
    pub r#type: String,
    pub schema: String,
    pub blueprint_stage: String,
    pub creation_time: ::aws_smithy_types::DateTime,
    pub last_modified_time: ::aws_smithy_types::DateTime,
    pub kms_key_id: Option<String>,
    pub kms_encryption_context: Option<HashMap<String, String>>,
    /// Schema snapshots keyed by their version number.
    pub versions: BTreeMap<u64, BlueprintVersionSnapshot>,
    /// The version number the NEXT cut takes. Monotonic: it is never rolled back
    /// when a snapshot is deleted, so one version number never names two
    /// different schemas over the life of the record.
    pub next_version: u64,
    pub tags: Vec<Tag>,
}

impl BlueprintRecord {
    /// The record's stable sort key: creation time, then ARN. Listing orders by
    /// this so a page boundary is reproducible across calls.
    pub fn order_key(&self) -> (i64, u32, &str) {
        (
            self.creation_time.secs(),
            self.creation_time.subsec_nanos(),
            self.blueprint_arn.as_str(),
        )
    }

    /// The highest version still stored for this blueprint, if any. This is what
    /// a summary reports; the number the next cut takes is [`Self::next_version`],
    /// which does not move back when a snapshot is released.
    pub fn latest_version(&self) -> Option<u64> {
        self.versions.keys().next_back().copied()
    }

    /// Reserve the next version number for a cut, advancing the counter so the
    /// number is never handed out twice even after its snapshot is deleted.
    pub fn take_next_version(&mut self) -> u64 {
        let version = self.next_version.max(1);
        self.next_version = version.saturating_add(1);
        version
    }

    /// Project the record's LIVE (unversioned) state onto the modeled
    /// `Blueprint` shape.
    ///
    /// INFALLIBLE: every member of the projection is a member of the record, so
    /// no request can make this fail and no handler needs an unwrap for it.
    pub fn to_blueprint(&self) -> Blueprint {
        Blueprint {
            blueprint_arn: self.blueprint_arn.clone(),
            schema: self.schema.clone(),
            r#type: self.r#type.clone(),
            creation_time: self.creation_time,
            last_modified_time: self.last_modified_time,
            blueprint_name: self.blueprint_name.clone(),
            blueprint_version: None,
            blueprint_stage: Some(self.blueprint_stage.clone()),
            kms_key_id: self.kms_key_id.clone(),
            kms_encryption_context: self.kms_encryption_context.clone(),
            // Blueprint optimization is a separate, still-unimplemented slice; a
            // blueprint nothing optimized carries neither member.
            optimization_samples: None,
            optimization_time: None,
        }
    }

    /// Project ONE schema snapshot of this record onto the modeled `Blueprint`
    /// shape: the versioned schema and the instant that version was cut, over
    /// the record's own identity. `None` when that version was never cut (or was
    /// deleted), which the caller answers with the modeled 404.
    pub fn to_blueprint_version(&self, version: u64) -> Option<Blueprint> {
        let snapshot = self.versions.get(&version)?;
        Some(Blueprint {
            schema: snapshot.schema.clone(),
            last_modified_time: snapshot.created_at,
            blueprint_version: Some(version.to_string()),
            ..self.to_blueprint()
        })
    }

    /// Project the record onto the modeled `BlueprintSummary` shape that
    /// `ListBlueprints` returns.
    pub fn to_summary(&self) -> BlueprintSummary {
        BlueprintSummary {
            blueprint_arn: self.blueprint_arn.clone(),
            blueprint_version: self.latest_version().map(|v| v.to_string()),
            blueprint_stage: Some(self.blueprint_stage.clone()),
            blueprint_name: Some(self.blueprint_name.clone()),
            creation_time: self.creation_time,
            last_modified_time: Some(self.last_modified_time),
        }
    }
}

/// Per-service state for com-amazonaws-bedrockdataautomation-amazonbedrockkeystonebuildtimeservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsBedrockdataautomationAmazonbedrockkeystonebuildtimeserviceState {
    /// Every stored blueprint, keyed by its ARN.
    pub blueprints: HashMap<String, BlueprintRecord>,
    /// `blueprintName` -> `blueprintArn`; the uniqueness index behind
    /// `CreateBlueprint`'s `ConflictException`.
    pub blueprint_names: HashMap<String, String>,
    /// `CreateBlueprint` `clientToken` -> the ARN that call created, so an
    /// `@idempotencyToken` replay returns the original blueprint.
    pub create_idempotency: HashMap<String, String>,
    /// `CreateBlueprintVersion` `clientToken` -> (ARN, version) it cut.
    pub version_idempotency: HashMap<String, (String, u64)>,
    /// Every stored DataAutomationProject, keyed by its ARN.
    pub projects: HashMap<String, DataAutomationProjectRecord>,
    /// `projectName` -> `projectArn`; the uniqueness index behind
    /// `CreateDataAutomationProject`'s `ConflictException`.
    pub project_names: HashMap<String, String>,
    /// `CreateDataAutomationProject` `clientToken` -> the ARN that call created, so
    /// an `@idempotencyToken` replay returns the original project.
    pub project_create_idempotency: HashMap<String, String>,
    /// The sequence number the next minted resource ARN carries, for every
    /// resource this store mints one for (a blueprint, a DataAutomationProject). It
    /// counts the creates this store has served and is never rolled back, so every
    /// id it mints differs — two creates inside one nanosecond differ in the
    /// sequence, and deleting a record does not free its id. The whole 64-bit value
    /// goes into the id, so the counter saturates (at a count no in-memory store can
    /// reach) rather than wrapping onto an id already handed out.
    pub next_arn_sequence: u64,
}

impl ComAmazonawsBedrockdataautomationAmazonbedrockkeystonebuildtimeserviceState {
    /// Every stored blueprint in the listing order (`creationTime`, then ARN).
    pub fn blueprints_in_order(&self) -> Vec<&BlueprintRecord> {
        let mut records: Vec<&BlueprintRecord> = self.blueprints.values().collect();
        records.sort_by(|a, b| a.order_key().cmp(&b.order_key()));
        records
    }

    /// Reserve the next blueprint-ARN sequence number, advancing the counter.
    pub fn take_arn_sequence(&mut self) -> u64 {
        let sequence = self.next_arn_sequence;
        self.next_arn_sequence = sequence.saturating_add(1);
        sequence
    }

    /// Forget a blueprint entirely: the record, its name reservation and any
    /// idempotency token that resolves to it.
    pub fn remove_blueprint(&mut self, arn: &str) -> Option<BlueprintRecord> {
        let record = self.blueprints.remove(arn)?;
        self.blueprint_names.remove(&record.blueprint_name);
        self.create_idempotency.retain(|_, stored| stored != arn);
        self.version_idempotency
            .retain(|_, (stored, _)| stored != arn);
        Some(record)
    }

    /// Forget the `CreateBlueprintVersion` client tokens that resolve to one
    /// snapshot, so a released snapshot releases its token with it: a token
    /// entry outlives nothing it points at.
    pub fn forget_version_token(&mut self, arn: &str, version: u64) {
        self.version_idempotency.retain(|_, (stored_arn, stored_version)| {
            stored_arn != arn || *stored_version != version
        });
    }

    /// Forget a DataAutomationProject entirely: the record, its name reservation
    /// and any idempotency token that resolves to it. This is what keeps the store
    /// inside [`PROJECT_QUOTA`] over the life of the process — every record a
    /// create inserts is releasable through `DeleteDataAutomationProject`.
    pub fn remove_project(&mut self, arn: &str) -> Option<DataAutomationProjectRecord> {
        let record = self.projects.remove(arn)?;
        self.project_names.remove(&record.project_name);
        self.project_create_idempotency
            .retain(|_, stored| stored != arn);
        Some(record)
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsBedrockdataautomationAmazonbedrockkeystonebuildtimeserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsBedrockdataautomationAmazonbedrockkeystonebuildtimeserviceState>)
}

/// The region this request was signed for, or [`DEFAULT_REGION`] when it carries
/// no SigV4 credential scope. Only ARN minting needs it; every other operation
/// keys on the ARN the caller supplies.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map_or_else(|| DEFAULT_REGION.to_string(), |scope| scope.region)
}

/// Mint a fresh blueprint ARN for `region`.
///
/// `smithy_query(shape="Blueprint")` constrains `blueprintArn` to
/// `^arn:aws(|-cn|-us-gov):bedrock:[a-zA-Z0-9-]*:(aws|[0-9]{12}):blueprint/(bedrock-data-automation-public-[a-zA-Z0-9-_]{1,30}|[a-zA-Z0-9-]{12,36})$`,
/// so the resource id is 12..36 characters of `[a-zA-Z0-9-]`. The id this mints
/// is 25 of them: the creating instant's nanoseconds-within-the-second, then the
/// store's sequence number in FULL 64-bit width. Uniqueness rests on that
/// sequence alone — it is never rolled back and never truncated here — so no two
/// records of one store ever share an id, whatever the clock does.
pub fn mint_blueprint_arn(
    region: &str,
    created_at: ::aws_smithy_types::DateTime,
    sequence: u64,
) -> String {
    let id = format!("{:08x}-{:016x}", created_at.subsec_nanos(), sequence);
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("blueprint/{id}"))
}

/// Mint a fresh DataAutomationProject ARN for `region`.
///
/// `smithy_query(shape="DataAutomationProject")` constrains `projectArn` to
/// `^arn:aws(|-cn|-us-gov):bedrock:[a-zA-Z0-9-]*:(aws|[0-9]{12}):data-automation-project/[a-zA-Z0-9-]{12,36}$`,
/// so the resource id is 12..36 characters of `[a-zA-Z0-9-]` — the same id space
/// [`mint_blueprint_arn`] mints into, under this service's other resource type, so
/// the two share one id format and one uniqueness argument (the store's sequence
/// number, never rolled back and never truncated).
pub fn mint_project_arn(
    region: &str,
    created_at: ::aws_smithy_types::DateTime,
    sequence: u64,
) -> String {
    let id = format!("{:08x}-{:016x}", created_at.subsec_nanos(), sequence);
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("data-automation-project/{id}"))
}

/// The current wall-clock instant as a modeled `date-time` timestamp.
pub fn now() -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(::std::time::SystemTime::now())
}

/// The current instant in BOTH forms one call needs: the `SystemTime` a stored due
/// instant is computed from and compared against, and the same instant as the
/// modeled `date-time` timestamp a response carries. Read once, so a record's
/// `creationTime` and its due instant cannot come from two different clock reads.
pub fn now_pair() -> (SystemTime, ::aws_smithy_types::DateTime) {
    let at = SystemTime::now();
    (at, ::aws_smithy_types::DateTime::from(at))
}

/// A `ListBlueprints` page cursor: the sort key of the last item the previous
/// page returned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageCursor {
    pub secs: i64,
    pub subsec_nanos: u32,
    pub blueprint_arn: String,
}

impl PageCursor {
    /// The cursor just after `record`.
    pub fn after(record: &BlueprintRecord) -> Self {
        Self {
            secs: record.creation_time.secs(),
            subsec_nanos: record.creation_time.subsec_nanos(),
            blueprint_arn: record.blueprint_arn.clone(),
        }
    }

    /// Is `record` strictly after this cursor in the listing order?
    pub fn precedes(&self, record: &BlueprintRecord) -> bool {
        record.order_key() > (self.secs, self.subsec_nanos, self.blueprint_arn.as_str())
    }

    /// Render as an opaque token. Base64 (the codec the wire protocols already
    /// use for blobs, `aws_smithy_types::base64`) keeps the token inside
    /// `NextToken`'s modeled `^\S*$` pattern and 1..2048 length whatever the ARN
    /// contains.
    ///
    /// The token carries no integrity tag: this emulator's listings are a single
    /// in-process store, the decoded cursor is only compared ordinally in
    /// [`Self::precedes`] and is never used to look up a record, so a client that
    /// edits one can only choose where its OWN listing resumes. Real AWS tokens
    /// are integrity-protected, so a test asserting token opacity will see a
    /// difference here.
    pub fn to_token(&self) -> String {
        ::aws_smithy_types::base64::encode(format!(
            "{}:{}:{}",
            self.secs, self.subsec_nanos, self.blueprint_arn
        ))
    }

    /// Parse a token this service minted. `None` for anything else — the caller
    /// answers that with the modeled `ValidationException`.
    pub fn from_token(token: &str) -> Option<Self> {
        let decoded =
            String::from_utf8(::aws_smithy_types::base64::decode(token).ok()?).ok()?;
        let mut fields = decoded.splitn(3, ':');
        let secs = fields.next()?.parse().ok()?;
        let subsec_nanos = fields.next()?.parse().ok()?;
        let blueprint_arn = fields.next()?;
        if blueprint_arn.is_empty() {
            return None;
        }
        Some(Self {
            secs,
            subsec_nanos,
            blueprint_arn: blueprint_arn.to_string(),
        })
    }
}
