// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-appflow-sandstoneconfigurationservicelambda service.
//!
//! The AppFlow control plane this emulator serves is a store of FLOWS, each one
//! carrying the definition `CreateFlow` persisted plus the run history `StartFlow`
//! appends. Every operation computes its answer from this store: nothing here is
//! derived from the request that is reading it.
//!
//! The modelled shapes (`TriggerConfig`, `SourceFlowConfig`, `Task`, …) are stored as
//! they arrived rather than re-modelled, so `DescribeFlow` can return exactly what
//! `CreateFlow` was given.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    DestinationFlowConfig, ErrorInfo, ExecutionDetails, ExecutionResult, FlowDefinition,
    MetadataCatalogConfig, SourceFlowConfig, Task, TriggerConfig,
};

/// The region an ARN carries when the request is not SigV4-signed (an anonymous
/// request names no region). Generated emulators default to `us-east-1`; see
/// `emulator_server::emulator_arn`.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The `triggerType` of a flow AppFlow runs only when asked
/// (`smithy_query(query="shape", shape="TriggerType")`).
pub const TRIGGER_TYPE_ON_DEMAND: &str = "OnDemand";

/// The `triggerType` of a flow AppFlow runs on a schedule
/// (`smithy_query(query="shape", shape="TriggerType")`).
pub const TRIGGER_TYPE_SCHEDULED: &str = "Scheduled";

/// How long after `StartFlow` a run stops being `InProgress`.
///
/// CHOSEN, not documented: AppFlow publishes no duration for a flow run, so this is
/// the emulator's own span — long enough that a caller which polls straight after
/// `StartFlow` still observes the `InProgress` run (and the state errors that go with
/// it), short enough that a test need not wait. The instant is stored on the run and
/// materialised by whichever call reads it next; nothing sleeps.
pub const EXECUTION_SETTLES_AFTER: Duration = Duration::from_millis(500);

/// Why a flow run cannot succeed in the emulator.
///
/// A flow run is a data transfer between the source and destination connector
/// endpoints — an outbound connection the emulator does not make — so no transfer
/// ever happens and reporting a successful one would be a fabricated result. The run
/// is therefore recorded, and then ends in the service's own modelled failure state
/// (`ExecutionStatus::Error`) carrying this reason.
///
/// The wording stays inside both message patterns it has to satisfy:
/// `ErrorInfo$executionMessage` (`^[\s\w/!@#+=.-]*$`) and
/// `ExecutionDetails$mostRecentExecutionMessage` (`^[\w!@#\-.?,\s]*$`) — so no comma,
/// colon, slash or apostrophe.
pub const EXECUTION_NOT_PERFORMED_MESSAGE: &str = "This emulator does not connect to source or destination connector endpoints so the flow run was not performed.";

/// The listing a `ListFlows` cursor belongs to.
pub const FLOW_CURSOR: &str = "flows";

/// The listing a `DescribeFlowExecutionRecords` cursor belongs to.
pub const EXECUTION_CURSOR: &str = "flow-executions";

/// The most flows one account may hold, which `CreateFlow` refuses to exceed with the
/// modelled `ServiceQuotaExceededException` ("The request would cause a service quota
/// (such as the number of flows) to be exceeded").
///
/// CHOSEN, not documented: AppFlow does publish a per-account flow quota, but it is
/// adjustable and not stated in the model, so this is the value this emulator enforces
/// rather than a cited one. It also bounds
/// [`ComAmazonawsAppflowSandstoneconfigurationservicelambdaState::flows`], so a
/// long-lived process cannot grow without limit.
pub const MAX_FLOWS_PER_ACCOUNT: usize = 1000;

/// How many runs one flow keeps.
///
/// CHOSEN: AppFlow publishes no retention for flow-run records, so this is the
/// emulator's own window. It bounds [`Flow::executions`] — and with it every per-flow
/// walk, so a flow that has been started many times still costs a bounded amount of
/// work and memory. The OLDEST record is dropped first, so the runs a caller is most
/// likely to read are the ones kept.
pub const MAX_EXECUTIONS_PER_FLOW: usize = 100;

