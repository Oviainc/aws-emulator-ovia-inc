Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock.content-type-required-json bedrock.converse-stream-message-stop-has-stop-reason bedrock.error-model-not-found-converse bedrock.error-model-not-found-invoke bedrock.error-validation-empty-messages bedrock.error-validation-malformed-body-invoke bedrock.put-model-invocation-logging-config-enforced-on-invoke bedrock.delete-model-invocation-logging-withholds-further-records bedrock.model-invocation-logging-text-delivery-disabled-omits-bodies bedrock.bedrock-model-invocation-logging-with-a-cross-region-bucket-rejected bedrock.deploy-guardrail-cfn-enforces-blocked-content bedrock.bedrock-tool-use-round-trip-returns-the-tool-result-in-the-final-answer
Issue-Key: bedrock-runtime|Converse|ValidationException
Issue-Key: bedrock-runtime|InvokeModel|ValidationException
Issue-Key: bedrock.delete-model-invocation-logging-withholds-further-records|_converse#18|KeyError|request_id = response["ResponseMetadata"]["RequestId"]
Issue-Key: bedrock.model-invocation-logging-text-delivery-disabled-omits-bodies|_converse#19|KeyError|request_id = response["ResponseMetadata"]["RequestId"]
Issue-Key: bedrock.bedrock-model-invocation-logging-with-a-cross-region-bucket-rejected|_error#11|_UnexpectedSuccess|raise _UnexpectedSuccess(
Issue-Key: bedrock|GetGuardrail|ValidationException
Issue-Key: bedrock.bedrock-tool-use-round-trip-returns-the-tool-result-in-the-final-answer|run#5|AssertionError|assert first["stopReason"] == "tool_use", (
Issue-Commit: cd7a0e788c437c5439a918b59f215cef2324a883

# bedrock: catalogued foundation model refused as invalid, responses carry no x-amzn-RequestId, cross-region logging bucket accepted, CFN guardrail id is the name

> _Note: this issue was identified against the code base at commit `cd7a0e788c437c5439a918b59f215cef2324a883`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InvokeModel

## Description
FIVE CAUSES reached through the `bedrock` behavioral checks. Each is a distinct contract a fixer must see answer correctly. Smithy models read: services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json and services/service-com-amazonaws-bedrockruntime-amazonbedrockfrontendservice/smithy-model/com-amazonaws-bedrockruntime-amazonbedrockfrontendservice.json.

Cause 1 of 5 - a catalogued foundation model id is refused as malformed, so seven checks cannot even reach their subject.
AWS: Smithy InvokeModel/Converse bind `modelId` as an httpLabel targeting InvokeModelIdentifier/ConversationalModelId, whose @pattern admits the bare-id alternative ([a-z0-9-]{1,63}[.]{1}[a-z0-9-]{1,63}([.:]?[a-z0-9-]{1,63})) - 'moonshotai.kimi-k2.5' satisfies it. Recorded behavior only: the suite's account can invoke that model, and the checks use it as their POSITIVE CONTROL (each one first issues a well-formed call and asserts it is served). The modeled success is 200.
Emulator: 400 ValidationException with message example 'The provided model identifier is invalid.' for every InvokeModel/Converse/ConverseStream naming moonshotai.kimi-k2.5 - the emulator's model catalogue does not contain it, so the control fails and the real subject (content-type handling, empty-messages validation, malformed-body validation, not-found classification, messageStop stopReason) is never exercised.
Why: a client using a model the region offers is told its identifier is invalid - a terminal 400 it cannot work around; and because the refusal happens on the control leg, it masks every downstream behavior on that model.
Fix: the model catalogue consulted by the InvokeModel/Converse handlers' identifier resolution.
Traces: bedrock.content-type-required-json, bedrock.converse-stream-message-stop-has-stop-reason, bedrock.error-model-not-found-converse, bedrock.error-model-not-found-invoke, bedrock.error-validation-empty-messages, bedrock.error-validation-malformed-body-invoke, bedrock.put-model-invocation-logging-config-enforced-on-invoke.

