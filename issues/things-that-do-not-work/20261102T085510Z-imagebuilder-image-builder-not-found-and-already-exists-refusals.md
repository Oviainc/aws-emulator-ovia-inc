Issue-Source: trace-sweep-divergence
Issue-Trace: imagebuilder-create-component-rejects-a-two-part-semantic-version
Issue-Step: teardown#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: imagebuilder-create-component-rejects-a-two-part-semantic-version imagebuilder-ibp-traj-cleanup-in-dependency-order imagebuilder-ibp-traj-component-request-mistakes imagebuilder-ibp-traj-recipe-reference-mistakes imagebuilder-ibp-traj-infra-config-before-instance-profile
Issue-Shape-Id: com.amazonaws.imagebuilder#imagebuilder
Issue-Key: imagebuilder|GetComponent|4xx->4xx|status
Issue-Key: imagebuilder|CreateImageRecipe|4xx->4xx|status
Issue-Key: imagebuilder|CreateInfrastructureConfiguration|4xx->4xx|$.message
Issue-Key: imagebuilder|CreateInfrastructureConfiguration|4xx->4xx|$.requestId
Issue-Related: 20261031T232326Z-imagebuilder-image-builder-error-responses-use-the-wrong-status.md
Issue-Related: 20261102T085510Z-imagebuilder-image-builder-accepts-creates-and-deletes-that-aws.md
Issue-Related: 20261102T085510Z-imagebuilder-image-builder-parameter-validation-refuses-with-validationexception-instead.md
Issue-Commit: 3879dd05c1f9087ace2750498c91b002c8086d49

# Image Builder not-found and already-exists refusals carry wrong status, error type and wording

> _Note: this issue was identified against the code base at commit `3879dd05c1f9087ace2750498c91b002c8086d49`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetComponent

