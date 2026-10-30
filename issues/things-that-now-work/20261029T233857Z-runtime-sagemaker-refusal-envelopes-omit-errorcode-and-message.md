Issue-Source: trace-sweep-divergence
Issue-Trace: sagemaker-runtime-invoke-endpoint-async-invalid-input
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sagemaker-runtime-invoke-endpoint-async-invalid-input sagemaker-runtime-invoke-endpoint-async-s3-workflow sagemaker-runtime-invoke-endpoint-basic-not-found sagemaker-runtime-invoke-endpoint-body-boundaries sagemaker-runtime-invoke-endpoint-multi-model-variant sagemaker-runtime-invoke-endpoint-stateful-session sagemaker-runtime-invoke-endpoint-invalid-name-pattern sagemaker-runtime-invoke-endpoint-response-stream
Issue-Shape-Id: com.amazonaws.sagemakerruntime#AmazonSageMakerRuntime
Issue-Key: runtime.sagemaker|InvokeEndpointAsync|4xx->4xx|$.Message
Issue-Key: runtime.sagemaker|InvokeEndpointAsync|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: runtime.sagemaker|InvokeEndpointAsync|4xx->4xx|$.ErrorCode
Issue-Key: runtime.sagemaker|InvokeEndpoint|4xx->4xx|$.ErrorCode
Issue-Key: runtime.sagemaker|InvokeEndpoint|4xx->4xx|$.Message
Issue-Key: runtime.sagemaker|InvokeEndpoint|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: runtime.sagemaker|InvokeEndpointWithResponseStream|4xx->4xx|$.ErrorCode
Issue-Commit: c8db4453ab7530e781096e27f57c405b79639576

# runtime.sagemaker refusal envelopes omit ErrorCode and Message and name ValidationException instead of ValidationError (6 causes)

> _Note: this issue was identified against the code base at commit `c8db4453ab7530e781096e27f57c405b79639576`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InvokeEndpoint

## Description
Six distinct root causes in the SageMaker Runtime data plane as reached on the runtime.sagemaker endpoint. All six must be closed before submitting. Every one of them is an error-response-shape defect: the HTTP status is already right (400), but the body members and the x-amzn-errortype header a client branches on are not. Related but NON-COVERING open issues (neither carries these replay keys): 20261029T203709Z-sagemaker-runtime-request-constraint-refusals-use-error-code.md is a behavioral-corpus issue about the error CODE in the JSON body and the sensitive-value echo, and 20261024T183933Z-sagemaker-runtime-invokeendpoint-evaluates-no-caller-authorization-a.md is about missing authorization. The two defects filed here that overlap it in spirit — the errortype header naming and the Body/InputLocation ordering — are a different observable (the HTTP header, and the wire-level message) and are not gated by those issues' keys.

Cause 1 of 6: InvokeEndpointAsync constraint-violation refusals carry no Message member and announce the wrong error shape in x-amzn-errortype. Fixture sagemaker-runtime-invoke-endpoint-async-invalid-input, step task#1.
AWS: InputLocation is modeled with pattern ^(https|s3)://([^/]+)/?(.*)$ (Smithy com.amazonaws.sagemakerruntime#InvokeEndpointAsyncInput$InputLocation, @length 1..1024, httpHeader X-Amzn-SageMaker-InputLocation). A violation is HTTP 400 ValidationError (the only client validation error the service models; ValidationError's single member is Message, @length 0..2048). The body carries Message with the standard constraint template "1 validation error detected: Value '<value>' at '<memberName>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>" — fixed text is the template; the value comes from the request header, the member name is the lowerCamel wire name of the violated member, and the pattern is the member's modeled pattern. The response header x-amzn-errortype is "ValidationError:http://internal.amazon.com/coral/com.amazonaws.sagemaker.runtime/" — the modeled shape name followed by the service's Coral namespace suffix.
Emulator: the same request answers 400 with NO Message member at all ($.Message absent) and x-amzn-errortype "ValidationException" — a shape that does not exist anywhere in com.amazonaws.sagemakerruntime, and with no Coral namespace suffix.
Why: an SDK resolves the modeled exception class from x-amzn-errortype, so the client never materializes ValidationError and surfaces an opaque ClientError; and because the body has no Message, the caller is told nothing about which member was malformed — the refusal is unactionable and no client-side assertion on the message can pass.
Fix: the service's restJson1 error serializer for sagemaker-runtime (the component that renders a modeled client error into status + x-amzn-errortype + body) must emit the modeled shape name with the Coral namespace suffix and populate the ValidationError Message member; and the request-validation layer must hand it the rendered constraint message rather than an empty generic envelope.
Traces: sagemaker-runtime-invoke-endpoint-async-invalid-input#task#1.

