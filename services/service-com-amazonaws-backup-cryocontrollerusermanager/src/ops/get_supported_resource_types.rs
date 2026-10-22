// SPDX-License-Identifier: CC0-1.0

//! @op GetSupportedResourceTypes
//!
//! HAND-AUTHORED, not scaffolded: the op-stub generator declined this operation
//! because its input is the Smithy `Unit` shape (`src/ops/SKIPPED_STUBS.md`), so
//! until now it was modeled-but-unwired and the framework's
//! unroutable-operation path answered it with an unmodeled HTTP 501. This is the
//! ONLY file to touch for this operation; its two shapes live in
//! `super::types::get_supported_resource_types_types`.
//!
//! ## What it does
//!
//! Capability discovery: it reports the resource types Backup supports, which is
//! a fact about the SERVICE and not about the caller's account, so the answer is
//! the same for every request and this handler writes nothing. The single
//! catalog it projects is [`crate::state::SUPPORTED_RESOURCE_TYPES`], so the
//! types advertised here and the types the rest of the service names cannot
//! drift apart.
//!
//! ---
//!
//! # Modeled contract for `GetSupportedResourceTypes`
//!
//! From this service's Smithy model. For the FULL text of any one item, ask the
//! model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-backup-cryocontrollerusermanager/smithy-model/com-amazonaws-backup-cryocontrollerusermanager.json")
//! ```
//!
//! Returns the Amazon Web Services resource types supported by Backup.
//!
//! Route: `GET /supported-resource-types -> 200`.
//!
//! ## Input
//!
//! (no members — the input is `smithy.api#Unit`)
//!
//! ## Output
//!
//! * `ResourceTypes` (ResourceTypes) — Contains a string with the supported
//!   Amazon Web Services resource types: `Aurora` for Amazon Aurora,
//!   `CloudFormation` for CloudFormation, `DocumentDB` for Amazon DocumentDB
//!   (with MongoDB compatibility), `DynamoDB` for Amazon DynamoDB, `EBS` for
//!   Amazon Elastic Block Store, `EC2` for Amazon Elastic Compute Cloud, `EFS`
//!   for Amazon Elastic File System, `EKS` for Amazon Elastic Kubernetes
//!   Service, `FSx` for Amazon FSx, `Neptune` for Amazon Neptune, `RDS` for
//!   Amazon Relational Database Service, `Redshift` for Amazon Redshift, `S3`
//!   for Amazon Simple Storage Service, `SAP HANA on Amazon EC2` for SAP HANA
//!   databases on Amazon Elastic Compute Cloud instances, `Storage Gateway` for
//!   Storage Gateway, `Timestream` for Amazon Timestream, `VirtualMachine` for
//!   VMware virtual machines. The catalog this emulator reports adds the two
//!   newer services that documentation predates — see
//!   [`crate::state::SUPPORTED_RESOURCE_TYPES`].
//!
//! ## Errors
//!
//! * `ServiceUnavailableException` (HTTP 500) — The request failed due to a
//!   temporary failure of the server.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `GetSupportedResourceTypesError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-backup-cryocontrollerusermanager.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation.
#[smithy_macros::operation_error("com.amazonaws.backup#GetSupportedResourceTypes")]
#[derive(Debug)]
pub enum GetSupportedResourceTypesErrorModeled {
    ServiceUnavailableException(ServiceUnavailableException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_supported_resource_types::GetSupportedResourceTypesError`).
pub type GetSupportedResourceTypesError =
    crate::scaffolded::Scaffolded<GetSupportedResourceTypesErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so the
/// handler writes `Err(SomeModeledError.into())` rather than naming the wrapper
/// variant.
impl From<GetSupportedResourceTypesErrorModeled> for GetSupportedResourceTypesError {
    fn from(modeled: GetSupportedResourceTypesErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetSupportedResourceTypes` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so
/// this crate names no concrete engine type.
pub struct GetSupportedResourceTypes<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetSupportedResourceTypes<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine
    /// is a construction-time dependency, so engine access is guaranteed by the
    /// time any request runs. Called by the generated `EmulatedService::new`.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetSupportedResourceTypes<Eng>
{
    type Input = GetSupportedResourceTypesInput;
    type Output = GetSupportedResourceTypesOutput;
    type Error = GetSupportedResourceTypesError;

    const NAME: &'static str = "GetSupportedResourceTypes";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::GET_SUPPORTED_RESOURCE_TYPES_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::GET_SUPPORTED_RESOURCE_TYPES_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // The input is `Unit` and the catalog is a service fact, so neither the
        // request metadata nor the account's stored resources can change the
        // answer. The engine is still reached the one legal way, so a request
        // served without this service's store answers its modelled
        // `ServiceUnavailableException` rather than a list from nowhere. The
        // handle is not a lock, so binding it holds nothing: there is no
        // per-account record of a supported resource type to read, and a
        // reporting operation writes nothing.
        let _ = (&input, ctx);
        let _state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            GetSupportedResourceTypesErrorModeled::ServiceUnavailableException(
                ServiceUnavailableException {
                    message: Some("the backup service state is unavailable".to_string()),
                    ..Default::default()
                },
            )
        })?;

        Ok(GetSupportedResourceTypesOutput {
            resource_types: Some(
                crate::state::SUPPORTED_RESOURCE_TYPES
                    .iter()
                    .map(|resource_type| (*resource_type).to_string())
                    .collect(),
            ),
        })
    }
}
