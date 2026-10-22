Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: a2986bf0fd3cc35fe32946cf5d2b8a785e7f69a4

# Bootstrap the com-amazonaws-mpa-awsfluffycoreservice (AWS MPA) emulator

_Filed: 2026-10-21T21:47:56Z_
_Operation: service-com-amazonaws-mpa-awsfluffycoreservice | Class: stub_

> _Note: this issue was identified against the code base at commit `a2986bf0fd3cc35fe32946cf5d2b8a785e7f69a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every handler under `services/service-com-amazonaws-mpa-awsfluffycoreservice/src/ops/` is an unmodified scaffold: it carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` -> HTTP 501. `src/state.rs` holds `ComAmazonawsMpaAwsfluffycoreserviceState { /* TODO: add service-specific state fields */ }` — zero state. No client workflow works.

Implement a FIRST SLICE: the approval-team lifecycle, the service's primary resource (protocol `restJson1`, sigv4 name `mpa`). Scope is exactly these 5 operations, in order:

1. **CreateApprovalTeam** — `POST /approval-teams`. Input: `Name` (required, `^[a-zA-Z0-9._-]+$`, <=64), `Description` (required, 1..256), `ApprovalStrategy` (required union, variant `MofN{MinApprovalsRequired}`), `Approvers` (required, 1..20 of `{PrimaryIdentityId, PrimaryIdentitySourceArn}`), `Policies` (required, 1..10 of `{PolicyArn}`), optional `Tags`, optional `ClientToken` (@idempotencyToken). Must MINT and STORE a team: generate `Arn` matching `^arn:aws(-[^:]+)?:mpa:[a-z0-9-]{1,20}:[0-9]{12}:approval-team/[a-zA-Z0-9._-]+$` from the request region/account and `Name`, generate a fresh `VersionId`, record `CreationTime`. Return `{Arn, Name, VersionId, CreationTime}` read back out of the stored record. Re-creating the same `Name` while a team with that name exists -> `ConflictException` (409). Same `ClientToken` replayed -> return the SAME `Arn`/`VersionId`, do not create a second team.
2. **GetApprovalTeam** — `GET /approval-teams/{Arn}`. Must look the stored team up by ARN and echo back from state: `Arn`, `Name`, `Description`, `CreationTime`, `ApprovalStrategy` (as `ApprovalStrategyResponse`), `Approvers` (as `GetApprovalTeamResponseApprover` with `ApproverId`, `PrimaryIdentityId`, `PrimaryIdentitySourceArn`, `PrimaryIdentityStatus`), `NumberOfApprovers` = stored approver count, `Policies`, `VersionId`, `Status`, `StatusCode`, `LastUpdateTime`. Unknown ARN -> `ResourceNotFoundException` (404), NOT 501 and not an empty 200.
3. **ListApprovalTeams** — `POST /approval-teams/?List`. Must enumerate the stored teams as `ListApprovalTeamsResponseApprovalTeam` entries (`Arn`, `Name`, `Description`, `CreationTime`, `ApprovalStrategy`, `NumberOfApprovers`, `Status`, `StatusCode`). Honour `MaxResults` (1..20, default 20) and round-trip `NextToken` over a stable ordering (e.g. creation order) so a paged client sees every team exactly once.
4. **UpdateApprovalTeam** — `PATCH /approval-teams/{Arn}`. Apply the supplied `Description`, `ApprovalStrategy` and/or `Approvers` to the stored team, bump `VersionId`, update `LastUpdateTime`, and return the NEW `VersionId`. A following GetApprovalTeam must show the updated fields and the new VersionId. Unknown ARN -> 404.
5. **StartActiveApprovalTeamDeletion** — `POST /approval-teams/{Arn}?Delete`. Mark the stored team `Status=DELETING` with `StatusCode=DELETE_PENDING_APPROVAL`, record `DeletionStartTime=now` and `DeletionCompletionTime = now + PendingWindowDays` (default 7 when absent), and return those two timestamps. After this call GetApprovalTeam must report `Status=DELETING`; once the pending window has elapsed the team is gone and GetApprovalTeam returns 404 / ListApprovalTeams omits it.

Every one of the five MUST compute its response from the persisted store reached via `crate::state::service_state(&*self.engine)` — no canned bodies, no echoing the input back without storing it, no random ARN that is never recorded. Delete the `SCAFFOLDED, NOT IMPLEMENTED` marker line in each file you implement.

