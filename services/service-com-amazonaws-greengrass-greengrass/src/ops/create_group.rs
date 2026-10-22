// SPDX-License-Identifier: CC0-1.0

//! @op CreateGroup
//!
//! Creates a Greengrass group: mints the group id, derives its ARN from that
//! stored id, and — when `InitialVersion` is supplied — stores it as the group's
//! first version and points `LatestVersion`/`LatestVersionArn` at it. With no
//! `InitialVersion` the group has no version yet, so both latest-version members
//! are absent rather than fabricated.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), reached through
//! [`crate::state::service_state`]. `ctx` carries request metadata only.
//!
//! ---
//!
//! # Modeled contract for `CreateGroup`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-greengrass-greengrass/smithy-model/com-amazonaws-greengrass-greengrass.json")
//! ```
//!
//! Creates a group. You may provide the initial version of the group or use
//! ''CreateGroupVersion'' at a later time. Tip: You can use the ''gg_group_setup'' package
//! (https://github.com/awslabs/aws-greengrass-group-setup) as a library or command-line
//! application to create and deploy Greengrass groups.
//!
//! ## Input
//!
//! * `AmznClientToken` (__string) — A client token used to correlate requests and responses.
//! * `InitialVersion` (GroupVersion) — Information about the initial version of the group.
//! * `Name` (__string) **required** — The name of the group.
//! * `tags` (Tags) — Tag(s) to add to the new resource.
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
//!
//! ## Errors
//!
//! * `BadRequestException` (HTTP 400) — General error information.

#[allow(unused_imports)]
use super::types::*;

use crate::state::{Group, GroupVersionRecord};

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `CreateGroupError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-greengrass-greengrass.json");

// The operation's MODELED errors. `InternalServerErrorException` is the service's own
// modelled server error (HTTP 500) and is widened in through `extra_errors` so the one
// unreachable-store path has a modelled answer instead of a 501 or a bogus 400.
#[smithy_macros::operation_error(
    "com.amazonaws.greengrass#CreateGroup",
    extra_errors = ["InternalServerErrorException"]
)]
#[derive(Debug)]
pub enum CreateGroupErrorModeled {
    BadRequestException(BadRequestException),
    InternalServerErrorException(InternalServerErrorException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::create_group::CreateGroupError`). The wrapper is kept so this
/// operation can still refuse a request in a shape its own model cannot express.
pub type CreateGroupError = crate::scaffolded::Scaffolded<CreateGroupErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant.
impl From<CreateGroupErrorModeled> for CreateGroupError {
    fn from(modeled: CreateGroupErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CreateGroup` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct CreateGroup<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreateGroup<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

/// The create response for a stored group, read wholly off the stored record.
fn output_for(group: &Group) -> CreateGroupOutput {
    CreateGroupOutput {
        arn: Some(group.arn.clone()),
        creation_timestamp: Some(group.creation_timestamp.clone()),
        id: Some(group.id.clone()),
        last_updated_timestamp: Some(group.last_updated_timestamp.clone()),
        latest_version: group.latest_version.clone(),
        latest_version_arn: group.latest_version_arn.clone(),
        name: Some(group.name.clone()),
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreateGroup<Eng>
{
    type Input = CreateGroupInput;
    type Output = CreateGroupOutput;
    type Error = CreateGroupError;

    const NAME: &'static str = "CreateGroup";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_GROUP_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_GROUP_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            CreateGroupErrorModeled::InternalServerErrorException(crate::state::store_unreachable())
        })?;

        // `Name` is `@required` but `clientOptional`, so an omitted or blank name
        // reaches the handler as an empty string. A group has to be nameable.
        if input.name.trim().is_empty() {
            return Err(CreateGroupErrorModeled::BadRequestException(
                crate::state::bad_request("Name must be a non-empty string"),
            )
            .into());
        }

        let region = crate::state::request_region(ctx.parts());
        let token_key = input
            .amzn_client_token
            .as_deref()
            .filter(|token| !token.is_empty())
            // `CreateGroup` is account-scoped: there is no parent resource to key on.
            .map(|token| crate::state::client_token_key(Self::NAME, "", token));

        let mut store = state.lock();

        // A retried create carrying the same client token answers with the group the
        // first attempt created, rather than minting a second one.
        let replayed = token_key
            .as_deref()
            .and_then(|key| store.client_tokens.resource_for(key))
            .cloned();
        if let Some(group) = replayed.and_then(|id| store.groups.get(&id)) {
            return Ok(output_for(group));
        }

        // The store is bounded: an emulator is a long-lived process, so a caller
        // cannot grow it without limit.
        if store.groups.len() >= crate::state::MAX_GROUPS {
            return Err(CreateGroupErrorModeled::BadRequestException(
                crate::state::bad_request(format!(
                    "This account already holds the maximum of {} groups",
                    crate::state::MAX_GROUPS
                )),
            )
            .into());
        }

        let timestamp = crate::state::now_timestamp();
        let id = crate::state::new_id();
        let arn = crate::state::group_arn(&region, &id);
        let seq = store.take_group_seq();
        let mut group = Group {
            id: id.clone(),
            arn: arn.clone(),
            name: input.name.clone(),
            creation_timestamp: timestamp.clone(),
            last_updated_timestamp: timestamp.clone(),
            latest_version: None,
            latest_version_arn: None,
            tags: input.tags.clone().unwrap_or_default(),
            versions: Vec::new(),
            deployments: Vec::new(),
            role_arn: None,
            seq,
        };

        // An `InitialVersion` becomes the group's first stored version; without one
        // the group has no version, so the latest-version members stay absent.
        if let Some(initial) = input.initial_version.as_ref() {
            let version = crate::state::new_id();
            let version_arn = crate::state::group_version_arn(&arn, &version);
            group.push_version(GroupVersionRecord::from_definition(
                version,
                version_arn,
                timestamp.clone(),
                initial,
            ));
        }

        let output = output_for(&group);
        store.groups.insert(id.clone(), group);
        if let Some(key) = token_key {
            store.client_tokens.remember(key, id);
        }
        Ok(output)
    }
}
