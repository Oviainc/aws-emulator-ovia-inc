// SPDX-License-Identifier: CC0-1.0

//! @op CreatePlan
//!
//! Mints a Region switch plan and persists it in this service's store, then answers with
//! the plan read back from that store. The ARN is minted from the store's monotonic seed
//! (`crate::state::plan_arn`), and the server-owned `owner`, `version` and `updatedAt`
//! come from the record — nothing in the response is echoed straight from the request.
//!
//! A plan's two Regions have to be two DIFFERENT Regions (the model constrains only their
//! count), so a request naming one Region twice is refused with a 400
//! `ValidationException` and nothing is stored — see
//! [`crate::state::region_pair_violation`], the one reading of that rule.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request. Reach
//! the store through [`crate::state::service_state`] and read and write it through the
//! guard it hands back. `ctx` ([`smithy_server_runtime::OperationContext`]) carries
//! request metadata only.
//!
//! ---
//!
//! # Modeled contract for `CreatePlan`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-arcregionswitch-arcregionswitch/smithy-model/com-amazonaws-arcregionswitch-arcregionswitch.json")
//! ```
//!
//! Creates a new Region switch plan. A plan defines the steps required to shift traffic
//! from one Amazon Web Services Region to another. You must specify a name for the plan,
//! the primary Region, and at least one additional Region. You can also provide a
//! description, execution role, recovery time objective, associated alarms, triggers, and
//! workflows that define the steps to execute during a Region switch.
//!
//! ## Input
//!
//! * `associatedAlarms` (AssociatedAlarmMap) — The alarms associated with a Region switch
//!   plan.
//! * `description` (String) — The description of a Region switch plan.
//! * `executionRole` (IamRoleArn, pattern ^arn:aws[a-zA-Z0-9-]*:iam::[0-9]{12}:role/.+$)
//!   **required** — An execution role is a way to categorize a Region switch plan.
//! * `name` (PlanName, len 1..32, pattern ^[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,30}[a-zA-Z0-9])?$)
//!   **required** — The name of a Region switch plan.
//! * `primaryRegion` (Region, pattern ^[a-z]{2}-[a-z-]+-\d+$) — The primary Amazon Web
//!   Services Region for the application. This is the Region where the application normally
//!   runs before any Region switch occurs.
//! * `recoveryApproach` (RecoveryApproach, one of: activeActive | activePassive) **required**
//!   — The recovery approach for a Region switch plan, which can be active/active
//!   (activeActive) or active/passive (activePassive).
//! * `recoveryTimeObjectiveMinutes` (Integer) — Optionally, you can specify an recovery time
//!   objective for a Region switch plan, in minutes.
//! * `regions` (RegionList, len 2..2) **required** — An array that specifies the Amazon Web
//!   Services Regions for a Region switch plan. Specify two Regions.
//! * `reportConfiguration` (ReportConfiguration) — Configuration for automatic report
//!   generation for plan executions. When configured, Region switch automatically generates a
//!   report after each plan execution that includes execution events, plan configuration, and
//!   CloudWatch alarm states.
//! * `serviceQuotaChecksEnabled` (Boolean) — Specifies whether to enable service quota checks
//!   for the Region switch plan.
//! * `tags` (Tags, len 0..200) — The tags to apply to the Region switch plan.
//! * `triggers` (TriggerList) — The triggers associated with a Region switch plan.
//! * `workflows` (WorkflowList) **required** — An array of workflows included in a Region
//!   switch plan.
//!
//! ## Output
//!
//! * `plan` (Plan) — The details of the created Region switch plan.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `CreatePlanError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-arcregionswitch-arcregionswitch.json");