## State Model

In `ComAmazonawsMpaAwsfluffycoreserviceState`:

- `approval_teams: BTreeMap<String /* team ARN */, ApprovalTeam>` — insertion/creation order preserved for list + token paging (keep a `seq: u64` counter or an ordered index).
- `ApprovalTeam { arn: String, name: String, description: String, approval_strategy: ApprovalStrategy /* MofN { min_approvals_required: i32 } */, approvers: Vec<Approver>, policies: Vec<String /* PolicyArn */>, version_id: String, status: ApprovalTeamStatus /* ACTIVE|PENDING|INACTIVE|DELETING */, status_code: Option<ApprovalTeamStatusCode>, status_message: Option<String>, creation_time: SystemTime, last_update_time: Option<SystemTime>, deletion_start_time: Option<SystemTime>, deletion_completion_time: Option<SystemTime>, tags: BTreeMap<String,String>, seq: u64 }`
- `Approver { approver_id: String /* minted */, primary_identity_id: String, primary_identity_source_arn: String, primary_identity_status: IdentityStatus, response_time: Option<SystemTime> }`
- `name_index: BTreeMap<String /* team name */, String /* arn */>` — backs the ConflictException on duplicate name.
- `idempotency_tokens: BTreeMap<String /* ClientToken */, String /* arn */>` — backs ClientToken replay returning the original Arn/VersionId.

Everything else in the model (identity sources, sessions, policies, resource policies, tagging, DeleteInactiveApprovalTeamVersion, StartApprovalTeamBaseline) stays 501 for now and will be filed as individual gap issues.

## Evidence

`services/service-com-amazonaws-mpa-awsfluffycoreservice/OPERATIONS.md` lines 89-135 define the approval-team lifecycle used above: CreateApprovalTeam (`POST /approval-teams`, required ApprovalStrategy/Approvers/Description/Policies/Name, output Arn/Name/VersionId/CreationTime, ConflictException 409), GetApprovalTeam (`GET /approval-teams/{Arn}`, output incl. Status/StatusCode/NumberOfApprovers/VersionId, ResourceNotFoundException 404), ListApprovalTeams (`POST /approval-teams/?List`, @paginated inputToken=NextToken items=ApprovalTeams, MaxResults 1..20 default 20), UpdateApprovalTeam (`PATCH /approval-teams/{Arn}`, output VersionId), StartActiveApprovalTeamDeletion (`POST /approval-teams/{Arn}?Delete`, input PendingWindowDays, output DeletionStartTime/DeletionCompletionTime). Member shapes confirmed via smithy_query on ApprovalStrategy (union with MofN), ApprovalTeamRequestApprover (PrimaryIdentityId + PrimaryIdentitySourceArn, both required), GetApprovalTeamResponseApprover, ListApprovalTeamsResponseApprovalTeam and PolicyReference. Stub status confirmed in `src/ops/create_approval_team.rs` (line 4 marker, line 189 `Scaffolded::not_implemented("CreateApprovalTeam")`); empty store confirmed in `src/state.rs` line 8 (`TODO: add service-specific state fields`). No tests exist beyond the generated `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. Start the emulator and point an `mpa` client at it.
2. CreateApprovalTeam with Name="demo-team", Description="demo", ApprovalStrategy={MofN:{MinApprovalsRequired:1}}, one Approver {PrimaryIdentityId, PrimaryIdentitySourceArn}, one Policy {PolicyArn} -> expect 200 with a well-formed approval-team Arn, the Name, a VersionId and CreationTime (today: 501).
3. GetApprovalTeam with that Arn -> expect 200 echoing Name/Description/ApprovalStrategy/Approvers/Policies as created, NumberOfApprovers=1, the same VersionId (today: 501).
4. ListApprovalTeams with MaxResults=20 -> expect the created team present exactly once with its Arn, Name and Status (today: 501).
5. UpdateApprovalTeam on that Arn with Description="updated" -> expect 200 with a NEW VersionId; GetApprovalTeam then shows Description="updated" and the new VersionId.
6. StartActiveApprovalTeamDeletion on that Arn with PendingWindowDays=7 -> expect 200 with DeletionStartTime/DeletionCompletionTime; GetApprovalTeam then reports Status=DELETING.
7. GetApprovalTeam with an Arn that was never created -> expect 404 ResourceNotFoundException, not 501 and not an empty 200.
