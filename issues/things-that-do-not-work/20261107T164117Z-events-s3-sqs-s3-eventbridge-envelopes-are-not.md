Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__s3__sqs.a-post-object-upload-is-withheld-from-a-putobject-scoped-rule events__s3__sqs.removing-eventbridge-configuration-withholds-envelope-but-native-queue-still-delivers
Issue-Key: events__s3__sqs.a-post-object-upload-is-withheld-from-a-putobject-scoped-rule|_drain#36|Timeout|resp = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Key: events__s3__sqs.removing-eventbridge-configuration-withholds-envelope-but-native-queue-still-delivers|poll#30|Timeout|resp = self._sqs.receive_message(QueueUrl=self._url, MaxNumberOfMessages=10, WaitTimeSeconds=2)
Issue-Commit: 6bba0ea93be6d4db7a2caa629527597bd59ab221

# events__s3__sqs: S3→EventBridge envelopes are not produced for a form POST upload and the EventBridgeConfiguration removal / dual-destination routing never settles (2 causes)

> _Note: this issue was identified against the code base at commit `6bba0ea93be6d4db7a2caa629527597bd59ab221`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketNotificationConfiguration

## Description
2 net-new root causes in this service.

Cause 1 of 2: a browser form POST upload (POST Object) to a bucket with EventBridgeConfiguration enabled does not produce an `Object Created` envelope carrying `detail.reason` = `POST Object`, so a rule scoped to that reason never receives it and the disjointness against a `PutObject`-scoped rule is never demonstrated. Failing check id: events__s3__sqs.a-post-object-upload-is-withheld-from-a-putobject-scoped-rule

