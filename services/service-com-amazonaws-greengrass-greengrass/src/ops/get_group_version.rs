// SPDX-License-Identifier: CC0-1.0

//! @op GetGroupVersion
//!
//! Reports one stored group version: its ARN, the parent group's id, the version id,
//! the creation timestamp, and `Definition` reconstructed from the component version
//! ARNs that were stored verbatim. A reporting operation: it writes nothing.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), reached through
//! [`crate::state::service_state`]. `ctx` carries request metadata only.
//!
//! ---
//!
//! # Modeled contract for `GetGroupVersion`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-greengrass-greengrass/smithy-model/com-amazonaws-greengrass-greengrass.json")
//! ```
//!
//! Retrieves information about a group version.
//!
//! ## Input
//!
//! * `GroupId` (__string) **required** — The ID of the Greengrass group.
//! * `GroupVersionId` (__string) **required** — The ID of the group version. This value maps
//!   to the ''Version'' property of the corresponding ''VersionInformation'' object, which is
//!   returned by ''ListGroupVersions'' requests. If the version is the last one that was
//!   associated with a group, the value also maps to the ''LatestVersion'' property of the
//!   corresponding ''GroupInformation'' object.
//!
//! ## Output
//!
//! * `Arn` (__string) — The ARN of the group version.
//! * `CreationTimestamp` (__string) — The time, in milliseconds since the epoch, when the
//!   group version was created.
//! * `Definition` (GroupVersion) — Information about the group version definition.
//! * `Id` (__string) — The ID of the group that the version is associated with.
//! * `Version` (__string) — The ID of the group version.
//!
//! ## Errors
//!
//! * `BadRequestException` (HTTP 400) — General error information.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `GetGroupVersionError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-greengrass-greengrass.json");

// The operation's MODELED errors. `InternalServerErrorException` is the service's own
// modelled server error (HTTP 500), widened in through `extra_errors` so the one
// unreachable-store path has a modelled answer.
#[smithy_macros::operation_error(
    "com.amazonaws.greengrass#GetGroupVersion",
    extra_errors = ["InternalServerErrorException"]
)]
#[derive(Debug)]
pub enum GetGroupVersionErrorModeled {
    BadRequestException(BadRequestException),
    InternalServerErrorException(InternalServerErrorException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_group_version::GetGroupVersionError`).
pub type GetGroupVersionError = crate::scaffolded::Scaffolded<GetGroupVersionErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel.
impl From<GetGroupVersionErrorModeled> for GetGroupVersionError {
    fn from(modeled: GetGroupVersionErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetGroupVersion` operation.
pub struct GetGroupVersion<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetGroupVersion<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetGroupVersion<Eng>
{
    type Input = GetGroupVersionInput;
    type Output = GetGroupVersionOutput;
    type Error = GetGroupVersionError;

    const NAME: &'static str = "GetGroupVersion";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_GROUP_VERSION_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_GROUP_VERSION_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            GetGroupVersionErrorModeled::InternalServerErrorException(
                crate::state::store_unreachable(),
            )
        })?;

        let store = state.lock();
        let group = store.groups.get(&input.group_id).ok_or_else(|| {
            GetGroupVersionErrorModeled::BadRequestException(crate::state::bad_request(format!(
                "That Group Definition does not exist: {}",
                input.group_id
            )))
        })?;
        let record = group.version(&input.group_version_id).ok_or_else(|| {
            GetGroupVersionErrorModeled::BadRequestException(crate::state::bad_request(format!(
                "Version {} of Group Definition {} does not exist",
                input.group_version_id, input.group_id
            )))
        })?;

        Ok(GetGroupVersionOutput {
            arn: Some(record.arn.clone()),
            creation_timestamp: Some(record.creation_timestamp.clone()),
            definition: Some(record.definition()),
            id: Some(group.id.clone()),
            version: Some(record.version.clone()),
        })
    }
}