/// A flow's `flowStatus` (`smithy_query(query="shape", shape="FlowStatus")`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlowStatus {
    Active,
    Deleted,
    Deprecated,
    Draft,
    Errored,
    Suspended,
}

impl FlowStatus {
    /// The wire value, exactly as the `FlowStatus` enum declares it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "Active",
            Self::Deleted => "Deleted",
            Self::Deprecated => "Deprecated",
            Self::Draft => "Draft",
            Self::Errored => "Errored",
            Self::Suspended => "Suspended",
        }
    }
}

/// A run's `executionStatus` (`smithy_query(query="shape", shape="ExecutionStatus")`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionStatus {
    CancelStarted,
    Canceled,
    Error,
    InProgress,
    Successful,
}

impl ExecutionStatus {
    /// The wire value, exactly as the `ExecutionStatus` enum declares it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CancelStarted => "CancelStarted",
            Self::Canceled => "Canceled",
            Self::Error => "Error",
            Self::InProgress => "InProgress",
            Self::Successful => "Successful",
        }
    }
}

/// One recorded run of a flow, as `StartFlow` appended it.
#[derive(Debug, Clone)]
pub struct FlowExecution {
    pub execution_id: String,
    /// The `@idempotencyToken` of the `StartFlow` that recorded this run, when it
    /// carried one. Held HERE rather than in a store-wide map, so the token is
    /// reclaimed with the run it identifies and cannot accumulate without bound.
    pub client_token: Option<String>,
    pub execution_status: ExecutionStatus,
    pub execution_result: Option<ExecutionResult>,
    pub started_at: SystemTime,
    pub last_updated_at: SystemTime,
    /// When the clock owes this run its terminal status. `None` once settled.
    pub settles_at: Option<SystemTime>,
    pub data_pull_start_time: Option<SystemTime>,
    pub data_pull_end_time: Option<SystemTime>,
}

impl FlowExecution {
    /// The wire `ExecutionRecord` for this run.
    pub fn to_execution_record(&self) -> crate::ops::types::ExecutionRecord {
        crate::ops::types::ExecutionRecord {
            execution_id: Some(self.execution_id.clone()),
            execution_status: Some(self.execution_status.as_str().to_string()),
            execution_result: self.execution_result.clone(),
            started_at: Some(to_date_time(self.started_at)),
            last_updated_at: Some(to_date_time(self.last_updated_at)),
            data_pull_start_time: self.data_pull_start_time.map(to_date_time),
            data_pull_end_time: self.data_pull_end_time.map(to_date_time),
            // No catalog registration happens: nothing was transferred to catalog.
            metadata_catalog_details: None,
        }
    }
}

/// A flow: the definition `CreateFlow` persisted, plus its run history.
#[derive(Debug, Clone)]
pub struct Flow {
    pub flow_arn: String,
    pub flow_name: String,
    /// The `@idempotencyToken` of the `CreateFlow` that stored this flow, when it
    /// carried one. Held HERE rather than in a store-wide map, so the token is
    /// reclaimed by `DeleteFlow` with the flow it identifies.
    pub client_token: Option<String>,
    pub description: Option<String>,
    pub kms_arn: Option<String>,
    pub flow_status: FlowStatus,
    pub flow_status_message: Option<String>,
    pub trigger_config: TriggerConfig,
    pub source_flow_config: SourceFlowConfig,
    pub destination_flow_config_list: Vec<DestinationFlowConfig>,
    pub tasks: Vec<Task>,
    pub tags: BTreeMap<String, String>,
    pub metadata_catalog_config: Option<MetadataCatalogConfig>,
    pub created_at: SystemTime,
    pub last_updated_at: SystemTime,
    pub created_by: String,
    pub last_updated_by: String,
    pub schema_version: i64,
    /// Oldest first, as `StartFlow` appended them.
    pub executions: Vec<FlowExecution>,
}

impl Flow {
    /// `true` when the flow is run only on request, rather than on a schedule or an
    /// event.
    pub fn is_on_demand(&self) -> bool {
        self.trigger_config.trigger_type == TRIGGER_TYPE_ON_DEMAND
    }

