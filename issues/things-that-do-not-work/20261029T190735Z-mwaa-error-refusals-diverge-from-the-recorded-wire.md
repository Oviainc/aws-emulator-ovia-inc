Issue-Source: trace-sweep-divergence
Issue-Trace: mwaa-create-cli-token-nonexistent-environment-not-found
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mwaa-create-cli-token-nonexistent-environment-not-found mwaa-create-web-login-token-nonexistent-environment-not-found mwaa-untag-resource-invalid-arn-format-rejected
Issue-Shape-Id: com.amazonaws.mwaa#AmazonMWAA
Issue-Key: mwaa|CreateCliToken|4xx->4xx|$
Issue-Key: mwaa|CreateWebLoginToken|4xx->4xx|$
Issue-Key: mwaa|UntagResource|4xx->4xx|$.message
Issue-Key: mwaa|UntagResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Commit: 69bb2c4e0c47afe7ac06c67651974f0cf752730a

# MWAA error refusals diverge from the recorded wire on three operations

> _Note: this issue was identified against the code base at commit `69bb2c4e0c47afe7ac06c67651974f0cf752730a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCliToken

## Description
Replay of the wire corpus (trace set simparity-wire-aws-v1) leaves 3 of 19 steps mismatched, all of them on this service and all of them the SAME class of fault: the status is right, but the refusal's BODY does not carry the error document AWS publishes for it. Two distinct root causes are bundled here, one of them shared by two operations. THIS ISSUE HAS 3 CAUSES; all three must be closed before submitting.

Cause 1 of 3: CreateCliToken, 404 on an environment that does not exist.
AWS: Smithy (services/service-com-amazonaws-mwaa-amazonmwaa/smithy-model/com-amazonaws-mwaa-amazonmwaa.json) models CreateCliToken POST /clitoken/{Name} with exactly one error, ResourceNotFoundException @httpError 404, a structure whose single member is `message` (String) -- doc "ResourceNotFoundException: The resource is not available." The service is restJson1, so the refusal is a JSON error DOCUMENT: a JSON object carrying the sentence in its `message` member. The recorded response confirms it: status 404 with body {"message": "Environment not found"}. Fixed text of that sentence is "Environment not found"; it names no value from the request, so there is no variable part to fill in here (the environment name the request asked for does not appear in it).
Emulator: answers 404 with the body `"Environment not found"` -- a bare JSON string, not an object. Replay detail: `$: type_mismatch (expected {'message': 'Environment not found'}, got 'Environment not found')`. The handler does raise the modelled ResourceNotFoundException correctly; the envelope is then STRIPPED after rendering by the env-endpoint front door (see Traces), which deliberately replaces the whole body with the sentence alone and claims the recorded wire shows no JSON envelope. The corpus shows the opposite.
Why: a client gets no parseable error document. An SDK deserialising restJson1 ResourceNotFoundException reads `message` out of a JSON object; given a bare string the document has no `message` member, so `e.message()` is empty (or the parse is reported as a protocol error) and any code that logs or branches on the reason text loses it. Code-based dispatch survives only because the x-amzn-errortype header happens to be left alone -- the body contract itself is broken.
Fix: the response post-processing in the service layer -- the env-endpoint front door that rewrites a rendered 404 body (`fronted` / `front_door_sentence`). The modelled error's own serializer already produces the correct document, so this cause is the extra reshaping step, not the handler.
Traces: mwaa-create-cli-token-nonexistent-environment-not-found#task#1. Code: services/service-com-amazonaws-mwaa-amazonmwaa/src/service/env_endpoint.rs:1-52 (the module's stated rule and its claimed recorded wire), :106-126 (`front_door_sentence`), :134-153 (`fronted` replaces the body and rewrites content-length), :65-66 (ENVIRONMENT_DATA_OPERATIONS, the three operations it is applied to), :248-268 and :366-372 (the unit tests that pin the stripped body).

Cause 2 of 3: CreateWebLoginToken, 404 on an environment that does not exist -- the same defect through the same front door, on a second operation.
AWS: Smithy models CreateWebLoginToken POST /webtoken/{Name} with ResourceNotFoundException @httpError 404 (also AccessDeniedException 403, ValidationException 400, InternalServerException 500), ResourceNotFoundException again being a structure whose single member is `message`. restJson1, so the 404 body is a JSON object; the recorded response is 404 with {"message": "Environment not found"} -- fixed text, no request-derived part.
Emulator: 404 with the bare JSON string `"Environment not found"`. Replay detail: `$: type_mismatch (expected {'message': 'Environment not found'}, got 'Environment not found')`.
Why: same as cause 1 -- the modelled error document is not on the wire, so a client reading `message` gets nothing. It is listed as its own cause because it is its own operation and its own fixture step: a fix verified only against CreateCliToken must still be re-replayed here, and because InvokeRestApi is routed through the same front door its 404 should be checked for the same reshaping.
Fix: same component as cause 1 (the env-endpoint front door); the correct outcome is that the renderer's error document reaches the client unmodified for every operation in ENVIRONMENT_DATA_OPERATIONS.
Traces: mwaa-create-web-login-token-nonexistent-environment-not-found#task#1. Code: services/service-com-amazonaws-mwaa-amazonmwaa/src/service/env_endpoint.rs:134-153, :198-215 (EnvironmentDataEndpoint::run applies `fronted` to every env.-hosted operation).

Cause 3 of 3: UntagResource with a ResourceArn that is not an MWAA ARN -- refused with the generic boundary-validator sentence and a different error code than the recording.
AWS: Smithy models UntagResource DELETE /tags/{ResourceArn} with ResourceArn @httpLabel, required, EnvironmentArn length 1..1224, pattern ^arn:aws(-[a-z]+)?:airflow:[a-z0-9\-]+:\d{12}:environment/\w+, and errors ValidationException 400 / ResourceNotFoundException 404 / InternalServerException 500. The recorded refusal is HTTP 400 with message "Invalid input resource arn: bad syntax": fixed text "Invalid input resource arn: " followed by the parse reason for the value supplied ("bad syntax" where the value is not an ARN at all) -- it quotes neither the offending value, nor the member name, nor the pattern. The recorded x-amzn-errortype is NOT the bare "ValidationException" the emulator sends (the replay reports it as a shape mismatch against the recorded value). Hypothesis: MWAA's /tags/{ResourceArn} operations publish wire error codes that differ from their modelled shape names -- this crate already documents exactly that for their not-found class (the model declares ResourceNotFoundException, the service publishes NotFoundException; see scaffolded.rs:288-301) -- so the invalid-ARN refusal most likely publishes a tagging-front-end code rather than the modelled shape name. The fixer must read the recorded header value off the fixture rather than guess it.
Emulator: HTTP 400 with the framework boundary validator's generic sentence -- `1 validation error detected: Value 'not-an-mwaa-arn' at 'resourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:aws(-[a-z]+)?:airflow:[a-z0-9\-]+:\d{12}:environment/\w+` -- and x-amzn-errortype: ValidationException. Replay detail: `$.message: value_mismatch (expected 'Invalid input resource arn: bad syntax', got "1 validation error detected: ...")` plus `$headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'ValidationException')`. The malformed ARN never reaches the handler: the @pattern constraint is enforced at the boundary, so the handler's own refusal wording is never consulted. Note the generic text also lowercases the member as 'resourceArn' while the model names the member ResourceArn, and it echoes the pattern back to the caller, neither of which AWS does.
Why: a client that matches on the published error code does not see the code it dispatches on, so an invalid-ARN refusal is misclassified (and SDK-level retry/handling branches on it); and a client that surfaces the message shows a Smithy constraint dump instead of MWAA's one-line reason, with the caller's own value and the service's internal regex leaked into it.
Fix: UntagResource's input validation -- the ARN-shaped refusal must be raised by the operation (or a service-level pre-handler for the /tags/{ResourceArn} family) with MWAA's own wording and published code, instead of being left to the generic boundary pattern validator. TagResource and ListTagsForResource take the same ResourceArn label through the same boundary, so they word this condition the same way and should be fixed together. The error-code side belongs in the same place the crate already keeps a published-vs-modelled code mapping for this family (crate::scaffolded).
Traces: mwaa-untag-resource-invalid-arn-format-rejected#task#1. Code: services/service-com-amazonaws-mwaa-amazonmwaa/src/ops/untag_resource.rs:145-199 (the handler, which the malformed ARN never reaches), :62-65 (the modelled pattern enforced at the boundary); services/service-com-amazonaws-mwaa-amazonmwaa/src/scaffolded.rs:288-301 and :355-377 (the existing published-vs-modelled wire-code seam for this operation family).

