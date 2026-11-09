Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail__kms__sqs.data-key-reuse-period-bounds-the-generate-data-key-calls-a-send-burst-makes
Issue-Key: cloudtrail__kms__sqs.data-key-reuse-period-bounds-the-generate-data-key-calls-a-send-burst-makes|run#18|AssertionError|raise AssertionError(
Issue-Commit: 9cecc173ac9c5f86564428d1030fb1c1f0593211

# cloudtrail__kms__sqs: SQS SSE-KMS sends never call KMS GenerateDataKey and no sqs.amazonaws.com CreateQueue management event is ever recorded, so the data-key reuse period is unobservable (2 causes)

> _Note: this issue was identified against the code base at commit `9cecc173ac9c5f86564428d1030fb1c1f0593211`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GenerateDataKey

## Description
Two net-new root causes, both grounded in the single failing check cloudtrail__kms__sqs.data-key-reuse-period-bounds-the-generate-data-key-calls-a-send-burst-makes. Neither is covered by an existing open issue: 20261024T114720Z-cloudtrail-event-history-is-empty-for-everything-but.md and 20261029T173328Z-cloudtrail-records-and-delivers-no-audit-record-for.md describe CloudTrail's empty event history for other service principals and other identity surfaces, and 20261107T185637Z-kms-s3-sse-kms-is-a-stored-header-not-envelope.md / 20261029T175759Z-kms-s3-s3-never-calls-kms-for-a.md describe the same stored-header shortcut in S3, not in SQS; none of them names SQS's send path, its KmsDataKeyReusePeriodSeconds cache, or the sqs.amazonaws.com management-event stream. Checks: cloudtrail__kms__sqs.data-key-reuse-period-bounds-the-generate-data-key-calls-a-send-burst-makes.

Cause 1 of 2: SendMessage on an SSE-KMS queue performs no envelope encryption, so KMS is never called and the data-key reuse period has no effect.
AWS: a queue configured with KmsMasterKeyId (a customer CMK) is server-side encrypted with envelope encryption. Smithy (SQS CreateQueue.Attributes, member doc): "KmsDataKeyReusePeriodSeconds - The length of time, in seconds, for which Amazon SQS can reuse a data key to encrypt or decrypt messages before calling KMS again. An integer representing seconds, between 60 seconds (1 minute) and 86,400 seconds (24 hours). Default: 300 (5 minutes). A shorter time period provides better security but results in more calls to KMS". So SQS calls kms:GenerateDataKey (Smithy KMS GenerateDataKey: input KeyId required, EncryptionContext map; errors DisabledException/KMSInvalidStateException/NotFoundException) on the first send of each reuse window and reuses the cached data key for sends inside that window. Per the SQS Developer Guide's server-side-encryption page, each such call carries an encryption context naming the queue at the key `aws:sqs:arn` (value = the queue's ARN, taken from the request's queue), and because SQS makes the call on the caller's behalf the resulting KMS record is attributed to the CALLER's identity with userIdentity.invokedBy = "sqs.amazonaws.com". The documented consequence is one-sided and count-based: six sends inside one 3600 s window produce exactly one GenerateDataKey for that queue, while six sends spanning a 60 s window boundary produce two or more.
Emulator: the queue accepts KmsMasterKeyId and round-trips KmsDataKeyReusePeriodSeconds, and every SendMessage returns a MessageId, but no KMS call is made at all. LookupEvents(EventName=GenerateDataKey) yields zero records over the whole 255 s indexing budget: "actual 0 for the long queue and 0 for the short one, out of 0 GenerateDataKey records examined". There is therefore no encryption context, no key-state dependency and no reuse-window behaviour; SSE-KMS is stored configuration only.
Why: a client that proves or audits encryption-at-rest through KMS sees nothing. Key-state and key-policy failures that must refuse a send (KMS DisabledException / KMSInvalidStateException / AccessDenied via the key policy) never occur, so an SSE-KMS queue appears healthy with a revoked or disabled CMK; cost/security tuning via KmsDataKeyReusePeriodSeconds is inert; and any consumer joining a KMS record to a queue through encryptionContext["aws:sqs:arn"] finds no record to join.
Fix: the SQS SendMessage/ReceiveMessage data path (its encryption component) must obtain a data key from the KMS GenerateDataKey handler, keyed by queue ARN with an encryption context of {"aws:sqs:arn": <queue ARN>}, and must cache it for exactly KmsDataKeyReusePeriodSeconds before calling KMS again; the call must be recorded as a kms.amazonaws.com management event attributed to the caller with invokedBy "sqs.amazonaws.com".
Traces: cloudtrail__kms__sqs.data-key-reuse-period-bounds-the-generate-data-key-calls-a-send-burst-makes (assertion at line 273 of the check).

