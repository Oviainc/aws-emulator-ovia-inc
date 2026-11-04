Issue-Source: trace-sweep-divergence
Issue-Trace: sqs-refarch-s3ev-queue-lookup-mistakes
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sqs-refarch-s3ev-queue-lookup-mistakes
Issue-Shape-Id: com.amazonaws.sqs#AmazonSQS
Issue-Key: sqs|GetQueueAttributes|2xx->4xx|$.Attributes
Issue-Key: sqs|GetQueueAttributes|2xx->4xx|status
Issue-Commit: 7cd635da05c853b3aee981acc4bc8b13c0b954d8

# SQS GetQueueAttributes refuses an existing queue with 400 instead of returning its QueueArn

> _Note: this issue was identified against the code base at commit `7cd635da05c853b3aee981acc4bc8b13c0b954d8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetQueueAttributes

## Description
This issue carries 1 cause in SQS's GetQueueAttributes queue resolution. It must be closed before submitting.

Cause 1 of 1 — GetQueueAttributes answers HTTP 400 for a queue that exists, instead of HTTP 200 with that queue's QueueArn.

AWS: GetQueueAttributes takes a required QueueUrl (Smithy: GetQueueAttributesRequest.QueueUrl, String, required — "The URL of the Amazon SQS queue whose attribute information is retrieved. Queue URLs and names are case-sensitive.") and an optional AttributeNames list of com.amazonaws.sqs#QueueAttributeName, which enumerates QueueArn. For a request that names a live queue and selects QueueArn, AWS answers HTTP 200 with Attributes containing exactly the selected name: Attributes.QueueArn = the queue's ARN, i.e. the fixed text 'arn:aws:sqs:' + region + ':' + account id + ':' + QueueName, whose region/account/name parts come from the queue's own identity (recorded behavior for the ARN composition; the recorded run's region, account and generated queue name are per-run values and are not part of the requirement). AWS only refuses the call when the address names no queue — QueueDoesNotExist (Smithy: awsQueryError code 'AWS.SimpleQueueService.NonExistentQueue', httpResponseCode 400, message 'The specified queue does not exist.', fixed text with no variable part) — or when the selected attribute name is not a QueueAttributeName (InvalidAttributeName, httpError 400, doc "The specified attribute doesn't exist."). Neither applies here: the recorded response is a 200 whose body carries the queue's ARN, so AWS resolved the address this step supplied.

Emulator: answers HTTP 400 with no Attributes member at all (replay: 'status: expected 200, got 400' and '$.Attributes: missing'). The queue was created and addressed earlier in the same fixture, so the emulator has the record and still fails to resolve the address this step hands it.

Hypothesis for the mechanism (the recording states the outcome, not the cause): GetQueueAttributes resolves its input only through crate::state::queue_by_url -> queue_name_from_url, which requires the value to contain at least two '/'-separated path segments whose second-to-last segment equals the emulator's own account id, and otherwise returns None, which the handler turns straight into the QueueDoesNotExist refusal. Any address form AWS resolves but that does not have that exact '<...>/<accountId>/<QueueName>' shape — a bare QueueName, an address whose account segment is written differently, or any other abbreviated form the probe uses when it re-looks-up the queue — therefore reads as a nonexistent queue here even though the queue is present in the store. The fixture is named for exactly that: it makes queue-lookup mistakes AWS tolerates.

Why: this is the step an S3-event wiring workflow uses to learn the queue's ARN before it is put into a bucket notification or a queue policy. A client relying on AWS's behavior gets a 400 refusal (and, from the error class, concludes the queue does not exist) where AWS hands back the ARN, so the whole downstream wiring never happens and the failure is attributed to a missing queue rather than to an address form. Conversely, any test asserting that a bogus address IS refused can pass here for the wrong reason.

Fix: the queue-address resolution the GetQueueAttributes handler calls — crate::state::queue_by_url / queue_name_from_url in the SQS state, and with it the handler's decision to raise QueueDoesNotExist — must accept every address form AWS resolves to a live queue of this account, and raise QueueDoesNotExist only when no queue of that name exists in the request's region.

Traces: sqs-refarch-s3ev-queue-lookup-mistakes#task#4.

## Evidence
Replay of trace set simparity-wire-aws-v1 (270 steps, 20 mismatched, score 92.6%). This cluster (affected_count 1) is the only divergence in the run not already owned by an open issue.

sqs-refarch-s3ev-queue-lookup-mistakes#task#4, GetQueueAttributes, divergence key 'service=sqs|GetQueueAttributes|status=200->400|keys=$.Attributes,status':
  status: expected 200, got 400
  $.Attributes: missing (expected {'QueueArn': 'arn:aws:sqs:us-west-2:000000000000:sim2real-probe-s3ev-q-f3e7f6b0'}, got None)

The recorded ARN's region ('us-west-2'), account ('000000000000') and queue name ('sim2real-probe-s3ev-q-f3e7f6b0') are per-run values filling the ARN template; the requirement is a 200 carrying the addressed queue's own ARN under Attributes.QueueArn. The emulator's 400 body is not asserted by the fixture, so the error code it carries is not recorded; the reachable 400 refusals for this operation are QueueDoesNotExist ('AWS.SimpleQueueService.NonExistentQueue' / 'The specified queue does not exist.') and InvalidAttributeName ('Unknown Attribute <name>.'), and neither is a response AWS produces for this request.

Source read while triaging (citation only, not a patch): services/service-com-amazonaws-sqs-amazonsqs/src/ops/get_queue_attributes.rs lines 183-199 (region + queue_by_url lookup, else QueueDoesNotExist) and services/service-com-amazonaws-sqs-amazonsqs/src/state.rs lines 2219-2227 and 3904-3914 (queue_by_url / queue_name_from_url: last path segment plus an exact account-segment match, None otherwise).

## Steps to Reproduce
1. Replay fixture sqs-refarch-s3ev-queue-lookup-mistakes against the emulator and stop at step task#4.
2. The fixture creates the queue earlier in the same run, then at task#4 calls GetQueueAttributes with AttributeNames=['QueueArn'] for that queue, using the address form the recorded step supplies.
3. Observed: HTTP 400 with no Attributes member. Expected: HTTP 200 whose body is Attributes = {'QueueArn': '<the queue's ARN>'}.
4. Equivalent minimal check: CreateQueue a queue, then call GetQueueAttributes(AttributeNames=['QueueArn']) addressing that same queue by each address form AWS resolves (the full canonical queue URL, and the abbreviated forms a client may hand back). Every one of them must answer 200 with the queue's ARN; only a name no queue holds may answer 400 QueueDoesNotExist / 'The specified queue does not exist.'
5. Re-replay sqs-refarch-s3ev-queue-lookup-mistakes#task#4 — it must pass.
