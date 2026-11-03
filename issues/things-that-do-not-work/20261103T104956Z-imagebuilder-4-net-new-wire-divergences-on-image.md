Issue-Source: trace-sweep-divergence
Issue-Trace: imagebuilder-create-image-pipeline-rejects-an-invalid-cron-schedule
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: imagebuilder-create-image-pipeline-rejects-an-invalid-cron-schedule imagebuilder-create-image-pipeline-rejects-a-test-timeout-above-1440-minutes imagebuilder-create-component-supported-os-versions-roundtrip imagebuilder-ibp-traj-component-request-mistakes
Issue-Shape-Id: com.amazonaws.imagebuilder#imagebuilder
Issue-Key: imagebuilder|CreateImagePipeline|4xx->2xx|$.message
Issue-Key: imagebuilder|CreateImagePipeline|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: imagebuilder|CreateImagePipeline|4xx->2xx|status
Issue-Key: imagebuilder|CreateImagePipeline|2xx->4xx|$.clientToken
Issue-Key: imagebuilder|CreateImagePipeline|2xx->4xx|$.imagePipelineArn
Issue-Key: imagebuilder|CreateImagePipeline|2xx->4xx|$.requestId
Issue-Key: imagebuilder|CreateImagePipeline|2xx->4xx|status
Issue-Key: imagebuilder|GetImagePipeline|4xx->2xx|$.message
Issue-Key: imagebuilder|GetImagePipeline|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: imagebuilder|GetImagePipeline|4xx->2xx|status
Issue-Key: imagebuilder|GetComponent|2xx->2xx|$.component.billable
Issue-Commit: 17254fcad66df5ce128052361e5c23fecb4b21db

# imagebuilder: 4 net-new wire divergences on image-pipeline create/read and GetComponent output

> _Note: this issue was identified against the code base at commit `17254fcad66df5ce128052361e5c23fecb4b21db`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateImagePipeline

## Description
FOUR net-new root causes on the imagebuilder image-pipeline and component read paths, from the wire replay of trace set simparity-wire-aws-v1. Every other diverging cluster in this replay is already owned (parameter-validation / not-found-and-already-exists / delete-and-recreate issues) or blocked by a filed stub, so only these four are filed. A fixer must close ALL FOUR before submitting.

Adjacent open issues that do NOT carry these replay keys and therefore do not cover these causes: 20261103T041241Z-ec2-iam-imagebuilder-image-builder-createimagepipeline-accepts-an-invalid-cron-schedule.md (same cron fault, but grounded only in the BEHAVIORAL corpus check ec2__iam__imagebuilder.ibp-create-image-pipeline-rejects-an-invalid-cron-schedule — this wire step is unowned), 20261102T085510Z-imagebuilder-image-builder-delete-and-recreate-lifecycle-leaves-stored.md and 20261102T085510Z-imagebuilder-image-builder-parameter-validation-refuses-with-validationexception-instead.md.

Cause 1 of 4: CreateImagePipeline accepts a syntactically invalid cron schedule expression instead of refusing it. Fixture imagebuilder-create-image-pipeline-rejects-an-invalid-cron-schedule, step task#7.
AWS: Smithy (imagebuilder, PUT /CreateImagePipeline): input member `schedule` targets structure `Schedule`, whose `scheduleExpression` is `NonEmptyString` @length 1..1024 with NO pattern, so the syntax of the cron/rate expression is checked by the service, not by a modeled constraint; the operation models client refusals ClientException and InvalidRequestException at HTTP 400. Recorded behavior: an unparseable expression of the form cron(<garbage>) is refused with HTTP 400 and the message "The value supplied for parameter 'schedule.scheduleExpression' is not valid. Invalid schedule expression. Details: Invalid CRON expression: error at position <n>, details: Illegal characters for this position: '<TOKEN>'" and no pipeline is created. Fixed text of that message: "The value supplied for parameter 'schedule.scheduleExpression' is not valid. Invalid schedule expression. Details: Invalid CRON expression: error at position ", ", details: Illegal characters for this position: '", "'". Variable parts come from the submitted expression: the 0-based position of the offending token and that token as upper-cased by the parser. (The recording's own values were position 0 and token 'NOT' for cron(not valid); those are the template's fill-ins, not the requirement.)
Emulator: answers HTTP 200 and returns requestId/clientToken/imagePipelineArn — the unparseable expression is persisted as a live pipeline schedule.
Why: an over-permissive accept. A client that relies on AWS to validate cron syntax is told its bad expression is legal, gets a pipeline ARN back, and the pipeline silently never fires; code that tests its own validation by submitting a bad expression gets the wrong answer.
Fix: input validation in the imagebuilder CreateImagePipeline handler (and the same member on UpdateImagePipeline) — parse schedule.scheduleExpression and refuse an unparseable expression with HTTP 400, error code InvalidParameterValueException and the message above, before anything is written to state.
Traces: imagebuilder-create-image-pipeline-rejects-an-invalid-cron-schedule#task#7.

