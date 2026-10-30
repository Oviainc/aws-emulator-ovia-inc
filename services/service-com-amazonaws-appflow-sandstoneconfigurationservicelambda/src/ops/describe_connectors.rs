// SPDX-License-Identifier: CC0-1.0

//! @op DescribeConnectors
//! Reports the connector catalog: a `ConnectorConfiguration` for each connector type
//! the request asks about, built from the capabilities AppFlow vends that connector
//! with ([`crate::state::VENDED_CONNECTORS`]), plus the connectors this account
//! registered, read from the store. A request that names no type asks about them all,
//! and the answer is paged.
//!
//! Reporting only: nothing here writes to the store.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DescribeConnectors`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-appflow-sandstoneconfigurationservicelambda/smithy-model/com-amazonaws-appflow-sandstoneconfigurationservicelambda.json")
//! ```
//!
//! Describes the connectors vended by Amazon AppFlow for specified connector types. If you
//! don't specify a connector type, this operation describes all connectors vended by Amazon
//! AppFlow. If there are more connectors than can be returned in one page, the response
//! contains a nextToken object, which can be be passed in to the next call to the
//! DescribeConnectors API operation to retrieve the next page.
//!
//! ## Input
//!
//! * `connectorTypes` (ConnectorTypeList, len 0..100) — The type of connector, such as
//!   Salesforce, Amplitude, and so on.
//! * `maxResults` (MaxResults, range 1..100) — The maximum number of items that should be
//!   returned in the result set. The default is 20.
//! * `nextToken` (NextToken, len 0..2048, pattern ^\S+$) — The pagination token for the next
//!   page of data.
//!
//! ## Output
//!
//! * `connectorConfigurations` (ConnectorConfigurationsMap) — The configuration that is
//!   applied to the connectors used in the flow.
//! * `connectors` (ConnectorList) — Information about the connectors supported in Amazon
//!   AppFlow.
//! * `nextToken` (NextToken, len 0..2048, pattern ^\S+$) — The pagination token for the next
//!   page of data.
//!
//! ## Errors
//!
//! * `InternalServerException` (HTTP 500) — An internal service error occurred during the
//!   processing of your request. Try again later.
//! * `ValidationException` (HTTP 400) — The request has invalid or missing parameters.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DescribeConnectorsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-appflow-sandstoneconfigurationservicelambda.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DescribeConnectorsError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.appflow#DescribeConnectors")]
#[derive(Debug)]
pub enum DescribeConnectorsErrorModeled {
    InternalServerException(InternalServerException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::describe_connectors::DescribeConnectorsError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DescribeConnectorsError = crate::scaffolded::Scaffolded<DescribeConnectorsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DescribeConnectorsErrorModeled> for DescribeConnectorsError {
    fn from(modeled: DescribeConnectorsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DescribeConnectors` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DescribeConnectors<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DescribeConnectors<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DescribeConnectors<Eng>
{
    type Input = DescribeConnectorsInput;
    type Output = DescribeConnectorsOutput;
    type Error = DescribeConnectorsError;

    const NAME: &'static str = "DescribeConnectors";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_CONNECTORS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_CONNECTORS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        _ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            DescribeConnectorsErrorModeled::InternalServerException(InternalServerException {
                message: Some("the AppFlow emulator holds no state on this engine".to_string()),
            })
        })?;

        // The connector types this request asks about. A request that names none — and a
        // request whose list is empty, which names none either — asks about every
        // connector type there is: the whole vended catalog, plus the custom-connector
        // type that stands for the connectors this account registered.
        //
        // Sorted and deduplicated, so the answer does not depend on the order the
        // request happened to list the types in, a type named twice is reported once,
        // and a cursor resumes at a position in a stable sequence.
        let mut requested: Vec<String> = match input.connector_types {
            Some(connector_types) if !connector_types.is_empty() => connector_types,
            _ => crate::state::VENDED_CONNECTORS
                .iter()
                .map(|connector| connector.connector_type.to_string())
                .chain(::std::iter::once(
                    crate::state::CONNECTOR_TYPE_CUSTOM.to_string(),
                ))
                .collect(),
        };
        requested.sort();
        requested.dedup();

        // The cursor names the connector type the next page starts at. A token this
        // listing did not mint is refused rather than silently served from the
        // beginning.
        let start_at = match input.next_token.as_deref() {
            Some(token) => Some(
                crate::state::decode_cursor(crate::state::CONNECTOR_CURSOR, token).ok_or_else(
                    || {
                        DescribeConnectorsErrorModeled::ValidationException(ValidationException {
                            message: Some(
                                "nextToken is not a cursor this operation issued".to_string(),
                            ),
                        })
                    },
                )?,
            ),
            None => None,
        };
        let page_size =
            crate::state::page_size_or(input.max_results, crate::state::DEFAULT_CONNECTOR_PAGE_SIZE);

        // One type past the page is the cursor, so a token is issued only when a type is
        // actually left over and the last page ends the listing.
        let page: Vec<String> = requested
            .into_iter()
            .filter(|connector_type| {
                start_at
                    .as_deref()
                    .is_none_or(|start| connector_type.as_str() >= start)
            })
            .take(page_size + 1)
            .collect();
        let next_token = page.get(page_size).map(|connector_type| {
            crate::state::encode_cursor(crate::state::CONNECTOR_CURSOR, connector_type)
        });
        let page = &page[..page.len().min(page_size)];

        let store = state.lock();
        let mut connector_configurations: ::std::collections::HashMap<
            String,
            ConnectorConfiguration,
        > = ::std::collections::HashMap::new();
        let mut connectors: Vec<ConnectorDetail> = Vec::new();
        for connector_type in page {
            match crate::state::vended_connector(connector_type) {
                // A connector AppFlow vends: its configuration is its catalog entry.
                Some(vended) => {
                    connector_configurations
                        .insert(connector_type.clone(), vended.to_connector_configuration());
                }
                // The custom-connector type names no single connector: a registered
                // connector is identified by its `connectorLabel`, and
                // `connectorConfigurations` is keyed by TYPE, which cannot carry a
                // label. So that type contributes no configuration entry, and the
                // connectors this account registered are reported as the details they
                // were registered with — `DescribeConnector` is the call that names one
                // label and answers with that connector's configuration. The list is as
                // bounded as the store is: the registration slice bounds what it writes
                // there, the way flow storage is bounded.
                None => connectors.extend(store.registered_connectors.values().cloned()),
            }
        }

        Ok(DescribeConnectorsOutput {
            connector_configurations: Some(connector_configurations),
            connectors: Some(connectors),
            next_token,
        })
    }
}
