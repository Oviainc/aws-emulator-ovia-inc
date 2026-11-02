Issue-Source: trace-sweep-divergence
Issue-Trace: imagebuilder-ibp-traj-component-request-mistakes
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: imagebuilder-ibp-traj-component-request-mistakes imagebuilder-ibp-traj-infra-config-before-instance-profile imagebuilder-ibp-traj-recipe-reference-mistakes imagebuilder-ibp-traj-cleanup-in-dependency-order
Issue-Shape-Id: com.amazonaws.imagebuilder#imagebuilder
Issue-Key: imagebuilder|CreateComponent|4xx->2xx|$.message
Issue-Key: imagebuilder|CreateComponent|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: imagebuilder|CreateComponent|4xx->2xx|status
Issue-Key: imagebuilder|CreateInfrastructureConfiguration|4xx->2xx|$.__type
Issue-Key: imagebuilder|CreateInfrastructureConfiguration|4xx->2xx|$.message
Issue-Key: imagebuilder|CreateInfrastructureConfiguration|4xx->2xx|status
Issue-Key: imagebuilder|CreateImageRecipe|4xx->2xx|$.message
Issue-Key: imagebuilder|CreateImageRecipe|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: imagebuilder|CreateImageRecipe|4xx->2xx|status
Issue-Key: imagebuilder|CreateImageRecipe|4xx->4xx|$.message
Issue-Key: imagebuilder|CreateImageRecipe|4xx->4xx|$.requestId
Issue-Key: imagebuilder|CreateImageRecipe|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: imagebuilder|DeleteImageRecipe|4xx->2xx|$.message
Issue-Key: imagebuilder|DeleteImageRecipe|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: imagebuilder|DeleteImageRecipe|4xx->2xx|status
Issue-Key: imagebuilder|DeleteComponent|4xx->2xx|$.message
Issue-Key: imagebuilder|DeleteComponent|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: imagebuilder|DeleteComponent|4xx->2xx|status
Issue-Commit: 3879dd05c1f9087ace2750498c91b002c8086d49

# Image Builder accepts creates and deletes that AWS refuses, answering HTTP 200

> _Note: this issue was identified against the code base at commit `3879dd05c1f9087ace2750498c91b002c8086d49`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateComponent

## Description
ONE root cause with SIX causes: the imagebuilder emulator performs only Smithy shape validation and no semantic validation, so requests AWS refuses are accepted with HTTP 200 and a resource is created (or a referenced resource is destroyed). Because the phantom resource then exists, later steps in the same trajectory get the wrong answer too. There are 6 separate causes below; all 6 must be closed. Related open issue 20261101T001020Z-imagebuilder-6-net-new-refusal-divergences-on-create.md is behavioral and does not carry these replay keys.

