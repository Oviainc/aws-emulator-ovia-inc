Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 34e6e58e29d15417fc6ff05c31f4f08593fa6d9a

# Bootstrap the workspaces emulator

_Filed: 2026-10-21T21:18:15Z_
_Operation: service-com-amazonaws-workspaces-workspacesservice | Class: missing_

> _Note: this issue was identified against the code base at commit `34e6e58e29d15417fc6ff05c31f4f08593fa6d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every operation in `services/service-com-amazonaws-workspaces-workspacesservice/src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no WorkSpaces client workflow can run. Implement a FIRST, SMALL stateful slice: the WorkSpace lifecycle for the service's primary resource (`Workspace`).

### Happy path to implement (in order)

1. **CreateWorkspaces** — input `Workspaces` (list<WorkspaceRequest>, 1..25, required; each carries required `DirectoryId`, `UserName`, `BundleId`, plus optional `WorkspaceName`, `VolumeEncryptionKey`, `UserVolumeEncryptionEnabled`, `RootVolumeEncryptionEnabled`, `WorkspaceProperties`, `Tags`). For each accepted request mint a `WorkspaceId` matching `^ws-[0-9a-z]{8,63}$`, persist a `Workspace` record carrying back every field supplied (DirectoryId, UserName, BundleId, WorkspaceName, WorkspaceProperties, encryption flags, Tags) with `State = PENDING` (an immediately-following DescribeWorkspaces may report `AVAILABLE`), and return it in `PendingRequests`. Requests that cannot be created go to `FailedRequests` as `FailedCreateWorkspaceRequest{WorkspaceRequest, ErrorCode, ErrorMessage}` — not as a top-level error.
2. **DescribeWorkspaces** — must return the workspaces actually stored, never canned data. Filters: `WorkspaceIds`, `DirectoryId`, `UserName`, `BundleId`, `WorkspaceName`; paginate with `Limit` (1..25) + `NextToken`/`NextToken` as modeled by `@paginated(inputToken=NextToken, outputToken=NextToken, items=Workspaces)`. Unknown `WorkspaceIds` yield an empty `Workspaces` list (filtering is not an error); a mutually-exclusive mix of filters is `InvalidParameterValuesException`.
3. **StopWorkspaces** — input `StopWorkspaceRequests` (list<StopRequest>, 1..25). For each stored workspace flip persisted `State` to `STOPPED` (via `STOPPING` is acceptable); a subsequent DescribeWorkspaces must show the new state. Unknown or non-stoppable ids come back in `FailedRequests` as `FailedWorkspaceChangeRequest{WorkspaceId, ErrorCode, ErrorMessage}` — this operation models no top-level errors.
4. **StartWorkspaces** — input `StartWorkspaceRequests` (list<StartRequest>, 1..25). Flip persisted `State` back to `AVAILABLE` (via `STARTING` acceptable) for stopped workspaces; unknown ids / wrong-state ids go to `FailedRequests`.
5. **TerminateWorkspaces** — input `TerminateWorkspaceRequests` (list<TerminateRequest>, 1..25). Remove the workspace from the store (or mark `TERMINATED` then drop it) so a later DescribeWorkspaces no longer returns it; unknown ids go to `FailedRequests`, output is `FailedRequests` only.

All five must compute their responses from persisted state shared across requests through `crate::state::service_state(&*self.engine)`. No canned bodies, no echoing the input back without storing it, no minting an id that is never saved. Operations outside this list stay 501 and will be filed as individual gap issues later.

## State Model

In `src/state.rs` (`ComAmazonawsWorkspacesWorkspacesserviceState`):

