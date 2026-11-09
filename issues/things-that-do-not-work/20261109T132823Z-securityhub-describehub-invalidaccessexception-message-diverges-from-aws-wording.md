Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__securityhub.securityhub-hub-invalid-mode-rejected-and-children-round-trip
Issue-Key: cloudformation__securityhub|DescribeHub|InvalidAccessException
Issue-Commit: e901ea078181297e70b2ac3a5c8be54768ac76d3

# securityhub DescribeHub InvalidAccessException message diverges from AWS wording (adds a Region clause, drops "AWS")

> _Note: this issue was identified against the code base at commit `e901ea078181297e70b2ac3a5c8be54768ac76d3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeHub

## Description
One net-new root cause.

Cause 1 of 1: DescribeHub's not-subscribed refusal carries the wrong error message text. Failing check: cloudformation__securityhub.securityhub-hub-invalid-mode-rejected-and-children-round-trip

AWS: Smithy (services/service-com-amazonaws-securityhub-securityhubapiservice/smithy-model/com-amazonaws-securityhub-securityhubapiservice.json): DescribeHub, http GET /accounts, errors include InvalidAccessException (client, httpError 401, documented "The account doesn't have permission to perform this action"). Recorded behavior: when the calling account has no Security Hub CSPM subscription in the Region, DescribeHub is refused with HTTP 401, error code InvalidAccessException, and the message template
    Account <account-id> is not subscribed to AWS Security Hub CSPM
The fixed text is "Account ", " is not subscribed to AWS Security Hub CSPM" — there is NO trailing Region clause and the product name is spelled "AWS Security Hub CSPM". The only variable part is <account-id>, taken from the caller's credentials (the recording's 000000000000 is one run's value, not part of the template).

Emulator: returns the correct status (401) and code (InvalidAccessException) but the message
    Account <account-id> is not subscribed to Security Hub CSPM in Region <region>
i.e. it omits the "AWS " prefix of the product name and appends an " in Region <region>" clause AWS does not emit. Replay output: expected ('InvalidAccessException', 'Account 000000000000 is not subscribed to AWS Security Hub CSPM'), got ('InvalidAccessException', 'Account 000000000000 is not subscribed to Security Hub CSPM in Region us-west-2').

Why: the message is part of the contract for this error class. A client (or test harness, or runbook) that asserts on, logs, or pattern-matches the not-subscribed message — the common way to distinguish "Security Hub is not enabled here" from "you lack IAM permission", since both arrive as InvalidAccessException/401 — sees text that never appears in AWS and mis-classifies the refusal. Here it also halts the whole account-singleton check at its very first precondition assertion, so every later behavior in that trace (CloudFormation AWS::SecurityHub::Hub validation rejection, the hub/child round trip, and the unsubscribe-on-delete) was never exercised against the emulator.

Fix: the DescribeHub handler's error construction — the InvalidAccessException built when no Hub record exists for the account/Region key must use AWS's fixed wording with only the account id interpolated, and no Region interpolation. Any other Security Hub operation that gates on "account not subscribed" should emit the identical fixed text so the wording is uniform, as it is in AWS.

Traces: cloudformation__securityhub.securityhub-hub-invalid-mode-rejected-and-children-round-trip (mismatch at the check's _assert_hub_not_subscribed precondition, which calls DescribeHub with no subscription present).

## Evidence
Check: cloudformation__securityhub.securityhub-hub-invalid-mode-rejected-and-children-round-trip [mismatch]

AssertionError: expected ('InvalidAccessException', 'Account 000000000000 is not subscribed to AWS Security Hub CSPM'), got ('InvalidAccessException', 'Account 000000000000 is not subscribed to Security Hub CSPM in Region us-west-2')

Expected (AWS):
  HTTP 401, Code=InvalidAccessException,
  Message="Account <account-id> is not subscribed to AWS Security Hub CSPM"

Actual (emulator):
  HTTP 401, Code=InvalidAccessException,
  Message="Account <account-id> is not subscribed to Security Hub CSPM in Region <region>"

Status and error code match; only the message text diverges: "AWS " is missing before "Security Hub CSPM", and an extra " in Region <region>" suffix is added.

Source of the diverging text: services/service-com-amazonaws-securityhub-securityhubapiservice/src/ops/describe_hub.rs lines 179-189, which formats "Account {} is not subscribed to Security Hub CSPM in Region {}" from the (account, region) hub key.

## Steps to Reproduce
1. Boot the emulator with no Security Hub CSPM subscription for the calling account/Region (fresh state, i.e. EnableSecurityHub never called, or called and then DisableSecurityHub).
2. Call securityhub DescribeHub (GET /accounts) with no HubArn.
3. Observe the refusal: HTTP 401 with Code=InvalidAccessException is correct, but the Message reads "Account <account-id> is not subscribed to Security Hub CSPM in Region <region>" instead of AWS's "Account <account-id> is not subscribed to AWS Security Hub CSPM".
4. Re-replay check cloudformation__securityhub.securityhub-hub-invalid-mode-rejected-and-children-round-trip to confirm; note it currently fails at its first precondition, so fixing this unblocks the remainder of that trace.
