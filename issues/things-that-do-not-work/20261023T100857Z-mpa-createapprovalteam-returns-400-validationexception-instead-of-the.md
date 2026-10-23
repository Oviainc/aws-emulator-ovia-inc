Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: mpa.create-approval-team-invalid-name-pattern-rejected
Issue-Shape-Id: com.amazonaws.mpa#AWSFluffyCoreService
Issue-Key: mpa.create-approval-team-invalid-name-pattern-rejected|run#12|AssertionError|assert invalid_code == ("AccessDeniedException", MANAGEMENT_ACCOUNT_ONLY), invalid_code
Issue-Commit: 3b7cd50bec204a941815a8ace6b2a1778291c65c

# mpa: CreateApprovalTeam returns 400 ValidationException instead of the 403 AccessDeniedException AWS raises for a non-management-account caller (org gate not enforced, and not enforced ahead of input validation)

> _Note: this issue was identified against the code base at commit `3b7cd50bec204a941815a8ace6b2a1778291c65c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApprovalTeam

## Description
Cause 1 of 1: CreateApprovalTeam does not implement the AWS Organizations management-account authorization gate, and therefore also does not enforce it ahead of request-shape validation. Failing check: mpa.create-approval-team-invalid-name-pattern-rejected.

AWS: CreateApprovalTeam is restricted to a principal in the organization's management account. A caller outside that account is refused with HTTP 403 and error code AccessDeniedException, and that authorization decision is made BEFORE the request body is validated, so a request whose Name violates the service pattern gets exactly the same AccessDeniedException as an otherwise identical request with a well-formed Name (recorded behavior; the recorded message is an example: "Action can only be performed by an organization’s management account."). Smithy (com.amazonaws.mpa#CreateApprovalTeam) models both outcomes as possible errors: AccessDeniedException (httpError 403, doc "You do not have sufficient access to perform this action.") and ValidationException (httpError 400); CreateApprovalTeamRequest.Name is @required with @length max 64 and @pattern ^[a-zA-Z0-9._-]+$. The model does not order the two checks — the ordering is recorded behavior only.

Emulator: the request is run straight through Smithy constraint validation with no authorization gate. A CreateApprovalTeam whose Name contains a space is answered with HTTP 400 ValidationException (observed message: "1 validation error detected: Value 'sim2real invalid name <hex>' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9._-]+$"). No AccessDeniedException is produced for any CreateApprovalTeam request. Because the check fails on this first probe, the emulator's answer for the well-formed-Name control request is not observed; AWS answers that one with the same 403 AccessDeniedException, so the emulator is at risk of ACCEPTING a team creation real AWS refuses (the check's group is 'overly-permissive' for that reason).

Why: a client that handles the documented refusal path gets the wrong class of failure. It sees a 400 client-input error and will try to "fix" the request (sanitize the Name, retry with a different Name) when the real condition is an unfixable 403 authorization refusal; AccessDeniedException-specific handling (surface a permissions/org-membership message, stop retrying, escalate) never runs. Worse, if the emulator then accepts a well-formed request, tests that rely on MPA refusing approval-team creation outside the management account will pass against the emulator and fail against AWS, and the emulator will hold state (an approval team) that cannot exist in the recorded account class.

Fix: CreateApprovalTeam's handler in services/service-com-amazonaws-mpa-awsfluffycoreservice (src/ops/create_approval_team.rs) — add the org-management-account authorization gate and run it before input validation, so any caller that is not the organization's management-account principal gets 403 AccessDeniedException regardless of the body, and no team is created.

Traces: mpa.create-approval-team-invalid-name-pattern-rejected (assert at line 66: expected ('AccessDeniedException', management-account message), got ('ValidationException', '1 validation error detected: ... at 'name' failed to satisfy constraint ...')).

## Evidence
Check: mpa.create-approval-team-invalid-name-pattern-rejected [mismatch]

Request: CreateApprovalTeam (us-east-1) with Name="sim2real invalid name <uuid4hex>" (space violates ^[a-zA-Z0-9._-]+$), Description set, ApprovalStrategy={MofN:{MinApprovalsRequired:1}}, one Approver, one Policy taken from ListPolicies.

Expected (real AWS, recorded): HTTP 403, Error.Code = "AccessDeniedException" (example message: "Action can only be performed by an organization’s management account."), and the identical 403 for the follow-up request with a well-formed Name; ListApprovalTeams shows no team for either Name.

Actual (emulator): HTTP 400, Error.Code = "ValidationException", message "1 validation error detected: Value 'sim2real invalid name a248f6fda3d443fbbc2ca0dc6ef9d58c' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9._-]+$". Traceback: check_mpa_create_approval_team_invalid_name_pattern_rejected.py line 66, AssertionError: ('ValidationException', "1 validation error detected: ...").

Smithy consulted: com.amazonaws.mpa#CreateApprovalTeam errors = AccessDeniedException(403), ConflictException(409), InternalServerException(500), ServiceQuotaExceededException(402), ThrottlingException(429), ValidationException(400); CreateApprovalTeamRequest.Name @required @length(max 64) @pattern ^[a-zA-Z0-9._-]+$.

## Steps to Reproduce
1. Point an MPA client at the emulator in us-east-1 (MPA is single-Region): client = boto3.client('mpa', region_name='us-east-1').
2. policy = client.list_policies(MaxResults=1)['Policies'][0]; policy_arn = policy['Arn'] + '/' + policy['DefaultVersion'].
3. Call client.create_approval_team(Name='sim2real invalid name ' + uuid.uuid4().hex, Description='probe', ApprovalStrategy={'MofN': {'MinApprovalsRequired': 1}}, Approvers=[{'PrimaryIdentityId': uuid.uuid4().hex[:20], 'PrimaryIdentitySourceArn': 'arn:aws:mpa:us-east-1:<account>:identity-source/' + uuid.uuid4().hex}], Policies=[{'PolicyArn': policy_arn}]).
4. Observe HTTP 400 / ValidationException with a Name-pattern constraint message. Expected HTTP 403 / AccessDeniedException (management-account-only refusal, decided before body validation).
5. Repeat step 3 with a well-formed Name (e.g. 'sim2real-mpa-ctrl-<hex>'): AWS also answers 403 AccessDeniedException and creates nothing; verify the emulator does not instead create a team, and that ListApprovalTeams contains neither Name.