Cause 2 of 4: the follow-up CreateImagePipeline with a VALID schedule expression is then refused with HTTP 400 where AWS creates the pipeline. Fixture imagebuilder-create-image-pipeline-rejects-an-invalid-cron-schedule, step task#8.
AWS: Smithy: CreateImagePipeline returns HTTP 200 with output members requestId, clientToken and imagePipelineArn (pattern ^arn:aws[^:]*:imagebuilder:[^:]+:(?:[0-9]{12}|aws):image-pipeline/[a-z0-9-_]+$). Recorded behavior: after the invalid-schedule attempt is refused, re-submitting the SAME pipeline name with a valid expression succeeds with 200 and returns the pipeline ARN, because the refused attempt created nothing.
Emulator: answers HTTP 400 with none of clientToken, imagePipelineArn or requestId present.
Why: the retry-after-refusal flow — the normal way a client recovers from a rejected request — fails outright, so no pipeline can be created under a name whose first attempt was malformed.
Hypothesis: this is the downstream half of Cause 1 — the emulator persisted the pipeline during the 200 at task#7, so the valid retry collides with the stored name and is refused as already existing. Fixing Cause 1 so the invalid create writes nothing is expected to clear this, but the step must be re-replayed to confirm, and the handler must in any case leave no state behind on a refused create.
Fix: the imagebuilder CreateImagePipeline handler and its state writes — validate before persisting, so a refused create leaves the name free.
Traces: imagebuilder-create-image-pipeline-rejects-an-invalid-cron-schedule#task#8.

Cause 3 of 4: GetImagePipeline still reports a pipeline that has been deleted, answering HTTP 200 where AWS answers 404. Fixture imagebuilder-create-image-pipeline-rejects-a-test-timeout-above-1440-minutes, step teardown#11.
AWS: Smithy (imagebuilder, GET /GetImagePipeline): a pipeline ARN that resolves to nothing is reported as missing. Recorded behavior: after the pipeline is deleted, GetImagePipeline on its ARN returns HTTP 404 with the message "The following required resource 'ImagePipeline' cannot be found: '<imagePipelineArn>'." Fixed text: "The following required resource 'ImagePipeline' cannot be found: '" and "'."; the variable part is the request's imagePipelineArn (the recording's probe ARN is just that fill-in).
Emulator: answers HTTP 200 with an imagePipeline body, and emits no x-amzn-errortype header — the deleted pipeline is still served.
Why: a client cannot observe deletion. Teardown/idempotency logic that polls Get until it 404s loops forever, and recreate-after-delete flows see a resource that should be gone.
Fix: the pipeline's state lifecycle — DeleteImagePipeline is still a scaffold stub that answers 501 and removes nothing, so GetImagePipeline's lookup (which correctly 404s on a missing entry) never sees the removal. Implement the delete so it evicts the pipeline from the service store; see Traces for the file.
Traces: imagebuilder-create-image-pipeline-rejects-a-test-timeout-above-1440-minutes#teardown#11; stub at services/service-com-amazonaws-imagebuilder-imagebuilder/src/ops/delete_image_pipeline.rs lines 3-6 and 179 (returns Scaffolded::not_implemented); the working lookup is services/service-com-amazonaws-imagebuilder-imagebuilder/src/ops/get_image_pipeline.rs lines 176-191.

