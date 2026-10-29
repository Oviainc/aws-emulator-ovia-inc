Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-runtime-async-invoke-lifecycle-and-errors
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-runtime-async-invoke-lifecycle-and-errors bedrockruntime-async-invoke-read-and-rejections bedrock-runtime-apply-guardrail-error-recovery bedrock-runtime-invoke-guardrail-checks-error-paths
Issue-Shape-Id: com.amazonaws.bedrockruntime#AmazonBedrockFrontendService
Issue-Key: bedrock-runtime|GetAsyncInvoke|4xx->4xx|$.Message
Issue-Key: bedrock-runtime|GetAsyncInvoke|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: bedrock-runtime|GetAsyncInvoke|4xx->4xx|status
Issue-Key: bedrock-runtime|StartAsyncInvoke|4xx->2xx|$.message
Issue-Key: bedrock-runtime|StartAsyncInvoke|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: bedrock-runtime|StartAsyncInvoke|4xx->2xx|status
Issue-Key: bedrock-runtime|ApplyGuardrail|4xx->4xx|$.message
Issue-Key: bedrock-runtime|InvokeGuardrailChecks|4xx->4xx|$.message
Issue-Commit: 7240e32f2500d20910730c71d4ad0714ff1f3217

# bedrock-runtime error paths diverge: wrong status and code on async-invoke refusals, invalid input accepted 200, re-worded validation messages

> _Note: this issue was identified against the code base at commit `7240e32f2500d20910730c71d4ad0714ff1f3217`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAsyncInvoke

## Description
Four net-new causes in bedrock-runtime's client-error path: one refusal gets the wrong HTTP status and the wrong error code, one request AWS refuses is accepted with 200, and two refusals carry the right status but message text the emulator authored rather than the service's.

