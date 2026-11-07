Issue-Source: trace-sweep-divergence
Issue-Trace: kinesis-resource-policy-stream-not-active
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: kinesis-resource-policy-stream-not-active
Issue-Shape-Id: com.amazonaws.kinesis#Kinesis_20131202
Issue-Key: kinesis|GetResourcePolicy|4xx->2xx|$.__type
Issue-Key: kinesis|GetResourcePolicy|4xx->2xx|$.message
Issue-Key: kinesis|GetResourcePolicy|4xx->2xx|status
Issue-Commit: 06f0bea8eccdb88f075e8abcb7607fa04392625b

# kinesis GetResourcePolicy answers 200 on a non-ACTIVE stream instead of refusing with ResourceInUseException

> _Note: this issue was identified against the code base at commit `06f0bea8eccdb88f075e8abcb7607fa04392625b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResourcePolicy

## Description
ONE net-new root cause in the kinesis resource-policy read path, replayed from the wire corpus simparity-wire-aws-v1: GetResourcePolicy applies no stream-state precondition and now answers a SUCCESS for a stream that is not ACTIVE.

Related but NOT covering: 20261029T202053Z-kinesis-resource-policy-operations-skip-stream-state-gating.md (open) describes the missing stream-state gate on Put/Get/DeleteResourcePolicy, and its Cause 3 was recorded when GetResourcePolicy still answered HTTP 400 ResourceNotFoundException for this step. The emulator's behavior on this step has since changed shape: it no longer errors at all but answers HTTP 200, so the replay key for this step (kinesis|GetResourcePolicy|status=400->200) is not carried by that issue and the step is unowned. Filing it here so the 400->200 behavior is gated by a replay key. 20261029T200552Z-kinesis-getresourcepolicy-refuses-a-stream-that-has-no.md covers GetResourcePolicy on an ACTIVE stream with NO policy attached (should be 200 with Policy "{}") — a different precondition; here the stream is not ACTIVE at all, which AWS refuses before any policy lookup.

=== Cause 1 of 1: GetResourcePolicy performs no stream-state check, so a stream that exists but is not ACTIVE is answered with a policy document instead of ResourceInUseException ===
AWS: Smithy models GetResourcePolicy input { ResourceARN (required, @length 1..2048, pattern ^arn:aws.*:kinesis:.*:\d{12}:.*(stream|channel)/\S+$), StreamId (@length 1..24, pattern ^[a-z0-9]{20}-[a-z0-9]{3}$) } and output { Policy (required) }, with modeled errors AccessDeniedException, InvalidArgumentException, LimitExceededException, ResourceInUseException and ResourceNotFoundException, all HTTP 400. Smithy ResourceInUseException is documented as "The resource is not available for this operation. For successful operation, the resource must be in the ACTIVE state." Recorded behavior: GetResourcePolicy against a stream that exists but whose StreamStatus is not ACTIVE (nor UPDATING) is REFUSED with HTTP 400, error code ResourceInUseException and message `Stream <stream-name> under account <account-id> not ACTIVE or UPDATING, instead in state <state>`. In that message the FIXED text is `Stream `, ` under account `, ` not ACTIVE or UPDATING, instead in state `; the VARIABLE parts are the stream name parsed out of the request's ResourceARN, the caller's 12-digit account id, and the stream's current status read from service state. The state gate runs before any policy lookup, so no policy document is returned in this window.
Emulator: the call SUCCEEDS — HTTP 200, and the response body carries neither __type nor message (a policy-shaped success instead of the refusal). The stream-state precondition is absent from the handler entirely, so a stream still in a transitional state is served as if it were ACTIVE.
Why: a client relying on AWS's behavior uses the refusal as a readiness signal — retry GetResourcePolicy until the stream leaves the transitional state, then read the policy. Against the emulator that loop never runs: the client accepts the first answer as authoritative, concludes the resource's policy is whatever was served in a window AWS does not serve, and then performs read-modify-write against a document AWS never handed out. Code paths that distinguish "not ready, retry" (ResourceInUseException) from "ready, here is the policy" (200) cannot be exercised against the emulator at all, and the emulator's view of the resource diverges from AWS's from that point on.
Fix: GetResourcePolicy's handler in the kinesis service crate must resolve the ResourceARN to the stream and gate on its StreamStatus BEFORE touching the policy store, raising ResourceInUseException (HTTP 400) with the AWS message template above whenever that status is neither ACTIVE nor UPDATING.
Traces: kinesis-resource-policy-stream-not-active#task#2.

## Evidence
Replayed from the wire corpus simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, 129/154 steps passed, score 83.8%). The stream name and account id below are per-run values that fill the message template; the requirements are the HTTP status, the error code and the fixed text of the message.

Cause 1 — GetResourcePolicy on a stream in state CREATING, fixture kinesis-resource-policy-stream-not-active, step task#2:
  expected: HTTP 400, body { "__type": "ResourceInUseException", "message": "Stream <stream-name> under account <account-id> not ACTIVE or UPDATING, instead in state CREATING" }
  actual:   HTTP 200 (replay verdict mismatch: `status: expected 400, got 200`; `$.__type: missing (expected 'ResourceInUseException', got None)`; `$.message: missing (expected 'Stream ... not ACTIVE or UPDATING, instead in state CREATING', got None)`)
  divergence key: service=kinesis|GetResourcePolicy|status=400->200|keys=$.__type,$.message,status

Smithy citations used: GetResourcePolicy input { ResourceARN required @length 1..2048 pattern ^arn:aws.*:kinesis:.*:\d{12}:.*(stream|channel)/\S+$, StreamId @length 1..24 }, output { Policy required }; modeled errors AccessDeniedException, InvalidArgumentException, LimitExceededException, ResourceInUseException, ResourceNotFoundException — all client errors with HTTP 400; ResourceInUseException doc: "The resource is not available for this operation. For successful operation, the resource must be in the ACTIVE state."

## Steps to Reproduce
1. kinesis CreateStream(StreamName=<n>, ShardCount=1) and do NOT wait for it to become ACTIVE — act while DescribeStreamSummary still reports StreamStatus == CREATING.
2. Build the stream ARN for <n> (arn:aws:kinesis:<region>:<account-id>:stream/<n>).
3. GetResourcePolicy(ResourceARN=<that ARN>) -> observe HTTP 200 with a policy-shaped body instead of HTTP 400 with { "__type": "ResourceInUseException", "message": "Stream <n> under account <account-id> not ACTIVE or UPDATING, instead in state CREATING" }.
4. Replay to confirm the fix: replay_wire_corpus and check that fixture kinesis-resource-policy-stream-not-active step task#2 passes (and that the fixture reports no divergent steps).
