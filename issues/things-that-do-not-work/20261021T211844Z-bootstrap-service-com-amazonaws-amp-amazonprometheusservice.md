Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 34e6e58e29d15417fc6ff05c31f4f08593fa6d9a

# Bootstrap the Amazon Managed Service for Prometheus (amp) emulator

_Filed: 2026-10-21T21:18:44Z_
_Operation: service-com-amazonaws-amp-amazonprometheusservice | Class: missing_

> _Note: this issue was identified against the code base at commit `34e6e58e29d15417fc6ff05c31f4f08593fa6d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 44 operations in this crate is an unmodified scaffold answering HTTP 501 `NotImplemented` and holding zero state, so no client workflow works at all. Implement a FIRST, SMALL slice: the workspace lifecycle, which is the service's primary resource and the prerequisite for every other amp resource (rule groups, alert manager, logging, scrapers).

### Happy path to implement (in order)

1. **CreateWorkspace** — `POST /workspaces -> 202`. Input: `alias` (1..100), `clientToken` (@idempotencyToken), `tags` (map, <=50), `kmsKeyArn`. Must mint a new workspace id of the real shape (`ws-<uuid>`, matching `[0-9A-Za-z][-.0-9A-Z_a-z]*`, 1..64), build `arn` as `arn:aws:aps:<region>:<account>:workspace/<workspaceId>` (must satisfy `^arn:aws[-a-z]*:aps:[-a-z0-9]+:[0-9]{12}:workspace/.+$`), record `createdAt`, store `alias`/`tags`/`kmsKeyArn`, and return `workspaceId`, `arn`, `status` (`WorkspaceStatus{statusCode}` — `CREATING`, becoming `ACTIVE` on the next read is acceptable; `ACTIVE` immediately is also acceptable) and `tags`/`kmsKeyArn` echoed from what was PERSISTED. Replaying the same `clientToken` must return the same workspace, not create a second one.
2. **DescribeWorkspace** — `GET /workspaces/{workspaceId} -> 200`. Output: `workspace` (`WorkspaceDescription`) built from the stored record: `workspaceId`, `alias`, `arn`, `status`, `prometheusEndpoint` (`https://aps-workspaces.<region>.amazonaws.com/workspaces/<workspaceId>/api/v1/`), `createdAt` (required), `tags`, `kmsKeyArn`. Unknown id -> `ResourceNotFoundException` (HTTP 404), not 501 and not an empty 200.
3. **ListWorkspaces** — `GET /workspaces -> 200`. Must return one `WorkspaceSummary` per stored workspace (`workspaceId`, `alias`, `arn`, `status`, `createdAt`, `tags`, `kmsKeyArn`), honour the `alias` query filter (prefix match on alias), honour `maxResults`, and emit/accept `nextToken` per the `@paginated(inputToken=nextToken, outputToken=nextToken, items=workspaces)` trait. A workspace created in step 1 MUST appear here.
4. **UpdateWorkspaceAlias** — `POST /workspaces/{workspaceId}/alias -> 204`. Must mutate the stored `alias` so a subsequent DescribeWorkspace/ListWorkspaces returns the new value. Unknown id -> 404.
5. **DeleteWorkspace** — `DELETE /workspaces/{workspaceId} -> 202`. Must remove the workspace (and its tags) from state. A following DescribeWorkspace MUST return `ResourceNotFoundException` (404) and ListWorkspaces MUST no longer include it. Deleting an unknown id -> 404.
6. **ListTagsForResource / TagResource / UntagResource** — `GET|POST|DELETE /tags/{resourceArn}`. Resolve `resourceArn` back to the stored workspace; `TagResource` merges into the workspace's tag map, `UntagResource` removes the given `tagKeys`, `ListTagsForResource` returns exactly the tags currently stored (also reflecting tags passed to CreateWorkspace). Unknown ARN -> `ResourceNotFoundException` (404).

All six must compute responses from persisted state. No canned bodies, no echoing the request back, no minting an id without storing the record — a create followed by a describe must return precisely what was created, and a delete must make the resource observably gone.

## State Model

In `src/state.rs` (`ComAmazonawsAmpAmazonprometheusserviceState`, reached via `crate::state::service_state(&*self.engine)`):