    /// The DEFINITION this flow was created with, as one comparable value, so a retried
    /// create carrying the same `@idempotencyToken` can be told apart from a different
    /// request reusing that token. Computed from the stored members rather than held
    /// beside them, so the two cannot disagree.
    pub fn definition_descriptor(&self) -> Option<::serde_json::Value> {
        definition_descriptor(
            &self.flow_name,
            self.description.as_deref(),
            self.kms_arn.as_deref(),
            &self.trigger_config,
            &self.source_flow_config,
            &self.destination_flow_config_list,
            &self.tasks,
            &self.tags,
            self.metadata_catalog_config.as_ref(),
        )
    }

    /// Give every run the terminal status the CLOCK already owed it.
    ///
    /// This is the one write a reporting operation may make: the transition was due at
    /// `settles_at` whether or not anybody called, so materialising it on read reports
    /// the state the clock had already reached — it never moves a status because
    /// something was read.
    pub fn settle_due_executions(&mut self, now: SystemTime) {
        for execution in &mut self.executions {
            let Some(due) = execution.settles_at else {
                continue;
            };
            if due > now {
                continue;
            }
            execution.settles_at = None;
            execution.execution_status = ExecutionStatus::Error;
            execution.last_updated_at = due;
            // No data was pulled, so there is no data-pull window to report: both
            // instants stay `None` rather than describing a transfer that never ran.
            execution.execution_result = Some(ExecutionResult {
                error_info: Some(ErrorInfo {
                    put_failures_count: Some(0),
                    execution_message: Some(EXECUTION_NOT_PERFORMED_MESSAGE.to_string()),
                }),
                // Nothing was transferred, so every counter is a true zero rather
                // than an invented volume.
                bytes_processed: Some(0),
                bytes_written: Some(0),
                records_processed: Some(0),
                num_parallel_processes: None,
                max_page_size: None,
            });
        }
    }

    /// Record a run, keeping at most [`MAX_EXECUTIONS_PER_FLOW`] of them.
    pub fn record_execution(&mut self, execution: FlowExecution) {
        self.executions.push(execution);
        if self.executions.len() > MAX_EXECUTIONS_PER_FLOW {
            let overflow = self.executions.len() - MAX_EXECUTIONS_PER_FLOW;
            self.executions.drain(..overflow);
        }
    }

    /// The run a repeated `StartFlow` token already recorded, if any.
    pub fn execution_for_client_token(&self, client_token: &str) -> Option<&FlowExecution> {
        self.executions
            .iter()
            .find(|execution| execution.client_token.as_deref() == Some(client_token))
    }

    /// `true` while one of this flow's runs is still in progress.
    ///
    /// Call [`Flow::settle_due_executions`] first, or a run the clock has already
    /// finished still reads as running.
    pub fn has_running_execution(&self) -> bool {
        self.executions
            .iter()
            .any(|execution| execution.execution_status == ExecutionStatus::InProgress)
    }

    /// This flow's runs, most recent first — the order `DescribeFlowExecutionRecords`
    /// reports them in.
    pub fn executions_newest_first(&self) -> Vec<&FlowExecution> {
        let mut ordered: Vec<&FlowExecution> = self.executions.iter().collect();
        ordered.sort_by(|a, b| {
            b.started_at
                .cmp(&a.started_at)
                .then_with(|| b.execution_id.cmp(&a.execution_id))
        });
        ordered
    }

    /// The most recent run, or `None` when the flow has never run.
    ///
    /// Picks the maximum rather than ordering the whole history, so reading one flow's
    /// summary allocates nothing.
    pub fn last_run(&self) -> Option<&FlowExecution> {
        self.executions.iter().max_by(|a, b| {
            a.started_at
                .cmp(&b.started_at)
                .then_with(|| a.execution_id.cmp(&b.execution_id))
        })
    }

    /// `lastRunExecutionDetails`, derived from the most recent run so the detail and
    /// the run it summarises cannot disagree.
    pub fn last_run_execution_details(&self) -> Option<ExecutionDetails> {
        let last = self.last_run()?;
        Some(ExecutionDetails {
            most_recent_execution_message: last
                .execution_result
                .as_ref()
                .and_then(|result| result.error_info.as_ref())
                .and_then(|info| info.execution_message.clone()),
            most_recent_execution_time: Some(to_date_time(last.last_updated_at)),
            most_recent_execution_status: Some(last.execution_status.as_str().to_string()),
        })
    }

