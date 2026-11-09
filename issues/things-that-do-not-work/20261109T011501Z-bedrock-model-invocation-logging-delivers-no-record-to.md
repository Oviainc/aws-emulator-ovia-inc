Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock.put-model-invocation-logging-config-enforced-on-invoke bedrock.model-invocation-logging-text-delivery-disabled-omits-bodies bedrock.delete-model-invocation-logging-withholds-further-records bedrock.bedrock-model-invocation-logging-with-a-cross-region-bucket-rejected
Issue-Key: bedrock.put-model-invocation-logging-config-enforced-on-invoke|_poll_until#20|AssertionError|raise AssertionError(f"condition not met within {timeout_s}s; last={last!r}")
Issue-Key: bedrock.model-invocation-logging-text-delivery-disabled-omits-bodies|_await_record#22|AssertionError|raise AssertionError(
Issue-Key: bedrock.delete-model-invocation-logging-withholds-further-records|_await_record#19|AssertionError|raise AssertionError(
Issue-Key: bedrock.bedrock-model-invocation-logging-with-a-cross-region-bucket-rejected|run#22|Timeout|time.sleep(2)
Issue-Commit: 016c7defff76bf6b624ee66d2926fa081d59376e

# bedrock model invocation logging delivers no record to an accepted s3Config destination: the enablement, text-delivery-disabled and stop-after-delete scenarios all hang out their delivery waits (1 cause, 4 checks)

