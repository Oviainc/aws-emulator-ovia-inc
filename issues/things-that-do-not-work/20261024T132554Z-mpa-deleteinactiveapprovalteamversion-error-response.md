Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: d9cda5a9b6bc2cdf59d194d1abe409fa5e2cae3f

# DeleteInactiveApprovalTeamVersion answers 200 for an unknown team ARN or VersionId instead of ResourceNotFoundException (404)

_Filed: 2026-10-24T13:25:54Z_
_Operation: DeleteInactiveApprovalTeamVersion | Class: error-response_

> _Note: this issue was identified against the code base at commit `d9cda5a9b6bc2cdf59d194d1abe409fa5e2cae3f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DELETE /approval-teams/{Arn}/{VersionId}` for a pair that names nothing stored — an approval-team ARN under which no team exists, or a `VersionId` that is not the stored team's current one — is answered with HTTP 200 and an empty body, as if a version had been deleted.

Expected (real service): the operation declares `ResourceNotFoundException` (HTTP 404) and an approval-team version that does not exist cannot be deleted, so AWS refuses the call with 404 `ResourceNotFoundException` (this service's fixed not-found message, the same one `GetApprovalTeam` / `StartActiveApprovalTeamDeletion` / `ListPolicyVersions` / `GetResourcePolicy` already answer with in this crate). `@idempotent` only means a retry of the SAME successful call is safe; it does not turn a missing resource into a success.

Actual (emulator): the handler returns `Ok(DeleteInactiveApprovalTeamVersionOutput {})` before any lookup failure is reported, so a client that deletes a version it never created, or that passes a stale/typo'd `VersionId` for an existing team, is told the delete succeeded. The modelled `ResourceNotFoundException` is unreachable for this operation: no code path constructs it. Because the version id is also silently ignored when it does not match, a caller cannot distinguish "deleted" from "never existed" or from "you named the wrong version".

Contrast inside the same service: `DeleteIdentitySource` does NOT model `ResourceNotFoundException` (its errors are AccessDenied/Conflict/InternalServer/Throttling/Validation), and there answering success for an already-absent source is correct. `DeleteInactiveApprovalTeamVersion` is the delete that DOES model the not-found error, which is exactly the missing-resource condition it exists for.

## Evidence

Smithy model (OPERATIONS.md / smithy_query for `DeleteInactiveApprovalTeamVersion`): `DELETE /approval-teams/{Arn}/{VersionId} -> 200`, errors = `AccessDeniedException (403)`, `ConflictException (409)`, `InternalServerException (500)`, `ResourceNotFoundException (404)`, `ThrottlingException (429)`, `ValidationException (400)` — the declared 404 has no other condition it could describe for this operation. Compare `DeleteIdentitySource`, which declares no `ResourceNotFoundException`.

Handler: `services/service-com-amazonaws-mpa-awsfluffycoreservice/src/ops/delete_inactive_approval_team_version.rs` lines 189-195:
```rust
let Some(status) = state
    .team(&input.arn)
    .filter(|team| team.version_id == input.version_id)
    .map(|team| team.status)
else {
    return Ok(DeleteInactiveApprovalTeamVersionOutput {});
};
```
`crate::state::resource_not_found()` exists and is used by the other ARN-addressed operations, but is never constructed here.

Test coverage: the existing suite ASSERTS the divergent behaviour — `tests/all/inactive_team_version_deletion.rs::deleting_a_version_that_was_never_stored_is_a_success_that_changes_nothing` (module doc: "a pair that names nothing stored — an unknown team ARN, or a `VersionId` other than the stored team's current one — is a success that changes nothing, the operation being `@idempotent`"). No test asserts the modelled 404, so the gap is not covered.

Reachability note: because the management-account gate refuses every `CreateApprovalTeam` over the wire, EVERY client call to this operation currently lands on this branch and is answered 200.

## Steps to Reproduce

1. Point an MPA client at the emulator in us-east-1: `client = boto3.client('mpa', region_name='us-east-1')`.
2. Call `client.delete_inactive_approval_team_version(Arn='arn:aws:mpa:us-east-1:<emulator-account>:approval-team/does-not-exist-<hex>', VersionId='1')`.
3. Observe HTTP 200 with an empty body. Expected HTTP 404 with error code `ResourceNotFoundException` (the service's fixed not-found message), since no such approval-team version exists.
4. Repeat with an existing team's ARN but a `VersionId` the team does not carry (e.g. '999'): the emulator again answers 200 and deletes nothing, instead of reporting that the named version does not exist.