    /// The connector type data flows FROM, as the stored source configuration names it.
    pub fn source_connector_type(&self) -> String {
        self.source_flow_config.connector_type.clone()
    }

    /// The connector type data flows TO, as the stored destination configuration names
    /// it. AppFlow transfers to one destination at a time, so the first entry is the
    /// flow's destination.
    pub fn destination_connector_type(&self) -> Option<String> {
        self.destination_flow_config_list
            .first()
            .map(|config| config.connector_type.clone())
    }

    /// The wire `FlowDefinition` for this flow — the summary `ListFlows` returns.
    pub fn to_flow_definition(&self) -> FlowDefinition {
        FlowDefinition {
            flow_arn: Some(self.flow_arn.clone()),
            description: self.description.clone(),
            flow_name: Some(self.flow_name.clone()),
            flow_status: Some(self.flow_status.as_str().to_string()),
            source_connector_type: Some(self.source_connector_type()),
            // A connector LABEL names a registered custom connector, which lives on a
            // connector profile; profile and connector registration are not part of
            // this slice, so there is nothing stored to label a flow with.
            source_connector_label: None,
            destination_connector_type: self.destination_connector_type(),
            destination_connector_label: None,
            trigger_type: Some(self.trigger_config.trigger_type.clone()),
            created_at: Some(to_date_time(self.created_at)),
            last_updated_at: Some(to_date_time(self.last_updated_at)),
            created_by: Some(self.created_by.clone()),
            last_updated_by: Some(self.last_updated_by.clone()),
            tags: Some(self.tags.iter().map(|(k, v)| (k.clone(), v.clone())).collect()),
            last_run_execution_details: self.last_run_execution_details(),
        }
    }
}

/// Per-service state for com-amazonaws-appflow-sandstoneconfigurationservicelambda. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsAppflowSandstoneconfigurationservicelambdaState {
    /// Every flow in the account, keyed by `flowName` — which AppFlow treats as the
    /// flow's unique name. `BTreeMap`, so `ListFlows` pages in a stable order, and
    /// bounded by [`MAX_FLOWS_PER_ACCOUNT`].
    pub flows: BTreeMap<String, Flow>,
    /// Reserved for the connector-profile slice (`CreateConnectorProfile`,
    /// `DescribeConnectorProfiles`), which is still scaffolded — nothing writes this
    /// yet, and the slice that does must bound it the way [`Flow`] storage is bounded.
    pub connector_profiles: BTreeMap<String, crate::ops::types::ConnectorProfile>,
    /// Reserved for the connector-registration slice (`RegisterConnector`), which is
    /// still scaffolded. Keyed by `connectorLabel`.
    pub registered_connectors: BTreeMap<String, crate::ops::types::ConnectorDetail>,
}

impl ComAmazonawsAppflowSandstoneconfigurationservicelambdaState {
    /// The flow a repeated `CreateFlow` `@idempotencyToken` already stored, if any.
    ///
    /// Scans the flows, which [`MAX_FLOWS_PER_ACCOUNT`] bounds, instead of consulting a
    /// store-wide token map that nothing would ever reclaim.
    pub fn flow_for_client_token(&self, client_token: &str) -> Option<&Flow> {
        self.flows
            .values()
            .find(|flow| flow.client_token.as_deref() == Some(client_token))
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsAppflowSandstoneconfigurationservicelambdaState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsAppflowSandstoneconfigurationservicelambdaState>)
}

/// A stored instant as the modelled `timestamp` the wire carries.
///
/// Converted with the SDK's own `From<SystemTime>`, which keeps the seconds and the
/// subsecond nanoseconds the instant was stored with — a round trip through `f64`
/// seconds would silently drop sub-microsecond resolution for a present-day epoch.
pub fn to_date_time(at: SystemTime) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(at)
}