Cause 2 of 6: InvokeEndpointAsync endpoint-not-found refusals omit the ErrorCode member. Fixture sagemaker-runtime-invoke-endpoint-async-invalid-input, step task#2.
AWS: recorded behavior only — ValidationError models only Message, but the recording shows the service's endpoint-lookup refusal body additionally carrying the member ErrorCode with the fixed value "NO_SUCH_ENDPOINT" alongside the Message "Endpoint <endpointName> of account <accountId> not found." (fixed text "Endpoint … of account … not found."; the endpoint name comes from the request's EndpointName label and the account id from the caller's credentials). ErrorCode is a fixed discriminator, not a per-run value.
Emulator: the body omits ErrorCode entirely ($.ErrorCode absent); only the Message is present.
Why: ErrorCode is the only machine-readable discriminator that distinguishes "this endpoint does not exist" from every other 400 ValidationError on this operation. A client that branches on it (retry-vs-provision logic, negative tests, dashboards) sees nothing and must fall back to substring-matching English prose.
Fix: the InvokeEndpointAsync handler's endpoint-lookup refusal path — it must populate the ErrorCode discriminator on the not-found error it constructs, and the error serializer must carry that member onto the wire.
Traces: sagemaker-runtime-invoke-endpoint-async-invalid-input#task#2.

Cause 3 of 6: InvokeEndpoint endpoint-not-found refusals omit the ErrorCode member. Fixture sagemaker-runtime-invoke-endpoint-basic-not-found, step task#1.
AWS: recorded behavior only — same envelope as Cause 2: 400 with Message "Endpoint <endpointName> of account <accountId> not found." and ErrorCode "NO_SUCH_ENDPOINT". This is the dominant refusal of the operation (Smithy InvokeEndpoint, POST /endpoints/{EndpointName}/invocations) whenever EndpointName names no deployed endpoint.
Emulator: ErrorCode is absent from the body on the synchronous invoke path too, across the basic not-found, multi-model/variant and stateful-session request shapes.
Why: as Cause 2 — the not-found condition is indistinguishable from any other validation refusal without parsing prose, so client code that keys on ErrorCode is dead code against the emulator.
Fix: the InvokeEndpoint handler's endpoint-lookup refusal path (shared helper with the async and streaming handlers is the natural home) must set the ErrorCode discriminator.
Traces: sagemaker-runtime-invoke-endpoint-basic-not-found#task#1 (cluster also covers sagemaker-runtime-invoke-endpoint-body-boundaries#task#2, sagemaker-runtime-invoke-endpoint-multi-model-variant#task#1 and #task#2, sagemaker-runtime-invoke-endpoint-stateful-session#task#1).

Cause 4 of 6: an InvokeEndpoint request at the Body size boundary is refused with the empty generic validation envelope instead of reaching the endpoint lookup. Fixture sagemaker-runtime-invoke-endpoint-body-boundaries, step task#1.
AWS: BodyBlob is @length 0..6291456 and @sensitive (Smithy InvokeEndpointInput$Body, httpPayload). A payload inside that ceiling is a well-formed request, so validation passes and the endpoint lookup runs, producing 400 ValidationError with ErrorCode "NO_SUCH_ENDPOINT" and Message "Endpoint <endpointName> of account <accountId> not found.", header x-amzn-errortype "ValidationError:http://internal.amazon.com/coral/com.amazonaws.sagemaker.runtime/".
Emulator: the request is refused by the generic constraint layer instead — x-amzn-errortype "ValidationException" with neither ErrorCode nor Message in the body — so the caller cannot tell that the real condition is a missing endpoint, and a payload AWS accepts for validation purposes is treated as a constraint violation.
Why: the Body length gate and the endpoint lookup are ordered and sized differently from AWS, so a client probing its payload ceiling gets a refusal that names neither the constraint nor the endpoint; capacity-planning and chunking logic calibrated on the emulator will not match AWS.
Fix: the InvokeEndpoint handler's validation step — the Body length bound must match the modeled 6291456-byte ceiling and a request inside it must fall through to the endpoint lookup, whose refusal is rendered as the modeled ValidationError (Causes 1 and 3).
Traces: sagemaker-runtime-invoke-endpoint-body-boundaries#task#1.

