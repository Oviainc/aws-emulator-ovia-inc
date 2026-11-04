Issue-Source: trace-sweep-divergence
Issue-Trace: lambda-cfnuf-alias-repair-attempts-in-lambda
Issue-Step: task#15
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lambda-cfnuf-alias-repair-attempts-in-lambda lam-err-capacity-provider-duplicate-create lambda-batch-diag-submit-wrapper-function-handle-mistakes lambda-function-invocation-paths lambda-lsrno-traj-invoke-reproduces-timeout lambda-invoke-modes-comparison lambda-x2x-xlsx-layer-attach-mistakes-and-size-ceiling
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda|DeleteFunction|2xx->4xx|status
Issue-Key: lambda|CreateCapacityProvider|4xx->4xx|$.message
Issue-Key: lambda|Invoke|4xx->4xx|$.message
Issue-Key: lambda|InvokeAsync|4xx->4xx|$.message
Issue-Key: lambda|GetFunctionConfiguration|2xx->2xx|$.Layers[*].UncompressedCodeSize
Issue-Related: 20261102T111455Z-lambda-runtimeversionarn-carries-a-short-non-aws-identifier.md
Issue-Commit: f5f38caeaa03e56870175fb074275b9cd1e72c06

# lambda: 5 net-new causes — DeleteFunction answers 404 for an existing version, and refusal messages plus layer size metadata diverge

> _Note: this issue was identified against the code base at commit `f5f38caeaa03e56870175fb074275b9cd1e72c06`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteFunction

## Description
Five net-new wire divergences in the `lambda` service, all of them the emulator answering a correct-shaped response whose status, error text or payload field is not what AWS returns. They are bundled because they are all in the lambda function/capacity-provider surface and share one service token; each is an independent root cause and ALL FIVE must be fixed and re-replayed before this issue is closed.

--- Cause 1 of 5: DeleteFunction answers 404 where AWS deletes and answers 204 ---
AWS: Smithy: DeleteFunction, DELETE /2015-03-31/functions/{FunctionName} with optional `Qualifier` query, output has only the httpResponseCode member; the recorded response is 204 No Content with an empty body. Modeled errors are InvalidParameterValueException(400), ResourceConflictException(409), ResourceNotFoundException(404). AWS deletes the named function (or the named qualified version) and returns 204 as long as that identifier still resolves.
Emulator: the same DeleteFunction request returns 404 (ResourceNotFoundException) — the identifier the client deletes does not resolve in emulator state.
Hypothesis: in this fixture the emulator's version ledger has already drifted from AWS's by this step. At an earlier step of the same fixture AWS refused to delete a version an alias still referenced (409 ResourceConflictException, "Unable to delete version because the following aliases reference it: [<alias names>]") while the emulator deleted it with 204, after which the emulator's PublishVersion numbering ran one ahead of AWS's. The later delete therefore targets a version the emulator never had or already removed. That earlier unguarded delete is already filed as "Lambda DeleteFunction deletes a version an alias still references" (20261103T165236Z-lambda-deletefunction-deletes-a-version-an-alias-still.md) and the version-numbering drift as "Lambda function and version read payloads carry wrong identity and code metadata" (20261103T165236Z-lambda-function-and-version-read-payloads-carry-wrong.md); NEITHER covers this step — this cluster is reported un-owned, and what is wrong here is the 404 on a delete AWS completes.
Why: a client that deletes a function/version it just created or published gets ResourceNotFoundException instead of a successful 204. Cleanup and teardown paths (CloudFormation-style delete, test fixtures, idempotent re-delete loops) treat that as a hard failure, and any flow that deletes by version number after a publish is unusable.
Fix: the DeleteFunction handler's function/qualifier resolution and the version state it reads, in the lambda service crate — the delete must resolve the same identifier space PublishVersion minted, and must only answer 404 when no such function or version exists.
Traces: lambda-cfnuf-alias-repair-attempts-in-lambda#task#15 (expected 204, got 404).

--- Cause 2 of 5: CreateCapacityProvider duplicate-name refusal uses emulator-authored text ---
AWS: Smithy: CreateCapacityProvider, POST /2025-11-30/capacity-providers, error ResourceConflictException (httpError 409, members Type + message). Status and code match; only the message text diverges. AWS's message for this class is the fixed sentence pair "A capacity provider with ARN <capacityProviderArn> already exists. Capacity provider names must be unique within your account." — the only variable part is the ARN of the already-existing capacity provider, taken from the request name and the emulator's own account/region/state. Everything else is fixed text.
Emulator: answers 409 with `message` = "Capacity provider already exists: <arn>" — a terse emulator-authored sentence that keeps only the ARN and drops both of AWS's sentences, including the uniqueness explanation.
Why: clients and operators match on this message (it is the only human-readable part of ResourceConflictException, whose `Type`/code are shared by many conflicts). A diverging sentence breaks log assertions, user-facing error surfacing, and any retry logic that distinguishes a duplicate-name conflict from an in-progress-operation conflict by text.
Fix: the CreateCapacityProvider handler's duplicate-name conflict path / its error construction in the lambda service crate — build the ResourceConflictException message from AWS's template with the existing provider's ARN interpolated.
Traces: lam-err-capacity-provider-duplicate-create#task#8 (409 -> 409, $.message mismatch).