Cause 2 of 2: sqs.amazonaws.com management events are not recorded in CloudTrail event history — a successful CreateQueue is never returned by LookupEvents.
AWS: SQS control-plane calls are management events recorded by CloudTrail and visible through LookupEvents with no trail required. A CreateQueue made by the caller yields a record whose eventSource is sqs.amazonaws.com, whose eventName is CreateQueue, whose requestParameters name the created queue (its QueueName/queueName as sent) and whose userIdentity is the real calling principal (recorded behavior only for the exact field layout; the model has no CloudTrail-record shape).
Emulator: after two CreateQueue calls that both returned a QueueUrl, LookupEvents(LookupAttributes=[{AttributeKey: EventName, AttributeValue: CreateQueue}]) returns no record naming either queue, stably across the full poll: "CreateQueue record missing". The check uses this record as the identity control for the KMS records, so the absence also removes the only way to compare an on-behalf-of identity against a direct call.
Why: a client auditing queue creation (compliance, change tracking, EventBridge rules on "AWS API Call via CloudTrail" for SQS) gets an empty event history for a call that demonstrably succeeded, and cannot correlate the SDK request id of a successful CreateQueue with any audit record.
Fix: the SQS control-plane handlers must emit management events into the CloudTrail event-history store (the same recording hook CloudTrail's LookupEvents reads), with eventSource sqs.amazonaws.com and the request parameters actually received.
Traces: cloudtrail__kms__sqs.data-key-reuse-period-bounds-the-generate-data-key-calls-a-send-burst-makes.

## Evidence
Failing check (mismatch): cloudtrail__kms__sqs.data-key-reuse-period-bounds-the-generate-data-key-calls-a-send-burst-makes

Emulator output:
  File "check_cloudtrail__kms__sqs_data_key_reuse_period_bounds_the_generate_data_key_calls_a_send_burst_makes.py", line 273, in run
    raise AssertionError(
  AssertionError: DATAPLANE_UNIMPLEMENTED: GenerateDataKey records for both queues, or the long queue's own CreateQueue record, were not indexed and stable within 255s; expected >=1 GenerateDataKey each and 1 CreateQueue, stable across 3 consecutive reads, actual 0 for the long queue and 0 for the short one, out of 0 GenerateDataKey records examined, CreateQueue record missing.

Expected (AWS):
  - LookupEvents(EventName=GenerateDataKey) returns >=1 record per encrypted queue; for the 3600 s-reuse queue exactly 1 across six contiguous sends, for the 60 s-reuse queue >=2 across six sends spanning the window boundary.
  - Each such record: eventSource kms.amazonaws.com, requestParameters.encryptionContext["aws:sqs:arn"] == the queue's ARN, userIdentity = the caller with invokedBy == "sqs.amazonaws.com".
  - LookupEvents(EventName=CreateQueue) returns the caller's own record whose requestParameters name the created queue.

Actual (emulator):
  - 0 GenerateDataKey records exist (no KMS call is made by SendMessage at all), so no encryption context and no reuse-window effect.
  - 0 CreateQueue records exist, although both CreateQueue calls returned a QueueUrl and GetQueueAttributes round-tripped KmsMasterKeyId and KmsDataKeyReusePeriodSeconds.

Model citations:
  - Smithy SQS CreateQueue.Attributes doc: KmsDataKeyReusePeriodSeconds, 60..86400, default 300, "before calling KMS again"; "A shorter time period ... results in more calls to KMS".
  - Smithy KMS GenerateDataKey: input {KeyId (required), EncryptionContext: map, ...}; errors DisabledException (409), KMSInvalidStateException (409), NotFoundException (404).

## Steps to Reproduce
1. kms:CreateKey (symmetric, ENCRYPT_DECRYPT) and PutKeyPolicy allowing kms:GenerateDataKey/Decrypt/DescribeKey to the sqs.amazonaws.com service principal.
2. sqs:CreateQueue twice against the SAME CMK: queue A with Attributes {KmsMasterKeyId: <key id>, KmsDataKeyReusePeriodSeconds: "3600"}, queue B with {KmsMasterKeyId: <key id>, KmsDataKeyReusePeriodSeconds: "60"}. Both return a QueueUrl; GetQueueAttributes round-trips the reuse period.
3. SendMessage 6 times to queue A back to back (one 3600 s window). SendMessage 3 times to queue B, wait ~150 s (past the 60 s minimum), SendMessage 3 more times to queue B. Every send returns a MessageId.
4. cloudtrail:LookupEvents with LookupAttributes=[{AttributeKey: "EventName", AttributeValue: "GenerateDataKey"}], paging all pages.
   Observed: zero events. Expected: >=1 per queue, selected by requestParameters.encryptionContext["aws:sqs:arn"] == that queue's ARN; exactly 1 for queue A, >=2 for queue B; each carrying userIdentity.invokedBy == "sqs.amazonaws.com".
5. cloudtrail:LookupEvents with LookupAttributes=[{AttributeKey: "EventName", AttributeValue: "CreateQueue"}].
   Observed: no record naming queue A. Expected: the caller's own CreateQueue record with the queue name in requestParameters.
