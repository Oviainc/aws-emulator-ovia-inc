Issue-Source: trace-sweep-divergence
Issue-Trace: batch-scheduling-policy-standalone-lifecycle
Issue-Step: task#9
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: batch-scheduling-policy-standalone-lifecycle
Issue-Shape-Id: com.amazonaws.batch#AWSBatchV20160810
Issue-Key: batch|DescribeSchedulingPolicies|4xx->4xx|$.message
Issue-Commit: e4b73bef21735acad04abca14d336dea5ddc072f

# batch DescribeSchedulingPolicies malformed-ARN refusal message is emulator-authored and wrapped in a per-call envelope

> _Note: this issue was identified against the code base at commit `e4b73bef21735acad04abca14d336dea5ddc072f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeSchedulingPolicies

## Description
One net-new root cause in the AWS Batch scheduling-policy plane. It is the only divergence in the replay report for this service that is neither blocked by an unimplemented operation nor OWNED by an open issue (the report's owned_by is empty for it).

Cause 1 of 1 — DescribeSchedulingPolicies refuses a scheduling-policy ARN that does not match the scheduling-policy ARN pattern with a message of the emulator's own construction, wrapped in the "Error executing request, Exception : … , RequestId: <uuid>" envelope.

AWS: Smithy: DescribeSchedulingPolicies (POST /v1/describeschedulingpolicies) takes a required arns -> StringList and declares exactly two errors, ClientException (httpError 400) and ServerException (httpError 500). The model carries no pattern trait on the list member and states no message text, so the refusal wording is recorded behavior only. The recording shows HTTP 400 with x-amzn-errortype ClientException — which the emulator does match — and a message asserted by shape rather than by literal value ('<shaped-not-asserted>' in the replay detail), i.e. a bare refusal sentence naming the offending ARN. For this refusal class AWS emits the sentence alone: its fixed text is the refusal wording and its only variable part is the ARN taken verbatim from the request's arns entry. AWS does NOT prefix it with "Error executing request, Exception : " and does NOT append ", RequestId: <id>"; that envelope belongs to other Batch refusal classes (the create/validation classes), and the already-open issue 20261029T173319Z-batch-8-net-new-root-causes-on-the.md establishes exactly that split for DescribeQuotaShare/UpdateQuotaShare (its Cause 8: the bare sentence is correct, the envelope is not). That issue covers the quota-share operations only and does not cover DescribeSchedulingPolicies, so this cause is net-new.

Emulator: answers HTTP 400 ClientException with the message 'Error executing request, Exception : Scheduling policy arn should match a valid Scheduling policy arn pattern, RequestId: <uuid>'. Three things diverge, all of them FIXED text rather than per-run values: the "Error executing request, Exception : " prefix, the trailing ", RequestId: " suffix that embeds the call's own request id INSIDE the message body, and the inner sentence itself, which is generic prose that never names the ARN that was refused. (The uuid in that suffix is a per-run value and is not itself the finding; the suffix's presence is.)

Why: a client relying on AWS's behavior cannot recover the offending ARN from the refusal — the emulator's sentence omits it entirely — so a batch of arns that is refused gives the caller no way to tell WHICH entry was malformed, and per-entry retry or repair logic has nothing to key on. Exact-message matching (and the common practice of using the envelope's presence to tell a validation refusal from a resource refusal) fails, and any message the client logs or re-raises carries a request id in a position AWS never puts one, so log scrapers and error-equality assertions both break.

Fix: the DescribeSchedulingPolicies handler's arns validation and its error-rendering path in the batch service crate — the ARN-pattern refusal must raise its own ClientException with a bare sentence naming the submitted ARN, and the "Error executing request … RequestId" envelope must be applied per refusal class instead of globally wrapping every ClientException.

Traces: batch-scheduling-policy-standalone-lifecycle#task#9.

## Evidence
Replay cluster divergence_key=service=batch|DescribeSchedulingPolicies|status=400->400|keys=$.message, verdict=mismatch, affected_count=1.
Status: expected 400, got 400 (matches). x-amzn-errortype: ClientException (matches).
$.message: value_mismatch — expected '<shaped-not-asserted>' (the recording asserts the message by shape: a bare refusal sentence whose only variable part is the submitted ARN); got 'Error executing request, Exception : Scheduling policy arn should match a valid Scheduling policy arn pattern, RequestId: d0ee1c14-9e22-4e44-bb30-4332d7919363'.
Diverging FIXED text in the actual message: the prefix 'Error executing request, Exception : ', the suffix ', RequestId: ', and the inner sentence 'Scheduling policy arn should match a valid Scheduling policy arn pattern' which contains no slot for the request's ARN. The uuid 'd0ee1c14-9e22-4e44-bb30-4332d7919363' is a per-run value filling that suffix and is not part of the requirement.
Smithy (services/service-com-amazonaws-batch-awsbatchv20160810): DescribeSchedulingPolicies input arns -> StringList (required), errors ClientException(400)/ServerException(500), http POST /v1/describeschedulingpolicies, success code 200.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus).
2. Replay fixture batch-scheduling-policy-standalone-lifecycle and stop at step task#9: it calls DescribeSchedulingPolicies with an arns entry that is not a valid scheduling-policy ARN, after the fixture's earlier steps have created and described a real scheduling policy.
3. Read the HTTP status (400, correct), the x-amzn-errortype header (ClientException, correct) and the JSON body's $.message.
4. Observed: $.message == 'Error executing request, Exception : Scheduling policy arn should match a valid Scheduling policy arn pattern, RequestId: <uuid>'.
5. Expected: a bare refusal sentence with no 'Error executing request, Exception : ' prefix and no ', RequestId: <uuid>' suffix, naming the ARN supplied in the request's arns entry.