// The operation's MODELED errors. `CreatePlan` itself declares none, so two shapes the
// SERVICE models (and other operations declare) are widened in through `extra_errors`:
// `InternalServerException`, for an unreachable shared store, and
// `IllegalStateException` — "the current state of the resource doesn't allow the
// operation to proceed" — for a store already holding `crate::state::MAX_PLANS` plans.
#[smithy_macros::operation_error(
    "com.amazonaws.arcregionswitch#CreatePlan",
    extra_errors = ["IllegalStateException", "InternalServerException"]
)]
#[derive(Debug)]
pub enum CreatePlanErrorModeled {
    IllegalStateException(IllegalStateException),
    InternalServerException(InternalServerException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::create_plan::CreatePlanError`). The wrapper is also the only place this
/// operation can refuse a bad request its own model declares no error for.
pub type CreatePlanError = crate::scaffolded::Scaffolded<CreatePlanErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant.
impl From<CreatePlanErrorModeled> for CreatePlanError {
    fn from(modeled: CreatePlanErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CreatePlan` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type.
pub struct CreatePlan<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreatePlan<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreatePlan<Eng>
{
    type Input = CreatePlanInput;
    type Output = CreatePlanOutput;
    type Error = CreatePlanError;

    const NAME: &'static str = "CreatePlan";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_PLAN_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_PLAN_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(CreatePlanErrorModeled::InternalServerException(
                InternalServerException {
                    message: "the Region switch plan store is unavailable".to_string(),
                },
            )
            .into());
        };

        let now = ::std::time::SystemTime::now();
        let mut store = state.lock();
        store.settle(now);

        // The store is bounded: this emulator authenticates nobody, so it refuses rather
        // than letting a caller grow it without limit. `DeletePlan` is the release path.
        if store.plans.len() >= crate::state::MAX_PLANS {
            return Err(CreatePlanErrorModeled::IllegalStateException(
                IllegalStateException {
                    message: format!(
                        "This emulator already holds the maximum of {} Region switch plans; \
                         delete one before creating another",
                        crate::state::MAX_PLANS
                    ),
                },
            )
            .into());
        }

        // The two Regions of a plan name one switch between them, so they have to be two
        // different Regions. The model constrains the COUNT (`regions` is
        // `@length(2, 2)`, already refused at the boundary) but not their distinctness,
        // so nothing before this handler refuses a pair that names one Region twice —
        // and such a plan describes no switch at all. Refused as a 400
        // `ValidationException`, with nothing stored.
        if let Some(violation) = crate::state::region_pair_violation(&input.regions) {
            return Err(crate::scaffolded::Scaffolded::validation(violation));
        }

        // The ARN suffix comes from the store's monotonic seed, so two plans that share a
        // name still get distinct ARNs and a plan's ARN never changes.
        let seed = store.next_seed();
        let arn = crate::state::plan_arn(&input.name, seed);

        let record = crate::state::StoredPlan {
            plan: Plan {
                arn: arn.clone(),
                description: input.description,
                workflows: input.workflows,
                execution_role: input.execution_role,
                recovery_time_objective_minutes: input.recovery_time_objective_minutes,
                associated_alarms: input.associated_alarms,
                triggers: input.triggers,
                report_configuration: input.report_configuration,
                service_quota_checks_enabled: input.service_quota_checks_enabled,
                name: input.name,
                regions: input.regions,
                recovery_approach: input.recovery_approach,
                primary_region: input.primary_region,
                // Server-owned: the account that owns the plan, and when it last changed.
                owner: ::engine::emulator_account_id().to_string(),
                // `version` is derived from the record's revision by `as_plan`.
                version: None,
                updated_at: Some(::aws_smithy_types::DateTime::from(now)),
            },
            revision: 1,
            tags: input.tags.unwrap_or_default().into_iter().collect(),
            executions: Vec::new(),
            active_execution: None,
        };
        store.plans.insert(arn.clone(), record);

        // Project the response from the STORED record, so what the caller sees is what a
        // later GetPlan will read back.
        let Some(created) = store.plans.get(&arn) else {
            return Err(CreatePlanErrorModeled::InternalServerException(
                InternalServerException {
                    message: format!("the plan {arn} could not be read back after it was stored"),
                },
            )
            .into());
        };
        Ok(CreatePlanOutput {
            plan: Some(created.as_plan()),
        })
    }
}