Cause 1 of 6: CreateComponent accepts a component document with no `phases`. AWS: the document in `data` is parsed and must contain at least one build phase; the refusal is 400 InvalidParameterValueException (Smithy: com.amazonaws.imagebuilder#InvalidParameterValueException, client error, @httpError 400) with message "The value supplied for parameter 'data' is not valid. Missing phases." — the whole sentence is fixed text for this class, with 'data' the request member name. The model only constrains data by @length 1..16000 and @pattern ^[^\\x00]+$, so the document-content rule is recorded behavior only. Emulator: 200 with a componentBuildVersionArn, no error body at all. Trace: imagebuilder-ibp-traj-component-request-mistakes#task#5.

Cause 2 of 6: CreateInfrastructureConfiguration accepts an instanceProfileName that does not exist in IAM. AWS: 400, $.__type 'InvalidParameterValueException', message "The value supplied for parameter 'instanceProfileName' is not valid. The provided instance profile does not exist. Please specify a different instance profile and try again." — fixed text for this class, with 'instanceProfileName' the request member name; the instance profile is resolved against IAM before the configuration is stored. Model constrains instanceProfileName only by @length 1..256 / @pattern ^[\\w+=,.@-]+$, so the existence check is recorded behavior only. Emulator: 200, configuration created. Trace: imagebuilder-ibp-traj-infra-config-before-instance-profile#task#1.

Cause 3 of 6: CreateImageRecipe accepts a parentImage AMI id that does not exist. AWS: 404 ResourceNotFoundException (Smithy: client error, @httpError 404, "At least one of the resources referenced by your request does not exist.") with message "The following required resource 'Image' cannot be found: '<imageId>'." — fixed text "The following required resource '", "' cannot be found: '", "'." with resource-type token Image and the AMI id taken verbatim from the request's parentImage. Emulator: 200, recipe created. Trace: imagebuilder-ibp-traj-recipe-reference-mistakes#task#3.

Cause 4 of 6: CreateImageRecipe accepts a parentImage that is not a supported image identifier at all. AWS: 400 with message "The value supplied for parameter 'parentImage' is not valid. The supplied image identifier is not in a supported format." (fixed text for this class). Emulator: 400 but with the wrong cause — ResourceAlreadyExistsException and 'An image recipe named <name> at version <version> already exists in this Amazon Web Services Region.' — because cause 3 already stored a recipe under that name/version, so the format of parentImage is never examined. Trace: imagebuilder-ibp-traj-recipe-reference-mistakes#task#4 (also task#5, #6, #7).

Cause 5 of 6: DeleteImageRecipe deletes a recipe that an image pipeline still references. AWS: 400 ResourceDependencyException (Smithy: client error, @httpError 400, modeled on DeleteImageRecipe, documented "You have attempted to mutate or delete a resource with a dependency that prohibits this action.") with message "Resource dependency error: The resource ARN '<arn>' has other resources depended on it." — fixed text "Resource dependency error: The resource ARN '" and "' has other resources depended on it.", with the ARN taken from the request's imageRecipeArn. Emulator: 200 and the recipe is removed while the pipeline still points at it. Trace: imagebuilder-ibp-traj-cleanup-in-dependency-order#task#9.

Cause 6 of 6: DeleteComponent deletes a component that an image recipe still references. AWS: 400 ResourceDependencyException (modeled on DeleteComponent) with the same template, ARN taken from componentBuildVersionArn. Emulator: 200 and the component is removed. Trace: imagebuilder-ibp-traj-cleanup-in-dependency-order#task#12.

Why: a client that relies on the service to validate gets silent success for input AWS rejects, so the emulator cannot be used to test error handling, and the phantom resources it creates change the result of every later call in the session (cause 4 shows the wrong error class being raised purely because of a resource that should never have existed). For the delete paths, dependency protection is the mechanism that stops a pipeline being left pointing at a deleted recipe/component; removing it lets callers build states AWS makes unreachable, and teardown ordering bugs in client code go undetected.

Fix: the semantic-validation stage of the imagebuilder create and delete handlers — document parsing in create_component, IAM instance-profile resolution in create_infrastructure_configuration, parentImage format and existence resolution in create_image_recipe, and reverse-reference lookups in state before delete_image_recipe/delete_component commit — must refuse with the modeled error (InvalidParameterValueException 400, ResourceNotFoundException 404, ResourceDependencyException 400) and AWS's message template instead of returning 200.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator:
- imagebuilder-ibp-traj-component-request-mistakes#task#5 (CreateComponent): status expected 400, got 200; $.message expected "The value supplied for parameter 'data' is not valid. Missing phases.", got None; x-amzn-errortype expected, got None.
- imagebuilder-ibp-traj-infra-config-before-instance-profile#task#1 (CreateInfrastructureConfiguration): status expected 400, got 200; $.__type expected 'InvalidParameterValueException', got None; $.message expected "The value supplied for parameter 'instanceProfileName' is not valid. The provided instance profile does not exist. Please specify a different instance profile and try again.", got None.
- imagebuilder-ibp-traj-recipe-reference-mistakes#task#3 (CreateImageRecipe): status expected 404, got 200; $.message expected "The following required resource 'Image' cannot be found: 'ami-0123456789abcdef0'.", got None.
- imagebuilder-ibp-traj-recipe-reference-mistakes#task#4 (CreateImageRecipe, also #task#5..#7): $.message expected "The value supplied for parameter 'parentImage' is not valid. The supplied image identifier is not in a supported format.", got 'An image recipe named sim2real-probe-ibp-recipe-017311de at version 1.0.0 already exists in this Amazon Web Services Region.'; errortype got 'ResourceAlreadyExistsException'; $.requestId missing.
- imagebuilder-ibp-traj-cleanup-in-dependency-order#task#9 (DeleteImageRecipe): status expected 400, got 200; $.message expected "Resource dependency error: The resource ARN 'arn:aws:imagebuilder:us-west-2:000000000000:image-recipe/<name>/1.0.0' has other resources depended on it.", got None.
- imagebuilder-ibp-traj-cleanup-in-dependency-order#task#12 (DeleteComponent): status expected 400, got 200; same template with the component build-version ARN.
Model (smithy_query): ResourceDependencyException is a modeled error of both DeleteImageRecipe and DeleteComponent (client, httpError 400); ResourceNotFoundException is client/httpError 404; InvalidParameterValueException is client/httpError 400.

## Steps to Reproduce
1. CreateComponent(name=<unique>, semanticVersion='1.0.0', platform='Linux', data=<YAML with name/description/schemaVersion but no `phases` key>, clientToken=<uuid>) — expect 400 InvalidParameterValueException "The value supplied for parameter 'data' is not valid. Missing phases."; the emulator answers 200 and creates the component.
2. CreateInfrastructureConfiguration(name=<unique>, instanceProfileName=<a profile that was never created in IAM>, clientToken=<uuid>) — expect 400 InvalidParameterValueException naming instanceProfileName; the emulator answers 200.
3. CreateImageRecipe(name=<unique>, semanticVersion='1.0.0', parentImage='ami-0123456789abcdef0' (nonexistent), components=[<valid component>], clientToken=<uuid>) — expect 404 ResourceNotFoundException "The following required resource 'Image' cannot be found: 'ami-0123456789abcdef0'."; the emulator answers 200.
4. CreateImageRecipe(same name/version, parentImage='not-an-image-identifier') — expect 400 InvalidParameterValueException "...'parentImage' is not valid. The supplied image identifier is not in a supported format."; the emulator answers ResourceAlreadyExistsException instead.
5. CreateComponent + CreateImageRecipe referencing it + CreateImagePipeline referencing the recipe, then DeleteImageRecipe(<recipe arn>) and DeleteComponent(<component arn>) while the referents still exist — each must refuse 400 ResourceDependencyException with "Resource dependency error: The resource ARN '<arn>' has other resources depended on it."; the emulator answers 200 and deletes.
6. Re-replay the fixture steps listed in Traces.
