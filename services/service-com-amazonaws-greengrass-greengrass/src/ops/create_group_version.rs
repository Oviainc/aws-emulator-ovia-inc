// SPDX-License-Identifier: CC0-1.0

//! @op CreateGroupVersion
//!
//! Appends an immutable version to a group that already exists: a fresh version id,
//! a version ARN derived from the parent group's stored ARN, and the seven component
//! version ARNs stored verbatim. The parent group's `LatestVersion`,
//! `LatestVersionArn` and `LastUpdatedTimestamp` advance to the new version.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), reached through
//! [`crate::state::service_state`]. `ctx` carries request metadata only.
//!
//! ---
//!
//! # Modeled contract for `CreateGroupVersion`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-greengrass-greengrass/smithy-model/com-amazonaws-greengrass-greengrass.json")
//! ```
//!
//! Creates a version of a group which has already been defined.
//!
//! ## Input
//!
//! * `AmznClientToken` (__string) — A client token used to correlate requests and responses.
//! * `ConnectorDefinitionVersionArn` (__string) — The ARN of the connector definition version
//!   for this group.
//! * `CoreDefinitionVersionArn` (__string) — The ARN of the core definition version for this
//!   group.
//! * `DeviceDefinitionVersionArn` (__string) — The ARN of the device definition version for
//!   this group.
//! * `FunctionDefinitionVersionArn` (__string) — The ARN of the function definition version
//!   for this group.
//! * `GroupId` (__string) **required** — The ID of the Greengrass group.
//! * `LoggerDefinitionVersionArn` (__string) — The ARN of the logger definition version for
//!   this group.
//! * `ResourceDefinitionVersionArn` (__string) — The ARN of the resource definition version
//!   for this group.
//! * `SubscriptionDefinitionVersionArn` (__string) — The ARN of the subscription definition
//!   version for this group.
//!
//! ## Output
//!
//! * `Arn` (__string) — The ARN of the version.
//! * `CreationTimestamp` (__string) — The time, in milliseconds since the epoch, when the
//!   version was created.
//! * `Id` (__string) — The ID of the parent definition that the version is associated with.
//! * `Version` (__string) — The ID of the version.
//!
//! ## Errors
//!
//! * `BadRequestException` (HTTP 400) — General error information.

#[allow(unused_imports)]
use super::types::*;

use crate::state::GroupVersionRecord;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `CreateGroupVersionError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-greengrass-greengrass.json");

// The operation's MODELED errors. `InternalServerErrorException` is the service's own
// modelled server error (HTTP 500), widened in through `extra_errors` so the one
// unreachable-store path has a modelled answer.
#[smithy_macros::operation_error(
    "com.amazonaws.greengrass#CreateGroupVersion",
    extra_errors = ["InternalServerErrorException"]
)]
#[derive(Debug)]
pub enum CreateGroupVersionErrorModeled {
    BadRequestException(BadRequestException),
    InternalServerErrorException(InternalServerErrorException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::create_group_version::CreateGroupVersionError`).
pub type CreateGroupVersionError = crate::scaffolded::Scaffolded<CreateGroupVersionErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel.
impl From<CreateGroupVersionErrorModeled> for CreateGroupVersionError {
    fn from(modeled: CreateGroupVersionErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CreateGroupVersion` operation.
pub struct CreateGroupVersion<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreateGroupVersion<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreateGroupVersion<Eng>
{
    type Input = CreateGroupVersionInput;
    type Output = CreateGroupVersionOutput;
    type Error = CreateGroupVersionError;

    const NAME: &'static str = "CreateGroupVersion";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_GROUP_VERSION_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_GROUP_VERSION_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            CreateGroupVersionErrorModeled::InternalServerErrorException(
                crate::state::store_unreachable(),
            )
        })?;

        let token_key = input
            .amzn_client_token
            .as_deref()
            .filter(|token| !token.is_empty())
            // Scoped to the parent group: the same token against another group names
            // another version.
            .map(|token| crate::state::client_token_key(Self::NAME, &input.group_id, token));

        let mut store = state.lock();

        // A version cannot be added to a group the service does not hold.
        if !store.groups.contains_key(&input.group_id) {
            return Err(CreateGroupVersionErrorModeled::BadRequestException(
                crate::state::bad_request(format!(
                    "That Group Definition does not exist: {}",
                    input.group_id
                )),
            )
            .into());
        }

        // A retried create carrying the same client token answers with the version
        // the first attempt stored, rather than appending a second one.
        let replayed = token_key
            .as_deref()
            .and_then(|key| store.client_tokens.resource_for(key))
            .cloned();
        if let Some(record) = replayed.and_then(|version| {
            store
                .groups
                .get(&input.group_id)
                .and_then(|group| group.version(&version))
        }) {
            return Ok(CreateGroupVersionOutput {
                arn: Some(record.arn.clone()),
                creation_timestamp: Some(record.creation_timestamp.clone()),
                id: Some(input.group_id.clone()),
                version: Some(record.version.clone()),
            });
        }

        let timestamp = crate::state::now_timestamp();
        let version = crate::state::new_id();
        let group = store
            .groups
            .get_mut(&input.group_id)
            .expect("group presence checked above");

        // A group's version history is bounded: an emulator is a long-lived
        // process, so a caller cannot grow one group without limit.
        if group.versions.len() >= crate::state::MAX_VERSIONS_PER_GROUP {
            return Err(CreateGroupVersionErrorModeled::BadRequestException(
                crate::state::bad_request(format!(
                    "Group {} already holds the maximum of {} versions",
                    input.group_id,
                    crate::state::MAX_VERSIONS_PER_GROUP
                )),
            )
            .into());
        }
        let version_arn = crate::state::group_version_arn(&group.arn, &version);
        let record = GroupVersionRecord {
            version: version.clone(),
            arn: version_arn.clone(),
            creation_timestamp: timestamp.clone(),
            core_definition_version_arn: input.core_definition_version_arn.clone(),
            function_definition_version_arn: input.function_definition_version_arn.clone(),
            subscription_definition_version_arn: input.subscription_definition_version_arn.clone(),
            device_definition_version_arn: input.device_definition_version_arn.clone(),
            logger_definition_version_arn: input.logger_definition_version_arn.clone(),
            resource_definition_version_arn: input.resource_definition_version_arn.clone(),
            connector_definition_version_arn: input.connector_definition_version_arn.clone(),
        };
        group.push_version(record);

        if let Some(key) = token_key {
            store.client_tokens.remember(key, version.clone());
        }

        Ok(CreateGroupVersionOutput {
            arn: Some(version_arn),
            creation_timestamp: Some(timestamp),
            id: Some(input.group_id),
            version: Some(version),
        })
    }
}