Cause 1 of 4 - GetAsyncInvoke on a cross-account invocationArn answers 400 ValidationException instead of 403 AccessDeniedException (fixture bedrock-runtime-async-invoke-lifecycle-and-errors, step task#5).
AWS: Smithy GetAsyncInvoke errors include AccessDeniedException (@httpError 403, documented "The request is denied because you do not have sufficient permissions to perform the requested action") and ValidationException (@httpError 400). invocationArn is @httpLabel with pattern ^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:async-invoke/[a-z0-9]{12}$ - an ARN whose 12-digit account differs from the caller's is well-formed, so it is not a constraint violation but an authorization failure. Recorded behavior: HTTP 403, x-amzn-errortype AccessDeniedException, body member `Message` = "The provided resource ARN is from a different account." That text is entirely fixed - it names no id, so there is no variable part to fill. (The model declares the member lowercase `message`; the recording shows the service emitting it capitalized as `Message` on this path, so that casing is the requirement here.)
Emulator: HTTP 400 with x-amzn-errortype ValidationException and no `Message` member at all.
Why: an SDK maps 403/AccessDeniedException to a non-retryable authorization failure that surfaces as a permissions problem; a 400 ValidationException tells the caller its own request was malformed, so credential/ARN-ownership bugs are mis-diagnosed, and `Message` being absent leaves error handlers that read it with no text.
Fix: the GetAsyncInvoke handler's ARN resolution - compare the ARN's account to the caller's and raise AccessDeniedException before (not as) input validation, and serialize the error body with the `Message` member.

Cause 2 of 4 - StartAsyncInvoke accepts an output location with unusable S3 credentials and answers 200 (fixture bedrockruntime-async-invoke-read-and-rejections, step task#4).
AWS: Smithy StartAsyncInvoke requires outputDataConfig (AsyncInvokeOutputDataConfig) and models ValidationException @httpError 400. The service validates at submit time that it can write the configured S3 destination and refuses the submission rather than starting a job that cannot deliver output. Recorded behavior: HTTP 400, x-amzn-errortype "ValidationException:http://internal.amazon.com/coral/com.amazon.bedrock/", body $.message = "Invalid S3 credentials" - fixed text with no variable part.
Emulator: HTTP 200 with a minted invocationArn and no error member, so the job is accepted.
Why: the caller never learns its destination is unwritable; code that treats a 200 from StartAsyncInvoke as "output will appear" polls GetAsyncInvoke forever or reads a prefix that is never written, and the negative test for this configuration passes against the emulator and fails on AWS.
Fix: StartAsyncInvoke's outputDataConfig validation (the operation's validation stage, before any job state is created).

Cause 3 of 4 - ApplyGuardrail's malformed-input refusal uses a generic emulator message (fixture bedrock-runtime-apply-guardrail-error-recovery, step task#2).
AWS: Smithy ApplyGuardrail models ValidationException @httpError 400 and requires source, content and the @httpLabel guardrailIdentifier/guardrailVersion. Recorded behavior for content the guardrail cannot parse: HTTP 400 with $.message = "Guardrail was enabled but input is in incorrect format." - fixed text in full, naming no request value.
Emulator: HTTP 400 (status correct) with $.message = "The provided request is not valid", a generic string that appears on unrelated paths too.
Why: callers and tests match on the service's sentence to distinguish "guardrail rejected the input format" from any other 400; a generic message makes the two indistinguishable and breaks message assertions and log-based alarms.
Fix: ApplyGuardrail's input-format validation in its handler - raise the format-specific ValidationException message rather than falling through to the shared generic 400.

Cause 4 of 4 - the enum-constraint ValidationException lists the enum value set in the wrong order (fixture bedrock-runtime-invoke-guardrail-checks-error-paths, step task#3).
AWS: Smithy InvokeGuardrailChecks models ValidationException @httpError 400; checks.sensitiveInformation.entities[].type targets the enum GuardrailChecksSensitiveInformationEntityType. The refusal uses AWS's standard constraint template - fixed text "<n> validation error detected: Value '<value>' at '<member path>' failed to satisfy constraint: Member must satisfy enum value set: [<values>]" - whose variable parts are the count, the rejected value and the member path, both taken from the request. Recorded behavior shows the service's value list for this enum beginning US_INDIVIDUAL_TAX_IDENTIFICATION_NUMBER, US_BANK_ROUTING_NUMBER, MAC_ADDRESS, AWS_ACCESS_KEY, ... - i.e. the set order the service emits, which is neither alphabetical nor the model's declaration order (recorded behavior only for the ordering rule itself).
Emulator: HTTP 400 and the template, count, rejected value and member path all correct, but the bracketed set is re-sorted alphabetically (ADDRESS, AGE, AWS_ACCESS_KEY, ...), so the message body differs from the service's byte for byte.
Why: the whole message is a requirement, so any client, golden-file test or support tool that compares the refusal text sees a mismatch on every enum violation of this member; the same alphabetical-resort defect is already filed for another service (open issue 20261029T211113Z, bcm-recommended-actions ListRecommendedActions), which indicates a shared validation helper and does not cover this service.
Emulator component to fix: the shared enum-constraint message builder used by InvokeGuardrailChecks' validation - emit the enum's value set in the service's order instead of sorting it.

## Evidence
Replay, all verdict=mismatch:
1) bedrock-runtime-async-invoke-lifecycle-and-errors#task#5 (GetAsyncInvoke) - status: expected 403, got 400; $.Message: missing (expected "The provided resource ARN is from a different account.", got None); $headers.x-amzn-errortype: got "ValidationException" where a 403 AccessDeniedException is expected.
2) bedrockruntime-async-invoke-read-and-rejections#task#4 (StartAsyncInvoke) - status: expected 400, got 200; $.message: missing (expected "Invalid S3 credentials"); $headers.x-amzn-errortype: missing (expected "ValidationException:http://internal.amazon.com/coral/com.amazon.bedrock/").
3) bedrock-runtime-apply-guardrail-error-recovery#task#2 (ApplyGuardrail) - 400->400, $.message: expected "Guardrail was enabled but input is in incorrect format.", got "The provided request is not valid".
4) bedrock-runtime-invoke-guardrail-checks-error-paths#task#3 (InvokeGuardrailChecks) - 400->400, $.message value_mismatch: both sides read "1 validation error detected: Value 'NOT_A_REAL_ENTITY_TYPE' at 'checks.sensitiveInformation.entities.1.member.type' failed to satisfy constraint: Member must satisfy enum value set: [...]" but the expected set starts [US_INDIVIDUAL_TAX_IDENTIFICATION_NUMBER, US_BANK_ROUTING_NUMBER, MAC_ADDRESS, AWS_ACCESS_KEY, ...] while the emulator emits the same 31 values sorted alphabetically [ADDRESS, AGE, AWS_ACCESS_KEY, ...].

Smithy (com.amazonaws.bedrockruntime): GetAsyncInvoke errors = AccessDeniedException(403), InternalServerException(500), ThrottlingException(429), ValidationException(400); StartAsyncInvoke errors include ValidationException(400) with outputDataConfig @required; ApplyGuardrail errors include ValidationException(400); GuardrailChecksSensitiveInformationEntityType has the 31 values listed in the message.

## Steps to Reproduce
1. git clone and boot the emulator; 2. replay bedrock-runtime-async-invoke-lifecycle-and-errors and inspect step task#5 - GetAsyncInvoke on an async-invoke ARN owned by another account returns 400/ValidationException instead of 403/AccessDeniedException with a `Message` member; 3. replay bedrockruntime-async-invoke-read-and-rejections step task#4 - StartAsyncInvoke with an unwritable S3 outputDataConfig returns 200 and an invocationArn instead of 400 "Invalid S3 credentials"; 4. replay bedrock-runtime-apply-guardrail-error-recovery step task#2 - the 400 body says "The provided request is not valid"; 5. replay bedrock-runtime-invoke-guardrail-checks-error-paths step task#3 - the enum value set in the 400 message is alphabetically sorted.