Cause 5 of 6: InvokeEndpoint endpointName pattern refusals carry no Message and the wrong errortype. Fixture sagemaker-runtime-invoke-endpoint-invalid-name-pattern, step task#1.
AWS: EndpointName is @length 0..63 with pattern ^[a-zA-Z0-9](-*[a-zA-Z0-9])*$ (Smithy InvokeEndpointInput$EndpointName, httpLabel, required). A name violating it is 400 ValidationError with Message on the standard constraint template — "1 validation error detected: Value '<value>' at 'endpointName' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>" — where the fixed text is the template, the quoted value is the request's own EndpointName, 'endpointName' is the member's wire name and the pattern is as the model declares it; header x-amzn-errortype "ValidationError:http://internal.amazon.com/coral/com.amazonaws.sagemaker.runtime/".
Emulator: body has no Message member and the header says "ValidationException". Same for the SessionId sentinel path on the stateful-session fixture.
Why: the caller learns neither that the name is malformed nor which member failed; and the error class it resolves does not exist in this service's model.
Fix: same component as Cause 1 — the request-validation layer must render the constraint message into the modeled ValidationError and the serializer must name that shape (with its Coral namespace) in x-amzn-errortype.
Traces: sagemaker-runtime-invoke-endpoint-invalid-name-pattern#task#1 (cluster also covers #task#2 and sagemaker-runtime-invoke-endpoint-stateful-session#task#2).

Cause 6 of 6: InvokeEndpointWithResponseStream endpoint-not-found refusals omit the ErrorCode member. Fixture sagemaker-runtime-invoke-endpoint-response-stream, step task#1.
AWS: recorded behavior only — the streaming operation (Smithy InvokeEndpointWithResponseStream, POST /endpoints/{EndpointName}/invocations-response-stream) refuses an unknown endpoint with the same envelope as the other two: 400, ErrorCode "NO_SUCH_ENDPOINT", Message "Endpoint <endpointName> of account <accountId> not found."
Emulator: ErrorCode is absent, so the streaming path diverges exactly as the other two do — confirming the discriminator is missing from the shared refusal construction rather than from one handler.
Why: as Causes 2 and 3; a streaming client cannot distinguish an absent endpoint from a malformed request before it starts consuming the event stream.
Fix: the InvokeEndpointWithResponseStream handler's endpoint-lookup refusal path / the shared not-found error constructor.
Traces: sagemaker-runtime-invoke-endpoint-response-stream#task#1.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator: 6 divergence clusters on service token runtime.sagemaker, all verdict=mismatch, expected_status 400 and actual_status 400 — the divergences are entirely in the error body members and the x-amzn-errortype header.

Cause 1 — sagemaker-runtime-invoke-endpoint-async-invalid-input#task#1 (InvokeEndpointAsync):
  $.Message: missing — expected "1 validation error detected: Value 'ftp://<bucket>/input/payload.json' at 'inputLocation' failed to satisfy constraint: Member must satisfy regular expression pattern: ^(https|s3)://([^/]+)/?(.*)$", got None. (Fixed text: the "1 validation error detected: Value '…' at '…' failed to satisfy constraint: Member must satisfy regular expression pattern: …" template. Variable: the InputLocation value from the request header, the member name inputLocation, and the modeled pattern.)
  $headers.x-amzn-errortype: value_mismatch — expected 'ValidationError:http://internal.amazon.com/coral/com.amazonaws.sagemaker.runtime/', got 'ValidationException'.

Cause 2 — sagemaker-runtime-invoke-endpoint-async-invalid-input#task#2 (InvokeEndpointAsync): $.ErrorCode: missing — expected 'NO_SUCH_ENDPOINT', got None. (ErrorCode is fixed text; the Message's endpoint name and account id are request/credential-derived.) Cluster also contains sagemaker-runtime-invoke-endpoint-async-s3-workflow#task#1.

Cause 3 — sagemaker-runtime-invoke-endpoint-basic-not-found#task#1 (InvokeEndpoint): $.ErrorCode: missing — expected 'NO_SUCH_ENDPOINT', got None. Cluster of 5: + sagemaker-runtime-invoke-endpoint-body-boundaries#task#2, sagemaker-runtime-invoke-endpoint-multi-model-variant#task#1, #task#2, sagemaker-runtime-invoke-endpoint-stateful-session#task#1.