--- Cause 3 of 5: Invoke's enum validation message lists InvocationType values in the wrong order ---
AWS: Smithy: Invoke, InvocationType is the enum shape `InvocationType`; an unmodeled value is refused 400 InvalidParameterValueException. The recorded refusal message is the standard AWS constraint sentence: "1 validation error detected: Value '<supplied value>' at 'invocationType' failed to satisfy constraint: Member must satisfy enum value set: [DryRun, RequestResponse, Event]". Fixed text is everything but the supplied value; the bracketed value set is itself fixed text — AWS emits it in the order DryRun, RequestResponse, Event (recorded behavior only: the Smithy model declares the members in the order Event, RequestResponse, DryRun, so the model's declaration order is NOT the wire order).
Emulator: same status 400 and same sentence, but the value set is rendered in model-declaration order: "... enum value set: [Event, RequestResponse, DryRun]".
Why: this message is the entire payload of the refusal, and it is what an SDK surfaces and what tests/log scrapers assert on. A client comparing the refusal text against AWS's (or a human reading it next to AWS documentation) sees a different allowed-value list, which also makes the emulator's refusal non-reproducible across the three fixtures that exercise it.
Fix: the Invoke request validation that renders the enum-constraint message in the lambda service crate — the allowed-value list must be emitted in AWS's fixed wire order rather than iterated from the model's member order.
Traces: lambda-batch-diag-submit-wrapper-function-handle-mistakes#task#5 (representative), lambda-function-invocation-paths#task#5, lambda-lsrno-traj-invoke-reproduces-timeout#task#6 — all 400 -> 400 with $.message mismatch.

--- Cause 4 of 5: InvokeAsync's unparseable-payload refusal drops the parser detail ---
AWS: Smithy: InvokeAsync (deprecated), POST /2014-11-13/functions/{FunctionName}/invoke-async, required streaming `InvokeArgs` payload; modeled error InvalidRequestContentException (httpError 400, "The request body could not be parsed as JSON"). Status 400 and the error code match. AWS's message for this class is the prefix "Could not parse request body into json: Could not parse payload into json: " followed by the JSON parser's own diagnostic — the unexpected token and what was expected at that position, then a newline and the parser's source/location note. The fixed parts are those two prefixes and the parser's own fixed phrasing ("Unrecognized token '<token>': was expecting (JSON String, Number, Array, Object or token 'null', 'true' or 'false')" plus the location clause); the variable parts are the offending token and the line/column taken from the request body.
Emulator: answers 400 with `message` = "Could not parse request body into json" only — the outer prefix with nothing after it, so the whole parser diagnostic (offending token, what was expected, line/column) is missing.
Why: the diagnostic is the only thing that tells a caller WHY its payload was rejected and where; a client (or a developer) that logs or displays this refusal gets no actionable information, and anything asserting on AWS's refusal text fails. It also makes the emulator's 400 indistinguishable from any other body-level rejection.
Fix: the InvokeAsync handler's payload-parse error path and its error serializer in the lambda service crate — carry the JSON parser's token/expected/location detail into the InvalidRequestContentException message instead of emitting only the prefix.
Traces: lambda-invoke-modes-comparison#task#5 (400 -> 400, $.message mismatch).

--- Cause 5 of 5: GetFunctionConfiguration omits Layers[].UncompressedCodeSize ---
AWS: Smithy: GetFunctionConfiguration -> FunctionConfiguration.Layers (LayersReferenceList of `Layer`). Recorded behavior only: the on-disk `Layer` shape declares Arn/CodeSize/SigningProfileVersionArn/SigningJobArn and does NOT declare UncompressedCodeSize, but the recorded AWS 200 response carries an `UncompressedCodeSize` member on each attached layer entry — the uncompressed (unzipped) size in bytes of that layer version's content, which is the quantity Lambda measures against the function's unzipped deployment-package limit. Its value is derived from the layer content the emulator already stores, not from the recording.
Emulator: returns 200 with the layer entry present but `Layers[0].UncompressedCodeSize` absent (null).
Why: the unzipped size is how a client checks a function against Lambda's unzipped-size ceiling before attaching another layer; with the member absent, tooling that sums layer sizes sees zero/None, computes the wrong headroom and either proceeds into a deploy AWS refuses or reports no usage at all. The same member is absent from every layer-carrying configuration read.
Note: this cluster also reports $.RuntimeVersionConfig.RuntimeVersionArn carrying a short non-AWS identifier; that key is already owned by "lambda RuntimeVersionArn carries a short non-AWS identifier" (20261102T111455Z-lambda-runtimeversionarn-carries-a-short-non-aws-identifier.md) and is NOT part of this cause — only the missing UncompressedCodeSize is net-new here.
Fix: the layer-reference projection used by the GetFunctionConfiguration serializer (and the layer state it reads) in the lambda service crate — populate UncompressedCodeSize per attached layer version alongside CodeSize.
Traces: lambda-x2x-xlsx-layer-attach-mistakes-and-size-ceiling#task#9 (200 -> 200, $.Layers[0].UncompressedCodeSize missing).

## Evidence
Replay of trace set simparity-wire-aws-v1 (161 fixtures, 1715 steps, 1438 passed, score 83.8%) against the emulator built from this checkout. Five divergence clusters are reported with no owning issue and no blocking stub; every expected value below is from the recorded AWS response, every actual value from the emulator's.

1) DeleteFunction — fixture lambda-cfnuf-alias-repair-attempts-in-lambda, step task#15:
   expected status 204 (empty body); actual status 404.
   Same fixture, earlier step task#9: AWS expected 409 with {'Type': 'User', 'message': 'Unable to delete version because the following aliases reference it: [LATEST]'}, emulator answered 204 — and at task#10 PublishVersion returned Version '2'/...:2 where AWS returned '1'/...:1, i.e. the version ledger is one ahead by task#15.