Not covered by any open issue: the nearest one, 20261029T135357Z-mwaa-publishmetrics-error-response.md, is about which WORDING helper an ops.-hosted PublishMetrics reaches for and carries no replay key for these steps; it does not touch the 404 body shape of the env.-hosted token operations nor the invalid-ARN refusal of the tagging operations.

## Evidence
Replay summary: trace_set_id simparity-wire-aws-v1, emulator_up true, bootstrap_signal false, 19 steps, 16 passed, 3 mismatched, score 84.2%, divergence_count 3 -- all three on service mwaa, none carrying stub_path or owned_by.

1) fixture mwaa-create-cli-token-nonexistent-environment-not-found, step task#1, CreateCliToken, verdict mismatch, expected_status 404 / actual_status 404.
   Expected body: {"message": "Environment not found"}
   Actual body:   "Environment not found"   (a bare JSON string)
   detail: `$: type_mismatch (expected {'message': 'Environment not found'}, got 'Environment not found')`
   divergence_key: service=mwaa|CreateCliToken|status=404->404|keys=$

2) fixture mwaa-create-web-login-token-nonexistent-environment-not-found, step task#1, CreateWebLoginToken, verdict mismatch, expected_status 404 / actual_status 404.
   Expected body: {"message": "Environment not found"}
   Actual body:   "Environment not found"
   detail: `$: type_mismatch (expected {'message': 'Environment not found'}, got 'Environment not found')`
   divergence_key: service=mwaa|CreateWebLoginToken|status=404->404|keys=$