## Description
ONE root cause with FIVE causes: the imagebuilder emulator authors its own sentences for the two resource-state refusals (absent resource, conflicting resource) and, for the absent case, classifies them as the operation's modeled InvalidRequestException at HTTP 400 rather than Image Builder's 404 ResourceNotFoundException. There are 5 separate causes below; all 5 must be closed. Two open issues record neighbouring instances of the same branch but carry different replay keys and do not cover these steps: 20261031T232326Z-imagebuilder-image-builder-error-responses-use-the-wrong-status.md (bound to GetImageRecipe teardown#8 and ListImagePackages) and the behavioral 20261101T001020Z-imagebuilder-image-builder-not-found-lookups-refuse-400-invalidrequestexception.md.

Shared AWS requirement for causes 1-3: Smithy com.amazonaws.imagebuilder#ResourceNotFoundException is a client error with @httpError 404, documented "At least one of the resources referenced by your request does not exist.", carrying one `message` member; it is not in the error lists of GetComponent/GetImageRecipe/CreateImageRecipe (which list ClientException 400, InvalidRequestException 400, ForbiddenException 403, ServiceException 500, ServiceUnavailableException 503, ...), so the 404 for a missing resource is recorded behavior only. The message template is: fixed text "The following required resource '", "' cannot be found: '" and "'."; the VARIABLE parts are the resource-type token (Component, ImageRecipe) and the ARN taken verbatim from the failing request member. All these operations model a `requestId` output member, and the recordings carry requestId on the error body; the emulator emits none.

Cause 1 of 5: GetComponent on an ARN that resolves to nothing. AWS: 404 ResourceNotFoundException, message "The following required resource 'Component' cannot be found: '<componentBuildVersionArn>'.", plus requestId. Emulator: 400 with x-amzn-errortype 'InvalidRequestException' and its own sentence 'The component <arn> does not exist.', no requestId. Trace: imagebuilder-create-component-rejects-a-two-part-semantic-version#teardown#6 (also teardown#8, imagebuilder-ibp-traj-cleanup-in-dependency-order#task#21, imagebuilder-ibp-traj-component-request-mistakes#teardown#10).

Cause 2 of 5: GetImageRecipe on an ARN that resolves to nothing. AWS: 404 ResourceNotFoundException, message "The following required resource 'ImageRecipe' cannot be found: '<imageRecipeArn>'.", plus requestId. Emulator: 400 'InvalidRequestException' with 'The image recipe <arn> does not exist.'. Trace: imagebuilder-ibp-traj-cleanup-in-dependency-order#task#15.

Cause 3 of 5: CreateImageRecipe naming a component ARN that does not exist. AWS: 404 ResourceNotFoundException, message "The following required resource 'Component' cannot be found: '<componentArn>'." — same template, resource-type token Component, ARN taken from the request's components[].componentArn. Emulator: 400 'InvalidRequestException' with 'The componentArn <arn> does not name a component in this Amazon Web Services Region.'. Trace: imagebuilder-ibp-traj-recipe-reference-mistakes#task#2.

Cause 4 of 5: CreateInfrastructureConfiguration for a name that already exists. AWS: 400 with message 'The resource already exists.' — the entire sentence is fixed text for this class, with no name or region interpolated — plus requestId. Emulator: 400 but with its own composed sentence 'An infrastructure configuration named <name> already exists in this Amazon Web Services Region.' and no requestId. Trace: imagebuilder-ibp-traj-infra-config-before-instance-profile#task#6.

Cause 5 of 5: CreateImageRecipe for a name/version that already exists. AWS: 400 ResourceAlreadyExistsException (Smithy: client error, @httpError 400, modeled on CreateImageRecipe, documented "The resource that you are trying to create already exists.") with message "The following resource 'ImageRecipe' already exists: '<arn>'." — fixed text "The following resource '", "' already exists: '", "'.", with the resource-type token ImageRecipe and the existing resource's ARN from the emulator's own state. Emulator: 'An image recipe named <name> at version <version> already exists in this Amazon Web Services Region.' and no requestId. Trace: imagebuilder-ibp-traj-recipe-reference-mistakes#task#9.

Why: SDKs and callers discriminate these two conditions by status and error type. Code that catches ResourceNotFoundException — idempotent teardown that swallows it, delete polling that treats 404 as 'gone', a create-if-absent probe — instead receives a 400 InvalidRequestException it classifies as a malformed request, so the branch never runs and the loop aborts. For the conflict cases the status matches but the human-visible text does not, so operators and assertions keyed on AWS's wording diverge, and the absent requestId removes the correlation id the response shapes model.

Fix: the lookup/conflict branches of the imagebuilder handlers — the resolve-to-None arms of get_component.rs, get_image_recipe.rs and the component-reference resolution in create_image_recipe.rs must return 404 ResourceNotFoundException instead of InvalidRequestException; the name-conflict arms of create_infrastructure_configuration.rs and create_image_recipe.rs must return AWS's conflict wording; and the shared message builders in src/state.rs must produce these templates while the error serializer populates requestId.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator:
- imagebuilder-create-component-rejects-a-two-part-semantic-version#teardown#6 (GetComponent): status expected 404, got 400; $.message expected "The following required resource 'Component' cannot be found: 'arn:aws:imagebuilder:us-west-2:000000000000:component/<name>/1.0.0/2'.", got 'The component <arn> does not exist.'; $.requestId missing; x-amzn-errortype expected 'ResourceNotFoundException', got 'InvalidRequestException'. Affects 4 steps.
- imagebuilder-ibp-traj-cleanup-in-dependency-order#task#15 (GetImageRecipe): status expected 404, got 400; expected "The following required resource 'ImageRecipe' cannot be found: '<arn>'.", got 'The image recipe <arn> does not exist.'; errortype 'InvalidRequestException'.
- imagebuilder-ibp-traj-recipe-reference-mistakes#task#2 (CreateImageRecipe): status expected 404, got 400; expected "The following required resource 'Component' cannot be found: '<component arn>'.", got 'The componentArn <arn> does not name a component in this Amazon Web Services Region.'; errortype 'InvalidRequestException'.
- imagebuilder-ibp-traj-infra-config-before-instance-profile#task#6 (CreateInfrastructureConfiguration): $.message expected 'The resource already exists.', got 'An infrastructure configuration named sim2real-probe-ibp-infra-94ae2aee already exists in this Amazon Web Services Region.'; $.requestId missing.
- imagebuilder-ibp-traj-recipe-reference-mistakes#task#9 (CreateImageRecipe): $.message expected "The following resource 'ImageRecipe' already exists: '<recipe arn>'.", got 'An image recipe named sim2real-probe-ibp-recipe-017311de at version 1.0.0 already exists in this Amazon Web Services Region.'; $.requestId missing.
Model (smithy_query): ResourceNotFoundException client/httpError 404; ResourceAlreadyExistsException client/httpError 400 and modeled on CreateImageRecipe and CreateInfrastructureConfiguration; GetComponent/GetImageRecipe/CreateImageRecipe all model a requestId output member.

## Steps to Reproduce
1. CreateComponent(name=<unique>, semanticVersion='1.0.0', platform='Linux', data=<valid document>, clientToken=<uuid>), DeleteComponent(<arn>), then GetComponent(componentBuildVersionArn=<arn>) — expect 404 ResourceNotFoundException with "The following required resource 'Component' cannot be found: '<arn>'." and a requestId; the emulator answers 400 InvalidRequestException with 'The component <arn> does not exist.'.
2. Same sequence with CreateImageRecipe/DeleteImageRecipe then GetImageRecipe(<arn>) — expect the 'ImageRecipe' form of the template.
3. CreateImageRecipe(name=<unique>, semanticVersion='1.0.0', parentImage=<valid AMI>, components=[{'componentArn': '<a component ARN that was never created>'}], clientToken=<uuid>) — expect 404 ResourceNotFoundException with the 'Component' form of the template; the emulator answers 400 InvalidRequestException with its own sentence.
4. CreateInfrastructureConfiguration(name=X, ...) twice — the second call must return 400 with message exactly 'The resource already exists.'; the emulator composes its own sentence naming X.
5. CreateImageRecipe(name=Y, semanticVersion='1.0.0', ...) twice — the second call must return 400 ResourceAlreadyExistsException with "The following resource 'ImageRecipe' already exists: '<arn of the existing recipe>'."; the emulator composes its own sentence naming Y and the version.
6. Re-replay the fixture steps listed in Traces.