2) CreateCapacityProvider — fixture lam-err-capacity-provider-duplicate-create, step task#8, status 409 -> 409:
   expected $.message = 'A capacity provider with ARN arn:aws:lambda:<region>:<account>:capacity-provider:<name> already exists. Capacity provider names must be unique within your account.'
   actual   $.message = 'Capacity provider already exists: arn:aws:lambda:<region>:<account>:capacity-provider:<name>'
   (the recording's concrete name/account/region fill the template; only the sentences are the requirement.)

3) Invoke — fixture lambda-batch-diag-submit-wrapper-function-handle-mistakes, step task#5, status 400 -> 400 (also lambda-function-invocation-paths#task#5, lambda-lsrno-traj-invoke-reproduces-timeout#task#6):
   expected $.message = "1 validation error detected: Value '<value>' at 'invocationType' failed to satisfy constraint: Member must satisfy enum value set: [DryRun, RequestResponse, Event]"
   actual   $.message = "1 validation error detected: Value '<value>' at 'invocationType' failed to satisfy constraint: Member must satisfy enum value set: [Event, RequestResponse, DryRun]"
   (differs only in the order of the fixed bracketed value set.)

4) InvokeAsync — fixture lambda-invoke-modes-comparison, step task#5, status 400 -> 400:
   expected $.message = "Could not parse request body into json: Could not parse payload into json: Unrecognized token '<token>': was expecting (JSON String, Number, Array, Object or token 'null', 'true' or 'false')\n at [Source: REDACTED (`StreamReadFeature.INCLUDE_SOURCE_IN_LOCATION` disabled); line: <n>, column: <n>]"
   actual   $.message = 'Could not parse request body into json'

5) GetFunctionConfiguration — fixture lambda-x2x-xlsx-layer-attach-mistakes-and-size-ceiling, step task#9, status 200 -> 200:
   expected $.Layers[0].UncompressedCodeSize = <uncompressed bytes of the attached layer version> (157286422 in the recording); actual: member absent (None).
   The same cluster's $.RuntimeVersionConfig.RuntimeVersionArn key is already owned by 20261102T111455Z-lambda-runtimeversionarn-carries-a-short-non-aws-identifier.md and is excluded from this issue.

## Steps to Reproduce
Replay the simparity-wire-aws-v1 wire corpus against the emulator built from this checkout and inspect these steps:

1. lambda-cfnuf-alias-repair-attempts-in-lambda#task#15 — the CloudFormation-style alias-repair trajectory: create a function, create an alias on it, delete the aliased version (emulator wrongly succeeds at task#9), publish new versions (task#10/#13), then DELETE /2015-03-31/functions/<name>[?Qualifier=<version>] at task#15. Expect 204 and an empty body; the emulator answers 404.
2. lam-err-capacity-provider-duplicate-create#task#8 — POST /2025-11-30/capacity-providers twice with the same CapacityProviderName. The second call must answer 409 ResourceConflictException with AWS's two-sentence duplicate-ARN message; compare $.message.
3. lambda-batch-diag-submit-wrapper-function-handle-mistakes#task#5 (or lambda-function-invocation-paths#task#5, lambda-lsrno-traj-invoke-reproduces-timeout#task#6) — POST /2015-03-31/functions/<name>/invocations with header X-Amz-Invocation-Type set to a non-enum value such as 'requestresponse'. Expect 400 with the enum value set rendered [DryRun, RequestResponse, Event]; compare $.message.
4. lambda-invoke-modes-comparison#task#5 — POST /2014-11-13/functions/<name>/invoke-async with a body that is not valid JSON (a bare word). Expect 400 InvalidRequestContentException whose message carries the parser diagnostic after the two 'Could not parse ...' prefixes; compare $.message.
5. lambda-x2x-xlsx-layer-attach-mistakes-and-size-ceiling#task#9 — publish a layer version, attach it to a function, then GET /2015-03-31/functions/<name>/configuration. Expect each $.Layers[i] to carry UncompressedCodeSize alongside CodeSize; the emulator omits it.