3) fixture mwaa-untag-resource-invalid-arn-format-rejected, step task#1, UntagResource, verdict mismatch, expected_status 400 / actual_status 400.
   Expected message: 'Invalid input resource arn: bad syntax'
   Actual message:   "1 validation error detected: Value 'not-an-mwaa-arn' at 'resourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:aws(-[a-z]+)?:airflow:[a-z0-9\-]+:\d{12}:environment/\w+"
   Expected x-amzn-errortype: a recorded value that does not match the actual; actual 'ValidationException'
   detail: `$.message: value_mismatch (...)` and `$headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'ValidationException')`
   divergence_key: service=mwaa|UntagResource|status=400->400|keys=$.message,$headers.x-amzn-errortype

Model evidence (smithy_query, model_path services/service-com-amazonaws-mwaa-amazonmwaa/smithy-model/com-amazonaws-mwaa-amazonmwaa.json):
  CreateCliToken -> errors [{ResourceNotFoundException, client, httpError 404}], http POST /clitoken/{Name}, endpoint hostPrefix env.
  CreateWebLoginToken -> errors [AccessDeniedException 403, InternalServerException 500, ResourceNotFoundException 404, ValidationException 400], http POST /webtoken/{Name}, endpoint hostPrefix env.
  UntagResource -> ResourceArn httpLabel/required, pattern ^arn:aws(-[a-z]+)?:airflow:[a-z0-9\-]+:\d{12}:environment/\w+, errors [InternalServerException 500, ResourceNotFoundException 404, ValidationException 400], http DELETE /tags/{ResourceArn}, endpoint hostPrefix api.
  shape ResourceNotFoundException -> structure, httpError 404, members {message: String}
  shape ValidationException -> structure, httpError 400, members {message: String}
So the modelled 404 for both token operations is a structure with a `message` member, i.e. a JSON object on restJson1 -- exactly what the fixtures recorded and not what the emulator sends.

Code evidence for causes 1-2 -- the reshaping is deliberate and documented on a premise the corpus contradicts, services/service-com-amazonaws-mwaa-amazonmwaa/src/service/env_endpoint.rs:12-28:
    //! carries the failure SENTENCE as its whole body, as the recorded wire shows:
    //! HTTP/1.1 404 Not Found
    //! x-amzn-errortype: ResourceNotFoundException
    //! Environment not found
    //! No JSON envelope around it and no discriminator member inside it
and :151 `*resp.body_mut() = SdkBody::from(sentence);` applied to every response of ENVIRONMENT_DATA_OPERATIONS (:65-66, :198-215). The replayed fixtures for both token operations record the JSON envelope, so the premise behind `fronted` is wrong and its unit tests (:248-268, :366-372) pin the wrong body.

Code evidence for cause 3: services/service-com-amazonaws-mwaa-amazonmwaa/src/ops/untag_resource.rs:145-199 shows the handler refuses reserved tag keys, an unresolvable ARN (404) and a non-changeable environment, but has no branch for a syntactically invalid ARN -- that value is rejected at the boundary by the modelled @pattern, which is where the generic '1 validation error detected: ...' text comes from. services/service-com-amazonaws-mwaa-amazonmwaa/src/scaffolded.rs:288-301 documents that this very operation family publishes a wire code other than its modelled shape name for its not-found class, which is the seam the invalid-ARN code belongs in too.

## Steps to Reproduce
Cause 1: replay fixture mwaa-create-cli-token-nonexistent-environment-not-found, step task#1 -- POST /clitoken/{Name} on the env. endpoint with a Name matching ^[a-zA-Z][0-9a-zA-Z-_]*$ that no environment uses. Expect HTTP 404 whose body parses as a JSON object with a `message` member; observe HTTP 404 whose body is the bare JSON string "Environment not found".

Cause 2: replay fixture mwaa-create-web-login-token-nonexistent-environment-not-found, step task#1 -- POST /webtoken/{Name} for an environment that does not exist. Same expectation, same bare-string body. (Check InvokeRestApi's 404 as well: env_endpoint.rs:65-66 routes it through the same `fronted` call.)

Cause 3: replay fixture mwaa-untag-resource-invalid-arn-format-rejected, step task#1 -- DELETE /tags/not-an-mwaa-arn?tagKeys=<key> on the api. endpoint, i.e. a ResourceArn that does not match the modelled EnvironmentArn pattern. Expect HTTP 400 with message "Invalid input resource arn: " + the parse reason and the x-amzn-errortype value the fixture records; observe HTTP 400 with the boundary validator's "1 validation error detected: Value '<the value sent>' at 'resourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: <the model's regex>" and x-amzn-errortype: ValidationException. Read the recorded header value off the fixture to get the published code.

All three: re-run replay_wire_corpus() after the fix; the corpus must reach 19/19 passed (it is 16/19, score 84.2%, before).
