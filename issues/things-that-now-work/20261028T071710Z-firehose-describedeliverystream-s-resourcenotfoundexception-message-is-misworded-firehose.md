Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__firehose__s3.firehose-role-without-trust-policy-rejected firehose__kinesis__s3.cli-firehose-describe-nonexistent-delivery-stream-negative
Issue-Key: events__firehose__s3.firehose-role-without-trust-policy-rejected|run#11|AssertionError|assert not_found_message == expected_not_found, (
Issue-Key: firehose__kinesis__s3.cli-firehose-describe-nonexistent-delivery-stream-negative|run#20|AssertionError|assert message == want_describe, f"expected describe message {want_describe!r}, got {message!r}"
Issue-Commit: 2865277deda94116fb87301a2e2c2eebcbb36c62

# firehose: DescribeDeliveryStream's ResourceNotFoundException message is misworded — "Firehose under accountId <id> not found" instead of "Firehose under account <id> not found." (1 cause)

> _Note: this issue was identified against the code base at commit `2865277deda94116fb87301a2e2c2eebcbb36c62`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDeliveryStream

## Description
Cause 1 of 1: the control-plane not-found sentence Firehose returns for a delivery stream that does not exist is worded wrongly — it says "accountId" where AWS says "account", and it omits the terminating period. Both failing checks reach DescribeDeliveryStream for a delivery-stream name that was never created, get the right error code and the right HTTP status, and fail only on the message text.

AWS: Smithy (services/service-com-amazonaws-firehose-firehose-20150804/smithy-model/com-amazonaws-firehose-firehose-20150804.json): DescribeDeliveryStream takes DeliveryStreamName (@required, @length 1..64, @pattern ^[a-zA-Z0-9_.-]+$) and models exactly one error, ResourceNotFoundException, errorKind client, HTTP 400, documented "The specified resource could not be found." The model does not state the message text, so the message is recorded behavior only: for a name that resolves to no stream, the recording shows error code ResourceNotFoundException at HTTP 400 with the message template

    Firehose under account <accountId> not found.

Its FIXED text is "Firehose under account " … " not found." — including the trailing period — and its single VARIABLE part is the 12-digit AWS account id the lookup was scoped to (taken from the caller's credentials / the emulator's account, not from the request's DeliveryStreamName; the recorded run happens to fill it with the emulator account, which is a value, not part of the template). Note that AWS words the DATA-plane counterpart differently — PutRecord on a missing stream answers "Firehose not found under account <accountId>." — so the two sentences are not interchangeable; the same two checks assert that data-plane sentence and it already matches.

Emulator: the handler resolves the stream, misses, and answers ResourceNotFoundException (correct code, correct 400) with the message "Firehose under accountId <accountId> not found" — the noun is "accountId" instead of "account" and the sentence has no terminating period. Replay output: "expected the not-found message 'Firehose under account 000000000000 not found.', got 'Firehose under accountId 000000000000 not found'" (identical divergence in both checks).

Why: a client that reads or asserts on the service's not-found sentence — a CLI user, a log-scraping or alerting rule, an SDK integration test, or any code that string-matches the message to tell "stream absent" apart from other 400s — sees text real Firehose never emits. Because the code and status are right, the divergence is invisible to code-based branching and only surfaces where the message itself is the signal, which is exactly where it is hardest to debug. It also makes the control-plane sentence collide conceptually with the data-plane one ("Firehose not found under account …"), hiding the fact that AWS deliberately words them differently.

Fix: the shared refusal-wording helper this service uses for control-plane not-found, not the individual operation handlers — every control-plane read/delete/update/encryption path funnels through the one helper, so the one sentence has to be corrected there and all of those paths pick the fix up together. The helper must emit "Firehose under account ", the account id, " not found." The data-plane helper's wording is already correct and must NOT be changed to match.

Traces: events__firehose__s3.firehose-role-without-trust-policy-rejected (the trust-policy half of that check passes; it fails on the effect-absent DescribeDeliveryStream probe), firehose__kinesis__s3.cli-firehose-describe-nonexistent-delivery-stream-negative (same divergence through the aws CLI). Code citations: services/service-com-amazonaws-firehose-firehose-20150804/src/service.rs lines 533-538 (`not_found_message()`), called from services/service-com-amazonaws-firehose-firehose-20150804/src/ops/describe_delivery_stream.rs line 174; the correctly-worded data-plane sibling is `put_not_found_message()` at src/service.rs lines 625-630.

## Evidence
Both checks: expected error code ResourceNotFoundException at HTTP 400 (modeled) — the emulator returns that code and status correctly.

Expected message (AWS template, account id filled from the caller's account): "Firehose under account <accountId> not found."
Actual message from the emulator: "Firehose under accountId <accountId> not found"

Two differences, both in the FIXED text: the noun is "accountId" where AWS writes "account", and the terminating period is missing.

Verbatim replay assertions:
- events__firehose__s3.firehose-role-without-trust-policy-rejected (check line 52): AssertionError: expected the not-found message 'Firehose under account 000000000000 not found.', got 'Firehose under accountId 000000000000 not found'
- firehose__kinesis__s3.cli-firehose-describe-nonexistent-delivery-stream-negative (check line 75): AssertionError: expected describe message 'Firehose under account 000000000000 not found.', got 'Firehose under accountId 000000000000 not found'

(000000000000 is the emulator's account id — the template's variable fill for this run, not a requirement.)

The same two checks assert the DATA-plane sentence for PutRecord on the same missing stream, "Firehose not found under account <accountId>.", and that one matches — confirming the defect is scoped to the control-plane not-found helper only.

Source: services/service-com-amazonaws-firehose-firehose-20150804/src/service.rs:533-538
    pub fn not_found_message() -> String {
        format!("Firehose under accountId {} not found", ::engine::emulator_account_id())
    }

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Call firehose DescribeDeliveryStream with a DeliveryStreamName that was never created (any name satisfying @length 1..64 / @pattern ^[a-zA-Z0-9_.-]+$), e.g.
   aws firehose describe-delivery-stream --delivery-stream-name sim2real-fh-missing-xyz
3. Read Error.Code and Error.Message from the 400 response. Code is ResourceNotFoundException (correct); Message reads "Firehose under accountId <accountId> not found" instead of AWS's "Firehose under account <accountId> not found." — "accountId" for "account", and no trailing period.
4. For contrast, call PutRecord on the same missing stream: its message "Firehose not found under account <accountId>." is already correct, so the fix must touch only the control-plane sentence.
5. Re-replay the two grounding checks: events__firehose__s3.firehose-role-without-trust-policy-rejected and firehose__kinesis__s3.cli-firehose-describe-nonexistent-delivery-stream-negative.