Cause 4 — sagemaker-runtime-invoke-endpoint-body-boundaries#task#1 (InvokeEndpoint): $.ErrorCode: missing (expected 'NO_SUCH_ENDPOINT', got None); $.Message: missing (expected 'Endpoint <endpointName> of account <accountId> not found.' — recorded as "Endpoint sim2real-probe-sagemaker-runtime-bodysz-68ba0ee9 of account 000000000000 not found.", where the name is the request's and the account is the caller's — got None); $headers.x-amzn-errortype: value_mismatch (expected 'ValidationError:http://internal.amazon.com/coral/com.amazonaws.sagemaker.runtime/', got 'ValidationException').

Cause 5 — sagemaker-runtime-invoke-endpoint-invalid-name-pattern#task#1 (InvokeEndpoint): $.Message: missing — expected "1 validation error detected: Value '<malformed endpoint name>' at 'endpointName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9](-*[a-zA-Z0-9])*" (recorded value sim2real_probe_sagemaker_runtime_bad_e1ce3666 — underscores violate the modeled pattern), got None; $headers.x-amzn-errortype: value_mismatch — expected 'ValidationError:http://internal.amazon.com/coral/com.amazonaws.sagemaker.runtime/', got 'ValidationException'. Cluster of 3: + #task#2 and sagemaker-runtime-invoke-endpoint-stateful-session#task#2.

Cause 6 — sagemaker-runtime-invoke-endpoint-response-stream#task#1 (InvokeEndpointWithResponseStream): $.ErrorCode: missing — expected 'NO_SUCH_ENDPOINT', got None.

Model evidence (services/service-com-amazonaws-sagemakerruntime-amazonsagemakerruntime/smithy-model/com-amazonaws-sagemakerruntime-amazonsagemakerruntime.json): sdkId "SageMaker Runtime", protocol restJson1, 3 operations; common errors InternalFailure(500), ServiceUnavailable(503), ValidationError(400). ValidationError is errorKind=client, httpError 400, members {Message: @length 0..2048} — there is NO ValidationException shape in com.amazonaws.sagemakerruntime, and ErrorCode is not modeled (hence "recorded behavior only" for that member). EndpointName: required httpLabel, @length 0..63, pattern ^[a-zA-Z0-9](-*[a-zA-Z0-9])*$. InvokeEndpointAsyncInput$InputLocation: @length 1..1024, pattern ^(https|s3)://([^/]+)/?(.*)$. InvokeEndpointInput$Body: httpPayload, @sensitive, @length 0..6291456.

## Steps to Reproduce
1. Boot the emulator and replay trace set simparity-wire-aws-v1 (fixtures sagemaker-runtime-invoke-endpoint-*), or drive the calls directly against the runtime.sagemaker endpoint.
2. Cause 1: POST /endpoints/<well-formed-but-absent-name>/async-invocations with header X-Amzn-SageMaker-InputLocation set to a URI whose scheme is neither https nor s3 (e.g. ftp://<bucket>/input/payload.json). Observe 400 with an empty-of-Message body and x-amzn-errortype "ValidationException"; AWS returns the constraint Message quoting inputLocation and its modeled pattern, with x-amzn-errortype "ValidationError:http://internal.amazon.com/coral/com.amazonaws.sagemaker.runtime/".
3. Causes 2, 3 and 6: invoke InvokeEndpointAsync, InvokeEndpoint and InvokeEndpointWithResponseStream against a well-formed endpoint name that was never created (plain, multi-model/TargetVariant and stateful-session request shapes all qualify). Observe 400 with Message "Endpoint <name> of account <account> not found." but NO ErrorCode member; AWS also returns ErrorCode "NO_SUCH_ENDPOINT".
4. Cause 4: call InvokeEndpoint on an absent endpoint with a Body at the upper end of the modeled 0..6291456-byte range. Observe the generic 400 "ValidationException" with neither ErrorCode nor Message instead of the ValidationError endpoint-not-found envelope.
5. Cause 5: call InvokeEndpoint with an EndpointName containing underscores (violating ^[a-zA-Z0-9](-*[a-zA-Z0-9])*$), and separately with an X-Amzn-SageMaker-Session-Id that is not NEW_SESSION and not pattern-conformant. Observe 400 with no Message member and x-amzn-errortype "ValidationException".
6. In each case compare against the recorded response: status matches, so assert on $.ErrorCode, $.Message and the x-amzn-errortype header.
