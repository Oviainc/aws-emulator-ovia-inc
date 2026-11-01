// SPDX-License-Identifier: CC0-1.0

//! @op CreateHarness
//!
//! Mints a harness and PERSISTS it in [`crate::state`]: the agent-loop configuration the
//! caller supplied, the AgentCore Runtime environment the service manages for it, and the
//! `CREATING` -> `READY` transition the clock owes it. Every later read of the harness
//! ([`crate::ops::get_harness`]) comes from that record.
//!
//! HAND-AUTHORED, not generated: the scaffold's op-stub generator declined this operation
//! ("shape missing from model: smithy.api#Unit", see `src/ops/SKIPPED_STUBS.md`), so it
//! was not in the service's `operations(..)` allow-list at all — and a route the model
//! binds but the service does not wire is not claimed by any service, which the router
//! answers `404 UnknownService` in its own envelope. Its shapes live in
//! `types/harness_types.rs`.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `CreateHarness::new(engine)` and drives it through the centralized request walk:
//! deserialize → validate → `handle` → serialize. Only the `handle` body is this file's.
//!
//! Every modelled input constraint — `harnessName`'s `@pattern`, `executionRoleArn`'s,
//! `clientToken`'s `@length` — is enforced by that walk BEFORE `handle` runs, and refused
//! with HTTP 400 `ValidationException`. Reaching `handle` means the input satisfies all of
//! them.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request. Reach the
//! store through [`crate::state::service_state`] and read and write it through the guard it
//! hands back. `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata
//! only — the engine is NOT reached through `ctx`.
//!
//! ---
//!
//! # Modeled contract for `CreateHarness`
//!
//! From this service's Smithy model
//! (`smithy_query(query="operation",operation="CreateHarness",model_path="services/service-com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol/smithy-model/com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol.json")`).
//!
//! Operation to create a harness. `POST /harnesses`, success status 201.
//!
//! ## Input
//!
//! * `harnessName` (HarnessName, pattern ^[a-zA-Z][a-zA-Z0-9_]{0,39}$) **required** — The
//!   name of the harness.
//! * `executionRoleArn` (RoleArn, len 1..2048, pattern
//!   ^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/.+$) **required** — The role the harness
//!   assumes when running.
//! * `clientToken` (ClientToken, len 33..256, @idempotencyToken) — ensures the request
//!   completes no more than one time.
//! * `environment` (HarnessEnvironmentProviderRequest) — the compute environment, including
//!   network and lifecycle settings.
//! * `environmentArtifact` (HarnessEnvironmentArtifact) — a custom container image.
//! * `environmentVariables` (EnvironmentVariablesMap, len 0..50, @sensitive)
//! * `authorizerConfiguration` (AuthorizerConfiguration)
//! * `model` (HarnessModelConfiguration) — the model provider the agent loop calls.
//! * `systemPrompt` (HarnessSystemPrompt), `tools` (HarnessTools), `skills` (HarnessSkills),
//!   `allowedTools` (HarnessAllowedTools), `hooks` (HarnessHooks, max 20, @uniqueItems)
//! * `memory` (HarnessMemoryConfiguration), `truncation` (HarnessTruncationConfiguration)
//! * `maxIterations`, `maxTokens`, `timeoutSeconds` (Integer)
//! * `tags` (TagsMap, len 0..50)
//!
//! ## Output
//!
//! * `harness` (Harness) **required** — the harness that was created.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — the request is denied per access permissions.
//! * `ConflictException` (HTTP 409) — the request conflicts with the resource's state.
//! * `InternalServerException` (HTTP 500) — an unexpected error during processing.
//! * `ServiceQuotaExceededException` (HTTP 402) — a service quota was exceeded.
//! * `ThrottlingException` (HTTP 429) — the number of requests exceeds the limit.
//! * `ValidationException` (HTTP 400) — the input fails to satisfy the service's
//!   constraints.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `CreateHarnessError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol.json");

