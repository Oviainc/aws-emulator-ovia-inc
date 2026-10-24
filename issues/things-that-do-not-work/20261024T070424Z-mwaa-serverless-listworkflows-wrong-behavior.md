Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: ee21be04fa6cb5cf6de6f84766dd2013524605ad

# Workflows are stored without a Region, so ListWorkflows leaks workflows created in other Regions

_Filed: 2026-10-24T07:04:24Z_
_Operation: ListWorkflows | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `ee21be04fa6cb5cf6de6f84766dd2013524605ad`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: AWS regional endpoints are isolated. A workflow created with credentials scoped to `us-east-1` is only visible to `ListWorkflows`/`GetWorkflow` calls made against the `us-east-1` endpoint; a `ListWorkflows` call signed for `eu-west-1` returns an empty `Workflows` list (and the account-level name-uniqueness rule applies per Region, so the same workflow `Name` can be created in two Regions).

Actual: the service keeps ONE flat, Region-agnostic store. `crate::state::request_region(ctx.parts())` is consulted in exactly one place — `CreateWorkflow`, where it is only used to stamp the Region segment into the minted `WorkflowArn` — and the Region is never recorded on the `Workflow` record nor used as a lookup/scoping key anywhere else. `ListWorkflows` walks every entry of `store.workflows` (`store.workflows.range::<str, _>((lower, Unbounded))`) without filtering by the caller's credential-scope Region, so a workflow whose ARN says `us-east-1` is returned verbatim — ARN included — to a request signed for `eu-west-1`. The same root cause produces two further observable divergences: (a) `GetWorkflow`/`ListTagsForResource`/`StartWorkflowRun` happily resolve an ARN belonging to a different Region than the request was signed for, and (b) `CreateWorkflow`'s `store.workflow_names` map is global, so creating workflow `etl` in `us-east-1` and then `etl` in `eu-west-1` is refused with `ConflictException` (409) where the real service creates a second, independent workflow.

Fix direction: key (or tag) stored workflows by the request's credential-scope Region, filter `ListWorkflows` to that Region, treat an ARN whose Region segment differs from the request Region as an unresolvable reference, and make the workflow-name uniqueness check per Region.

## Evidence

Spec/well-known semantics: `airflow-serverless` is a regional service — the model's `awsService.endpointPrefix`/`arnNamespace` is `airflow-serverless` and `WorkflowArn` embeds the Region segment `([a-z]{2}-[a-z]+-[0-9]{1})`; `ListWorkflows` is documented as "Lists all workflows in your account", which in AWS means the account *in the Region the endpoint serves*. Handler evidence: `src/ops/list_workflows.rs` (handle() iterates `store.workflows` with no Region predicate — it does not even read `ctx`, which it discards with `let _ = ctx;`), `src/ops/create_workflow.rs` line ~264 (`let region = crate::state::request_region(ctx.parts());` used only for `mint_workflow_arn`) and lines ~296-305 (`store.workflow_names.get(&input.name)` → ConflictException, no Region in the key), `src/state.rs` (`pub workflows: BTreeMap<String, Workflow>` / `workflow_names` with no Region dimension; `struct Workflow` has no region field). Test coverage: `tests/all/workflow_lifecycle.rs::list_workflows_pages_over_every_persisted_workflow_exactly_once` exercises paging only and signs every request with the same scope — no test signs two requests for different Regions, so nothing asserts Region isolation. An equivalent gap is already tracked for the sibling MWAA emulator (issue `20261024T052540Z-mwaa-listenvironments-wrong-behavior`), but nothing covers mwaa-serverless workflows.

## Steps to Reproduce

1. Call CreateWorkflow with a SigV4 Authorization header whose credential scope Region is `us-east-1` (Name=`etl`, valid RoleArn and DefinitionS3Location); note the returned WorkflowArn contains `us-east-1`.
2. Call ListWorkflows with a SigV4 Authorization header whose credential scope Region is `eu-west-1`.
3. Observe the `us-east-1` workflow is returned in `Workflows` (with its `us-east-1` ARN), where the real `eu-west-1` endpoint returns an empty list.
4. Call GetWorkflow with the `us-east-1` ARN while signing for `eu-west-1` and observe a 200 instead of an unresolvable-reference refusal.
5. Call CreateWorkflow again with Name=`etl` while signing for `eu-west-1` and observe `ConflictException` (409) instead of a second, independent workflow with a `eu-west-1` ARN.