/// The region this request was signed for; [`DEFAULT_REGION`] when it was not signed.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The ARN AppFlow records in `createdBy` / `lastUpdatedBy`: the caller, identified by
/// the access key id its request was signed with.
pub fn caller_arn(parts: &::http::request::Parts) -> String {
    let access_key_id = ::emulator_server::extract_akid(&parts.headers);
    ::emulator_server::emulator_arn("iam", "", &format!("user/{access_key_id}"))
}

/// A flow's ARN: `arn:aws:appflow:<region>:<account>:flow/<flowName>`.
pub fn flow_arn(region: &str, flow_name: &str) -> String {
    ::emulator_server::emulator_arn("appflow", region, &format!("flow/{flow_name}"))
}

/// One modelled shape as the JSON `Value` the service's own codec renders for it.
///
/// Rendering through the codec is what makes two shapes comparable at all: several of
/// them hold `std::collections::HashMap` members (`Task$taskProperties`, a connector's
/// `customProperties`, …) whose iteration order comes from a per-instance hash seed, so
/// neither `Debug` nor the rendered bytes of two equal values match. `serde_json::Value`
/// then supplies the equality: its objects compare by member, not by order, whichever
/// map backs them, so no local canonicalisation is needed — and lists keep their order,
/// which is right, because `tasks` and `destinationFlowConfigList` are sequences.
///
/// A value the codec cannot render answers `None` — the comparison is then impossible
/// rather than false, which the caller reports as a server fault instead of blaming the
/// request.
fn rendered_shape(
    schema: &::aws_smithy_schema::Schema,
    value: &dyn ::aws_smithy_schema::serde::SerializableStruct,
) -> Option<::serde_json::Value> {
    use ::aws_smithy_schema::serde::ShapeSerializer;

    let mut serializer = ::aws_smithy_schema_server::json::JsonShapeSerializer::new();
    if let Err(error) = serializer.write_struct(schema, value) {
        ::tracing::warn!(
            %error,
            "the JSON codec could not render a stored shape, so two flow definitions cannot be compared"
        );
        return None;
    }
    let rendered = serializer.into_string();
    match ::serde_json::from_str::<::serde_json::Value>(&rendered) {
        Ok(parsed) => Some(parsed),
        Err(error) => {
            ::tracing::warn!(
                %error,
                "the JSON this service renders for a stored shape did not parse back"
            );
            None
        }
    }
}

/// The flow definition one `CreateFlow` carries, as one comparable value: every member
/// the operation persists, each modelled shape rendered by the service's own codec.
///
/// It exists for ONE comparison — the contract's "If you specify input parameters that
/// differ from your first request, an error occurs" — and `serde_json::Value`'s own
/// equality decides it: two definitions are equal exactly when every member is. Compared
/// in FULL rather than digested, so no collision can ever replay a success for a
/// definition that was never stored; it is built on demand for that one comparison,
/// never stored and never put on the wire, which is also why the modelled shapes need no
/// `Hash` or `PartialEq` of their own.
#[allow(clippy::too_many_arguments)]
pub fn definition_descriptor(
    flow_name: &str,
    description: Option<&str>,
    kms_arn: Option<&str>,
    trigger_config: &TriggerConfig,
    source_flow_config: &SourceFlowConfig,
    destination_flow_config_list: &[DestinationFlowConfig],
    tasks: &[Task],
    tags: &BTreeMap<String, String>,
    metadata_catalog_config: Option<&MetadataCatalogConfig>,
) -> Option<::serde_json::Value> {
    let destinations = destination_flow_config_list
        .iter()
        .map(|config| rendered_shape(&DestinationFlowConfig::SCHEMA, config))
        .collect::<Option<Vec<_>>>()?;
    let tasks = tasks
        .iter()
        .map(|task| rendered_shape(&Task::SCHEMA, task))
        .collect::<Option<Vec<_>>>()?;
    let trigger = rendered_shape(&TriggerConfig::SCHEMA, trigger_config)?;
    let source = rendered_shape(&SourceFlowConfig::SCHEMA, source_flow_config)?;
    let catalog = match metadata_catalog_config {
        Some(config) => Some(rendered_shape(&MetadataCatalogConfig::SCHEMA, config)?),
        None => None,
    };
    Some(::serde_json::json!({
        "flowName": flow_name,
        "description": description,
        "kmsArn": kms_arn,
        "tags": tags,
        "triggerConfig": trigger,
        "sourceFlowConfig": source,
        "destinationFlowConfigList": destinations,
        "tasks": tasks,
        "metadataCatalogConfig": catalog,
    }))
}

