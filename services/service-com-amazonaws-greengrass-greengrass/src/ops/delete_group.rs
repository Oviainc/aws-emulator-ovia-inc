// SPDX-License-Identifier: CC0-1.0

//! @op DeleteGroup
//!
//! Removes a group and everything stored under it — its version history and its
//! deployments — so a later `GetGroup` is a `BadRequestException` and `ListGroups`
//! no longer lists it. Deleting a group the service does not hold is a
//! `BadRequestException`, not a silent success.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), reached through
//! [`crate::state::service_state`]. `ctx` carries request metadata only.
//!
//! ---
//!
//! # Modeled contract for `DeleteGroup`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-greengrass-greengrass/smithy-model/com-amazonaws-greengrass-greengrass.json")
//! ```
//!
//! Deletes a group.
//!
//! ## Input
//!
//! * `GroupId` (__string) **required** — The ID of the Greengrass group.
//!
//! ## Errors
//!
//! * `BadRequestException` (HTTP 400) — General error information.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DeleteGroupError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-greengrass-greengrass.json");

// The operation's MODELED errors. `InternalServerErrorException` is the service's own
// modelled server error (HTTP 500), widened in through `extra_errors` so the one
// unreachable-store path has a modelled answer.
#[smithy_macros::operation_error(
    "com.amazonaws.greengrass#DeleteGroup",
    extra_errors = ["InternalServerErrorException"]
)]
#[derive(Debug)]
pub enum DeleteGroupErrorModeled {
    BadRequestException(BadRequestException),
    InternalServerErrorException(InternalServerErrorException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::delete_group::DeleteGroupError`).
pub type DeleteGroupError = crate::scaffolded::Scaffolded<DeleteGroupErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel.
impl From<DeleteGroupErrorModeled> for DeleteGroupError {
    fn from(modeled: DeleteGroupErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DeleteGroup` operation.
pub struct DeleteGroup<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DeleteGroup<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DeleteGroup<Eng>
{
    type Input = DeleteGroupInput;
    type Output = DeleteGroupOutput;
    type Error = DeleteGroupError;

    const NAME: &'static str = "DeleteGroup";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_GROUP_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_GROUP_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            DeleteGroupErrorModeled::InternalServerErrorException(crate::state::store_unreachable())
        })?;

        let mut store = state.lock();
        // Removing the group removes its versions and deployments with it (both are
        // stored on the group record) and forgets the client tokens that named any
        // of them, so a create/delete loop releases everything it allocated.
        if !store.remove_group(&input.group_id) {
            return Err(DeleteGroupErrorModeled::BadRequestException(
                crate::state::bad_request(format!(
                    "That Group Definition does not exist: {}",
                    input.group_id
                )),
            )
            .into());
        }

        Ok(DeleteGroupOutput {})
    }
}
