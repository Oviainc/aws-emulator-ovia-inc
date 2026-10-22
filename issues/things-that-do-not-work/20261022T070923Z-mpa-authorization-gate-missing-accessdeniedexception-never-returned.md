Issue-Source: trace-sweep-divergence
Issue-Trace: mpa-approval-team-context-and-tagging
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mpa-approval-team-context-and-tagging mpa-approval-team-create-validation-error mpa-approval-team-mutation-error-paths
Issue-Shape-Id: com.amazonaws.mpa#AWSFluffyCoreService
Issue-Key: mpa|GetApprovalTeam|4xx->4xx|$.Message
Issue-Key: mpa|GetApprovalTeam|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: mpa|GetApprovalTeam|4xx->4xx|status
Issue-Key: mpa|CreateApprovalTeam|4xx->2xx|$.Message
Issue-Key: mpa|CreateApprovalTeam|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: mpa|CreateApprovalTeam|4xx->2xx|status
Issue-Key: mpa|UpdateApprovalTeam|4xx->4xx|$.Message
Issue-Key: mpa|UpdateApprovalTeam|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: mpa|UpdateApprovalTeam|4xx->4xx|status
Issue-Key: mpa|StartActiveApprovalTeamDeletion|4xx->4xx|$.Message
Issue-Key: mpa|StartActiveApprovalTeamDeletion|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: mpa|StartActiveApprovalTeamDeletion|4xx->4xx|status
Issue-Commit: d6cf4ead3dce27ec3b8bcbc3d4a389da0fa571ea

# MPA authorization gate missing: AccessDeniedException never returned

> _Note: this issue was identified against the code base at commit `d6cf4ead3dce27ec3b8bcbc3d4a389da0fa571ea`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetApprovalTeam

