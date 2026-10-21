Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 34e6e58e29d15417fc6ff05c31f4f08593fa6d9a

# Bootstrap the Auto Scaling Plans (AnyScaleScalingPlannerFrontendService) emulator

_Filed: 2026-10-21T21:18:39Z_
_Operation: service-com-amazonaws-autoscalingplans-anyscalescalingplannerfrontendservice | Class: stub_

> _Note: this issue was identified against the code base at commit `34e6e58e29d15417fc6ff05c31f4f08593fa6d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 6 handlers under `services/service-com-amazonaws-autoscalingplans-anyscalescalingplannerfrontendservice/src/ops/` is an unmodified scaffold: each carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and its `handle` body ends in `Err(crate::scaffolded::Scaffolded::not_implemented(..))`, i.e. HTTP 501 with zero persisted state. This issue defines the FIRST buildable slice: the scaling-plan lifecycle, implemented statefully over `crate::state::service_state`.

Wire protocol (from OPERATIONS.md): `awsJson1_1`, `POST /` with `X-Amz-Target: AnyScaleScalingPlannerFrontendService.<OperationName>`, sigv4 signing name `autoscaling-plans`.

## Happy path to implement (in order)

1. **CreateScalingPlan** — input `ScalingPlanName` (required, len 1..128, pattern `^[\p{Print}&&[^|:/]]+$`), `ApplicationSource` (required: `CloudFormationStackARN` or `TagFilters`), `ScalingInstructions` (required, list<ScalingInstruction>). Output `ScalingPlanVersion` (long, required) — AWS documents this as always `1`. Must INSERT a new plan record keyed by plan name, storing the application source, the scaling instructions verbatim, `StatusCode = Active`, and `CreationTime`/`StatusStartTime` set to now. Creating a name that already exists must fail (`ValidationException`, HTTP 400), not silently overwrite.
2. **DescribeScalingPlans** — input `ScalingPlanNames` (list<string>, optional), `ScalingPlanVersion` (long, optional), `ApplicationSources` (optional), `MaxResults`, `NextToken`. Output `ScalingPlans` (list<ScalingPlan>), `NextToken`. Must return the plans READ BACK FROM STATE — each `ScalingPlan` populated with the required members `ScalingPlanName`, `ScalingPlanVersion`, `ApplicationSource`, `ScalingInstructions`, `StatusCode`, plus `StatusStartTime`/`CreationTime` as stored. With no filter, return all plans; with `ScalingPlanNames`, return only those. Honour `MaxResults` and round-trip `NextToken` (an unparseable token ⇒ `InvalidNextTokenException`, HTTP 400).
3. **DescribeScalingPlanResources** — input `ScalingPlanName` + `ScalingPlanVersion` (both required), `MaxResults`, `NextToken`. Output `ScalingPlanResources` (list<ScalingPlanResource>), `NextToken`. Must DERIVE one `ScalingPlanResource` per stored `ScalingInstruction` of that plan, copying `ServiceNamespace`, `ResourceId`, `ScalableDimension` from the instruction and setting `ScalingPlanName`, `ScalingPlanVersion`, `ScalingStatusCode = Active`. An unknown plan name/version must not return an empty 200 by accident — see the error rules below.
4. **UpdateScalingPlan** — input `ScalingPlanName` + `ScalingPlanVersion` (required), `ApplicationSource` and `ScalingInstructions` (both optional). Empty output. Must MUTATE the stored plan in place: replace only the members present in the request, leave the others intact, and refresh `StatusStartTime`. An unknown plan name or mismatched version ⇒ `ObjectNotFoundException` (HTTP 400). A subsequent DescribeScalingPlans must show the new values.
5. **DeleteScalingPlan** — input `ScalingPlanName` + `ScalingPlanVersion` (required). Empty output. Must REMOVE the plan from state; a second delete of the same plan ⇒ `ObjectNotFoundException` (HTTP 400), and a later DescribeScalingPlans must no longer list it.

Every one of these five must compute its response from persisted state. No canned bodies, no echoing the request back, and no returning a version number without storing the plan it belongs to: a create followed by a describe must return exactly what was created, and a delete must be observable by the next describe.

`GetScalingPlanResourceForecastData` is explicitly OUT of this first slice and may keep answering 501; it will be filed as its own gap issue.

## State Model

Service state (behind the `service_state(..).lock()` guard) holds:

- `scaling_plans: BTreeMap<String, ScalingPlanRecord>` — keyed by `ScalingPlanName`; a `BTreeMap` so DescribeScalingPlans has a stable, deterministic ordering for pagination.
- `ScalingPlanRecord { name: String, version: i64 /* always 1 */, application_source: ApplicationSource, scaling_instructions: Vec<ScalingInstruction>, status_code: ScalingPlanStatusCode /* Active */, status_message: Option<String>, status_start_time: DateTime, creation_time: DateTime }` — the fields the `ScalingPlan` output shape requires, so DescribeScalingPlans is a pure projection of this record.
- Pagination cursor: `NextToken` encodes the index/last-returned key into the `BTreeMap` (for DescribeScalingPlans) or the index into `scaling_instructions` (for DescribeScalingPlanResources); tokens that do not decode produce `InvalidNextTokenException`.

No global counters are needed: the service models exactly one version per plan (`ScalingPlanVersion` is documented as always 1), so the version is a constant on the record and version mismatches are an `ObjectNotFoundException`, not a lookup over a version map.

## Evidence

OPERATIONS.md at the crate root lists all 6 operations with the input/output members and errors quoted above (CreateScalingPlan → `ScalingPlanVersion` required output; DeleteScalingPlan/UpdateScalingPlan → `ObjectNotFoundException` HTTP 400; both Describe ops → `InvalidNextTokenException` HTTP 400 plus `MaxResults`/`NextToken` inputs). The Smithy model's `ScalingPlan` structure requires `ScalingPlanName`, `ScalingPlanVersion`, `ApplicationSource`, `ScalingInstructions`, `StatusCode` and offers `StatusMessage`/`StatusStartTime`/`CreationTime`; `ScalingPlanResource` requires `ScalingPlanName`, `ScalingPlanVersion`, `ServiceNamespace`, `ResourceId`, `ScalableDimension`, `ScalingStatusCode`; `ApplicationSource` carries `CloudFormationStackARN` and `TagFilters`. The CreateScalingPlan doc comment in src/ops/create_scaling_plan.rs states the version "value is always 1". All handler files (create_scaling_plan.rs, delete_scaling_plan.rs, describe_scaling_plan_resources.rs, describe_scaling_plans.rs, update_scaling_plan.rs, get_scaling_plan_resource_forecast_data.rs) still carry the `SCAFFOLDED, NOT IMPLEMENTED` marker and return `Scaffolded::not_implemented`. There are no tests asserting any of this behaviour.

## Steps to Reproduce

1. Start the emulator and point an awsJson1_1 client at it with target prefix `AnyScaleScalingPlannerFrontendService.`.
2. CreateScalingPlan {"ScalingPlanName":"my-plan","ApplicationSource":{"CloudFormationStackARN":"arn:aws:cloudformation:us-east-1:123456789012:stack/my-stack/abc"},"ScalingInstructions":[{...one instruction for autoscaling:autoScalingGroup:DesiredCapacity...}]} → expect 200 with {"ScalingPlanVersion":1}; today returns 501 NotImplemented.
3. DescribeScalingPlans {"ScalingPlanNames":["my-plan"]} → expect 200 listing one ScalingPlan whose ScalingPlanName, ApplicationSource, ScalingInstructions match step 2 exactly, with StatusCode "Active" and a CreationTime.
4. DescribeScalingPlanResources {"ScalingPlanName":"my-plan","ScalingPlanVersion":1} → expect 200 with one ScalingPlanResource per instruction, carrying the instruction's ServiceNamespace/ResourceId/ScalableDimension and ScalingStatusCode "Active".
5. UpdateScalingPlan {"ScalingPlanName":"my-plan","ScalingPlanVersion":1,"ScalingInstructions":[{...changed min/max capacity...}]} → expect 200 empty body; re-run step 3 and expect the UPDATED instructions and the original ApplicationSource preserved.
6. DeleteScalingPlan {"ScalingPlanName":"my-plan","ScalingPlanVersion":1} → expect 200 empty body.
7. DescribeScalingPlans {} → expect the plan is gone; repeat DeleteScalingPlan for "my-plan" → expect ObjectNotFoundException (HTTP 400).
8. UpdateScalingPlan on a never-created name → expect ObjectNotFoundException (HTTP 400); DescribeScalingPlans with "NextToken":"garbage" → expect InvalidNextTokenException (HTTP 400).