- `workspaces: HashMap<String /*workspaceId*/, Workspace>`
- `struct Workspace { workspace_id: String, arn: String, alias: Option<String>, status_code: WorkspaceStatusCode, prometheus_endpoint: String, created_at: SystemTime, tags: BTreeMap<String,String>, kms_key_arn: Option<String> }`
- `arn_index: HashMap<String /*arn*/, String /*workspaceId*/>` so the tagging ops can resolve `resourceArn` -> workspace without scanning.
- `client_tokens: HashMap<String /*clientToken*/, String /*workspaceId*/>` to make `CreateWorkspace` idempotent per its `@idempotencyToken`.
- (room to grow, left empty by this slice) `rule_groups_namespaces`, `alert_manager_definitions`, `scrapers`, `workspace_configuration` keyed off the same `workspaceId`.

Everything outside this list (scrapers, rule groups namespaces, alert manager definitions, anomaly detectors, logging/query-logging configurations, resource policies, workspace configuration, GetDefaultScraperConfiguration) stays at 501 and will be filed as individual gap issues later.

## Evidence

`services/service-com-amazonaws-amp-amazonprometheusservice/OPERATIONS.md` lists all 44 operations with their routes, input/output members and errors — including CreateWorkspace (`POST /workspaces -> 202`, outputs `workspaceId`/`arn`/`status`/`tags`/`kmsKeyArn`), DescribeWorkspace (`GET /workspaces/{workspaceId}`, output `workspace: WorkspaceDescription` required), ListWorkspaces (`GET /workspaces`, `@paginated(items=workspaces)`, query params `alias`/`maxResults`/`nextToken`), UpdateWorkspaceAlias (`POST /workspaces/{workspaceId}/alias -> 204`), DeleteWorkspace (`DELETE /workspaces/{workspaceId} -> 202`) and the three `/tags/{resourceArn}` operations; protocol is `restJson1`, sigv4 signing name `aps`. `smithy_query` on `WorkspaceDescription`/`WorkspaceSummary`/`WorkspaceStatus` confirms the required fields (`workspaceId`, `arn`, `status`, `createdAt`) plus `prometheusEndpoint`, `alias`, `tags`, `kmsKeyArn`, and the ARN/id patterns quoted above. `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches all 44 handler files in this crate (e.g. `src/ops/create_workspace.rs:4`, `src/ops/describe_workspace.rs:4`, `src/ops/list_workspaces.rs:4`, `src/ops/delete_workspace.rs:4`), each returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` -> HTTP 501. No test asserts any workspace behaviour; `tests/state_roundtrip.rs` is the generated state-API example only.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI `amp` client at it.
2. `aws amp create-workspace --alias my-ws --tags team=obs` -> expect 202 with a fresh `workspaceId` (`ws-...`), an `arn` of the form `arn:aws:aps:<region>:<account>:workspace/<workspaceId>`, a `status.statusCode`, and `tags` echoed from stored state. Currently: HTTP 501 NotImplemented.
3. Repeat the same call with the identical `--client-token` -> expect the SAME workspaceId (idempotent), not a second workspace.
4. `aws amp describe-workspace --workspace-id <id>` -> expect 200 with `workspace.alias == my-ws`, the matching `arn`, a populated `createdAt` and `prometheusEndpoint`, and `tags` containing `team=obs`.
5. `aws amp list-workspaces` -> expect the new workspace present as a summary; `aws amp list-workspaces --alias my-` -> expect it still present; `--max-results 1` with two workspaces -> expect one item plus a `nextToken` that pages to the rest.
6. `aws amp update-workspace-alias --workspace-id <id> --alias renamed` -> expect 204, then `describe-workspace` returns `alias == renamed`.
7. `aws amp tag-resource --resource-arn <arn> --tags env=dev` then `aws amp list-tags-for-resource --resource-arn <arn>` -> expect both `team=obs` and `env=dev`; `aws amp untag-resource --resource-arn <arn> --tag-keys team` -> expect only `env=dev` remains.
8. `aws amp delete-workspace --workspace-id <id>` -> expect 202, then `describe-workspace --workspace-id <id>` returns ResourceNotFoundException (HTTP 404) and `list-workspaces` no longer includes it.
9. `aws amp describe-workspace --workspace-id ws-does-not-exist` -> expect ResourceNotFoundException (HTTP 404), not 501.