/// A fresh run identifier. Opaque and unique, matching `ExecutionId`'s `^\S+$`
/// (`smithy_query(query="shape", shape="ExecutionId")`: len 0..256, `^\S+$`, which a
/// canonical hyphenated UUID satisfies).
///
/// The randomness is the `uuid` crate's v4 generator, reached through
/// `emulator_telemetry`'s uuid-backed id newtype (`BatchId::random()` is
/// `uuid::Uuid::new_v4()` rendered canonically) — a crate this service ALREADY depends
/// on — rather than by declaring a second path to the same crate. Nothing here is a
/// hand-rolled generator, and this service therefore adds no `Cargo.lock` entry, which
/// is what lets it land beside the other fixes merging into that one lock file
/// concurrently.
pub fn new_execution_id() -> String {
    ::emulator_telemetry::BatchId::random().as_str().to_string()
}

/// The smallest page the model allows a caller to ask for (`MaxResults`'s lower
/// `@range` bound, `smithy_query(query="shape", shape="MaxResults")`).
pub const MIN_PAGE_SIZE: usize = 1;

/// The largest page the model allows a caller to ask for (`MaxResults`'s upper
/// `@range` bound, `smithy_query(query="shape", shape="MaxResults")`).
pub const MAX_PAGE_SIZE: usize = 100;

/// The largest page a list operation returns when the caller names no `maxResults`.
///
/// An unbounded request is served the biggest page the model allows.
pub const DEFAULT_PAGE_SIZE: usize = MAX_PAGE_SIZE;

/// The page size to serve for a `maxResults`.
///
/// A value outside the modelled `MaxResults` range is REFUSED before this runs: the
/// framework checks `@range` at the request boundary and answers 400
/// `ValidationException`, so a `maxResults` of 0 or 101 never reaches a handler (pinned
/// by `tests/all/flow_lifecycle.rs`). The clamp here is therefore defence in depth, not
/// the enforcement: it guarantees the page this process builds is bounded whatever
/// value arrives, and it clamps to the SAME constants the model's bounds are recorded
/// in, so the two cannot drift apart.
pub fn page_size(max_results: Option<i32>) -> usize {
    match max_results {
        Some(requested) => (requested.max(0) as usize).clamp(MIN_PAGE_SIZE, MAX_PAGE_SIZE),
        None => DEFAULT_PAGE_SIZE,
    }
}

/// Encode a pagination cursor: the `kind` of listing and the key the NEXT page starts
/// at, as one opaque token.
///
/// Encoded for OPACITY and shape — a client reads the token as a blob, and the standard
/// base64 alphabet (`A-Za-z0-9+/=`) carries no whitespace, so the token satisfies
/// `NextToken`'s `^\S+$` (`smithy_query(query="shape", shape="NextToken")`) whatever the
/// key contains. The codec is `aws_smithy_types::base64`, the one the wire serializer
/// already uses for every Smithy blob and already in this crate's dependency tree, so no
/// second encoder (and no `Cargo.lock` entry) is added for it.
///
/// It is deliberately NOT integrity-protected and claims no tamper resistance: base64 is
/// reversible, so a caller can mint one. Nothing downstream trusts it beyond choosing
/// where this account's own listing resumes, and a token another listing minted (or one
/// that does not decode) is refused rather than silently served from the start.
pub fn encode_cursor(kind: &str, start_at: &str) -> String {
    ::aws_smithy_types::base64::encode(format!("{kind}\u{1f}{start_at}"))
}

/// The key a cursor resumes at, or `None` when the token was not minted by `kind`.
pub fn decode_cursor(kind: &str, token: &str) -> Option<String> {
    let decoded = ::aws_smithy_types::base64::decode(token).ok()?;
    let decoded = String::from_utf8(decoded).ok()?;
    let (token_kind, start_at) = decoded.split_once('\u{1f}')?;
    (token_kind == kind).then(|| start_at.to_string())
}
