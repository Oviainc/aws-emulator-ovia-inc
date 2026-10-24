Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: ee21bdc0559bb252101e2375a2c132d26bd8f920

# Workspace store is not region-scoped: a workspace created in one region is listed and mutated from every other region

_Filed: 2026-10-24T18:42:09Z_
_Operation: ListWorkspaces | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `ee21bdc0559bb252101e2375a2c132d26bd8f920`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Amazon Managed Service for Prometheus is a regional service — `CreateWorkspace`'s documentation says "You can have one or more workspaces in each Region in your account", and a workspace id minted in us-west-2 does not exist in eu-west-1 (DescribeWorkspace there answers ResourceNotFoundException). The emulator keeps ONE un-partitioned store: `ComAmazonawsAmpAmazonprometheusserviceState { workspaces: HashMap<String, Workspace>, query_logging, scrapers, arn_index, client_tokens }` (`src/state.rs:727-745`), reached by every handler through `crate::state::service_state(...)`. The request's region is read (`crate::state::request_region`) only to COMPOSE ARNs and the Prometheus endpoint, never to scope a lookup or a listing.

Expected: `ListWorkspaces` signed for region B reports only workspaces created in region B; `DescribeWorkspace`/`UpdateWorkspaceAlias`/`DeleteWorkspace`/`CreateRuleGroupsNamespace`/... for an id created in region A answer 404 when signed for region B; and the per-account workspace quota applies per region.

Actual: a workspace created in us-west-2 appears in a eu-north-1 `ListWorkspaces` page — carrying an `arn` and `prometheusEndpoint` that name us-west-2, i.e. a summary no real eu-north-1 response can contain — and every workspace-scoped operation succeeds cross-region. `ListWorkspaces`'s `nextToken` cursor and the `WORKSPACE_QUOTA` bound are likewise account-global instead of per region.

## Evidence

Spec: `CreateWorkspace` doc ("one or more workspaces in each Region in your account") plus well-established AWS regional isolation; `WorkspaceSummary$arn` is `^arn:aws[-a-z]*:aps:[-a-z0-9]+:[0-9]{12}:workspace/.+$`, so a summary returned to a region-B caller carrying a region-A ARN is unreachable in real AWS. Handler/state: `src/ops/list_workspaces.rs:170-190` iterates `store.workspaces.values()` with no region predicate; `src/state.rs:727-745` (single map) and `src/state.rs:962` (`request_region` used only for ARN composition). Tests: every case in `tests/all/workspace_lifecycle.rs` builds a fresh engine per region (`let amp = Amp::new();` inside the loop at line 426), so no test exercises — or asserts — cross-region visibility.

## Steps to Reproduce

1. POST /workspaces signed for us-west-2 with {"alias":"west"} — note the workspaceId and arn (arn names us-west-2).
2. GET /workspaces signed for eu-north-1 — expected an empty `workspaces` list, actual the us-west-2 workspace, whose arn/prometheusEndpoint name us-west-2.
3. GET /workspaces/{workspaceId} signed for eu-north-1 — expected 404 ResourceNotFoundException, actual 200.
4. POST /workspaces/{workspaceId}/alias signed for eu-north-1 with {"alias":"north"} — expected 404, actual 204 and the rename is visible to the us-west-2 caller.
