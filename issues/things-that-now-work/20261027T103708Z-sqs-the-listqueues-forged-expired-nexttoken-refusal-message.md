Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sqs.list-queues-pagination-bad-input-rejected
Issue-Shape-Id: com.amazonaws.sqs#AmazonSQS
Issue-Key: sqs.list-queues-pagination-bad-input-rejected|run#10|AssertionError|assert msg == MSG_TOKEN, f"a forged NextToken gave message {msg!r}"
Issue-Commit: 415bc37b6ebe55c6c7e638f2212d4379947a3e7c

# sqs: the ListQueues forged/expired NextToken refusal message is the emulator's own wording instead of AWS's 'Invalid or expired next token.'

> _Note: this issue was identified against the code base at commit `415bc37b6ebe55c6c7e638f2212d4379947a3e7c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListQueues

## Description
One net-new behavioral root cause in SQS's ListQueues pagination validation, grounded in a check the replay report marks neither ALREADY FILED nor OWNED.

Cause 1 of 1: a NextToken that this service did not issue is refused with the right code but the wrong message.

AWS: ListQueues is a paginated operation (Smithy: paginated {inputToken: NextToken, outputToken: NextToken, items: QueueUrls, pageSize: MaxResults}; input {QueueNamePrefix, NextToken: Token, MaxResults: BoxedInteger}). A NextToken value that was not issued by a previous ListQueues page — or that has expired — is refused with Error.Code `InvalidParameterValue` and the message `Invalid or expired next token.` That message is entirely FIXED text: it echoes no part of the request, names no parameter and carries no variable component at all. The sibling MaxResults bound uses the other template in the same family, `Value for parameter MaxResults is invalid. Reason: MaxResults must be an integer between 1 and 1000.`, and the emulator already matches that one exactly (that leg of the check passes), which isolates the divergence to the token leg.

Emulator: the code is correct (`InvalidParameterValue`) but the message is emulator-authored prose that additionally echoes the supplied token: `Value sim2real-not-a-token for parameter NextToken is invalid. Reason: it was not issued by this listing.` The fixed text 'for parameter NextToken is invalid. Reason: it was not issued by this listing.' appears in no AWS message template, and AWS's template has no slot for the token value at all — the variable `sim2real-not-a-token` here is the NextToken taken from the request, which AWS does not reproduce.

Why: an error message is part of the contract. A paginating client (or an SDK pagination helper, or an operator reading a log) that recognises 'Invalid or expired next token.' to restart a listing from the first page does not match the emulator's string, so a resumable listing becomes an unhandled failure; and because the emulator echoes the token, any code that logs or compares the message leaks and keys on a value AWS never returns.

Fix: the ListQueues handler's NextToken validation message template in the sqs service — refuse an unissued or expired token with the fixed string 'Invalid or expired next token.' under Error.Code InvalidParameterValue, with no interpolation of the supplied token.

Traces: sqs.list-queues-pagination-bad-input-rejected.

## Evidence
From the behavioral replay report (suite simparity-behavioral-ga-prod-v1), verdict [mismatch]:

  Error: File "check_sqs_list_queues_pagination_bad_input_rejected.py", line 45, in run
    assert msg == MSG_TOKEN, f"a forged NextToken gave message {msg!r}"
  AssertionError: a forged NextToken gave message 'Value sim2real-not-a-token for parameter NextToken is invalid. Reason: it was not issued by this listing.'

Call: ListQueues(QueueNamePrefix=<unique prefix>, MaxResults=5, NextToken='sim2real-not-a-token').

Expected (AWS): Error.Code 'InvalidParameterValue', Error.Message 'Invalid or expired next token.' (wholly fixed text).
Actual (emulator): Error.Code 'InvalidParameterValue' (correct — the preceding code assertion passed), Error.Message 'Value {token} for parameter NextToken is invalid. Reason: it was not issued by this listing.' — emulator-authored fixed text, plus the request's NextToken interpolated where AWS interpolates nothing.

The same check's earlier leg passes: ListQueues(MaxResults=1001) is already refused with InvalidParameterValue and the exact AWS message 'Value for parameter MaxResults is invalid. Reason: MaxResults must be an integer between 1 and 1000.', so only the token template diverges.

## Steps to Reproduce
1. Boot the emulator and replay sqs.list-queues-pagination-bad-input-rejected.
2. Direct repro: sqs CreateQueue with a unique name, then sqs ListQueues(QueueNamePrefix=<that name>, MaxResults=5, NextToken='not-a-token').
3. Compare Error.Code and Error.Message. AWS: InvalidParameterValue with the fixed message 'Invalid or expired next token.'. Emulator: InvalidParameterValue with 'Value not-a-token for parameter NextToken is invalid. Reason: it was not issued by this listing.'
4. Control (already passing, keep it passing): ListQueues(QueueNamePrefix=<name>, MaxResults=1001) must stay InvalidParameterValue with 'Value for parameter MaxResults is invalid. Reason: MaxResults must be an integer between 1 and 1000.', and ListQueues(QueueNamePrefix=<name>, MaxResults=1000) must return exactly the one created queue URL and no NextToken.