Cause 4 of 4: GetComponent omits the component.billable member. Fixture imagebuilder-create-component-supported-os-versions-roundtrip, step task#2.
AWS: recorded behavior only — this checkout's on-disk Smithy model for the Component shape declares arn, name, version, description, changeDescription, type, platform, supportedOsVersions, state, parameters, owner, data, kmsKeyId, encrypted, dateCreated, tags, publisher, obfuscate and productCodes, and does NOT declare `billable`. The recording shows AWS's GetComponent 200 response carrying component.billable as a boolean, false for a customer-owned (non-Marketplace) component.
Emulator: the member is absent from the 200 body (status, and every other asserted component member, match).
Why: a client that reads component.billable to decide whether a component incurs Marketplace charges gets a missing/None value instead of the documented boolean, so billing-sensitive branches take the wrong path or fail to deserialize under a strict decoder.
Fix: the GetComponent response serializer and the Component view it builds — emit `billable` with the component's billable flag (false for self-owned components). Because the bundled model omits the member, this also needs the Component shape to carry it before the serializer can emit it.
Traces: imagebuilder-create-component-supported-os-versions-roundtrip#task#2 (cluster also covers imagebuilder-ibp-traj-component-request-mistakes#task#8).

## Evidence
Replay of trace set simparity-wire-aws-v1 (27 fixtures, 299 steps, 125 passed, score 41.8%). The four net-new clusters, verbatim from the replay output:

Cause 1 — imagebuilder-create-image-pipeline-rejects-an-invalid-cron-schedule#task#7, CreateImagePipeline, verdict mismatch: "status: expected 400, got 200 | $.message: missing (expected \"The value supplied for parameter 'schedule.scheduleExpression' is not valid. Invalid schedule expression. Details: Invalid CRON expression: error at position 0, details: Illegal characters for this position: 'NOT'\", got None) | $headers.x-amzn-errortype: missing (expected '<shaped-not-asserted>', got None)".

Cause 2 — imagebuilder-create-image-pipeline-rejects-an-invalid-cron-schedule#task#8, CreateImagePipeline, verdict mismatch: "status: expected 200, got 400 | $.clientToken: missing | $.imagePipelineArn: missing (expected an arn:aws:imagebuilder:<region>:<account>:image-pipeline/<name>) | $.requestId: missing".

Cause 3 — imagebuilder-create-image-pipeline-rejects-a-test-timeout-above-1440-minutes#teardown#11, GetImagePipeline, verdict mismatch: "status: expected 404, got 200 | $.message: missing (expected \"The following required resource 'ImagePipeline' cannot be found: '<imagePipelineArn>'.\", got None) | $headers.x-amzn-errortype: missing (expected '<shaped-not-asserted>', got None)".

Cause 4 — imagebuilder-create-component-supported-os-versions-roundtrip#task#2, GetComponent, verdict mismatch, expected 200 / actual 200: "$.component.billable: missing (expected False, got None)"; same cluster affects imagebuilder-ibp-traj-component-request-mistakes#task#8.

Model evidence: smithy_query(operation="CreateImagePipeline,GetImagePipeline,GetComponent") and smithy_query(shape="Component,Schedule") on services/service-com-amazonaws-imagebuilder-imagebuilder/smithy-model/com-amazonaws-imagebuilder-imagebuilder.json — Schedule.scheduleExpression has no pattern (service-side syntax check), CreateImagePipeline output = requestId/clientToken/imagePipelineArn, and Component has no `billable` member.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 wire corpus.
2. Cause 1: replay fixture imagebuilder-create-image-pipeline-rejects-an-invalid-cron-schedule to step task#7 — CreateImagePipeline(name=<n>, imageRecipeArn=<recipe>, infrastructureConfigurationArn=<infra>, clientToken=<t>, schedule={'scheduleExpression': 'cron(not valid)'}). Emulator returns 200 with an imagePipelineArn; AWS returns 400 with the InvalidParameterValueException message for schedule.scheduleExpression.
3. Cause 2: continue the same fixture to task#8 — re-submit the SAME pipeline name with a valid expression (e.g. cron(0 3 ? * SUN *)). Emulator returns 400; AWS returns 200 with clientToken, requestId and imagePipelineArn.
4. Cause 3: replay fixture imagebuilder-create-image-pipeline-rejects-a-test-timeout-above-1440-minutes through its teardown and stop at teardown#11 — GET /GetImagePipeline?imagePipelineArn=<arn of the pipeline just deleted>. Emulator returns 200 with the pipeline body; AWS returns 404 with "The following required resource 'ImagePipeline' cannot be found: '<arn>'." (the preceding DeleteImagePipeline answers 501 and removes nothing).
5. Cause 4: replay fixture imagebuilder-create-component-supported-os-versions-roundtrip to task#2 — GET /GetComponent?componentBuildVersionArn=<the component just created>. The 200 body omits component.billable, which AWS returns as false.
