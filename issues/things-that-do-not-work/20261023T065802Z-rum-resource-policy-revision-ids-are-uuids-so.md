Issue-Source: trace-sweep-divergence
Issue-Trace: rum-multi-appmonitor-policy-isolation
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rum-multi-appmonitor-policy-isolation rum-resource-policy-lifecycle rum-resource-policy-put-get-update-delete rum-resource-policy-revision-conflict rum-resource-policy-bad-revision-id
Issue-Shape-Id: com.amazonaws.rum#RUM
Issue-Key: rum|PutResourcePolicy|2xx->2xx|$.PolicyDocument
Issue-Key: rum|PutResourcePolicy|2xx->2xx|$.PolicyRevisionId
Issue-Key: rum|GetResourcePolicy|2xx->2xx|$.PolicyDocument
Issue-Key: rum|GetResourcePolicy|2xx->2xx|$.PolicyRevisionId
Issue-Key: rum|GetResourcePolicy|2xx->2xx|$headers.content-type
Issue-Key: rum|DeleteResourcePolicy|2xx->4xx|$.PolicyRevisionId
Issue-Key: rum|DeleteResourcePolicy|2xx->4xx|$headers.content-type
Issue-Key: rum|DeleteResourcePolicy|2xx->4xx|status
Issue-Key: rum|GetResourcePolicy|4xx->2xx|$
Issue-Key: rum|GetResourcePolicy|4xx->2xx|$.message
Issue-Key: rum|GetResourcePolicy|4xx->2xx|$headers.content-type
Issue-Key: rum|GetResourcePolicy|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: rum|GetResourcePolicy|4xx->2xx|status
Issue-Key: rum|PutResourcePolicy|2xx->4xx|$.PolicyDocument
Issue-Key: rum|PutResourcePolicy|2xx->4xx|$.PolicyRevisionId
Issue-Key: rum|PutResourcePolicy|2xx->4xx|status
Issue-Key: rum|DeleteResourcePolicy|2xx->2xx|$.PolicyRevisionId
Issue-Key: rum|DeleteResourcePolicy|2xx->2xx|$headers.content-type
Issue-Commit: c036e6d6272f3d2084b4ffba5fd23a45ba05e284

# RUM resource-policy revision IDs are UUIDs so revision-gated writes fail

> _Note: this issue was identified against the code base at commit `c036e6d6272f3d2084b4ffba5fd23a45ba05e284`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutResourcePolicy

## Description
Seven clusters, seven causes, all in service rum's resource-policy surface. A fixer must close ALL SEVEN before submitting. (Most of these clusters also report $headers.content-type, filed as its own service-wide issue.)

=== Cause 1 of 7: PutResourcePolicy returns a UUID PolicyRevisionId instead of an incrementing counter ===
AWS: Smithy: PutResourcePolicy returns PolicyDocument + PolicyRevisionId (shape PolicyRevisionId, string @length 1..255), and models InvalidPolicyRevisionIdException (400) for a revision that 'doesn't match the latest policy revision ID'. Recorded behavior: the first policy written to an app monitor comes back with PolicyRevisionId '1', and the next successful write on the same monitor returns '2' — RUM's revision id is a small monotonically increasing integer rendered as a string, which is what makes an optimistic-concurrency round trip (read a revision, send it back) meaningful.
Emulator: returns an opaque UUID (e.g. '244bd48d-ffb9-4098-a308-16e36bf1c2f9').
Why: it is not the value shape clients persist/compare, it carries no ordering, and it is the root of causes 3 and 5: a caller that echoes back the revision it just received is rejected. Note the requirement is the counter rule ('1' for the first revision, incremented per successful write), not any recorded literal.
Fix: the resource-policy state for an app monitor (the revision-id generator) plus the PutResourcePolicy handler that stamps it.
Traces: rum-multi-appmonitor-policy-isolation#task#3 (cluster also covers rum-resource-policy-lifecycle#task#2, rum-resource-policy-put-get-update-delete#task#3, rum-resource-policy-revision-conflict#task#2).

=== Cause 2 of 7: GetResourcePolicy echoes the same non-conforming revision id ===
AWS: Smithy: GetResourcePolicy returns PolicyDocument + PolicyRevisionId. Recorded: the revision id read back equals the integer-string the write returned ('1').
Emulator: returns the UUID from cause 1.
Why: a read-modify-write cycle cannot produce a revision AWS would accept, and tooling that stores the revision sees a different value shape.
Fix: follows cause 1 (the shared resource-policy record), but verify the GetResourcePolicy projection returns the stored revision verbatim.
Traces: rum-multi-appmonitor-policy-isolation#task#5 (cluster also covers rum-resource-policy-lifecycle#task#3/#5, rum-resource-policy-put-get-update-delete#task#4/#6, rum-resource-policy-revision-conflict#task#5).

