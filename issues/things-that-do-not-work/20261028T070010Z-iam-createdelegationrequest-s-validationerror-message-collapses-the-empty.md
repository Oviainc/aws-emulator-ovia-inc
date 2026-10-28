Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam.op-contract-getDelegationRequest
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam.op-contract-getDelegationRequest|run#16|AssertionError|assert err.get("Message") == expected, f"expected message {expected!r}, got {err!r}"
Issue-Commit: 5b75342feea94a0290087428a6e90d9d2afc8417

# iam: CreateDelegationRequest's ValidationError message collapses the empty interpolated ARN, emitting 'Invalid ARN: Could not be parsed!' instead of AWS's 'Invalid ARN:  Could not be parsed!'

> _Note: this issue was identified against the code base at commit `5b75342feea94a0290087428a6e90d9d2afc8417`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDelegationRequest

## Description
REOPENS 20261027T161925Z-iam-createdelegationrequest-stub.md: its fix left the call refused. The operation is no longer a 501 scaffold and it now refuses the request with the right class of error, but the refusal message it emits is not AWS's text, so the check that exercises the refusal still fails.

Cause 1 of 1: the ValidationError message template for an unparseable PolicyTemplateArn drops the whitespace that surrounds the interpolated ARN value, so the message is one space short of AWS's.

AWS: CreateDelegationRequest is not available for general use without partner onboarding, so any Permissions.PolicyTemplateArn is refused. Smithy (com-amazonaws-iam-awsidentitymanagementv20100508.json) models CreateDelegationRequest's errors as ConcurrentModificationException/409, EntityAlreadyExistsException/409, InvalidInputException/400, LimitExceededException/400-ish (409 awsQueryError LimitExceeded) and ServiceFailureException/500; ValidationError is NOT a modeled error for this operation, so the refusal is recorded behavior only. The recording shows HTTP 400 with Type=Sender, Code=ValidationError and Message 'Invalid ARN:  Could not be parsed!' — i.e. the fixed text is 'Invalid ARN:' + ' ' + <the ARN as received> + ' ' + 'Could not be parsed!', and because this code path reports an empty ARN slot the rendered message carries two consecutive spaces after the colon. The fixed parts are 'Invalid ARN:' and 'Could not be parsed!'; the variable part is the ARN value taken from the request (empty here), and the surrounding single spaces are part of the template, not of the value.

Emulator: the same request is refused with the correct status, Type=Sender and Code=ValidationError, but Message 'Invalid ARN: Could not be parsed!' — a single space between the colon and 'Could'. The emulator builds the message by joining non-empty fragments (or by trimming/normalising whitespace), so the empty ARN slot disappears instead of rendering as an empty token between the two fixed spaces.

Why: clients and test suites that match IAM refusal text exactly (log scrapers, SDK retry/classification wrappers, and parity assertions like this check) see a message that does not equal AWS's, so an otherwise-correct refusal is read as an unknown error. Because the status and code are right, the divergence is invisible until something compares the message byte-for-byte, which makes it a silent fidelity gap rather than a loud one.

Fix: the CreateDelegationRequest handler's validation/refusal path in the IAM crate — the place that formats the ValidationError message for an ARN that cannot be parsed. It must render the ARN value verbatim into the template (including when that value is empty) rather than collapsing or trimming the surrounding whitespace.

Traces: iam.op-contract-getDelegationRequest — the GetDelegationRequest half of the check passes (NoSuchEntity with 'DelegationRequest not found: <id>'); the assertion that fails is the CreateDelegationRequest message comparison: expected 'Invalid ARN:  Could not be parsed!', got {'Type': 'Sender', 'Code': 'ValidationError', 'Message': 'Invalid ARN: Could not be parsed!'}.

## Evidence
Failing check iam.op-contract-getDelegationRequest (verdict: mismatch).

AssertionError from the replay:
  File "check_iam_op_contract_getDelegationRequest.py", line 48, in run
    assert err.get("Message") == expected, f"expected message {expected!r}, got {err!r}"
  AssertionError: expected message 'Invalid ARN:  Could not be parsed!', got {'Type': 'Sender', 'Code': 'ValidationError', 'Message': 'Invalid ARN: Could not be parsed!'}

Expected (AWS): HTTP 400, Type=Sender, Code=ValidationError, Message = 'Invalid ARN:' + ' ' + <PolicyTemplateArn as reported, empty on this path> + ' ' + 'Could not be parsed!' -> 'Invalid ARN:  Could not be parsed!' (two spaces).
Actual (emulator): HTTP 400, Type=Sender, Code=ValidationError, Message 'Invalid ARN: Could not be parsed!' (one space).

Smithy: CreateDelegationRequest errors = ConcurrentModificationException, EntityAlreadyExistsException, InvalidInputException, LimitExceededException, ServiceFailureException; ValidationError is unmodeled for this operation (recorded behavior only). Input requires Description, Permissions, RequestorWorkflowId, NotificationChannel, SessionDuration (300..43200).

Side observations from the same check that currently hold and must keep holding after the fix: ListDelegationRequests returns no request for the rejected Description, and GetDelegationRequest of an unknown id answers Code=NoSuchEntity with 'DelegationRequest not found: <DelegationRequestId>'.

## Steps to Reproduce
1. Boot the emulator and point an IAM client at it.
2. account_id = sts.get_caller_identity()['Account'].
3. Call iam.create_delegation_request(Description='<any 1..1000 char description>', Permissions={'PolicyTemplateArn': f'arn:aws:iam::{account_id}:policy-template/<any-name>'}, RequestorWorkflowId='<any 5..400 chars>', NotificationChannel='none', SessionDuration=3600).
4. Catch the ClientError and inspect response['Error']['Message'].
   Observed: 'Invalid ARN: Could not be parsed!' (single space).
   Expected: 'Invalid ARN:  Could not be parsed!' (the empty ARN slot rendered between the template's two spaces).
5. Re-replay behavioral check iam.op-contract-getDelegationRequest to confirm.