/// The operation's MODELED errors. `#[operation_error]` accepts only variants the
/// model declares for this operation.
#[smithy_macros::operation_error("com.amazonaws.bedrockagentcorecontrol#CreateHarness")]
#[derive(Debug)]
pub enum CreateHarnessErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ServiceQuotaExceededException(ServiceQuotaExceededException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::create_harness::CreateHarnessError`).
pub type CreateHarnessError = crate::scaffolded::Scaffolded<CreateHarnessErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel.
impl From<CreateHarnessErrorModeled> for CreateHarnessError {
    fn from(modeled: CreateHarnessErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CreateHarness` operation.
pub struct CreateHarness<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreateHarness<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreateHarness<Eng>
{
    type Input = CreateHarnessInput;
    type Output = CreateHarnessOutput;
    type Error = CreateHarnessError;

    const NAME: &'static str = "CreateHarness";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::CREATE_HARNESS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::CREATE_HARNESS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 201u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            CreateHarnessErrorModeled::InternalServerException(InternalServerException {
                message: Some("the harness store is unavailable".to_string()),
            })
        })?;
        let region = crate::state::request_region(ctx.parts());
        let now = ::std::time::SystemTime::now();
        let mut store = state.lock();

        // A replayed request carrying the same `clientToken` answers with the harness that
        // token first produced, rather than minting a second one.
        if let Some(harness_id) = store.replayed_harness_result(Self::NAME, input.client_token.as_deref())
        {
            if let Some(record) = store.harnesses.get(&harness_id) {
                return Ok(CreateHarnessOutput {
                    harness: record.harness(now),
                });
            }
        }

        // `Harness.model` is `@required` on every response while the request's is optional,
        // and this service has no default model to run a harness with, so a request that
        // names none is refused with the operation's own modelled error rather than served
        // with an invented model id.
        let model = input.model.ok_or_else(|| {
            CreateHarnessErrorModeled::ValidationException(crate::state::field_validation_failed(
                crate::state::HARNESS_MODEL_MEMBER,
                crate::state::HARNESS_MODEL_REQUIRED,
            ))
        })?;

        // One harness per name, so a repeated name is a conflict, not a second harness that
        // later reads cannot tell apart.
        if let Some(existing) = store.harness_names.get(&input.harness_name) {
            return Err(CreateHarnessErrorModeled::ConflictException(ConflictException {
                message: Some(format!(
                    "a harness named {} already exists (harnessId {existing})",
                    input.harness_name
                )),
            })
            .into());
        }

        // The store this request would grow is bounded, so a caller looping on create meets
        // the modelled quota instead of this process's memory limit.
        if store.harnesses.len() >= crate::state::MAX_HARNESSES {
            return Err(CreateHarnessErrorModeled::ServiceQuotaExceededException(
                ServiceQuotaExceededException {
                    message: Some(format!(
                        "this account already holds the maximum of {} harnesses",
                        crate::state::MAX_HARNESSES
                    )),
                },
            )
            .into());
        }

        let harness_id = store.mint_harness_id(&input.harness_name, now);
        let record = crate::state::HarnessRecord {
            harness_arn: crate::state::harness_arn(&region, &harness_id),
            harness_name: input.harness_name.clone(),
            created_at: now,
            updated_at: now,
            version: crate::state::FIRST_HARNESS_VERSION,
            // A new harness is created, not ready: the transition to READY is owed by the
            // clock, not by this call. `HarnessStatus` declares the same members
            // `AgentRuntimeStatus` does, so the two resources share these constants.
            transient_status: crate::state::STATUS_CREATING.to_string(),
            settled_status: crate::state::STATUS_READY.to_string(),
            settles_at: now + crate::state::HARNESS_SETTLE_DELAY,
            execution_role_arn: input.execution_role_arn,
            model,
            // Each of these is `@required` on the response and optional on the request, so
            // a harness configured without them reports the empty list the caller asked
            // for rather than omitting a member the wire requires.
            system_prompt: input.system_prompt.unwrap_or_default(),
            tools: input.tools.unwrap_or_default(),
            skills: input.skills.unwrap_or_default(),
            allowed_tools: input.allowed_tools.unwrap_or_default(),
            truncation: crate::state::effective_truncation_configuration(input.truncation),
            environment: crate::state::harness_runtime_environment(
                &region,
                &harness_id,
                &input.harness_name,
                input.environment,
            ),
            environment_artifact: input.environment_artifact,
            environment_variables: crate::state::sorted_map(input.environment_variables),
            authorizer_configuration: input.authorizer_configuration,
            memory: input.memory,
            hooks: input.hooks.unwrap_or_default(),
            max_iterations: input.max_iterations,
            max_tokens: input.max_tokens,
            timeout_seconds: input.timeout_seconds,
            tags: crate::state::sorted_map(input.tags),
            harness_id: harness_id.clone(),
            // A harness is created with no endpoints: only
            // `CreateHarnessEndpoint` adds one.
            endpoints: ::std::collections::BTreeMap::new(),
        };
        let output = CreateHarnessOutput {
            harness: record.harness(now),
        };

        store
            .harness_names
            .insert(input.harness_name, harness_id.clone());
        store.remember_harness_client_token(Self::NAME, input.client_token, &harness_id);
        store.harnesses.insert(harness_id, record);

        Ok(output)
    }
}
