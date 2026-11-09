// SPDX-License-Identifier: CC0-1.0

//! @op DescribeFlow
//! Answers with the PERSISTED flow record — the definition `CreateFlow` stored and the
//! run history `StartFlow` appended — never with an echo of the request.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DescribeFlow`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-appflow-sandstoneconfigurationservicelambda/smithy-model/com-amazonaws-appflow-sandstoneconfigurationservicelambda.json")
//! ```
//!
//! Provides a description of the specified flow.
//!
//! ## Input
//!
//! * `flowName` (FlowName, len 0..256, pattern ^[a-zA-Z0-9][\w!@#.-]+$) **required** — The
//!   specified name of the flow. Spaces are not allowed. Use underscores (_) or hyphens (-)
//!   only.
//!
//! ## Output
//!
//! * `createdAt` (Date) — Specifies when the flow was created.
//! * `createdBy` (CreatedBy, len 0..256, pattern ^\S+$) — The ARN of the user who created the
//!   flow.
//! * `description` (FlowDescription, len 0..2048, pattern ^[\w!@#\-.?,\s]*$) — A description
//!   of the flow.
//! * `destinationFlowConfigList` (DestinationFlowConfigList) — The configuration that
//!   controls how Amazon AppFlow transfers data to the destination connector.
//! * `flowArn` (FlowArn, len 0..512, pattern ^arn:aws:appflow:.*:[0-9]+:) — The flow's Amazon
//!   Resource Name (ARN).
//! * `flowName` (FlowName, len 0..256, pattern ^[a-zA-Z0-9][\w!@#.-]+$) — The specified name
//!   of the flow. Spaces are not allowed. Use underscores (_) or hyphens (-) only.
//! * `flowStatus` (FlowStatus, one of: Active | Deleted | Deprecated | Draft | Errored |
//!   Suspended) — Indicates the current status of the flow.
//! * `flowStatusMessage` (FlowStatusMessage, len 0..2048, pattern ^[\s\w/!@#+=.-]*$) —
//!   Contains an error message if the flow status is in a suspended or error state. This
//!   applies only to scheduled or event-triggered flows.
//! * `kmsArn` (KMSArn, len 20..2048, pattern ^arn:aws:kms:.*:[0-9]+:) — The ARN (Amazon
//!   Resource Name) of the Key Management Service (KMS) key you provide for encryption. This
//!   is required if you do not want to use the Amazon AppFlow-managed KMS key. If you don't
//!   provide anything here, Amazon AppFlow uses the Amazon AppFlow-managed KMS key.
//! * `lastRunExecutionDetails` (ExecutionDetails) — Describes the details of the most recent
//!   flow run.
//! * `lastRunMetadataCatalogDetails` (MetadataCatalogDetails) — Describes the metadata
//!   catalog, metadata table, and data partitions that Amazon AppFlow used for the associated
//!   flow run.
//! * `lastUpdatedAt` (Date) — Specifies when the flow was last updated.
//! * `lastUpdatedBy` (UpdatedBy, len 0..256, pattern ^\S+$) — Specifies the user name of the
//!   account that performed the most recent update.
//! * `metadataCatalogConfig` (MetadataCatalogConfig) — Specifies the configuration that
//!   Amazon AppFlow uses when it catalogs the data that's transferred by the associated flow.
//!   When Amazon AppFlow catalogs the data from a flow, it stores metadata in a data catalog.
//! * `schemaVersion` (Long) — The version number of your data schema. Amazon AppFlow assigns
//!   this version number. The version number increases by one when you change any of the
//!   following settings in your flow configuration: Source-to-destination field mappings
//!   Field data types Partition keys
//! * `sourceFlowConfig` (SourceFlowConfig) — The configuration that controls how Amazon
//!   AppFlow retrieves data from the source connector.
//! * `tags` (TagMap, len 0..50) — The tags used to organize, track, or control access for
//!   your flow.
//! * `tasks` (Tasks) — A list of tasks that Amazon AppFlow performs while transferring the
//!   data in the flow run.
//! * `triggerConfig` (TriggerConfig) — The trigger settings that determine how and when the
//!   flow runs.
//!
//! ## Errors
//!
//! * `InternalServerException` (HTTP 500) — An internal service error occurred during the
//!   processing of your request. Try again later.
//! * `ResourceNotFoundException` (HTTP 404) — The resource specified in the request (such as
//!   the source or destination connector profile) is not found.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DescribeFlowError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-appflow-sandstoneconfigurationservicelambda.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DescribeFlowError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.appflow#DescribeFlow")]
#[derive(Debug)]
pub enum DescribeFlowErrorModeled {
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::describe_flow::DescribeFlowError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DescribeFlowError = crate::scaffolded::Scaffolded<DescribeFlowErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DescribeFlowErrorModeled> for DescribeFlowError {
    fn from(modeled: DescribeFlowErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DescribeFlow` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DescribeFlow<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DescribeFlow<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DescribeFlow<Eng>
{
    type Input = DescribeFlowInput;
    type Output = DescribeFlowOutput;
    type Error = DescribeFlowError;

    const NAME: &'static str = "DescribeFlow";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_FLOW_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_FLOW_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        _ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            DescribeFlowErrorModeled::InternalServerException(InternalServerException {
                message: Some("the AppFlow emulator holds no state on this engine".to_string()),
            })
        })?;

        let mut store = state.lock();
        let flow = store.flows.get_mut(&input.flow_name).ok_or_else(|| {
            DescribeFlowErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
                message: Some(crate::state::no_such_flow(&input.flow_name)),
            })
        })?;

        // Reporting, not mutating: this only materialises the run transition the clock
        // had already made due, so the answer is the state at this instant.
        flow.settle_due_executions(::std::time::SystemTime::now());

        Ok(DescribeFlowOutput {
            flow_arn: Some(flow.flow_arn.clone()),
            description: flow.description.clone(),
            flow_name: Some(flow.flow_name.clone()),
            kms_arn: flow.kms_arn.clone(),
            flow_status: Some(flow.flow_status.as_str().to_string()),
            flow_status_message: flow.flow_status_message.clone(),
            source_flow_config: Some(flow.source_flow_config.clone()),
            destination_flow_config_list: Some(flow.destination_flow_config_list.clone()),
            last_run_execution_details: flow.last_run_execution_details(),
            trigger_config: Some(flow.trigger_config.clone()),
            tasks: Some(flow.tasks.clone()),
            created_at: Some(crate::state::to_date_time(flow.created_at)),
            last_updated_at: Some(crate::state::to_date_time(flow.last_updated_at)),
            created_by: Some(flow.created_by.clone()),
            last_updated_by: Some(flow.last_updated_by.clone()),
            tags: Some(
                flow.tags
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            ),
            metadata_catalog_config: flow.metadata_catalog_config.clone(),
            // The catalog details of the last run: no run registers a catalog table,
            // because no run transfers data.
            last_run_metadata_catalog_details: None,
            schema_version: Some(flow.schema_version),
        })
    }
}
