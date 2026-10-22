Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: workspaces.account-link-invitation-pending-state-lifecycle workspaces.account-link-invalid-target-account-id-rejected
Issue-Shape-Id: com.amazonaws.workspaces#WorkspacesService
Issue-Key: workspaces.account-link-invitation-pending-state-lifecycle|_client_error#5|AssertionError|raise AssertionError(f"expected rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: workspaces.account-link-invalid-target-account-id-rejected|run#6|AssertionError|assert error == expected_error, f"expected {expected_error!r}, got {error!r}"
Issue-Commit: 38b62e174af309db3e46216fa3b3f7ac7f6f5b84

# CreateAccountLinkInvitation accepts a non-existent target account and creates a PENDING link instead of AccessDeniedException

> _Note: this issue was identified against the code base at commit `38b62e174af309db3e46216fa3b3f7ac7f6f5b84`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAccountLinkInvitation

## Description
Cause 1 of 1: WorkSpaces CreateAccountLinkInvitation is overly permissive — it treats a syntactically well-formed TargetAccountId as sufficient grounds to create an account link, so any 12-digit account id is accepted and a PENDING link is persisted.

AWS: Smithy (com.amazonaws.workspaces#CreateAccountLinkInvitation) models TargetAccountId -> AwsAccount, required, pattern ^\d{12}$, and declares the errors AccessDeniedException, ConflictException, ValidationException, InternalServerException (all HTTP 400 in this model). Format validity is only the first gate: recorded behavior shows real WorkSpaces additionally requires that the target account actually exist and belong to the caller's parent AWS Organization, and otherwise rejects the call with error code AccessDeniedException (HTTP 400 per the model's defaulted httpError) and creates nothing — ListAccountLinks is unchanged. The recorded message, as an example of that rule, is "Please ensure both accounts to be linked exist and belong to the same parent AWS Organization."

Emulator: the call SUCCEEDS with HTTP 200 and returns a fabricated link, e.g. {'AccountLink': {'AccountLinkId': 'link-0000000000000001', 'AccountLinkStatus': 'PENDING_ACCEPTANCE_BY_TARGET_ACCOUNT', 'SourceAccountId': '000000000000', 'TargetAccountId': '<random 12-digit id>'}}. No existence/Organization-membership check is performed; the handler only maps store-level Conflict/Validation refusals (services/service-com-amazonaws-workspaces-workspacesservice/src/ops/create_account_link_invitation.rs, account_link_error at ~line 116 and handle at ~line 170 calling state.create_account_link_invitation), and AccessDeniedException is never constructed even though it is a modeled variant of CreateAccountLinkInvitationErrorModeled.

Why: a client gets a 200 and a PENDING AccountLinkId for a link real AWS refuses to create. Code that drives the invitation lifecycle (poll status, accept/reject, delete) then operates on a link that could never exist against AWS, and negative tests that verify an invitation to an unknown/out-of-organization account is denied pass against the emulator and fail against AWS. Orphaned PENDING state is also exposed through ListAccountLinks/GetAccountLink that AWS would never show.

Fix: the operation's handler/validation and the account-link state store — CreateAccountLinkInvitation must reject a TargetAccountId that is not an existing, same-Organization account with AccessDeniedException and must not persist any link record for it. The AccessDeniedException arm of the error mapping needs to exist alongside the Conflict/Validation arms.

Secondary note for the fixer (NOT filed as a separate defect, message text alone is not treated as a requirement here): workspaces.account-link-invalid-target-account-id-rejected fails one assertion earlier, on the ValidationException MESSAGE for a malformed TargetAccountId='12345'. The error CODE already matches (ValidationException); the emulator returns the generic constraint text "1 validation error detected: Value '12345' at 'targetAccountId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^\d{12}$" where the recording shows "The request is invalid.". That check will only pass once both its legs line up, so it is listed in the replay set.

Also not yet observed: because workspaces.account-link-invitation-pending-state-lifecycle aborts at this first call, its later legs (GetAccountLink and DeleteAccountLinkInvitation on a fabricated-but-well-formed LinkId must answer ResourceNotFoundException) were never exercised. Re-replay both listed checks after the fix to confirm them.

Traces: workspaces.account-link-invitation-pending-state-lifecycle (expected rejection, call SUCCEEDED returning a PENDING link); workspaces.account-link-invalid-target-account-id-rejected (same AccessDeniedException leg, plus the message note above).

## Evidence
Expected (real AWS, from the checks' recorded expectations):
  create_account_link_invitation(TargetAccountId=<well-formed, non-existent 12-digit id>)
  -> ClientError, Code='AccessDeniedException', Message='Please ensure both accounts to be linked exist and belong to the same parent AWS Organization.' (example text)
  -> list_account_links() unchanged, no new AccountLinkId

Actual (emulator):
  HTTP 200, {'AccountLink': {'AccountLinkId': 'link-0000000000000001', 'AccountLinkStatus': 'PENDING_ACCEPTANCE_BY_TARGET_ACCOUNT', 'SourceAccountId': '000000000000', 'TargetAccountId': '267370265480'}}
  (assertion: "expected rejection, but the call SUCCEEDED and returned ...")

Secondary, message-only (code already matches):
  TargetAccountId='12345' -> expected ('ValidationException', 'The request is invalid.'),
  got ('ValidationException', "1 validation error detected: Value '12345' at 'targetAccountId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^\\d{12}$")

## Steps to Reproduce
1. Boot the emulator and point a botocore 'workspaces' client at it.
2. source = sts.get_caller_identity()['Account']; pick any random 12-digit id != source that is not a real account in the caller's Organization.
3. before = {l['AccountLinkId'] for l in ws.list_account_links().get('AccountLinks', [])}
4. ws.create_account_link_invitation(TargetAccountId=<that id>)
   Expected: ClientError with Code 'AccessDeniedException' and no state change.
   Actual: HTTP 200 with a PENDING_ACCEPTANCE_BY_TARGET_ACCOUNT AccountLink.
5. ws.list_account_links() now contains a link that was never legitimately creatable.
6. Also: ws.create_account_link_invitation(TargetAccountId='12345') returns ValidationException with the generic constraint message instead of 'The request is invalid.'.