> _Note: this issue was identified against the code base at commit `016c7defff76bf6b624ee66d2926fa081d59376e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutModelInvocationLoggingConfiguration

## Description
This issue carries 1 root cause grounded in 4 failing checks under the suite's `bedrock` service heading. The open issue 20261102T125313Z-bedrock-model-invocation-logging-is-inert-an-accepted.md reports this same defect but carries only its own replay key (and 20261029T171601Z-bedrock-converse-serves-a-constant-notice-string-inline.md Cause 4 carries another); NONE of the four keys below is carried by any open issue, so this issue exists to add them, not to describe a second defect. The emulator answered every control-plane call here with 200 (Put and Get both succeed), so the service is up and answering wrongly, not unbootstrapped.

---
Cause 1 of 1: an accepted model-invocation-logging configuration is stored and echoed back, but no invocation ever produces a log record at the configured destination, so logging-on, logging-off and the per-datatype delivery switches are indistinguishable.
AWS: Smithy PutModelInvocationLoggingConfiguration PUT /logging/modelinvocations -> 200 and GetModelInvocationLoggingConfiguration GET /logging/modelinvocations -> 200 with `loggingConfig` (LoggingConfig); DeleteModelInvocationLoggingConfiguration DELETE -> 200. Delivery itself is not modeled, so recorded behavior only: with loggingConfig.s3Config={bucketName,keyPrefix} and textDataDeliveryEnabled true, every subsequent inference writes a JSON-lines object under that prefix in that bucket within the delivery window, and each record carries the invocation's own requestId, its `modelId`, its `operation`, and (when text delivery is enabled) the prompt and completion bodies; with textDataDeliveryEnabled false the record STILL arrives but omits the bodies. After DeleteModelInvocationLoggingConfiguration, further invocations produce NO new record. All of bucket name, prefix, requestId and model id are per-request values, never literals.
Emulator: Put is accepted and Get returns the stored configuration byte-for-byte (those assertions pass), but zero objects ever appear under the configured prefix: every check's bounded poll over ListObjectsV2/GetObject reports '0 record(s) present, requestIds []' and either fails its own delivery assertion (110s / 120s waits) or is killed at the suite's 180s budget. Because nothing is ever delivered, the text-delivery-disabled scenario cannot observe a body-less record, and the post-delete scenario has no positive-control record to contrast with the withheld one.
Why: 'logging is configured' and 'logging works' are indistinguishable against the emulator. Every downstream consumer — audit pipelines, prompt/completion review, token accounting, the delivery-switch semantics and the stop-after-delete guarantee — is untestable, and an operator who sees the configuration echoed back concludes records are being written.
Fix: an invocation-logging delivery path: a post-inference hook in the bedrock-runtime inference handlers that emits the record, plus the bedrock control plane's logging component that resolves the stored s3Config (bucketName/keyPrefix), writes the JSON-lines object, honours textDataDeliveryEnabled / imageDataDeliveryEnabled / embeddingDataDeliveryEnabled, and stops emitting once the configuration is deleted.
Hypothesis (for bedrock.bedrock-model-invocation-logging-with-a-cross-region-bucket-rejected only): that check reports a bare 'Check timed out after 180.0s' with no assertion text. Its earlier legs assert the same-region Put/Get and then require the cross-region Put to be REFUSED with 400 ValidationException 'Failed to validate permissions for bucket: <bucketName>. Verify the S3 bucket policy is correct.' (fixed text, with the refused bucket name taken from the request) — an unexpected success there raises immediately rather than hanging, so the only step that can consume the budget is its 240s delivery wait on the surviving same-region configuration, i.e. this same absent delivery. Once delivery works, re-replay that check to confirm its cross-region refusal leg also matches.
Traces: bedrock.put-model-invocation-logging-config-enforced-on-invoke, bedrock.model-invocation-logging-text-delivery-disabled-omits-bodies, bedrock.delete-model-invocation-logging-withholds-further-records, bedrock.bedrock-model-invocation-logging-with-a-cross-region-bucket-rejected.

## Evidence
Behavioral suite simparity-behavioral-aws-v1: 142/196 passed; service heading `bedrock` 142/191 (74%), 49 failed. bedrock is bootstrapped (get_bootstrap_status().bootstrapped contains service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice; `pending` is empty).

bedrock.put-model-invocation-logging-config-enforced-on-invoke [mismatch]
  Put + Get of loggingConfig{s3Config{bucketName,keyPrefix}} succeeded, then a Converse was issued and the bucket polled.
  Expected (AWS): a log object appears under the configured prefix.
  Actual: AssertionError: condition not met within 120.0s; last=[]  (the object listing stayed empty)

bedrock.model-invocation-logging-text-delivery-disabled-omits-bodies [mismatch]
  Expected (AWS): with textDataDeliveryEnabled=false a record for the invocation still arrives, carrying its requestId but no prompt/completion bodies.
  Actual: AssertionError: text-delivery-off: no invocation log record carrying requestId '<per-run id>' appeared under s3://<bucket>/sim2real within 110.0s; 0 record(s) were delivered, with requestIds []

bedrock.delete-model-invocation-logging-withholds-further-records [mismatch]
  Expected (AWS): the leg-A positive control (logging enabled) delivers a record for its invocation, so the post-delete leg can show no further record.
  Actual: AssertionError: leg-A-positive-control: no invocation log record carrying requestId '<per-run id>' appeared under s3://<bucket>/sim2real within 110.0s; 0 record(s) present, requestIds []

bedrock.bedrock-model-invocation-logging-with-a-cross-region-bucket-rejected [mismatch]
  Actual: Error: Check timed out after 180.0s — no assertion text; see the Hypothesis in the description (the only unbounded wait in this check is its 240s delivery poll on the accepted same-region bucket).

Model evidence: PutModelInvocationLoggingConfiguration PUT /logging/modelinvocations -> 200, GetModelInvocationLoggingConfiguration GET -> 200 with loggingConfig (LoggingConfig), DeleteModelInvocationLoggingConfiguration DELETE -> 200, on services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json. Record delivery and its contents are not modeled (recorded behavior only).

## Steps to Reproduce
1. git_clone, then run_behavioral_suite so the emulator is built and booted.
2. Re-replay exactly the four check ids in trace_ids.
3. Create an S3 bucket in the suite's own region, then bedrock.put_model_invocation_logging_configuration(loggingConfig={'s3Config':{'bucketName':<bucket>,'keyPrefix':'sim2real'},'textDataDeliveryEnabled':True,...}) and confirm get_model_invocation_logging_configuration echoes it.
4. Issue a bedrock-runtime Converse/InvokeModel call and note its response requestId; then poll ListObjectsV2/GetObject under s3://<bucket>/sim2real. Expected: a JSON-lines record carrying that requestId, the modelId and the operation; observed: zero objects.
5. Repeat step 3 with textDataDeliveryEnabled=False. Expected: the record still arrives but without the prompt/completion bodies; observed: no record at all.
6. With logging enabled, confirm one invocation is logged (positive control), then bedrock.delete_model_invocation_logging_configuration() and confirm a further invocation produces NO new record. Observed: the positive control never produces a record, so the guarantee is unobservable.
7. For the cross-region check: create a second bucket in another region with an identical Bedrock write policy, re-Put with it, and confirm the refusal is 400 ValidationException 'Failed to validate permissions for bucket: <bucket>. Verify the S3 bucket policy is correct.' and that the prior configuration is unchanged; then confirm the same-region configuration still delivers a record for a post-refusal invocation.
