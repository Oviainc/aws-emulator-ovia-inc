Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 4ffec8ed9c134267b14eefb2b325da9a0803e3ce

# Rescore execution plans are not scoped to the request's region: a plan created in one region is listed and addressable from every other region

_Filed: 2026-10-24T00:50:04Z_
_Operation: ListRescoreExecutionPlans | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `4ffec8ed9c134267b14eefb2b325da9a0803e3ce`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The service store holds one global `plans` map with no region dimension. The request's region is read only to build the ARN at create time (`crate::state::request_region` -> `plan_arn`); every subsequent lookup is by plan id alone. So a plan created with credentials scoped to `us-east-1` is returned by a `ListRescoreExecutionPlans` signed for `eu-west-1`, and is describable, updatable, deletable and rescorable from any region — and a repeated `ClientToken` dedupes across regions too.

Rescore execution plans are regional AWS resources (the ARN the service itself mints embeds the region, and the endpoint is per-region `kendra-ranking.<region>.amazonaws.com`). On real AWS a plan created in `us-east-1` is invisible in `eu-west-1`: ListRescoreExecutionPlans there returns an empty `SummaryItems`, and Describe/Update/Delete/Rescore with that id answer ResourceNotFoundException. Against this emulator the plan is fully live in every region, so a test or application that relies on regional isolation (fixtures that create in one region and assert a clean slate in another, multi-region tenancy checks, teardown scoped to one region) passes here and fails on AWS.

Several other crates in this repo do partition their store by region (e.g. dynamodb's `regions: HashMap<String, HashMap<String, Table>>`, elasticloadbalancing's `regions: BTreeMap<..>`), so the per-region store is an available shape, not a framework limitation.

## Evidence

Spec/well-established behavior: `aws.api#service` on com.amazonaws.kendraranking#AWSKendraRerankingFrontendService gives endpointPrefix `kendra-ranking` and arnNamespace `kendra-ranking`, and the emulator itself mints `arn:aws:kendra-ranking:<region>:<account>:rescore-execution-plan/<id>` from the SigV4 credential scope — i.e. the resource is regional. AWS regional resources are not visible across regions.

Handler/state: services/.../src/state.rs `ComAmazonawsKendrarankingAwskendrarerankingfrontendserviceState { plans: BTreeMap<String, RescoreExecutionPlan>, arn_index, client_tokens }` has no region key; `RescoreExecutionPlan` stores no region field. services/.../src/ops/list_rescore_execution_plans.rs walks `store.plans.values()` with no region filter (and never calls `request_region`); describe/delete/update/rescore all look plans up by `store.plans.get(&input.id)` only. Only create reads the region (create_rescore_execution_plan.rs: `let region = crate::state::request_region(ctx.parts())`), and only for the ARN string.

Tests: the current suite encodes the cross-region behaviour rather than regional isolation — tests/all/rescore_execution_plan_lifecycle.rs::a_plan_arn_names_the_region_the_request_was_signed_for creates in `eu-west-1`/`ap-southeast-2`/`us-gov-west-1` and then asserts a `us-east-1`-signed Describe returns 200, and tests/all/resource_tags.rs::tags_belong_to_the_resource_the_arn_names asserts a cross-region ListTagsForResource returns 200. Those assertions would need to change with the fix (ARN-addressed tagging calls may legitimately keep working when the ARN names the caller's own region).

## Steps to Reproduce

1. CreateRescoreExecutionPlan {Name: "us-plan"} with credentials signed for us-east-1 -> note the returned Id and the ARN (which embeds us-east-1).
2. Call ListRescoreExecutionPlans with credentials signed for eu-west-1.
3. Observe `SummaryItems` contains the us-east-1 plan; on AWS the eu-west-1 endpoint reports an empty list.
4. Call DescribeRescoreExecutionPlan {Id} signed for eu-west-1 -> emulator answers 200 with the plan; AWS answers ResourceNotFoundException ("The specified Resource Execution Plan ID does not exist.").
5. Call DeleteRescoreExecutionPlan {Id} (once ACTIVE) signed for eu-west-1 -> emulator deletes the us-east-1 plan; AWS refuses it as not found.
6. Also: CreateRescoreExecutionPlan with the same ClientToken in two different regions returns one shared plan, where AWS mints one plan per region.