## Description
The MPA emulator never produces AccessDeniedException. Authorization is not evaluated at all: requests are routed straight into the in-memory approval-team store, so a request AWS refuses with 403 either falls through to a store lookup that misses (404 ResourceNotFoundException) or is executed and succeeds (200). AccessDeniedException is a service-common error bound to every MPA operation (Smithy: com.amazonaws.mpa#AccessDeniedException, errorKind=client, httpError=403, required member Message @length max 1000), and it is modelled on all four operations below. There are 4 causes in this bundle; all four must be closed.

Cause 1 of 4 — GetApprovalTeam on an approval team in another account.
AWS: an ApprovalTeamArn whose account field is not the caller's account is not resolvable by the caller, and the request is denied before resource resolution: HTTP 403 with x-amzn-errortype AccessDeniedException:http://internal.amazon.com/coral/com.amazon.coral.service/ (Smithy: GetApprovalTeam errors AccessDeniedException httpError 403, ResourceNotFoundException httpError 404; the Arn member is an httpLabel constrained by pattern ^arn:aws(-[^:]+)?:mpa:[a-z0-9-]{1,20}:[0-9]{12}:approval-team/[a-zA-Z0-9._-]+$, so the account id is part of the identifier). The recorded response's Message is built from the request — it names the caller principal, the action mpa:GetApprovalTeam and the requested resource ARN, e.g. "User: arn:aws:iam::000000000000:user/emulator is not authorized to perform: mpa:GetApprovalTeam on resource: arn:aws:mpa:us-east-1:999999999999:approval-team/... because no resource-based policy allows the mpa:GetApprovalTeam action" — that construction rule is the requirement, the exact recorded text is only an example.
Emulator: HTTP 404 with x-amzn-errortype ResourceNotFoundException and Message "No approval team found for ARN '...'".
Why: a client cannot distinguish "not permitted" from "does not exist". SDK retry/error handling and permission-probing code branch on AccessDeniedException vs ResourceNotFoundException; against the emulator a cross-account access denial looks like a missing resource, so callers create a replacement team or treat a hard authorization failure as a benign 404.
Fix: GetApprovalTeam's handler — add the caller-identity/authorization check ahead of the state lookup, so a resource ARN outside the caller's account is denied rather than resolved.
Traces: mpa-approval-team-context-and-tagging#task#3.

Cause 2 of 4 — CreateApprovalTeam is not restricted to the organization's management account.
AWS: CreateApprovalTeam may only be called by the management account of an organization; a call from a member/standalone account is rejected with HTTP 403, x-amzn-errortype AccessDeniedException:http://internal.amazon.com/coral/com.amazon.awsfluffycoreservice/ and a Message AWS states for this condition, recorded as "Action can only be performed by an organization\u2019s management account." (Smithy: CreateApprovalTeam errors include AccessDeniedException httpError 403; the management-account restriction itself is recorded behavior only).
Emulator: HTTP 200 with a CreateApprovalTeamResponse — the team is actually created, and no Message or x-amzn-errortype header is emitted.
Why: this is a wrong SUCCESS, the most damaging shape of divergence. A client that exercises the management-account precondition records a created team, continues with its Arn/VersionId, and never sees the failure it must handle; test suites asserting the denial pass against the emulator and fail against AWS.
Fix: CreateApprovalTeam's handler/validation — evaluate the caller's organization role before mutating state and return the modelled AccessDeniedException.
Traces: mpa-approval-team-create-validation-error#task#2.

Cause 3 of 4 — UpdateApprovalTeam on an approval team in another account.
AWS: same denial-before-resolution rule as cause 1 for the mpa:UpdateApprovalTeam action: HTTP 403, x-amzn-errortype AccessDeniedException:http://internal.amazon.com/coral/com.amazon.coral.service/, Message naming the caller principal, mpa:UpdateApprovalTeam and the requested resource ARN (Smithy: UpdateApprovalTeam errors AccessDeniedException httpError 403, ResourceNotFoundException httpError 404; Arn is an httpLabel on PATCH /approval-teams/{Arn} carrying the account id).
Emulator: HTTP 404 with x-amzn-errortype ResourceNotFoundException and Message "No approval team found for ARN '...'".
Why: as cause 1, but on a mutation — a client reading 404 concludes the team is gone and may recreate it, when in reality it exists and the caller simply has no access to it.
Fix: UpdateApprovalTeam's handler — authorization check ahead of the state lookup.
Traces: mpa-approval-team-mutation-error-paths#task#2.

Cause 4 of 4 — StartActiveApprovalTeamDeletion on an approval team in another account.
AWS: same rule for the mpa:StartActiveApprovalTeamDeletion action: HTTP 403, x-amzn-errortype AccessDeniedException:http://internal.amazon.com/coral/com.amazon.coral.service/, Message naming the caller principal, the action and the requested resource ARN (Smithy: StartActiveApprovalTeamDeletion errors AccessDeniedException httpError 403, ResourceNotFoundException httpError 404; POST /approval-teams/{Arn}?Delete).
Emulator: HTTP 404 with x-amzn-errortype ResourceNotFoundException and Message "No approval team found for ARN '...'".
Why: a destructive operation reports "already absent" where AWS reports "forbidden", so cleanup/teardown logic silently treats an unauthorized deletion as a completed one.
Fix: StartActiveApprovalTeamDeletion's handler — authorization check ahead of the state lookup. Causes 1, 3 and 4 share one ARN-ownership check that belongs in MPA's request authorization path rather than in each handler's lookup.
Traces: mpa-approval-team-mutation-error-paths#task#4.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, /__admin/health 200, data listener accepting; bootstrap_signal=false, state_reset_failures=0 — so these are genuine fidelity gaps, not state bleed).

Cause 1 — mpa-approval-team-context-and-tagging#task#3, GetApprovalTeam:
  expected status 403, actual 404
  $headers.x-amzn-errortype: expected 'AccessDeniedException:http://internal.amazon.com/coral/com.amazon.coral.service/', actual 'ResourceNotFoundException'
  $.Message: expected the principal/action/resource denial text for mpa:GetApprovalTeam on arn:aws:mpa:us-east-1:999999999999:approval-team/sim2real-probe-mpa-team-82d878b7, actual "No approval team found for ARN '...'"

Cause 2 — mpa-approval-team-create-validation-error#task#2, CreateApprovalTeam:
  expected status 403, actual 200
  $headers.x-amzn-errortype: expected 'AccessDeniedException:http://internal.amazon.com/coral/com.amazon.awsfluffycoreservice/', actual missing (None)
  $.Message: expected 'Action can only be performed by an organization\u2019s management account.', actual missing (None)

Cause 3 — mpa-approval-team-mutation-error-paths#task#2, UpdateApprovalTeam:
  expected status 403, actual 404
  $headers.x-amzn-errortype: expected 'AccessDeniedException:http://internal.amazon.com/coral/com.amazon.coral.service/', actual 'ResourceNotFoundException'
  $.Message: expected the denial text for mpa:UpdateApprovalTeam on arn:aws:mpa:us-east-1:999999999999:approval-team/sim2real-probe-mpa-team-c43f7484, actual "No approval team found for ARN '...'"

Cause 4 — mpa-approval-team-mutation-error-paths#task#4, StartActiveApprovalTeamDeletion:
  expected status 403, actual 404
  $headers.x-amzn-errortype: expected 'AccessDeniedException:http://internal.amazon.com/coral/com.amazon.coral.service/', actual 'ResourceNotFoundException'
  $.Message: expected the denial text for mpa:StartActiveApprovalTeamDeletion on the same team ARN, actual "No approval team found for ARN '...'"

Model basis: smithy_query on services/service-com-amazonaws-mpa-awsfluffycoreservice/smithy-model/com-amazonaws-mpa-awsfluffycoreservice.json — sdkId MPA, restJson1, sigv4; common_errors = [AccessDeniedException, ThrottlingException, ValidationException]; AccessDeniedException httpError 403 with required Message; ResourceNotFoundException httpError 404. Status and error code are requirements; the recorded Message strings are examples of the stated construction rules.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the wire corpus (replay_wire_corpus) for trace set simparity-wire-aws-v1.
3. Cause 1: replay fixture mpa-approval-team-context-and-tagging, step task#3 — GET /approval-teams/arn%3Aaws%3Ampa%3Aus-east-1%3A999999999999%3Aapproval-team%2Fsim2real-probe-mpa-team-82d878b7 as a caller in account 000000000000. Observe 404/ResourceNotFoundException; AWS returns 403/AccessDeniedException.
4. Cause 2: replay fixture mpa-approval-team-create-validation-error, step task#2 — POST /approval-teams from a non-management account. Observe 200 with a created team; AWS returns 403/AccessDeniedException with the management-account message.
5. Cause 3: replay fixture mpa-approval-team-mutation-error-paths, step task#2 — PATCH /approval-teams/{cross-account team ARN}. Observe 404; AWS returns 403.
6. Cause 4: replay fixture mpa-approval-team-mutation-error-paths, step task#4 — POST /approval-teams/{cross-account team ARN}?Delete. Observe 404; AWS returns 403.
7. Compare each response's HTTP status, x-amzn-errortype header and body Message against the fixture's expectations.
