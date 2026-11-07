Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: imagebuilder.create-image-pipeline-duplicate-name-rejected imagebuilder.create-image-recipe-duplicate-version-rejected imagebuilder.delete-component-referenced-by-recipe-rejected
Issue-Shape-Id: com.amazonaws.imagebuilder#imagebuilder
Issue-Key: imagebuilder.create-image-pipeline-duplicate-name-rejected|run#12|AssertionError|assert msg == expected, f"expected message {expected!r}, got {msg!r}"
Issue-Key: imagebuilder.create-image-recipe-duplicate-version-rejected|run#10|AssertionError|assert msg == expected, f"expected message {expected!r}, got {msg!r}"
Issue-Key: imagebuilder.delete-component-referenced-by-recipe-rejected|run#13|AssertionError|assert msg == expected, f"expected message {expected!r}, got {msg!r}"
Issue-Commit: 56087ceb4bc65915084aa9f243ba54360138be3f

# imagebuilder: 3 net-new refusal-message divergences — ResourceAlreadyExistsException on CreateImagePipeline/CreateImageRecipe and ResourceDependencyException on DeleteComponent are emulator-authored, not AWS wording

> _Note: this issue was identified against the code base at commit `56087ceb4bc65915084aa9f243ba54360138be3f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateImagePipeline

## Description
THREE net-new root causes on Image Builder refusal paths, each grounded in its own failing behavioral check. In every case the emulator picks the CORRECT modeled error code, but composes its own English sentence instead of AWS's message template for that error class: the fixed text differs and the set of variable parts differs (the emulator interpolates resource NAMES and VERSIONS, and in one case a second ARN, where AWS interpolates exactly one ARN). All three causes must be closed before this issue is submitted.

Scope note on similar open issues (none carries these checks' replay keys, so none covers them): 20261102T085510Z-imagebuilder-image-builder-not-found-and-already-exists-refusals.md and 20261105T162014Z-imagebuilder-image-builder-refusal-error-types-and-messages-are.md are wire-corpus issues about refusal status/type/wording generally; 20261101T001020Z-imagebuilder-6-net-new-refusal-divergences-on-create.md covers create-path refusals for CreateComponent / CreateImageRecipe invalid semantic version / CreateInfrastructureConfiguration duplicate name, and does NOT carry the two duplicate-key checks below; 20261103T091826Z-imagebuilder-2-net-new-delete-path-divergences-deleteinfrastructureconfiguration.md describes the same DeleteComponent dependency-message defect but is keyed to the check imagebuilder.ibp-delete-component-referenced-by-a-recipe-rejected, not to the check in cause 3 — if that issue's fix lands first, cause 3 is confirmed by re-replaying the check named below.

Cause 1 of 3: CreateImagePipeline duplicate-name refusal uses an invented message.
AWS: Smithy: CreateImagePipeline (PUT /CreateImagePipeline, 200) models ResourceAlreadyExistsException (client, @httpError 400, documented "The resource that you are trying to create already exists."); input member name is required, pattern ^[-_A-Za-z-0-9][-_A-Za-z0-9 ]{1,126}[-_A-Za-z-0-9]$. Recorded behavior for the message of this error class: 400 ResourceAlreadyExistsException with "The following resource 'Pipeline' already exists: '<imagePipelineArn>'." — fixed text is "The following resource 'Pipeline' already exists: '" and "'."; the only variable part is the ARN of the already-existing image pipeline, taken from the emulator's state for the requested name. AWS does not name the pipeline name, the region, or the words "Amazon Web Services Region" in this message.
Emulator: the code is correct but the message is emulator-authored with a different template and different variable parts — "An image pipeline named <name> already exists in this Amazon Web Services Region." It substitutes the requested NAME where AWS substitutes the existing resource's ARN, and drops the "The following resource 'Pipeline' already exists: '...'." form entirely.
Why: the message is part of the contract for this error class. A client that matches AWS's documented already-exists text does not match, and tooling that parses the single quoted ARN out of the message to recover the conflicting resource's identity gets a bare name (no account, region or resource type) or nothing at all, so conflict-resolution logic cannot address the existing pipeline.
Fix: the duplicate-name refusal message construction in the CreateImagePipeline handler (and any shared already-exists helper it calls) must emit AWS's template with the existing pipeline's ARN as the sole substitution.
Traces: imagebuilder.create-image-pipeline-duplicate-name-rejected.

Cause 2 of 3: CreateImageRecipe duplicate name+semanticVersion refusal uses an invented message.
AWS: Smithy: CreateImageRecipe (PUT /CreateImageRecipe, 200) models ResourceAlreadyExistsException (client, @httpError 400); input members name and semanticVersion are required (semanticVersion pattern ^(?:[0-9]+|x)\\.(?:[0-9]+|x)\\.(?:[0-9]+|x)$). Recorded behavior for the message of this error class: 400 ResourceAlreadyExistsException with "The following resource 'ImageRecipe' already exists: '<imageRecipeArn>'." — fixed text "The following resource 'ImageRecipe' already exists: '" and "'."; the sole variable part is the versioned ARN of the already-registered recipe (which already encodes both the name and the semantic version), taken from the emulator's state. The resource token is 'ImageRecipe', distinct from cause 1's 'Pipeline'.
Emulator: correct code, invented message — "An image recipe named <name> at version <semanticVersion> already exists in this Amazon Web Services Region." It substitutes two request parameters (name and semanticVersion) where AWS substitutes one ARN, and omits the "The following resource 'ImageRecipe' already exists: '...'." form.
Why: same contract break as cause 1, and the per-resource token ('ImageRecipe' vs 'Pipeline') that lets a generic client classify WHICH resource kind collided is absent, so a caller handling several create calls behind one retry path cannot tell which create conflicted.
Fix: the duplicate name+version refusal message construction in the CreateImageRecipe handler (and the shared already-exists helper), parameterised by resource token and existing ARN.
Traces: imagebuilder.create-image-recipe-duplicate-version-rejected.

Cause 3 of 3: DeleteComponent's dependency refusal message is emulator-authored and leaks the referencing recipe.
AWS: Smithy: DeleteComponent (DELETE /DeleteComponent, 200) models ResourceDependencyException (client, @httpError 400, documented "You have attempted to mutate or delete a resource with a dependency that prohibits this action. See the error message for more details."); input member componentBuildVersionArn is required. Recorded behavior for the message of this error class: 400 ResourceDependencyException with "Resource dependency error: The resource ARN '<componentBuildVersionArn>' has other resources depended on it." — fixed text "Resource dependency error: The resource ARN '" and "' has other resources depended on it."; the only variable part is the component build-version ARN taken verbatim from the request. AWS does not name the dependent image recipe.
Emulator: correct code, wrong message — "The component <componentBuildVersionArn> can't be deleted because the image recipe <imageRecipeArn> includes it." It interpolates a SECOND value (the referencing image-recipe ARN) that AWS's message never carries, and drops both the "Resource dependency error: " prefix and the "' has other resources depended on it." suffix.
Why: assertions and log scrapers matching AWS's dependency-error text do not match, parsers that extract the one quoted ARN read the wrong field count, and the emulator discloses internal relationship detail that real AWS withholds.
Fix: the dependency-refusal message construction in the DeleteComponent handler and the shared dependency-error helper it uses; the same template applies to DeleteImageRecipe.
Traces: imagebuilder.delete-component-referenced-by-recipe-rejected.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator; all three checks are listed under the suite report heading '### imagebuilder'. Each failed as [mismatch] on a message-equality assertion AFTER the error-code assertion passed, so the HTTP status (400) and the error code are right and only the message diverges. ARNs, names and version suffixes in the recorded lines below are per-run generated values that fill the template; they are not part of the requirement.

Cause 1 — imagebuilder.create-image-pipeline-duplicate-name-rejected [mismatch]:
CreateImagePipeline is called twice with the same name (distinct clientTokens) against the same imageRecipeArn/infrastructureConfigurationArn, status=DISABLED. The second call is refused with code 'ResourceAlreadyExistsException' (assertion on the code passes), then:
  File "check_imagebuilder_create_image_pipeline_duplicate_name_rejected.py", line 47, in run
  AssertionError: expected message "The following resource 'Pipeline' already exists: 'arn:aws:imagebuilder:us-west-2:000000000000:image-pipeline/sim2real-ib-pipe-dd9230f0d984'.", got 'An image pipeline named sim2real-ib-pipe-dd9230f0d984 already exists in this Amazon Web Services Region.'

Cause 2 — imagebuilder.create-image-recipe-duplicate-version-rejected [mismatch]:
CreateImageRecipe is called twice with the same name and semanticVersion='1.0.0' (distinct clientTokens). The second call is refused with code 'ResourceAlreadyExistsException' (code assertion passes), then:
  File "check_imagebuilder_create_image_recipe_duplicate_version_rejected.py", line 45, in run
  AssertionError: expected message "The following resource 'ImageRecipe' already exists: 'arn:aws:imagebuilder:us-west-2:000000000000:image-recipe/sim2real-ib-recipe-5f66e26f4e3e/1.0.0'.", got 'An image recipe named sim2real-ib-recipe-5f66e26f4e3e at version 1.0.0 already exists in this Amazon Web Services Region.'

Cause 3 — imagebuilder.delete-component-referenced-by-recipe-rejected [mismatch]:
DeleteComponent on a component build version that an image recipe references is refused with code 'ResourceDependencyException' (code assertion passes), then:
  File "check_imagebuilder_delete_component_referenced_by_recipe_rejected.py", line 41, in run
  AssertionError: expected message "Resource dependency error: The resource ARN 'arn:aws:imagebuilder:us-west-2:000000000000:component/sim2real-ib-comp-referenced-23784a89bfe0/1.0.0/1' has other resources depended on it.", got "The component arn:aws:imagebuilder:us-west-2:000000000000:component/sim2real-ib-comp-referenced-23784a89bfe0/1.0.0/1 can't be deleted because the image recipe arn:aws:imagebuilder:us-west-2:000000000000:image-recipe/sim2real-ib-recipe-f6ab26667a2c/1.0.0 includes it."

Model (smithy_query on services/service-com-amazonaws-imagebuilder-imagebuilder/smithy-model/com-amazonaws-imagebuilder-imagebuilder.json):
- ResourceAlreadyExistsException: structure, errorKind client, httpError 400, member message:ErrorMessage; modeled error of both CreateImagePipeline and CreateImageRecipe.
- ResourceDependencyException: structure, errorKind client, httpError 400, member message:ErrorMessage; modeled error of DeleteComponent.
- CreateImagePipeline: PUT /CreateImagePipeline -> 200 {requestId, clientToken, imagePipelineArn}; required name, infrastructureConfigurationArn, clientToken.
- CreateImageRecipe: PUT /CreateImageRecipe -> 200 {requestId, clientToken, imageRecipeArn, latestVersionReferences}; required name, semanticVersion, parentImage, clientToken.
- DeleteComponent: DELETE /DeleteComponent -> 200 {requestId, componentBuildVersionArn}; required componentBuildVersionArn.

## Steps to Reproduce
Cause 1 (CreateImagePipeline duplicate name):
1. imagebuilder.CreateComponent(name=<unique>, semanticVersion='1.0.0', platform='Linux', data=<valid document with build and validate phases>) -> componentBuildVersionArn.
2. imagebuilder.CreateImageRecipe(name=<unique>, semanticVersion='1.0.0', parentImage='arn:aws:imagebuilder:<region>:aws:image/amazon-linux-2-x86/x.x.x', components=[{componentArn: <step 1>}], clientToken=<uuid>) -> imageRecipeArn.
3. iam.CreateRole + CreateInstanceProfile + AddRoleToInstanceProfile, then imagebuilder.CreateInfrastructureConfiguration(name=<unique>, instanceProfileName=<that profile>, clientToken=<uuid>) -> infrastructureConfigurationArn.
4. imagebuilder.CreateImagePipeline(name=P, imageRecipeArn=<step 2>, infrastructureConfigurationArn=<step 3>, status='DISABLED', clientToken=<uuid-1>) -> note imagePipelineArn.
5. imagebuilder.CreateImagePipeline(name=P, same recipe/infra, status='DISABLED', clientToken=<uuid-2>) — expect 400 ResourceAlreadyExistsException with message exactly "The following resource 'Pipeline' already exists: '<imagePipelineArn from step 4>'."; the emulator returns the right code with "An image pipeline named P already exists in this Amazon Web Services Region."
6. ListImagePipelines filtered on name=P — expect exactly the one ARN from step 4 (the refused duplicate registered nothing).

Cause 2 (CreateImageRecipe duplicate name+semanticVersion):
1. imagebuilder.CreateComponent as above -> componentBuildVersionArn.
2. imagebuilder.CreateImageRecipe(name=R, semanticVersion='1.0.0', parentImage='arn:aws:imagebuilder:<region>:aws:image/amazon-linux-2-x86/x.x.x', components=[{componentArn: <step 1>}], clientToken=<uuid-1>) -> note imageRecipeArn.
3. imagebuilder.CreateImageRecipe(name=R, semanticVersion='1.0.0', same parentImage/components, clientToken=<uuid-2>) — expect 400 ResourceAlreadyExistsException with message exactly "The following resource 'ImageRecipe' already exists: '<imageRecipeArn from step 2>'."; the emulator returns the right code with "An image recipe named R at version 1.0.0 already exists in this Amazon Web Services Region."
4. ListImageRecipes(filters=[{name:'name', values:[R]}]) — expect only the step-2 ARN.
5. CreateImageRecipe(name=R, semanticVersion='1.0.1', ...) — expect success (the block is scoped to the exact name+version key).

Cause 3 (DeleteComponent dependency message):
1. imagebuilder.CreateComponent(name=<unique>, semanticVersion='1.0.0', platform='Linux', data=<valid document>) -> componentBuildVersionArn C.
2. imagebuilder.CreateImageRecipe(name=<unique>, semanticVersion='1.0.0', parentImage='arn:aws:imagebuilder:<region>:aws:image/amazon-linux-2-x86/x.x.x', components=[{componentArn: C}]) -> imageRecipeArn.
3. imagebuilder.DeleteComponent(componentBuildVersionArn=C) — expect 400 ResourceDependencyException with message exactly "Resource dependency error: The resource ARN 'C' has other resources depended on it."; the emulator returns the right code with an invented message that also names the recipe ARN.
4. GetComponent(C) — expect 200 with arn == C (the refused delete had no effect).
5. DeleteImageRecipe(<step 2>), then DeleteComponent(C) — expect 200; GetComponent(C) -> 404 ResourceNotFoundException "The following required resource 'Component' cannot be found: 'C'."
6. Re-replay every check listed in trace_ids to confirm all three causes.