=== Cause 3 of 7: DeleteResourcePolicy with the current revision id is rejected with HTTP 400 ===
AWS: Smithy: DeleteResourcePolicy, DELETE /appmonitor/{Name}/policy with PolicyRevisionId bound to @httpQuery 'policyRevisionId'; success returns 200 with PolicyRevisionId; InvalidPolicyRevisionIdException (400) is for a MISMATCHED revision. Recorded: deleting with the policy's current revision succeeds with 200 and returns that revision.
Emulator: answers HTTP 400 and no PolicyRevisionId — i.e. it treats the caller's correct revision as a mismatch.
Why: the documented optimistic-concurrency delete is impossible; a correct client cannot remove a resource policy at all, and it sees a 400 for a request AWS accepts.
Fix: the DeleteResourcePolicy handler's revision comparison (compare against the stored revision produced by cause 1's generator) before deleting.
Traces: rum-multi-appmonitor-policy-isolation#task#7 (cluster also covers rum-multi-appmonitor-policy-isolation#task#8, rum-resource-policy-lifecycle#task#6, rum-resource-policy-put-get-update-delete#task#7).

=== Cause 4 of 7: after DeleteResourcePolicy the policy is still readable — GetResourcePolicy returns 200 instead of 404 ===
AWS: Smithy: GetResourcePolicy models PolicyNotFoundException (httpError 404), documented 'the resource-based policy doesn't exist on this app monitor'. Recorded: once the policy is deleted, GetResourcePolicy answers 404; the fixture polls and AWS converges to 404.
Emulator: keeps answering 200 with the full PolicyDocument and PolicyRevisionId for 15 polling attempts.
Why: the delete is not reflected in state, so a client cannot observe policy removal and a torn-down monitor keeps granting whatever its policy granted. This is a state-mutation defect, distinct from cause 3 (which is the request-gating defect) — the cluster shows deletes that DO return success still leaving the policy readable.
Fix: the DeleteResourcePolicy handler's state mutation — remove the policy from the app-monitor's resource-policy state so GetResourcePolicy raises PolicyNotFoundException/404.
Traces: rum-multi-appmonitor-policy-isolation#task#9 (cluster also covers rum-multi-appmonitor-policy-isolation#task#10).

=== Cause 5 of 7: PutResourcePolicy updating an existing policy with its current revision id is rejected with HTTP 400 ===
AWS: Smithy: PutResourcePolicy input PolicyRevisionId (optional) + InvalidPolicyRevisionIdException (400) for a mismatch. Recorded: replacing a policy while passing the current revision succeeds with 200 and returns the NEXT revision ('2') with the new PolicyDocument.
Emulator: answers HTTP 400, returning neither PolicyDocument nor PolicyRevisionId; the policy is not updated.
Why: policy updates are impossible for a correct client; a caller doing the documented read-revision-then-write loop gets a 400 for a request AWS accepts.
Fix: the PutResourcePolicy handler's revision check and the subsequent revision increment.
Traces: rum-resource-policy-lifecycle#task#4 (cluster also covers rum-resource-policy-put-get-update-delete#task#5).

=== Cause 6 of 7: a successful DeleteResourcePolicy returns a UUID revision id ===
AWS: Smithy: DeleteResourcePolicyResponse carries PolicyRevisionId. Recorded: the successful delete returns the revision that was deleted, as the integer-string ('1').
Emulator: returns a UUID (e.g. 'a6a76c8a-ba18-4d5e-8cac-1b54f7b844ab').
Why: same value-shape divergence as cause 1 on the delete response path; callers that log/compare the deleted revision see an unrelated format.
Fix: the DeleteResourcePolicy success response projection (shares the record from cause 1).
Traces: rum-resource-policy-revision-conflict#task#6.

