Issue-Source: trace-sweep-divergence
Issue-Trace: sagemakerruntime-invoke-endpoint-async-rejections
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sagemakerruntime-invoke-endpoint-async-rejections sagemakerruntime-invoke-endpoint-response-stream-rejections sagemakerruntime-invoke-endpoint-validation-precedence
Issue-Shape-Id: com.amazonaws.sagemakerruntime#AmazonSageMakerRuntime
Issue-Key: sagemaker-runtime|InvokeEndpointAsync|4xx->4xx|$.ErrorCode
Issue-Key: sagemaker-runtime|InvokeEndpointAsync|4xx->4xx|$.Message
Issue-Key: sagemaker-runtime|InvokeEndpointAsync|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: sagemaker-runtime|InvokeEndpointWithResponseStream|4xx->4xx|$.ErrorCode
Issue-Key: sagemaker-runtime|InvokeEndpoint|4xx->4xx|$.ErrorCode
Issue-Key: sagemaker-runtime|InvokeEndpoint|4xx->4xx|$.Message
Issue-Key: sagemaker-runtime|InvokeEndpoint|4xx->4xx|$headers.x-amzn-errortype
Issue-Commit: c8db4453ab7530e781096e27f57c405b79639576

# sagemaker-runtime drops ErrorCode/Message from refusal bodies, misnames the error shape, and runs the endpoint lookup before required-member and Body/InputLocation gates (7 causes)

> _Note: this issue was identified against the code base at commit `c8db4453ab7530e781096e27f57c405b79639576`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InvokeEndpointAsync

## Description
Seven distinct root causes in the sagemaker-runtime data plane. All seven must be closed before submitting. Causes 1, 4 and 5 are a missing body member on the not-found path; Causes 2 and 6 are the constraint-refusal envelope; Causes 3 and 7 are validation ORDERING — the endpoint lookup runs ahead of request-level gates AWS evaluates first. Related but NON-COVERING open issues: 20261029T203709Z-sagemaker-runtime-request-constraint-refusals-use-error-code.md (behavioral corpus) covers the JSON body's error code and the sensitive-value echo and describes the Body/InputLocation disjunction, but carries none of these fixture replay keys and does not cover the missing ErrorCode member, the absent Message member, the x-amzn-errortype header text, or the required-Body gate; 20261024T183933Z-sagemaker-runtime-invokeendpoint-evaluates-no-caller-authorization-a.md is about authorization.