- `workspaces: HashMap<String /* WorkspaceId, "ws-xxxxxxxx" */, WorkspaceRecord>`
- `WorkspaceRecord { workspace_id: String, directory_id: String, user_name: String, bundle_id: String, workspace_name: Option<String>, computer_name: Option<String>, ip_address: Option<String>, state: WorkspaceState /* PENDING|AVAILABLE|STOPPING|STOPPED|STARTING|TERMINATING|TERMINATED */, subnet_id: Option<String>, volume_encryption_key: Option<String>, user_volume_encryption_enabled: Option<bool>, root_volume_encryption_enabled: Option<bool>, workspace_properties: Option<WorkspaceProperties>, tags: Vec<Tag>, error_code: Option<String>, error_message: Option<String>, created: SystemTime }`
- `next_workspace_seq: u64` — monotonic counter used to mint ids matching `^ws-[0-9a-z]{8,63}$`
- `describe_tokens: HashMap<String /* NextToken */, usize /* offset into the ordered workspace id list */>` — or an equivalent deterministic encoding of the pagination offset, so `NextToken` resumes exactly where the previous page ended

Workspaces must be iterated in a stable order (e.g. creation order) so pagination is deterministic.

**Done when:** a client script can create a workspace, describe it and see back the DirectoryId/UserName/BundleId it supplied, stop it and observe `STOPPED`, start it and observe `AVAILABLE`, terminate it and observe it gone — with correct per-item `FailedRequests` entries for unknown workspace ids instead of top-level failures.

## Evidence

OPERATIONS.md for this crate: `## CreateWorkspaces` (input `Workspaces` list<WorkspaceRequest> len 1..25 required; output `FailedRequests`, `PendingRequests`; errors InvalidParameterValuesException, ResourceLimitExceededException), `## DescribeWorkspaces` (`@paginated(inputToken=NextToken, outputToken=NextToken, items=Workspaces)`; inputs WorkspaceIds/DirectoryId/UserName/BundleId/Limit/NextToken/WorkspaceName), `## StartWorkspaces` and `## StopWorkspaces` (inputs StartWorkspaceRequests/StopWorkspaceRequests list 1..25 required, output `FailedRequests`, errors: none), `## TerminateWorkspaces` (input TerminateWorkspaceRequests list 1..25 required, output `FailedRequests`). Shapes from smithy_query: `WorkspaceRequest` (DirectoryId/UserName/BundleId required), `Workspace` (WorkspaceId, State enum PENDING|AVAILABLE|...|TERMINATED), `FailedCreateWorkspaceRequest{WorkspaceRequest,ErrorCode,ErrorMessage}`, `FailedWorkspaceChangeRequest{WorkspaceId,ErrorCode,ErrorMessage}`. Scaffold proof: `src/ops/create_workspaces.rs:4` carries `//! SCAFFOLDED, NOT IMPLEMENTED:` and line 166 returns `Err(crate::scaffolded::Scaffolded::not_implemented("CreateWorkspaces"))`; every other file in `src/ops/` is the same generated stub.

## Steps to Reproduce

1. Start the emulator with the workspaces service mounted.
2. Call CreateWorkspaces with Workspaces=[{DirectoryId:"d-1234567890", UserName:"alice", BundleId:"wsb-12345678"}] — expect a PendingRequests entry with a fresh WorkspaceId (^ws-[0-9a-z]{8,63}$) and State=PENDING; today the call returns HTTP 501 NotImplemented.
3. Call DescribeWorkspaces with no filters — expect the created workspace with the DirectoryId/UserName/BundleId supplied.
4. Call DescribeWorkspaces with WorkspaceIds=[<the id>] — expect exactly that workspace; with WorkspaceIds=["ws-00000000"] expect an empty Workspaces list.
5. Call StopWorkspaces with StopWorkspaceRequests=[{WorkspaceId:<the id>}] — expect empty FailedRequests, then DescribeWorkspaces shows State=STOPPED.
6. Call StartWorkspaces with StartWorkspaceRequests=[{WorkspaceId:<the id>}] — then DescribeWorkspaces shows State=AVAILABLE.
7. Call StopWorkspaces with an unknown id — expect a FailedRequests entry naming that WorkspaceId with ErrorCode/ErrorMessage, HTTP 200.
8. Call TerminateWorkspaces with TerminateWorkspaceRequests=[{WorkspaceId:<the id>}] — then DescribeWorkspaces no longer returns it.
9. Create 3 workspaces and call DescribeWorkspaces with Limit=2 — expect 2 items plus a NextToken, and the follow-up call with that NextToken returns the remaining 1 with no NextToken.
