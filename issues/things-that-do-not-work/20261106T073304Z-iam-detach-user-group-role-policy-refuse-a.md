Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam.invalid-policy-arn-on-attach-detach-rejected
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam.invalid-policy-arn-on-attach-detach-rejected|run#14|AssertionError|assert message == DETACH_MALFORMED_MESSAGE, (
Issue-Commit: c83662acd782fe5bfa18e86152cd5905ede91863

# iam: Detach{User,Group,Role}Policy refuse a malformed PolicyArn with the Attach-side message 'ARN <value> is not valid.' instead of AWS's 'Invalid ARN:  Could not be parsed!'

> _Note: this issue was identified against the code base at commit `c83662acd782fe5bfa18e86152cd5905ede91863`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DetachUserPolicy

## Description
One root cause. Number of causes to close: 1.

Open/resolved issues that touch neighbouring wording but do NOT carry this replay key and do not cover it: 20261029T192356Z-iam-10-net-new-refusal-message-and-response.md (resolved; its cause 1 is the Detach* NoSuchEntity text for a policy that is not ATTACHED — 'Policy <arn> was not found.' — a different refusal class from the malformed-ARN ValidationError here) and 20261028T070010Z-iam-createdelegationrequest-s-validationerror-message-collapses-the-empty.md (resolved; it establishes the same 'Invalid ARN:  Could not be parsed!' template on the CreateDelegationRequest path).

Cause 1 of 1: the three Detach*Policy handlers reuse the Attach-side malformed-ARN message, so AWS's detach-side wording is never emitted.
AWS: IAM words the malformed-PolicyArn refusal DIFFERENTLY on the attach and the detach side, and the behavioral oracle is the recorded response — Smithy (com.amazonaws.iam#DetachUserPolicy / DetachRolePolicy / DetachGroupPolicy) models only InvalidInputException (awsQueryError InvalidInput, HTTP 400), LimitExceededException (409), NoSuchEntityException (404), ServiceFailureException (500) (DetachRolePolicy additionally UnmodifiableEntityException), so the ValidationError both AWS and the emulator actually use is recorded behavior only. For a PolicyArn that is not ARN-shaped at all ('not-a-valid-policy-arn' in the check), the recording shows HTTP 400 Code=ValidationError and:
  - Attach{User,Group,Role}Policy -> Message 'ARN <the PolicyArn as received> is not valid.' — fixed text 'ARN ' + the request's PolicyArn + ' is not valid.'
  - Detach{User,Group,Role}Policy -> Message 'Invalid ARN:  Could not be parsed!' — wholly fixed text; the value slot between 'Invalid ARN:' and 'Could not be parsed!' renders EMPTY (hence two consecutive spaces after the colon) because IAM reports nothing it could parse, so the request's value never appears in this message.
Emulator: the status (400) and the code (ValidationError) are right on both sides, and the three Attach* messages are right, but all three Detach* calls answer with the ATTACH text — observed 'ARN not-a-valid-policy-arn is not valid.' where AWS answers 'Invalid ARN:  Could not be parsed!'. ('not-a-valid-policy-arn' is the check's input value, not part of the requirement.)
Why: a client or log scraper matching IAM's refusal text byte-for-byte on the detach path never matches, and the emulator's message leaks the caller's raw input into a message AWS deliberately empties; because the status and code are correct the divergence is silent until something compares the message.
Fix: the malformed-ARN refusal path of the detach_user_policy / detach_role_policy / detach_group_policy handlers — they must emit IAM's detach-side ValidationError template ('Invalid ARN:' + ' ' + <unparsed value, empty on this path> + ' ' + 'Could not be parsed!') rather than reusing the attach-side 'ARN <value> is not valid.' formatter.
Traces: iam.invalid-policy-arn-on-attach-detach-rejected.

## Evidence
Suite simparity-behavioral-aws-v1 replayed against this checkout; service heading `### iam` 331/456, 124 failed. The check below is marked neither ALREADY FILED nor OWNED.

[mismatch] iam.invalid-policy-arn-on-attach-detach-rejected, at check line 86:
  assert message == DETACH_MALFORMED_MESSAGE, (...)
  AssertionError: DetachUserPolicy: expected AWS's exact message 'Invalid ARN:  Could not be parsed!' for a malformed ARN, got 'ARN not-a-valid-policy-arn is not valid.'

State at that point: the check created a user, a group and a role, and already asserted that AttachUserPolicy, AttachGroupPolicy and AttachRolePolicy each refuse the same malformed ARN with ValidationError and the attach-side message 'ARN <value> is not valid.' — those three passed, so the divergence is specific to the detach side. DetachUserPolicy is the first of the three detach cases (DetachGroupPolicy and DetachRolePolicy are unreached but share the handler family and the same expectation).

Expected (AWS): HTTP 400, Error.Code 'ValidationError', Error.Message 'Invalid ARN:  Could not be parsed!' (two spaces after the colon) for Detach{User,Group,Role}Policy; nothing attached afterwards; and the same three calls with a real managed-policy ARN (arn:aws:iam::aws:policy/ReadOnlyAccess) attach and detach cleanly on all three principal types.
Actual (emulator): HTTP 400, Error.Code 'ValidationError', Error.Message 'ARN not-a-valid-policy-arn is not valid.'

## Steps to Reproduce
1. Boot the emulator from this checkout and point an IAM client at it.
2. CreateUser U, CreateGroup G, CreateRole R (any valid EC2 trust policy).
3. Call attach_user_policy(UserName=U, PolicyArn='not-a-valid-policy-arn'), attach_group_policy(GroupName=G, ...), attach_role_policy(RoleName=R, ...). Each must answer Code='ValidationError' with Message 'ARN not-a-valid-policy-arn is not valid.' (these already pass).
4. Call detach_user_policy(UserName=U, PolicyArn='not-a-valid-policy-arn'), detach_group_policy(GroupName=G, ...), detach_role_policy(RoleName=R, ...).
   Observed: Code='ValidationError', Message 'ARN not-a-valid-policy-arn is not valid.'
   Expected: Code='ValidationError', Message 'Invalid ARN:  Could not be parsed!' (the value slot renders empty, two spaces after the colon), for all three.
5. Confirm ListAttachedUser/Group/RolePolicies are still empty, then confirm the positive control: attach and detach arn:aws:iam::aws:policy/ReadOnlyAccess on all three principal types.
6. Re-replay iam.invalid-policy-arn-on-attach-detach-rejected.
