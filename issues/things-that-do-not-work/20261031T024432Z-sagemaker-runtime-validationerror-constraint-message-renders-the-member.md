Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sagemaker-runtime.invoke-endpoint-new-session-not-established-when-unsupported
Issue-Shape-Id: com.amazonaws.sagemakerruntime#AmazonSageMakerRuntime
Issue-Key: sagemaker-runtime.invoke-endpoint-new-session-not-established-when-unsupported|run#12|AssertionError|assert session_msg == expected_session, (
Issue-Commit: 2a2cd1dd5d8908fe6bf8e0fe051e0b3d68ebbc2c

# sagemaker-runtime: ValidationError constraint message renders the member's @pattern with its anchors stripped (trailing '$' dropped), so InvokeEndpointWithResponseStream's SessionId refusal message does not match AWS

> _Note: this issue was identified against the code base at commit `2a2cd1dd5d8908fe6bf8e0fe051e0b3d68ebbc2c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InvokeEndpointWithResponseStream

## Description
One root cause (1 of 1). Related but NON-covering open issues: 20261029T203709Z-sagemaker-runtime-request-constraint-refusals-use-error-code.md owns the error-CODE divergence (ValidationException vs ValidationError), the sensitive-CustomAttributes echo and the InvokeEndpointAsync Body/InputLocation disjunction, and 20261029T233857Z-sagemaker-runtime-drops-errorcode-message-from-refusal-bodies.md owns missing ErrorCode/Message in refusal bodies and lookup-before-validation ordering. Neither covers this: here the error code IS the modeled ValidationError and the refusal IS evaluated before the endpoint lookup — only the rendered pattern text inside the message diverges.

Cause 1 of 1: the constraint-violation message formatter does not reproduce the member's @pattern verbatim; it emits the regex with the anchors trimmed (the trailing '$' is dropped; in other recorded refusals on this service the leading '^' is dropped too).
AWS: Smithy com.amazonaws.sagemakerruntime#InvokeEndpointWithResponseStream input member SessionId targets SessionIdHeader with @length min 0 max 256 and @pattern "^[a-zA-Z0-9](-*[a-zA-Z0-9])*$" — note that, unlike InvokeEndpoint.SessionId (pattern "^(NEW_SESSION)$|^[a-zA-Z0-9](-*[a-zA-Z0-9])*$"), the streaming operation's SessionId does NOT admit the NEW_SESSION sentinel, so SessionId=NEW_SESSION is a pattern violation on this operation. The operation's modeled client error is ValidationError (httpError 400). AWS refuses it with HTTP 400, Error.Code "ValidationError" and the standard constraint message "1 validation error detected: Value '<value>' at '<memberName>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>". Fixed text of that template: "1 validation error detected: Value '", "' at '", "' failed to satisfy constraint: Member must satisfy regular expression pattern: ". Variable parts: <value> is the rejected value taken from the request, <memberName> is the wire member name (lowerCamel, here "sessionId"), and <pattern> is the member's @pattern copied VERBATIM from the model — anchors included. For this member that yields "1 validation error detected: Value 'NEW_SESSION' at 'sessionId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9](-*[a-zA-Z0-9])*$". The rule, not the recorded literal, is the requirement: whatever the violated member is, the pattern slot must be the model's pattern string unmodified.
Emulator: the status (400), the error code (ValidationError) and the pre-lookup ordering are right, but the pattern slot is rendered with the trailing anchor removed: "1 validation error detected: Value 'NEW_SESSION' at 'sessionId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9](-*[a-zA-Z0-9])*" (no closing '$'). The same formatter is visible elsewhere on this service dropping the leading '^' as well (e.g. endpointName refusals rendered as "pattern: [a-zA-Z0-9](-*[a-zA-Z0-9])*" and SessionId on InvokeEndpoint as "pattern: (NEW_SESSION)$|^[a-zA-Z0-9](-*[a-zA-Z0-9])*"), so the defect is anchor trimming in the shared message formatter rather than one member's metadata. Hypothesis: the validator compiles/normalises the regex (stripping anchors because it already anchors the match itself) and then formats the message from the normalised regex instead of from the model's original pattern string.
Why: the pattern in the message is the only thing that tells a client which constraint it violated, and clients and conformance tests compare it to the model's pattern. A caller — or a negative test written against AWS — asserting the documented message text fails against the emulator even though the refusal is otherwise correct; and an operator reading the emulator's message is shown a regex that is not the service's actual constraint (an unanchored "[a-zA-Z0-9](-*[a-zA-Z0-9])*" matches any string containing such a run, which is a materially different rule from the anchored one).
Fix: the sagemaker-runtime request-validation layer's constraint-message formatter — it must interpolate the member's @pattern exactly as declared in the Smithy model (keep any leading '^' / trailing '$'), independently of whatever normalised form the matcher uses internally. The anchored-match behaviour itself is correct and must not change.
Traces: sagemaker-runtime.invoke-endpoint-new-session-not-established-when-unsupported (the baseline half of the same check — no SessionId against an absent endpoint returning ValidationError "Endpoint <name> of account <account> not found." — passes, isolating the divergence to the pattern rendering).

## Evidence
Check sagemaker-runtime.invoke-endpoint-new-session-not-established-when-unsupported, [mismatch]:

AssertionError: the streaming op must refuse SessionId=NEW_SESSION on the sessionId field itself, BEFORE the endpoint lookup; expected "1 validation error detected: Value 'NEW_SESSION' at 'sessionId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9](-*[a-zA-Z0-9])*$", got "1 validation error detected: Value 'NEW_SESSION' at 'sessionId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9](-*[a-zA-Z0-9])*"

Expected (AWS): HTTP 400, Error.Code "ValidationError", Error.Message ending in the model's pattern verbatim: ... "pattern: ^[a-zA-Z0-9](-*[a-zA-Z0-9])*$".
Actual (emulator): HTTP 400, Error.Code "ValidationError" (correct), Error.Message ending "pattern: ^[a-zA-Z0-9](-*[a-zA-Z0-9])*" — the trailing '$' anchor is missing.

Control in the same check that PASSED: invoke_endpoint_with_response_stream(EndpointName=<well-formed, never created>, Body=b"x") with no SessionId -> ValidationError "Endpoint <name> of account <account> not found." — so the code, status and validate-before-lookup ordering are already correct on this path.

Model evidence (smithy_query, services/service-com-amazonaws-sagemakerruntime-amazonsagemakerruntime/smithy-model/com-amazonaws-sagemakerruntime-amazonsagemakerruntime.json):
  InvokeEndpointWithResponseStream.SessionId -> SessionIdHeader, httpHeader X-Amzn-SageMaker-Session-Id, @length 0..256, @pattern "^[a-zA-Z0-9](-*[a-zA-Z0-9])*$"
  InvokeEndpointWithResponseStream errors include ValidationError (client, httpError 400)
  (contrast: InvokeEndpoint.SessionId pattern is "^(NEW_SESSION)$|^[a-zA-Z0-9](-*[a-zA-Z0-9])*$" — the sentinel is operation-scoped)

Corroborating emulator output from other refusals on the same formatter (reported in the related open issue): "pattern: [a-zA-Z0-9](-*[a-zA-Z0-9])*" for endpointName and "pattern: (NEW_SESSION)$|^[a-zA-Z0-9](-*[a-zA-Z0-9])*" for InvokeEndpoint.sessionId — both anchors trimmed.

## Steps to Reproduce
1. Boot the emulator.
2. Call sagemaker-runtime InvokeEndpointWithResponseStream with EndpointName=<a well-formed name that was never created>, Body=b"x" and NO SessionId. Confirm the control: HTTP 400, Error.Code "ValidationError", Error.Message "Endpoint <name> of account <account> not found."
3. Repeat the same call adding SessionId="NEW_SESSION" (the sentinel is valid only on InvokeEndpoint, so this violates SessionIdHeader's pattern on the streaming operation).
4. Observe HTTP 400 ValidationError with Error.Message "1 validation error detected: Value 'NEW_SESSION' at 'sessionId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9](-*[a-zA-Z0-9])*" — compare against AWS's message, which ends with the model's pattern including the trailing '$': "^[a-zA-Z0-9](-*[a-zA-Z0-9])*$".
5. Generalise: violate any other patterned member on this service (e.g. EndpointName="bad_name" with an underscore, or TargetVariant="bad_variant") and confirm the rendered pattern is likewise missing its '^'/'$' anchors relative to the Smithy @pattern.
