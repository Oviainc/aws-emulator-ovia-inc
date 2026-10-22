// SPDX-License-Identifier: CC0-1.0

//! @op GetGroup
//!
//! Reports the stored group record: the id, ARN, name, timestamps, latest-version
//! pointers and tags exactly as `CreateGroup`/`CreateGroupVersion`/`TagResource`
//! persisted them — never an echo of the current request. A reporting operation:
//! it writes nothing.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), reached through
//! [`crate::state::service_state`]. `ctx` carries request metadata only.
//!
//! ---
//!
//! # Modeled contract for `GetGroup`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-greengrass-greengrass/smithy-model/com-amazonaws-greengrass-greengrass.json")
//! ```
//!
//! Retrieves information about a group.
//!
//! ## Input
//!
//! * `GroupId` (__string) **required** — The ID of the Greengrass group.
//!
//! ## Output
//!
//! * `Arn` (__string) — The ARN of the definition.
//! * `CreationTimestamp` (__string) — The time, in milliseconds since the epoch, when the
//!   definition was created.
//! * `Id` (__string) — The ID of the definition.
//! * `LastUpdatedTimestamp` (__string) — The time, in milliseconds since the epoch, when the
//!   definition was last updated.
//! * `LatestVersion` (__string) — The ID of the latest version associated with the
//!   definition.
//! * `LatestVersionArn` (__string) — The ARN of the latest version associated with the
//!   definition.
//! * `Name` (__string) — The name of the definition.
//! * `tags` (Tags) — Tag(s) attached to the resource arn.
//!
//! ## Errors
//!
//! * `BadRequestException` (HTTP 400) — General error information.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `GetGroupError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-greengrass-greengrass.json");

// The operation's MODELED errors. `InternalServerErrorException` is the service's own
// modelled server error (HTTP 500), widened in through `extra_errors` so the one
// unreachable-store path has a modelled answer.
#[smithy_macros::operation_error(
    "com.amazonaws.greengrass#GetGroup",
    extra_errors = ["InternalServerErrorException"]
)]
#[derive(Debug)]
pub enum GetGroupErrorModeled {
    BadRequestException(BadRequestException),
    InternalServerErrorException(InternalServerErrorException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_group::GetGroupError`).
pub type GetGroupError = crate::scaffolded::Scaffolded<GetGroupErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel.
impl From<GetGroupErrorModeled> for GetGroupError {
    fn from(modeled: GetGroupErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetGroup` operation.
pub struct GetGroup<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetGroup<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetGroup<Eng>
{
    type Input = GetGroupInput;
    type Output = GetGroupOutput;
    type Error = GetGroupError;

    const NAME: &'static str = "GetGroup";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_GROUP_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_GROUP_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            GetGroupErrorModeled::InternalServerErrorException(crate::state::store_unreachable())
        })?;

        let store = state.lock();
        let group = store.groups.get(&input.group_id).ok_or_else(|| {
            GetGroupErrorModeled::BadRequestException(crate::state::bad_request(format!(
                "That Group Definition does not exist: {}",
                input.group_id
            )))
        })?;

        Ok(GetGroupOutput {
            arn: Some(group.arn.clone()),
            creation_timestamp: Some(group.creation_timestamp.clone()),
            id: Some(group.id.clone()),
            last_updated_timestamp: Some(group.last_updated_timestamp.clone()),
            latest_version: group.latest_version.clone(),
            latest_version_arn: group.latest_version_arn.clone(),
            name: Some(group.name.clone()),
            // An untagged group reports no `tags` member at all, rather than an
            // empty map the caller would have to distinguish from "no tags".
            tags: (!group.tags.is_empty()).then(|| group.tags.clone()),
        })
    }
}