Cause 2 of 5 - Bedrock responses carry no request id, so ResponseMetadata.RequestId is absent.
AWS: every Bedrock response carries the request-id header that botocore surfaces as ResponseMetadata['RequestId']; the model-invocation log record is correlated to the call by exactly that value, which is why a client reads it. (Recorded behavior only - the header is not a modeled member.)
Emulator: the Converse response's ResponseMetadata contains only HTTPStatusCode, HTTPHeaders and RetryAttempts, and HTTPHeaders is just {content-type, content-length, date}; reading ResponseMetadata['RequestId'] raises KeyError. Observed on both Converse and StartAsyncInvoke responses in this run.
Why: a client cannot correlate a response with its invocation-logging record, cannot quote a request id in a support case, and any generic SDK middleware that logs or retries by request id breaks.
Fix: the response serializer / HTTP layer shared by the Bedrock handlers (emit the request-id response header on every operation).
Traces: bedrock.delete-model-invocation-logging-withholds-further-records, bedrock.model-invocation-logging-text-delivery-disabled-omits-bodies.

Cause 3 of 5 - PutModelInvocationLoggingConfiguration accepts an S3 bucket in a different region (OVER-PERMISSIVE).
AWS: Smithy PutModelInvocationLoggingConfiguration -> PUT /logging/modelinvocations, input {loggingConfig required}, errors AccessDeniedException(403), InternalServerException(500), ThrottlingException(429), ValidationException(400). A loggingConfig whose s3Config bucket lives outside the caller's region is refused with a 400-class ValidationException (recorded behavior: the check's two buckets carry identical, unconditioned bucket policies, so region is the only difference between the accepted and the refused configuration). The status and error code are the requirement.
Emulator: 200 with an empty body - the cross-region bucket is accepted and becomes the live logging destination.
Why: an operator believes model-invocation logs are being delivered to a bucket Bedrock would never write to, so an audit/compliance trail silently does not exist; and the configuration cannot be validated against the emulator before it is applied to production.
Fix: PutModelInvocationLoggingConfiguration input validation (region check on loggingConfig.s3Config.bucketName).
Traces: bedrock.bedrock-model-invocation-logging-with-a-cross-region-bucket-rejected.

Cause 4 of 5 - a CloudFormation-created AWS::Bedrock::Guardrail resolves GuardrailId to the guardrail NAME, which no Bedrock API will accept.
AWS: Smithy CreateGuardrail returns required `guardrailId` constrained to @pattern ^[a-z0-9]+$ (@length max 64) and `guardrailArn` to ^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:guardrail/[a-z0-9]+$, while `name` is a distinct member with @pattern ^[0-9a-zA-Z-_]+$. GetGuardrail's `guardrailIdentifier` httpLabel admits only (([a-z0-9]+)|(arn:...:guardrail/[a-z0-9]+)). A stack's `!GetAtt Guardrail.GuardrailId` therefore resolves to the minted id, not the name, and feeding it to GetGuardrail succeeds.
Emulator: the stack reaches CREATE_COMPLETE and its GuardrailId output is the guardrail's NAME (observed value example 'sim2real-cfn-guardrail-d5e796cc6bcd' - it contains '-', so it cannot satisfy ^[a-z0-9]+$). The very next GetGuardrail then answers 400 ValidationException '1 validation error detected: Value ... at ''guardrailIdentifier'' failed to satisfy constraint: Member must satisfy regular expression pattern: (([a-z0-9]+)|(arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:guardrail/[a-z0-9]+))'.
Why: a stack that provisions a guardrail cannot hand it to anything - the id it exports is rejected by every Bedrock guardrail API, so infrastructure-as-code deployment of guardrails is unusable end to end.
Fix: the CloudFormation AWS::Bedrock::Guardrail resource provider (GetAtt GuardrailId/GuardrailArn resolution and physical resource id must come from the CreateGuardrail response, not the Name property).
Traces: bedrock.deploy-guardrail-cfn-enforces-blocked-content.

Cause 5 of 5 - Converse ignores toolConfig, so a tool-use round trip never starts.
AWS: ConverseRequest carries `toolConfig` -> ToolConfiguration, and ConverseResponse's required stopReason is the StopReason enum including `tool_use`; a request whose toolConfig offers a tool the prompt needs stops with stopReason='tool_use' and a toolUse content block, which the caller answers with a toolResult block.
Emulator: stopReason='end_turn' with a plain text block on the first turn, so the tool is never requested and the second turn (toolResult -> final answer) is unreachable.
Why: the whole tool-use protocol is inert - an agent client waits for a toolUse block that never arrives. (Same underlying defect as cause 2 of the companion bedrock-runtime issue, recorded here because this check is grounded in the `bedrock` service token.)
Fix: the Converse handler's toolConfig handling.
Traces: bedrock.bedrock-tool-use-round-trip-returns-the-tool-result-in-the-final-answer.