=== Cause 7 of 7: PolicyDocument is re-serialized instead of returned verbatim ===
AWS: Smithy: PutResourcePolicy/GetResourcePolicy PolicyDocument is a plain String member. Recorded: RUM stores and returns the document as the exact string the caller submitted — the recorded responses carry the caller's compact JSON ('{"Version":"2012-10-17","Statement":[...]}' with no spaces after ':' or ',').
Emulator: returns a re-serialized document with whitespace inserted ('{"Version": "2012-10-17", "Statement": [...]}'), i.e. it parses the document and re-prints it.
Why: PolicyDocument is an opaque string to clients; anything that hashes, diffs or signature-compares the policy it submitted against the policy read back (drift detection, idempotency checks) reports a change that did not happen. Round-tripping through a JSON printer also risks reordering/normalizing content the caller controls.
Fix: the resource-policy state and the Put/GetResourcePolicy serializers — persist and return the submitted PolicyDocument string byte-for-byte; validate by parsing a copy.
Traces: rum-resource-policy-bad-revision-id#task#5 (GetResourcePolicy; the same key also appears in the cause 1 and cause 2 clusters).

## Evidence
Cause 1+7 — service=rum|PutResourcePolicy|status=200->200 (5 steps, rep rum-multi-appmonitor-policy-isolation#task#3):
  $.PolicyRevisionId: expected '1', got '244bd48d-ffb9-4098-a308-16e36bf1c2f9'
  $.PolicyDocument: expected '{"Version":"2012-10-17","Statement":[{"Sid":"AllowPutRumEvents",...}]}', got '{"Version": "2012-10-17", "Statement": [{"Sid": "AllowPutRumEvents", ...}]}'

Cause 2 — service=rum|GetResourcePolicy|status=200->200 (7 steps, rep rum-multi-appmonitor-policy-isolation#task#5): same two keys.

Cause 3 — service=rum|DeleteResourcePolicy|status=200->400 (4 steps, rep rum-multi-appmonitor-policy-isolation#task#7):
  status: expected 200, got 400
  $.PolicyRevisionId: missing (expected '1', got None)

Cause 4 — service=rum|GetResourcePolicy|status=404->200 (2 steps, rep rum-multi-appmonitor-policy-isolation#task#9):
  status: expected 404, got 200; $.message missing (expected 'Resource policy does not exist'); $headers.x-amzn-errortype missing
  poll: status 200 not in [404] after 15 attempt(s) — body still {'PolicyDocument': '{...}', 'PolicyRevisionId': '244bd48d-...'}

Cause 5 — service=rum|PutResourcePolicy|status=200->400 (2 steps, rep rum-resource-policy-lifecycle#task#4):
  status: expected 200, got 400; $.PolicyDocument missing (expected the updated 'AllowRumPutUpdated' document); $.PolicyRevisionId missing (expected '2')

Cause 6 — service=rum|DeleteResourcePolicy|status=200->200 (rum-resource-policy-revision-conflict#task#6):
  $.PolicyRevisionId: expected '1', got 'a6a76c8a-ba18-4d5e-8cac-1b54f7b844ab'

Cause 7 — service=rum|GetResourcePolicy|status=200->200 (rum-resource-policy-bad-revision-id#task#5): PolicyDocument differs only by the whitespace the emulator's re-serialization inserts.

## Steps to Reproduce
Boot the emulator and point a botocore rum client at it. Create an app monitor 'probe' and let P1 = '{"Version":"2012-10-17","Statement":[{"Sid":"AllowRumPut","Effect":"Allow","Principal":{"AWS":"arn:aws:iam::000000000000:root"},"Action":"rum:PutRumEvents","Resource":"arn:aws:rum:us-west-2:000000000000:appmonitor/probe"}]}'.
1) Cause 1/7: rum.put_resource_policy(Name='probe', PolicyDocument=P1) -> PolicyRevisionId is a UUID (expected '1') and PolicyDocument comes back with inserted whitespace instead of P1 verbatim.
2) Cause 2: rum.get_resource_policy(Name='probe') -> same UUID revision and re-serialized document.
3) Cause 5: rum.put_resource_policy(Name='probe', PolicyDocument=<P1 with Sid 'AllowRumPutUpdated'>, PolicyRevisionId=<revision just read>) -> HTTP 400; AWS returns 200 with PolicyRevisionId '2'.
4) Cause 3/6: rum.delete_resource_policy(Name='probe', PolicyRevisionId=<current revision>) -> HTTP 400; AWS returns 200 with the deleted revision. Omitting the revision returns 200 but with a UUID revision.
5) Cause 4: after a delete that reported success, rum.get_resource_policy(Name='probe') -> HTTP 200 with the policy still present; AWS returns PolicyNotFoundException/404.
Or replay the fixtures/steps listed in trace_ids.
