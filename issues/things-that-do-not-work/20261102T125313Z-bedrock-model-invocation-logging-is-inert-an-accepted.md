Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock.bedrock-model-invocation-logging-records-the-prompt-and-the-completion
Issue-Key: bedrock.bedrock-model-invocation-logging-records-the-prompt-and-the-completion|run#25|Timeout|time.sleep(2)
Issue-Commit: e3e78cf83f0a145a8ff37896813888a6025b7749

# bedrock model invocation logging is inert: an accepted s3Config destination never receives an invocation log record, so the delivery check hangs to the 60 s cap (1 cause)

> _Note: this issue was identified against the code base at commit `e3e78cf83f0a145a8ff37896813888a6025b7749`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutModelInvocationLoggingConfiguration

## Description
Cause 1 of 1: model invocation logging is accepted and read back, but no invocation record is ever delivered to the configured S3 destination, so a caller polling for its own invocation's record polls forever.

AWS: Smithy (bedrock) models PutModelInvocationLoggingConfiguration with a required `loggingConfig` (LoggingConfig) carrying `s3Config` (S3Config{bucketName @length 3..63 required, keyPrefix @length ..1024}) and the per-modality switches `textDataDeliveryEnabled` / `imageDataDeliveryEnabled` / `embeddingDataDeliveryEnabled`; errors are AccessDeniedException (403), ValidationException (400), ThrottlingException (429), InternalServerException (500). The DELIVERY behavior itself is not in the model — recorded behavior only: once such a configuration is in force, every subsequent model invocation in that account and region is written by Bedrock into the named bucket under the configured keyPrefix as a JSON invocation record (one JSON document per line, optionally gzipped), and the record describes the invocation that actually ran: `operation` ("Converse" for a Converse call), `modelId` (the model id the request named), `input.inputBodyJson` (the request body as submitted, so the prompt text appears verbatim), `output.outputBodyJson` (the response body as returned, so the completion text appears verbatim), and `input.inputTokenCount` / `output.outputTokenCount` equal to the `usage.inputTokens` / `usage.outputTokens` the inline Converse response reported. Bedrock additionally writes zero-byte `amazon-bedrock-logs-permission-check` probe objects when the configuration is accepted; those are not invocation records. Delivery is asynchronous but prompt (seconds, not minutes).

Emulator: the configuration round-trips — PutModelInvocationLoggingConfiguration returns 200 and GetModelInvocationLoggingConfiguration reports back the bucket and textDataDeliveryEnabled=true — and the subsequent Converse against amazon.nova-lite-v1:0 returns 200 with text, but the configured bucket/prefix never receives any invocation record. The check loops on ListObjectsV2 + GetObject over s3://<bucket>/<keyPrefix> waiting for a record whose `input` contains the prompt nonce it just sent, matches nothing, and is killed by the harness: "Error: Check timed out after 60.0s" (the check's own delivery deadline is 240 s, so it never even reaches its own assertion). No object other than nothing at all appears: the positive half never fires, and the record-shape and token-count assertions downstream are never reached.

Why: a client that enables model invocation logging to audit what was prompted and what was generated gets an account that reports logging as configured while producing no audit trail at all. Compliance and observability code that waits for its invocation's record (the normal pattern, because the configuration is an account-and-region singleton shared with other invocations) blocks until its own timeout instead of finding the record, and code that asserts the logged prompt/completion/token counts against the inline response can never run. There is no error to key off — the emulator's answer is silence.

Fix: the delivery step is missing. The bedrock control-plane handler services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/ops/put_model_invocation_logging_configuration.rs only stores `loggingConfig` into state (`state.lock().model_invocation_logging = Some(input.logging_config)`), and nothing on the inference path consumes it: the bedrock-runtime inference handlers (Converse / ConverseStream / InvokeModel / InvokeModelWithResponseStream in services/service-com-amazonaws-bedrockruntime-amazonbedrockfrontendservice/src/ops/) must, after a successful invocation, read the account-and-region logging configuration and emit one invocation record to the configured s3Config destination (bucketName + keyPrefix) carrying operation, modelId, accountId, region, input.inputBodyJson, output.outputBodyJson and the same token counts the inline response reported, gated by the per-modality *DataDeliveryEnabled switches. Related but distinct open issues cover other parts of this surface and do NOT cover delivery: 20261030T033702Z-bedrock-putmodelinvocationloggingconfiguration-refuses-an-unverifiable-cloudwatchconfig-with-403.md and 20261030T100233Z-bedrock-logs-putmodelinvocationloggingconfiguration-accepts-a-cloudwatchconfig-the-recorded.md (cloudWatchConfig acceptance/refusal), 20261023T232022Z-bedrock-catalogued-foundation-model-refused-as-invalid-responses.md (cross-region logging bucket accepted, model catalogue refusals).

Traces: bedrock.bedrock-model-invocation-logging-records-the-prompt-and-the-completion — the only failing logging-delivery check in this suite that got past inference; the sibling logging checks in this service are all owned by other open issues because they fail earlier, at the inference call itself.

## Evidence
Expected (AWS): after PutModelInvocationLoggingConfiguration{loggingConfig:{s3Config:{bucketName:<bucket>,keyPrefix:"sim2real"},textDataDeliveryEnabled:true}} is accepted (200) and a Converse call runs, a JSON invocation record appears under s3://<bucket>/sim2real/ within seconds, with operation="Converse", modelId="amazon.nova-lite-v1:0", the prompt text verbatim at input.inputBodyJson.messages[0].content[0].text, the returned completion verbatim at output.outputBodyJson.output.message.content, input.inputTokenCount == usage.inputTokens and output.outputTokenCount == usage.outputTokens from the same Converse response. (bucket name, prompt nonce and token values above are per-run values filling that template, not requirements.)

Actual (emulator): Put returns 200; Get reports the configuration back with the right bucket and textDataDeliveryEnabled=true; Converse returns 200 with text; ListObjectsV2 over the configured bucket/prefix returns no invocation record at any point. The check's bounded poll never matches and the run is cut off with "Error: Check timed out after 60.0s" — i.e. zero records delivered, not a malformed record.

Source: services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/ops/put_model_invocation_logging_configuration.rs:170 stores the config and nothing reads it on the inference path.

## Steps to Reproduce
1. s3.CreateBucket on a fresh bucket in the request region.
2. bedrock.PutModelInvocationLoggingConfiguration with loggingConfig={"s3Config":{"bucketName":<bucket>,"keyPrefix":"sim2real"},"textDataDeliveryEnabled":true,"imageDataDeliveryEnabled":false,"embeddingDataDeliveryEnabled":false} -> 200.
3. bedrock.GetModelInvocationLoggingConfiguration -> reports that bucket and textDataDeliveryEnabled=true.
4. bedrock-runtime.Converse(modelId="amazon.nova-lite-v1:0", messages=[{role:user, content:[{text:"Repeat exactly this token and nothing else: <nonce>"}]}], inferenceConfig={maxTokens:40,temperature:0.0}) -> 200 with output text and usage.
5. Poll s3.ListObjectsV2(Bucket=<bucket>, Prefix="sim2real") and GetObject each key (gunzip .gz), parsing each line as JSON, looking for a record whose `input` JSON contains <nonce>.
6. AWS: such a record appears within seconds. Emulator: no record ever appears; the poll runs until the harness kills the check at 60 s.
