// SPDX-License-Identifier: CC0-1.0

//! @op DescribeConnector
//! Reports the configuration of the ONE connector the request names: for a connector
//! AppFlow vends, its catalog entry ([`crate::state::VENDED_CONNECTORS`]); for the
//! custom-connector type, the registration this account holds under the
//! `connectorLabel` the request names, read from the store.
//!
//! Reporting only: nothing here writes to the store.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DescribeConnector`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-appflow-sandstoneconfigurationservicelambda/smithy-model/com-amazonaws-appflow-sandstoneconfigurationservicelambda.json")
//! ```
//!
//! Describes the given custom connector registered in your Amazon Web Services account.
//! This API can be used for custom connectors that are registered in your account and also
//! for Amazon authored connectors.
//!
//! ## Input
//!
//! * `connectorLabel` (ConnectorLabel, len 0..256, pattern ^[a-zA-Z0-9][\w!@#.-]+$) — The
//!   label of the connector. The label is unique for each ConnectorRegistration in your
//!   Amazon Web Services account. Only needed if calling for CUSTOMCONNECTOR connector type/.
//! * `connectorType` (ConnectorType, one of: Amplitude | CustomConnector | CustomerProfiles |
//!   Datadog | Dynatrace | EventBridge | Googleanalytics | Honeycode | Infornexus |
//!   LookoutMetrics | Marketo | Pardot | Redshift | S3 | SAPOData | Salesforce | Servicenow |
//!   Singular | Slack | …) **required** — The connector type, such as CUSTOMCONNECTOR,
//!   Saleforce, Marketo. Please choose CUSTOMCONNECTOR for Lambda based custom connectors.
//!
//! ## Output
//!
//! * `connectorConfiguration` (ConnectorConfiguration) — Configuration info of all the
//!   connectors that the user requested.
//!
//! ## Errors
//!
//! * `InternalServerException` (HTTP 500) — An internal service error occurred during the
//!   processing of your request. Try again later.
//! * `ResourceNotFoundException` (HTTP 404) — The resource specified in the request (such as
//!   the source or destination connector profile) is not found.
//! * `ValidationException` (HTTP 400) — The request has invalid or missing parameters.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DescribeConnectorError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-appflow-sandstoneconfigurationservicelambda.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DescribeConnectorError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.appflow#DescribeConnector")]
#[derive(Debug)]
pub enum DescribeConnectorErrorModeled {
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::describe_connector::DescribeConnectorError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DescribeConnectorError = crate::scaffolded::Scaffolded<DescribeConnectorErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DescribeConnectorErrorModeled> for DescribeConnectorError {
    fn from(modeled: DescribeConnectorErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DescribeConnector` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DescribeConnector<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DescribeConnector<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DescribeConnector<Eng>
{
    type Input = DescribeConnectorInput;
    type Output = DescribeConnectorOutput;
    type Error = DescribeConnectorError;

    const NAME: &'static str = "DescribeConnector";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_CONNECTOR_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_CONNECTOR_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            DescribeConnectorErrorModeled::InternalServerException(InternalServerException {
                message: Some("the AppFlow emulator holds no state on this engine".to_string()),
            })
        })?;

        // A connector AppFlow vends is a property of the SERVICE: its configuration is
        // its catalog entry, the same for every caller. A label identifies a
        // registration rather than a vended connector, so it is no part of this lookup.
        if input.connector_type != crate::state::CONNECTOR_TYPE_CUSTOM {
            let vended = crate::state::vended_connector(&input.connector_type).ok_or_else(|| {
                DescribeConnectorErrorModeled::ResourceNotFoundException(
                    ResourceNotFoundException {
                        message: Some(format!(
                            "No connector of type {} is available",
                            input.connector_type
                        )),
                    },
                )
            })?;
            return Ok(DescribeConnectorOutput {
                connector_configuration: Some(vended.to_connector_configuration()),
            });
        }

        // The custom-connector type names no single connector: a label is what
        // identifies one registration in an account, so a request for that type without
        // one names nothing to describe.
        let connector_label = input.connector_label.ok_or_else(|| {
            DescribeConnectorErrorModeled::ValidationException(ValidationException {
                message: Some(format!(
                    "connectorLabel must be specified when connectorType is {}",
                    crate::state::CONNECTOR_TYPE_CUSTOM
                )),
            })
        })?;

        let store = state.lock();
        let detail = store
            .registered_connectors
            .get(&connector_label)
            .ok_or_else(|| {
                DescribeConnectorErrorModeled::ResourceNotFoundException(
                    ResourceNotFoundException {
                        message: Some(format!(
                            "No connector registered under the label {connector_label} exists in this account"
                        )),
                    },
                )
            })?;

        let region = crate::state::request_region(ctx.parts());
        Ok(DescribeConnectorOutput {
            connector_configuration: Some(crate::state::registered_connector_configuration(
                detail, &region,
            )),
        })
    }
}
