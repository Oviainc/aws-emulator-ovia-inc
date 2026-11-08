Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sqs.async-retry-budget-of-two-yields-exactly-three-attempts
Issue-Key: lambda__sqs.async-retry-budget-of-two-yields-exactly-three-attempts|_receive#16|Timeout|return out
Issue-Commit: 582ed21b383f102b650a2b551c0e79525419c00c

# lambda__sqs: an asynchronous invoke with MaximumRetryAttempts=2 never produces three handler attempts nor an OnFailure RetriesExhausted record, so the retry-budget check never observes a single attempt

> _Note: this issue was identified against the code base at commit `582ed21b383f102b650a2b551c0e79525419c00c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
1 net-new root cause, on a check the replay report did NOT mark ALREADY FILED or OWNED. Every sibling asynchronous-invocation check in this group IS owned (20261107T014900Z-lambda-sqs-the-asynchronous-invocation-plane-is-inert.md and 20261107T212447Z-lambda-sqs-asynchronous-invocation-destinations-and-deadletterconfig-never-deliver-a.md), and those two issues describe the same inert async plane; neither carries THIS check's replay key, so the retry-budget cause is grounded here on its own key. A fixer closing those two should re-replay this check as well.

--- Cause 1 of 1: the retry budget is never spent — no second or third attempt runs and no exhausted record is written ---
AWS: Smithy: PutFunctionEventInvokeConfig / UpdateFunctionEventInvokeConfig carry MaximumRetryAttempts (range 0-2) and DestinationConfig{OnSuccess, OnFailure}; Invoke with InvocationType='Event' returns 202 and queues the event. AWS's documented asynchronous semantics: MaximumRetryAttempts=2 means the original invocation plus up to two retries — exactly THREE handler entries for one failing event — and when the budget is exhausted the OnFailure destination receives the invocation record with requestContext.condition == 'RetriesExhausted' and requestContext.approximateInvokeCount == 3, written with the FUNCTION's execution role (so sqs:SendMessage on the destination queue must be enforced against that role). Retries reuse the same event payload. Recorded behavior only for the pacing (AWS backs off roughly 1 then 2 minutes between attempts) — the REQUIREMENT is the attempt COUNT, the record and its requestContext fields, not the delay.
Emulator: nothing observable happens. The function's own counting messages never appear on the counter queue and no record ever reaches the OnFailure queue, so the check's poll for attempts never advances and the check is cut off at the harness window (reported as 'Check timed out after 180.0s'); the emulator produced no attempt, no retry and no destination record in that window. This is the same inert asynchronous plane the two open issues named above describe from other checks: an Event invoke is accepted but the event is never re-driven and no event-invoke destination is ever written.
Why: the whole asynchronous-failure contract a client builds on is absent. Code that relies on Lambda retrying a transient failure twice before giving up sees one (or zero) attempts, so transient errors become permanent data loss; code that watches the OnFailure destination for RetriesExhausted records (the standard async dead-letter pattern, and the only place approximateInvokeCount is reported) never receives anything, so a failed event is silently dropped with no signal at all. A client cannot distinguish "still retrying" from "never scheduled".
Fix: Lambda's asynchronous invocation plane in the lambda crate — the Event-invocation queue/retry driver behind the Invoke handler and the event-invoke configuration it reads (PutFunctionEventInvokeConfig/UpdateFunctionEventInvokeConfig state): re-drive a failing async event up to MaximumRetryAttempts times, count the attempts (approximateInvokeCount) across them, and on exhaustion write the invocation record to the configured OnFailure destination with requestContext.condition = 'RetriesExhausted', authorizing that write against the function's execution role.
Note for the fixer on replaying this one: the check declares its own budget as 900 s (its H2 assumes AWS's ~1 min / ~2 min backoff) while the runner cut it at 180 s, so confirming the fix needs either a fast-settling retry schedule or a longer window — the attempt count, the record and its requestContext fields are the requirement, the inter-attempt delay is not.
Traces: lambda__sqs.async-retry-budget-of-two-yields-exactly-three-attempts.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the emulator; per-service pass rate lambda__sqs 8/33 (24%), with 24 of the 25 failures owned by the two open asynchronous-invocation issues named in the description and this one unowned.

[mismatch] lambda__sqs.async-retry-budget-of-two-yields-exactly-three-attempts
  Error: Check timed out after 180.0s

The check's declared expectations (from its own header):
  H1  MaximumRetryAttempts=2 produces exactly THREE handler entries for one asynchronous invocation (the original plus two retries), and the OnFailure record reports requestContext.approximateInvokeCount == 3.
  H3  Retries reuse the same event, so every attempt's counting message carries the same marker.
  H4  The OnFailure destination is written with the FUNCTION's execution role, so sqs:SendMessage on the destination queue must be granted to that role (as it is here).
  H5  requestContext.condition == 'RetriesExhausted'.
  H6  A quiet window calibrated to the observed span between the first and last attempt must catch a fourth attempt if one exists.
Expected: three counting messages on the attempt queue for one Event invoke, then exactly one OnFailure record with condition 'RetriesExhausted' and approximateInvokeCount 3, then no fourth attempt during the quiet window.
Actual: the poll for attempts never advances within the window — no attempt message and no OnFailure record is observed at all, and the check is cut off. (The 180 s figure is this runner's window, not part of the requirement; the attempt count and the record's requestContext fields are.)

## Steps to Reproduce
Boot the emulator and replay lambda__sqs.async-retry-budget-of-two-yields-exactly-three-attempts from suite simparity-behavioral-aws-v1 (report heading lambda__sqs):
1. Create two SQS queues — one the handler writes a counting message to on every entry, one as the OnFailure destination — and an execution role granted sqs:SendMessage on both.
2. CreateFunction with a handler that writes its counting message (carrying the event's marker) and then raises, so every attempt fails.
3. PutFunctionEventInvokeConfig with MaximumRetryAttempts=2 and DestinationConfig.OnFailure pointing at the destination queue.
4. Invoke the function once with InvocationType='Event' (expect 202).
5. Drain the counting queue: expect exactly THREE messages, all carrying the same event marker (the original invocation plus two retries).
6. Drain the OnFailure queue: expect exactly ONE invocation record whose requestContext.condition == 'RetriesExhausted' and requestContext.approximateInvokeCount == 3.
7. Hold a quiet window and confirm no fourth attempt and no second record arrive. Against the emulator nothing arrives on either queue and the poll in step 5 never satisfies.
