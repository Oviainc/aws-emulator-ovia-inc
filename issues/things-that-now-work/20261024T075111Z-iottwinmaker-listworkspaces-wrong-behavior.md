Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: a85c524bdc735756e5abfd799d7dcf6de3bb06dc

# Workspaces (and everything inside them) are stored without a Region, so reads in one Region see resources created in another

_Filed: 2026-10-24T07:51:11Z_
_Operation: ListWorkspaces | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `a85c524bdc735756e5abfd799d7dcf6de3bb06dc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

IoT TwinMaker is a regional service: a workspace created in us-east-1 does not exist in eu-west-1, and the same workspaceId may be created independently in two Regions. The emulator keeps ONE global workspace map keyed by workspaceId alone and uses the request's Region only to mint ARNs, so:

- Expected: ListWorkspaces/GetWorkspace signed for eu-west-1 reports nothing for a workspace created in us-east-1 (GetWorkspace -> 404 ResourceNotFoundException); CreateWorkspace of the same id in a second Region succeeds and yields a second, independent workspace whose ARN carries the second Region.
- Actual: the eu-west-1 call returns the us-east-1 workspace verbatim, including its `arn` whose region segment is `us-east-1` (an ARN that cannot belong to the endpoint that served it), and CreateWorkspace of the same id in the second Region is refused 409 ConflictException. Every contained resource (entities, component types, scenes, sync jobs) and every account-level resource (metadata transfer jobs, the pricing plan) leaks the same way, since they all hang off the same unscoped store.

## Evidence

Spec/semantics: `TwinMakerArn` carries a region segment (`^arn:...:iottwinmaker:[a-z0-9-]+:...`), and AWS regional-service semantics mean a control-plane resource exists only in the Region it was created in. Handler/state: `services/.../src/state.rs` — `ComAmazonawsIottwinmakerAwsiottwinmakerState { workspaces: BTreeMap<String, Workspace>, arn_index, metadata_transfer_jobs, pricing_plan }` carries no Region anywhere, and `Workspace` (state.rs, struct definition) has no region field; `request_region(parts)` (state.rs ~1405) is used ONLY to build ARNs (`create_workspace.rs:196-197`, `create_entity.rs:318-319`, `create_scene.rs:234-235`, `create_sync_job.rs:248-249`, `create_metadata_transfer_job.rs:289-290`). Readers do no Region filtering: `list_workspaces.rs:210-214` ranges the whole map, `get_workspace.rs:179-182` resolves by id/ARN only, `create_workspace.rs:175-182` conflicts on the bare id. Tests: `tests/all/workspace_listing.rs` and `tests/all/state_roundtrip.rs` assert listing/roundtrip behavior but nothing asserts Region scoping.

## Steps to Reproduce

1. Boot the emulator and point an iottwinmaker client signed for us-east-1 at it.
2. CreateWorkspace(workspaceId="ws-region-probe", role=<role arn>, s3Location=arn:aws:s3:::<bucket>) -> 200; note the returned ARN contains `us-east-1`.
3. Point a second client at the same emulator signed for eu-west-1 and call ListWorkspaces. Expected: "ws-region-probe" is absent. Actual: it is reported, with the us-east-1 ARN.
4. GetWorkspace(workspaceId="ws-region-probe") from the eu-west-1 client. Expected: 404 ResourceNotFoundException. Actual: 200 with the us-east-1 record.
5. CreateWorkspace(workspaceId="ws-region-probe") from the eu-west-1 client. Expected: 200, an independent workspace whose ARN carries eu-west-1. Actual: 409 ConflictException.
6. Repeat with an entity/scene created in one Region and read (GetEntity/GetScene/ListEntities) from the other — the same leak is observable.