## Evidence
Behavioral suite simparity-behavioral-ga-prod-v1, bedrock 2/18 passed. The emulator was serving (200s and modeled 4xx observed), so these are fidelity findings.

Cause 1 - six checks, identical:
  AssertionError: model moonshotai.kimi-k2.5 not usable for inference here (ValidationException)
  bedrock.error-validation-malformed-body-invoke: AssertionError: positive control failed: well-formed InvokeModel for moonshotai.kimi-k2.5 raised ValidationException
  bedrock.put-model-invocation-logging-config-enforced-on-invoke: botocore.errorfactory.ValidationException: An error occurred (ValidationException) when calling the InvokeModel operation: The provided model identifier is invalid.

Cause 2 - two checks, identical:
  File "check_bedrock_delete_model_invocation_logging_withholds_further_records.py", line 164, in _converse
    request_id = response["ResponseMetadata"]["RequestId"]
  KeyError: 'RequestId'
  Corroborating ResponseMetadata observed elsewhere in the same run: {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/json', 'content-length': '182', 'date': '...'}, 'RetryAttempts': 0}, ...} - no request-id header at all.

Cause 3 - bedrock.bedrock-model-invocation-logging-with-a-cross-region-bucket-rejected:
  _UnexpectedSuccess: the call was expected to be refused but returned {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/json', 'content-length': '2', ...}, 'RetryAttempts': 0}}

Cause 4 - bedrock.deploy-guardrail-cfn-enforces-blocked-content:
  botocore.errorfactory.ValidationException: An error occurred (ValidationException) when calling the GetGuardrail operation: 1 validation error detected: Value 'sim2real-cfn-guardrail-d5e796cc6bcd' at 'guardrailIdentifier' failed to satisfy constraint: Member must satisfy regular expression pattern: (([a-z0-9]+)|(arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:guardrail/[a-z0-9]+))
  (the value is the stack's GuardrailId output, taken from Outputs.GuardrailId = !GetAtt Guardrail.GuardrailId; it equals the GuardrailName parameter the stack was created with)

Cause 5 - bedrock.bedrock-tool-use-round-trip-returns-the-tool-result-in-the-final-answer:
  AssertionError: the model must stop to call the tool; got 'end_turn' with content [{'text': ...}]

Model evidence: smithy_query operation PutModelInvocationLoggingConfiguration/GetGuardrail/CreateGuardrail on the bedrock control-plane model (PutModelInvocationLoggingConfiguration PUT /logging/modelinvocations, errors AccessDenied 403 / Throttling 429 / Validation 400 / InternalServer 500; CreateGuardrail output guardrailId @pattern ^[a-z0-9]+$ and guardrailArn @pattern ^arn:...:guardrail/[a-z0-9]+$ distinct from name @pattern ^[0-9a-zA-Z-_]+$; GetGuardrail guardrailIdentifier @pattern (([a-z0-9]+)|(arn:...:guardrail/[a-z0-9]+))), and operation Converse/InvokeModel plus shape StopReason/ToolChoice on the bedrock-runtime model (StopReason includes tool_use; modelId patterns admit the bare 'vendor.model' form).

## Steps to Reproduce
1. git_clone the repo and let the behavioral suite build and boot the emulator.
2. Run the behavioral suite and re-replay the check ids in trace_ids.
3. Cause 1: InvokeModel (and Converse, and ConverseStream) with modelId='moonshotai.kimi-k2.5' and a well-formed body. Expected 200; observed 400 ValidationException 'The provided model identifier is invalid.'
4. Cause 2: issue any Converse call and read response['ResponseMetadata']['RequestId']. Expected the service's request id; observed KeyError because no request-id response header is emitted.
5. Cause 3: create an S3 bucket in a region other than the client's, give it the standard Bedrock-logging bucket policy, and call PutModelInvocationLoggingConfiguration with that bucket in loggingConfig.s3Config. Expected 400 ValidationException; observed 200. The same call with a same-region bucket must keep working.
6. Cause 4: CreateStack with a template declaring AWS::Bedrock::Guardrail (Name from a parameter) and Outputs GuardrailId=!GetAtt Guardrail.GuardrailId. Wait for CREATE_COMPLETE, then call GetGuardrail with that output. Expected the minted id (^[a-z0-9]+$) and a 200 from GetGuardrail; observed the guardrail's Name and a 400 ValidationException on the identifier pattern.
7. Cause 5: Converse with a toolConfig offering one tool and a prompt that needs it. Expected stopReason='tool_use' with a toolUse content block; observed stopReason='end_turn' with a text block.
