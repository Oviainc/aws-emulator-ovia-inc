Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: stepfunctions.get-execution-history-invalid-token stepfunctions.express-sync-failure-returns-a-result-not-an-error-with-billing-details
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: stepfunctions.get-execution-history-invalid-token|run#11|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: stepfunctions.express-sync-failure-returns-a-result-not-an-error-with-billing-details|run#22|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Commit: 6402e991ee1ffb1f37749ece0148ecf3009a760a

# Step Functions GetExecutionHistory refusal messages are emulator-authored: InvalidToken wraps and re-quotes its own sentence, and the Express-execution-ARN InvalidArn message reports a different rule than AWS (2 causes)

> _Note: this issue was identified against the code base at commit `6402e991ee1ffb1f37749ece0148ecf3009a760a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetExecutionHistory

## Description
Two net-new root causes in GetExecutionHistory's refusal messages. Both refusals reach the right error code and status (400) — only the message text diverges, and in both cases the whole message is fixed text with no variable part, so the divergence is unambiguous. Neither is covered by the open message-wording issues 20261025T131505Z-sfn-step-functions-alias-operations-resourcenotfound-returned-as-http.md (alias ResourceNotFound) or 20261025T022741Z-sfn-step-functions-data-plane-sendtasksuccess-refuses-with-unmodeled.md (SendTaskSuccess / InvalidExecutionInput).

Cause 1 of 2: InvalidToken's message is the emulator's own sentence, re-quoted inside the error name.
AWS: Smithy models `com.amazonaws.sfn#InvalidToken` on GetExecutionHistory as a client error, httpError 400, doc "The provided token is not valid.", with a single `message` member. AWS's message for a GetExecutionHistory nextToken it did not issue is the fixed text `Invalid Token` — no variable part, and in particular the offending token is never echoed.
Emulator: the message is `Invalid Token: 'Invalid token'` — the error name, a colon, and then the emulator's own lower-cased sentence in single quotes. The whole string is fixed text: there is no request- or state-derived value in it, so this is not a per-run difference.
Why: a client that matches the refusal message (common for pagination-recovery logic, which must tell a stale/forged cursor apart from other 400s) does not match, and the doubled "Invalid Token/Invalid token" is not text AWS ever sends.
Fix: the GetExecutionHistory handler's nextToken validation in the sfn crate — raise InvalidToken with the message `Invalid Token` and stop prefixing the error name and wrapping the emulator's internal sentence.
Traces: sfn.get-execution-history-invalid-token.

Cause 2 of 2: GetExecutionHistory on an Express execution ARN answers InvalidArn with a message that states a different rule than AWS's.
AWS: Smithy models `com.amazonaws.sfn#InvalidArn` on GetExecutionHistory as a client error, httpError 400, doc "The provided Amazon Resource Name (ARN) is not valid." AWS rejects an Express execution ARN shape outright rather than looking it up, and words the refusal `Invalid Arn: 'Resource type not valid in this context: express'` — the fixed text is `Invalid Arn: 'Resource type not valid in this context: <resource type>'`, with only the resource type taken from the submitted ARN (`express` here).
Emulator: the message is `Invalid Arn: 'Invalid ARN resource type: express, expected execution'`. The inner sentence is emulator-authored and asserts a different rule — that the resource type is simply wrong and `execution` was expected — where AWS says the resource type is not valid *in this context*. The resource type `express` is the only variable part in either form.
Why: the error code is right but the stated reason is wrong, so a caller (or a client matching on the message) is told the ARN is malformed rather than that an Express execution ARN is not accepted by this operation — the distinction a caller needs to know to switch to StartSyncExecution's returned result instead of the history API.
Fix: the GetExecutionHistory handler's execution-ARN validation in the sfn crate — word the InvalidArn message as AWS words it for this error class (`Invalid Arn: 'Resource type not valid in this context: <resource type from the request ARN>'`).
Traces: sfn.express-sync-failure-returns-a-result-not-an-error-with-billing-details (the check's H4 leg).

## Evidence
Cause 1 — sfn.get-execution-history-invalid-token:
  AssertionError: expected message 'Invalid Token', got "Invalid Token: 'Invalid token'"
  error code InvalidToken, HTTP 400 — both correct; only the Message diverges.
  Fixed text: the entire message ("Invalid Token"). Variable parts: none — the submitted token is not echoed by AWS.

Cause 2 — sfn.express-sync-failure-returns-a-result-not-an-error-with-billing-details:
  AssertionError: expected message "Invalid Arn: 'Resource type not valid in this context: express'", got "Invalid Arn: 'Invalid ARN resource type: express, expected execution'"
  error code InvalidArn, HTTP 400 — both correct; only the Message diverges.
  Fixed text: "Invalid Arn: 'Resource type not valid in this context: " + <resource type> + "'". Variable part: the resource type segment of the submitted ARN (here `express`), taken from the request.

Smithy reference (com-amazonaws-sfn-awsstepfunctions.json):
  GetExecutionHistory errors include InvalidArn (client, httpError 400) and InvalidToken (client, httpError 400); each is a structure with a single `message` member.

## Steps to Reproduce
Cause 1:
1. Create an execution role and a STANDARD state machine; StartExecution and let it reach a terminal state.
2. Call GetExecutionHistory(executionArn=<that execution>, nextToken="<a token this service never issued>").
3. Observe InvalidToken / HTTP 400 with Message "Invalid Token: 'Invalid token'" instead of "Invalid Token".

Cause 2:
1. Create an EXPRESS state machine and call StartSyncExecution on it.
2. Take the returned executionArn (an `express` resource-type ARN) and call GetExecutionHistory(executionArn=<that ARN>).
3. Observe InvalidArn / HTTP 400 with Message "Invalid Arn: 'Invalid ARN resource type: express, expected execution'" instead of "Invalid Arn: 'Resource type not valid in this context: express'".