Cause 1 of 7: InvokeEndpointAsync endpoint-not-found refusals omit the ErrorCode member. Fixture sagemakerruntime-invoke-endpoint-async-rejections, step task#1.
AWS: recorded behavior only — ValidationError (Smithy com.amazonaws.sagemakerruntime#ValidationError, errorKind client, httpError 400) models only Message, yet the recorded endpoint-lookup refusal body carries Message "Endpoint <endpointName> of account <accountId> not found." AND the member ErrorCode with the fixed value "NO_SUCH_ENDPOINT". Fixed text is "Endpoint … of account … not found." and the literal "NO_SUCH_ENDPOINT"; the endpoint name comes from the request's EndpointName label and the account id from the caller's credentials.
Emulator: the body omits ErrorCode entirely ($.ErrorCode absent).
Why: ErrorCode is the only machine-readable discriminator separating "endpoint does not exist" from any other 400 ValidationError on this operation, so client logic that keys on it never fires and must substring-match English prose instead.
Fix: the InvokeEndpointAsync handler's endpoint-lookup refusal construction (shared with the sync and streaming handlers) must set the ErrorCode discriminator, and the restJson1 error serializer must carry it onto the wire.
Traces: sagemakerruntime-invoke-endpoint-async-rejections#task#1.

Cause 2 of 7: InvokeEndpointAsync InputLocation constraint refusals carry no Message and announce a non-existent error shape. Fixture sagemakerruntime-invoke-endpoint-async-rejections, step task#2.
AWS: InputLocation is @length 1..1024 with pattern ^(https|s3)://([^/]+)/?(.*)$ (InvokeEndpointAsyncInput$InputLocation, httpHeader X-Amzn-SageMaker-InputLocation). A violation is 400 ValidationError whose Message follows the standard template "1 validation error detected: Value '<value>' at '<memberName>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>" (fixed template text; the value is the request's header value, the member name is the member's lowerCamel wire name, the pattern is the modeled one), with response header x-amzn-errortype "ValidationError:http://internal.amazon.com/coral/com.amazonaws.sagemaker.runtime/" — the modeled shape name plus the service's Coral namespace suffix.
Emulator: the body has no Message member at all and x-amzn-errortype is "ValidationException", a shape that does not exist in com.amazonaws.sagemakerruntime and which carries no Coral namespace suffix.
Why: the SDK resolves its modeled exception from x-amzn-errortype, so ValidationError handlers are dead code and the caller sees a generic client error; with no Message the caller is not even told which member was malformed.
Fix: the request-validation layer must render the constraint message into the modeled ValidationError, and the sagemaker-runtime restJson1 error serializer must emit that shape's name with the Coral namespace suffix in x-amzn-errortype.
Traces: sagemakerruntime-invoke-endpoint-async-rejections#task#2.

Cause 3 of 7: InvokeEndpointAsync does not enforce the Body/InputLocation disjunction before resolving the endpoint. Fixture sagemakerruntime-invoke-endpoint-async-rejections, step task#3.
AWS: the model makes both Body (AsyncBodyBlob, httpPayload, @length 0..128000) and InputLocation optional but documents them as mutually exclusive — "Body and InputLocation are mutually exclusive. Provide exactly one of them." A request carrying NEITHER is refused 400 ValidationError with the message "One of Body or InputLocation is required" — entirely fixed text, with no request-derived part — and the rule is evaluated BEFORE the endpoint is resolved, so the refusal never mentions the endpoint.
Emulator: the disjunction is never checked; the request falls through to the endpoint lookup and is answered with the not-found message "Endpoint <endpointName> of account <accountId> not found." instead.
Why: a client that omitted its input source is told its endpoint does not exist, sending the operator to fix infrastructure rather than the request; and a caller that relies on the emulator to catch a malformed invocation ships code AWS rejects. The ordering is what makes the two outcomes distinguishable at all.
Fix: the InvokeEndpointAsync handler's request-validation step — add the exactly-one-of(Body, InputLocation) gate ahead of endpoint resolution, emitting ValidationError with the fixed message above.
Traces: sagemakerruntime-invoke-endpoint-async-rejections#task#3.

Cause 4 of 7: InvokeEndpointWithResponseStream endpoint-not-found refusals omit the ErrorCode member. Fixture sagemakerruntime-invoke-endpoint-response-stream-rejections, step task#1.
AWS: recorded behavior only — the streaming operation (POST /endpoints/{EndpointName}/invocations-response-stream) refuses an unknown endpoint with the same envelope as the other two: 400, ErrorCode "NO_SUCH_ENDPOINT", Message "Endpoint <endpointName> of account <accountId> not found."
Emulator: ErrorCode is absent from the streaming path's refusal body as well.
Why: a streaming client cannot distinguish an absent endpoint from any other validation refusal before it begins consuming the event stream.
Fix: the InvokeEndpointWithResponseStream handler's not-found refusal path / the shared not-found error constructor.
Traces: sagemakerruntime-invoke-endpoint-response-stream-rejections#task#1.

Cause 5 of 7: InvokeEndpoint endpoint-not-found refusals omit the ErrorCode member. Fixture sagemakerruntime-invoke-endpoint-validation-precedence, step task#1.
AWS: recorded behavior only — as Cause 1, on the synchronous operation (POST /endpoints/{EndpointName}/invocations): 400 with ErrorCode "NO_SUCH_ENDPOINT" and Message "Endpoint <endpointName> of account <accountId> not found."
Emulator: ErrorCode absent.
Why: as Cause 1 — the dominant refusal of the service's most-used operation is missing its discriminator.
Fix: the InvokeEndpoint handler's endpoint-lookup refusal construction.
Traces: sagemakerruntime-invoke-endpoint-validation-precedence#task#1.

Cause 6 of 7: InvokeEndpoint endpointName constraint refusals carry no Message and the wrong errortype. Fixture sagemakerruntime-invoke-endpoint-validation-precedence, step task#2.
AWS: EndpointName is required, httpLabel, @length 0..63, pattern ^[a-zA-Z0-9](-*[a-zA-Z0-9])*$ (InvokeEndpointInput$EndpointName). A name violating it is 400 ValidationError with Message "1 validation error detected: Value '<value>' at 'endpointName' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>" (fixed template; value from the request, member name endpointName, pattern as modeled) and x-amzn-errortype "ValidationError:http://internal.amazon.com/coral/com.amazonaws.sagemaker.runtime/".
Emulator: no Message member in the body, and x-amzn-errortype "ValidationException".
Why: same as Cause 2 — wrong error class on the wire and no indication of which member failed.
Fix: same component as Cause 2 (request-validation message rendering + error serializer naming).
Traces: sagemakerruntime-invoke-endpoint-validation-precedence#task#2.

Cause 7 of 7: InvokeEndpoint does not enforce the required Body member before resolving the endpoint. Fixture sagemakerruntime-invoke-endpoint-validation-precedence, step task#4.
AWS: Body is @required (clientOptional, httpPayload, @sensitive, BodyBlob @length 0..6291456) on InvokeEndpointInput. A request with no payload is refused 400 ValidationError with Message "1 validation error detected: Value at 'body' failed to satisfy constraint: Member must not be null" — fixed text, with the member name 'body' as the only member-derived part and NO value slot (the member is @sensitive, so no value is echoed) — and this required-member check runs BEFORE the endpoint lookup, so the message never names the endpoint.
Emulator: the absent payload is not checked; the request reaches the endpoint lookup and is answered with "Endpoint <endpointName> of account <accountId> not found." instead.
Why: a caller that forgot its payload is told the endpoint is missing, inverting the diagnosis; and validation precedence differs from AWS, so a request AWS rejects at the request layer is allowed to reach state lookup here.
Fix: the InvokeEndpoint handler's request-validation step — enforce the @required Body member (and the other request-shape constraints) ahead of endpoint resolution, with the sensitive-safe "Value at 'body' …" message form.
Traces: sagemakerruntime-invoke-endpoint-validation-precedence#task#4.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator: 7 divergence clusters on service token sagemaker-runtime, all verdict=mismatch with expected_status 400 and actual_status 400 — status is correct everywhere; the divergences are in the error body members, the error message text and the x-amzn-errortype header.

Cause 1 — sagemakerruntime-invoke-endpoint-async-rejections#task#1 (InvokeEndpointAsync): $.ErrorCode: missing — expected 'NO_SUCH_ENDPOINT' (fixed literal), got None.

Cause 2 — sagemakerruntime-invoke-endpoint-async-rejections#task#2 (InvokeEndpointAsync): $.Message: missing — expected "1 validation error detected: Value '<non-URI InputLocation>' at 'inputLocation' failed to satisfy constraint: Member must satisfy regular expression pattern: ^(https|s3)://([^/]+)/?(.*)$" (recorded value sim2real-probe-smr-not-a-uri-4b71d0e2 — the request's own header value; the template text and the member name/pattern are fixed), got None. $headers.x-amzn-errortype: shape_mismatch — expected 'ValidationError:http://internal.amazon.com/coral/com.amazonaws.sagemaker.runtime/', got 'ValidationException'. Cluster also contains #task#4.

Cause 3 — sagemakerruntime-invoke-endpoint-async-rejections#task#3 (InvokeEndpointAsync, neither Body nor InputLocation): $.Message: value_mismatch — expected 'One of Body or InputLocation is required' (wholly fixed text), got 'Endpoint sim2real-probe-smr-async-4b71d0e2 of account 000000000000 not found.' (the emulator's endpoint-lookup message, whose name/account are request- and credential-derived) — i.e. the disjunction gate never ran and the lookup did.

Cause 4 — sagemakerruntime-invoke-endpoint-response-stream-rejections#task#1 (InvokeEndpointWithResponseStream): $.ErrorCode: missing — expected 'NO_SUCH_ENDPOINT', got None.

Cause 5 — sagemakerruntime-invoke-endpoint-validation-precedence#task#1 (InvokeEndpoint): $.ErrorCode: missing — expected 'NO_SUCH_ENDPOINT', got None.

Cause 6 — sagemakerruntime-invoke-endpoint-validation-precedence#task#2 (InvokeEndpoint): $.Message: missing — expected "1 validation error detected: Value '<malformed endpoint name>' at 'endpointName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9](-*[a-zA-Z0-9])*" (recorded value sim2real_probe_smr_4b71d0e2 — underscores violate the modeled pattern), got None; $headers.x-amzn-errortype: shape_mismatch — expected 'ValidationError:http://internal.amazon.com/coral/com.amazonaws.sagemaker.runtime/', got 'ValidationException'. Cluster of 4: + #task#3, #task#5, #task#6.

Cause 7 — sagemakerruntime-invoke-endpoint-validation-precedence#task#4 (InvokeEndpoint, no payload): $.Message: value_mismatch — expected "1 validation error detected: Value at 'body' failed to satisfy constraint: Member must not be null" (fixed text; no value slot because Body is @sensitive), got 'Endpoint sim2real-probe-smr-absent-4b71d0e2 of account 000000000000 not found.' — the required-member gate never ran and the endpoint lookup answered instead.

Model evidence (services/service-com-amazonaws-sagemakerruntime-amazonsagemakerruntime/smithy-model/com-amazonaws-sagemakerruntime-amazonsagemakerruntime.json): sdkId "SageMaker Runtime", restJson1, sigv4; common errors InternalFailure(500), ServiceUnavailable(503), ValidationError(400); ValidationError = client error, httpError 400, members {Message @length 0..2048} — no ValidationException shape exists in the namespace and ErrorCode is unmodeled (recorded behavior only). InvokeEndpointInput$Body: httpPayload, clientOptional+required, @sensitive, @length 0..6291456. InvokeEndpointAsyncInput$Body: AsyncBodyBlob @length 0..128000, documented mutually exclusive with InputLocation (@length 1..1024, pattern ^(https|s3)://([^/]+)/?(.*)$). EndpointName: required httpLabel, @length 0..63, pattern ^[a-zA-Z0-9](-*[a-zA-Z0-9])*$.

## Steps to Reproduce
1. Boot the emulator and replay trace set simparity-wire-aws-v1 fixtures sagemakerruntime-invoke-endpoint-async-rejections, sagemakerruntime-invoke-endpoint-response-stream-rejections and sagemakerruntime-invoke-endpoint-validation-precedence, or drive the calls directly.
2. Causes 1, 4 and 5: call InvokeEndpointAsync, InvokeEndpointWithResponseStream and InvokeEndpoint with a well-formed EndpointName that was never created (supplying a valid input source where required). Each answers 400 with Message "Endpoint <name> of account <account> not found." but no ErrorCode member; AWS also returns ErrorCode "NO_SUCH_ENDPOINT".
3. Cause 2: call InvokeEndpointAsync with X-Amzn-SageMaker-InputLocation set to a bare string that is not an https:// or s3:// URI. Observe 400 with no Message member and x-amzn-errortype "ValidationException" instead of the constraint Message naming inputLocation and its pattern, with x-amzn-errortype "ValidationError:http://internal.amazon.com/coral/com.amazonaws.sagemaker.runtime/".
4. Cause 3: call InvokeEndpointAsync against a well-formed absent endpoint name with ContentType set and NEITHER Body nor InputLocation. Observe the endpoint-not-found message instead of ValidationError "One of Body or InputLocation is required". Control: either source alone must reach the endpoint lookup and produce the not-found message.
5. Cause 6: call InvokeEndpoint with an EndpointName containing underscores (violating ^[a-zA-Z0-9](-*[a-zA-Z0-9])*$). Observe 400 with no Message member and x-amzn-errortype "ValidationException".
6. Cause 7: call InvokeEndpoint against a well-formed absent endpoint name with NO request payload at all. Observe the endpoint-not-found message instead of ValidationError "1 validation error detected: Value at 'body' failed to satisfy constraint: Member must not be null" — and note the message must not echo a value, since Body is @sensitive.
7. Assert on $.ErrorCode, $.Message and the x-amzn-errortype header; the HTTP status (400) already matches in every case.