AWS: S3 enables EventBridge delivery through PutBucketNotificationConfiguration with the empty `EventBridgeConfiguration` container (Smithy: s3#NotificationConfiguration member `EventBridgeConfiguration` -> s3#EventBridgeConfiguration, "Enables delivery of events to Amazon EventBridge"); GetBucketNotificationConfiguration must read it back. With it enabled, every object creation is published to the default bus as source `aws.s3`, detail-type `Object Created`, with `detail.bucket.name` = the bucket, `detail.object.key` = the RAW (unencoded) key, and `detail.reason` naming the API that created the object — `PutObject` for a PUT and `POST Object` for a browser form POST (https://docs.aws.amazon.com/AmazonS3/latest/userguide/EventBridge.html, https://docs.aws.amazon.com/AmazonS3/latest/API/API_PostObject.html). A signed form POST with success_action_status=201 answers HTTP 201 and the object is readable by HeadObject with the posted ContentLength. Because the reason literals differ, a rule whose pattern pins `detail.reason: ["PutObject"]` must receive nothing for a form-POST creation, and a rule pinning `["POST Object"]` must receive nothing for a PUT. The reason literals and the raw-key spelling are documented behavior, not modeled.

Emulator: the scenario never completes — the replay ends with `Check timed out after 60.0s`, with no assertion text, no HTTP status and no error code. The positive-control PutObject envelope, the 201 form POST, the `POST Object` envelope and the held negative on the PutObject-scoped queue are never all observed inside the run budget, so no form-POST creation is ever seen on the POST-Object-scoped rule.

Why: a client that distinguishes creation paths by `detail.reason` (the documented way to route browser uploads differently from server-side PUTs) gets no event at all for the form-POST path, so the rule that should fire never fires and nothing downstream of it runs. Worse, because the absence is silent, a consumer cannot tell "not created" from "created but unreported".

Fix: the S3 EventBridge publication path — the component that turns an object-creation into the `Object Created` envelope must cover the POST Object entry point and stamp `detail.reason` from the creating API, and the form-POST (multipart/form-data) object endpoint must create the object and trigger that publication.

Traces: events__s3__sqs.a-post-object-upload-is-withheld-from-a-putobject-scoped-rule.

Cause 2 of 2: with both `EventBridgeConfiguration` and a native `QueueConfigurations` entry on one bucket, the dual delivery (one native S3 `Records` message plus one EventBridge envelope per write) and the subsequent removal of `EventBridgeConfiguration` never settle into the state AWS reaches, so the "envelope stops, native keeps delivering" contract is never observable. Failing check id: events__s3__sqs.removing-eventbridge-configuration-withholds-envelope-but-native-queue-still-delivers

AWS: one PutObject on a bucket configured with both destinations yields exactly one native record and exactly one EventBridge envelope for that write. They differ by design: the native record spells the key FORM-ENCODED (space -> '+', '+' -> %2B, '=' -> %3D, UTF-8 percent-escaped, '~' -> %7E, '/' kept) while the envelope carries the RAW key; the native record uses `eventName` = `ObjectCreated:Put`, `s3.configurationId` = the Id of the matching QueueConfiguration, and `s3.object.eTag`, while the envelope uses `detail-type` = `Object Created`, `detail.reason` = `PutObject` and `detail.object.etag`; both carry the same `size` and the SAME `sequencer` for one write. A PutBucketNotificationConfiguration that replaces the document and omits `EventBridgeConfiguration` must make GetBucketNotificationConfiguration stop reporting it, and writes routed by the replacement configuration must continue reaching the native queue under the new configuration Id while producing NO further envelopes.

Emulator: this scenario also never completes — `Check timed out after 60.0s`, with no assertion text and no error code, so neither the dual-delivery leg nor the post-removal leg is ever satisfied within the budget.

Why: a client that runs both notification families off one bucket (the common migration shape: keep the native queue, add EventBridge, then drop one) cannot verify or rely on either side. Turning EventBridge off is a safety action; if the emulator cannot be observed to stop publishing, a consumer that double-processes cannot be detected, and a consumer that waits for the native path blocks.

Fix: the S3 notification dispatch and the PutBucketNotificationConfiguration state it reads — one write must fan out to each configured destination exactly once with the per-family key encoding, field spelling and shared sequencer, and a replace that omits `EventBridgeConfiguration` must take effect on subsequent writes.

Traces: events__s3__sqs.removing-eventbridge-configuration-withholds-envelope-but-native-queue-still-delivers.

Neither cause is covered by the open events__s3__sqs issues 20261025T171339Z-events-s3-sqs-a-rejected-putobjectacl-s-emits.md, 20261028T043345Z-events-s3-sqs-s3-notification-payloads-copy-the.md (quoted ETag in detail.object.etag) or 20261101T182649Z-events-s3-sqs-puttargets-accepts-an-sqs-fifo.md (FIFO target validation); those are different defects and neither is registered as owning these two checks.

## Evidence
Cause 1 — expected (AWS): PutBucketNotificationConfiguration{EventBridgeConfiguration:{}} reads back with EventBridgeConfiguration present; a PutObject delivers an envelope to the rule pinning detail.reason=["PutObject"] with detail.reason == "PutObject"; a signed multipart/form-data POST with success_action_status=201 answers HTTP 201 and HeadObject reports the posted ContentLength; the POST-Object-scoped rule (detail.reason=["POST Object"]) receives an envelope with detail.reason == "POST Object" and detail.bucket.name == the bucket; the PutObject-scoped queue receives nothing for that form upload and reports ApproximateNumberOfMessages 0; a later PutObject still reaches the PutObject-scoped rule.
Cause 1 — actual (emulator): `Check timed out after 60.0s`; no envelope carrying detail.reason == "POST Object" is ever observed and the replay produces no status/error code to report.

Cause 2 — expected (AWS): with {EventBridgeConfiguration:{}, QueueConfigurations:[{Id:<dual>, QueueArn:<N>, Events:["s3:ObjectCreated:*"]}]}, one PutObject yields exactly one native record on N (eventName "ObjectCreated:Put", s3.configurationId == <dual>, key FORM-ENCODED, eTag/size matching the write) and exactly one envelope on E (source "aws.s3", detail-type "Object Created", RAW key, detail.reason "PutObject", detail.object.etag/size matching), with the SAME sequencer on both. After a replace to {QueueConfigurations:[{Id:<native-only>, ...}]}, GetBucketNotificationConfiguration no longer reports EventBridgeConfiguration and lists exactly [<native-only>]; subsequent writes arrive on N under configurationId <native-only> with eventName "ObjectCreated:Put" and produce NO envelopes on E.
Cause 2 — actual (emulator): `Check timed out after 60.0s`; neither the dual-delivery leg nor the post-removal leg is ever satisfied and the replay produces no status/error code.

Note on values: bucket names, queue URLs, rule names, keys, ETags and sequencer values are per-run and are not part of the requirement; the requirements are the reason literals ("PutObject", "POST Object"), the per-family key encoding and field spelling, the one-event-per-write/shared-sequencer rule, and that removing EventBridgeConfiguration stops envelopes while the native queue keeps delivering.

## Steps to Reproduce
Cause 1:
1. s3:CreateBucket B; s3:PutBucketNotificationConfiguration(B, {EventBridgeConfiguration:{}}); confirm GetBucketNotificationConfiguration reports EventBridgeConfiguration.
2. sqs:CreateQueue Q_put and Q_post; events:PutRule R_put with pattern {source:["aws.s3"], detail-type:["Object Created"], detail:{bucket:{name:[B]}, reason:["PutObject"]}} and R_post with reason:["POST Object"]; events:PutTargets each rule to its queue (FailedEntryCount must be 0).
3. s3:PutObject(B, k1) and confirm Q_put receives an envelope for k1 with detail.reason == "PutObject"; confirm Q_post stays empty.
4. Build a SigV4-signed POST policy form and submit multipart/form-data (field order: policy fields, then `key`, then `file`) to the bucket endpoint with success_action_status=201; expect HTTP 201 and HeadObject(B, k2).ContentLength == len(payload).
5. Expect Q_post to receive an envelope for k2 with detail.reason == "POST Object" and Q_put to stay empty over a hold of 3x the measured S3->EventBridge->SQS latency. Observed: the run never gets there and hits the 60s budget.

Cause 2:
1. s3:CreateBucket B; sqs:CreateQueue N and E; set N's policy for s3.amazonaws.com/arn:aws:s3:::B and E's for events.amazonaws.com/<rule arn>.
2. events:PutRule R on {source:["aws.s3"], detail-type:["Object Created"], detail:{bucket:{name:[B]}}} targeting E.
3. s3:PutBucketNotificationConfiguration(B, {EventBridgeConfiguration:{}, QueueConfigurations:[{Id:"dual-native", QueueArn:<N>, Events:["s3:ObjectCreated:*"]}]}).
4. s3:PutObject(B, "dual path/report 2026+q3=é~.txt"); expect one native record on N keyed "dual+path/report+2026%2Bq3%3D%C3%A9%7E.txt" with configurationId "dual-native" and one envelope on E keyed with the RAW key, both carrying the same sequencer.
5. Replace the configuration with {QueueConfigurations:[{Id:"native-only", QueueArn:<N>, Events:["s3:ObjectCreated:*"]}]}; expect GetBucketNotificationConfiguration to omit EventBridgeConfiguration.
6. PutObject again; expect native records under configurationId "native-only" and NO further envelopes on E. Observed: the run never gets there and hits the 60s budget.
7. Replay check ids events__s3__sqs.a-post-object-upload-is-withheld-from-a-putobject-scoped-rule and events__s3__sqs.removing-eventbridge-configuration-withholds-envelope-but-native-queue-still-delivers to confirm.
