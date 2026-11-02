Issue-Source: trace-sweep-divergence
Issue-Trace: imagebuilder-ibp-traj-cleanup-in-dependency-order
Issue-Step: task#14
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: imagebuilder-ibp-traj-cleanup-in-dependency-order imagebuilder-ibp-traj-recipe-reference-mistakes imagebuilder-ibp-traj-infra-config-before-instance-profile imagebuilder-ibp-traj-component-request-mistakes
Issue-Shape-Id: com.amazonaws.imagebuilder#imagebuilder
Issue-Key: imagebuilder|DeleteImageRecipe|2xx->4xx|$.imageRecipeArn
Issue-Key: imagebuilder|DeleteImageRecipe|2xx->4xx|$.requestId
Issue-Key: imagebuilder|DeleteImageRecipe|2xx->4xx|status
Issue-Key: imagebuilder|DeleteComponent|2xx->4xx|$.componentBuildVersionArn
Issue-Key: imagebuilder|DeleteComponent|2xx->4xx|$.requestId
Issue-Key: imagebuilder|DeleteComponent|2xx->4xx|status
Issue-Key: imagebuilder|GetInfrastructureConfiguration|4xx->2xx|$.__type
Issue-Key: imagebuilder|GetInfrastructureConfiguration|4xx->2xx|$.message
Issue-Key: imagebuilder|GetInfrastructureConfiguration|4xx->2xx|status
Issue-Key: imagebuilder|CreateInfrastructureConfiguration|2xx->4xx|$.clientToken
Issue-Key: imagebuilder|CreateInfrastructureConfiguration|2xx->4xx|$.infrastructureConfigurationArn
Issue-Key: imagebuilder|CreateInfrastructureConfiguration|2xx->4xx|$.requestId
Issue-Key: imagebuilder|CreateInfrastructureConfiguration|2xx->4xx|status
Issue-Key: imagebuilder|CreateImageRecipe|2xx->4xx|$.clientToken
Issue-Key: imagebuilder|CreateImageRecipe|2xx->4xx|$.imageRecipeArn
Issue-Key: imagebuilder|CreateImageRecipe|2xx->4xx|$.latestVersionReferences
Issue-Key: imagebuilder|CreateImageRecipe|2xx->4xx|$.requestId
Issue-Key: imagebuilder|CreateImageRecipe|2xx->4xx|status
Issue-Key: imagebuilder|CreateComponent|2xx->2xx|$.componentBuildVersionArn
Issue-Related: 20261031T232326Z-imagebuilder-image-builder-component-and-image-recipe-responses-omit.md
Issue-Commit: 3879dd05c1f9087ace2750498c91b002c8086d49

# Image Builder delete and recreate lifecycle leaves stored resource state inconsistent

> _Note: this issue was identified against the code base at commit `3879dd05c1f9087ace2750498c91b002c8086d49`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteImageRecipe

## Description
ONE root cause with SEVEN causes: the imagebuilder emulator's resource store does not converge with AWS across a delete/recreate sequence. Deletes that AWS performs are refused with 400, a delete that AWS applies leaves the resource still readable, creates that AWS accepts after a delete are refused as conflicting, and component build-version numbering counts rejected attempts. There are 7 separate causes below; all 7 must be closed.

Cause 1 of 7: DeleteImageRecipe on an existing, now-unreferenced recipe is refused. AWS: Smithy DeleteImageRecipe is DELETE /DeleteImageRecipe, success @http code 200, output members requestId and imageRecipeArn (pattern ^arn:aws[^:]*:imagebuilder:[^:]+:(?:[0-9]{12}|aws):image-recipe/[a-z0-9-_]+/(?:[0-9]+|x)\\.(?:[0-9]+|x)\\.(?:[0-9]+|x)$) — it echoes the deleted ARN and a requestId. Emulator: 400, no imageRecipeArn, no requestId, so the recipe cannot be cleaned up once the dependent pipeline is gone. Trace: imagebuilder-ibp-traj-cleanup-in-dependency-order#task#14.

Cause 2 of 7: DeleteComponent on an existing, now-unreferenced component is refused. AWS: success 200 with output members requestId and componentBuildVersionArn echoing the request's ARN. Emulator: 400 with no body members. Trace: imagebuilder-ibp-traj-cleanup-in-dependency-order#task#20.

Cause 3 of 7: the same DeleteComponent refusal on a teardown path (a component created earlier in the trajectory), confirming it is not specific to one recipe/pipeline shape: AWS answers 200, the emulator answers 400. Trace: imagebuilder-ibp-traj-recipe-reference-mistakes#teardown#13.

Cause 4 of 7: GetInfrastructureConfiguration still serves an infrastructure configuration after it was deleted. AWS: 404 with $.__type 'ResourceNotFoundException' (Smithy: client error, @httpError 404) and message "The resource ARN '<arn>' was not found." — fixed text "The resource ARN '" and "' was not found.", with the ARN taken from the request's infrastructureConfigurationArn. Emulator: 200 with the configuration body, i.e. the delete was acknowledged but never applied to the store. Trace: imagebuilder-ibp-traj-cleanup-in-dependency-order#task#17.

Cause 5 of 7: CreateInfrastructureConfiguration is refused after the name became creatable. AWS: success 200 with requestId, clientToken and infrastructureConfigurationArn (pattern ^arn:aws[^:]*:imagebuilder:[^:]+:(?:[0-9]{12}|aws):infrastructure-configuration/[a-z0-9-_]+$). Emulator: 400, none of those members present — the earlier rejected attempt left the name occupied. Trace: imagebuilder-ibp-traj-infra-config-before-instance-profile#task#5.

Cause 6 of 7: CreateImageRecipe is refused for a name/version AWS accepts. AWS: success 200 with requestId, clientToken, imageRecipeArn and latestVersionReferences (latestVersionArn/latestMajorVersionArn/latestMinorVersionArn/latestPatchVersionArn, each derived from the created ARN by replacing the version components with x/major.x.x/major.minor.x/exact). Emulator: 400 with none of those members, because an earlier invalid attempt was stored. Trace: imagebuilder-ibp-traj-recipe-reference-mistakes#task#8.

Cause 7 of 7: CreateComponent's build-version counter advances for attempts that did not create anything. AWS: componentBuildVersionArn ends in the build number of the component versions that actually exist, so the first successful create of a name at 1.0.0 is '.../1.0.0/1'; the variable parts are the component name, semantic version and the build number derived from stored state. Emulator: returns '.../1.0.0/2' for that first successful create (and omits latestVersionReferences). Trace: imagebuilder-ibp-traj-component-request-mistakes#task#6.

Why: callers treat create/delete as the primitives that make a workflow repeatable. A delete that returns 400 makes teardown impossible, so test suites and CI loops leak resources and fail on the next run; a delete that returns success but leaves the resource readable makes 'wait until gone' loops spin forever and makes a caller believe a configuration is deleted when it still resolves. A create refused for a free name blocks the retry-after-fixing-the-request flow AWS supports. And ARNs are durable identifiers: a build version of /2 where AWS produces /1 means every ARN a client stored, logged or passed to a recipe points at nothing on AWS.

Fix: the imagebuilder state layer and the delete/create handlers — delete_image_recipe/delete_component must remove the resource and return the modeled 200 response (echoed ARN + requestId) once no dependents remain, delete_infrastructure_configuration must actually evict from the store so the get path resolves to the 404 not-found error, create_infrastructure_configuration/create_image_recipe must not reserve a name on a rejected request, and the component build-version allocator in state.rs must number from committed versions only.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator:
- imagebuilder-ibp-traj-cleanup-in-dependency-order#task#14 (DeleteImageRecipe): status expected 200, got 400; $.imageRecipeArn expected the recipe ARN, got None; $.requestId missing.
- imagebuilder-ibp-traj-cleanup-in-dependency-order#task#20 (DeleteComponent): status expected 200, got 400; $.componentBuildVersionArn expected the component build-version ARN, got None; $.requestId missing.
- imagebuilder-ibp-traj-recipe-reference-mistakes#teardown#13 (DeleteComponent): status expected 200, got 400.
- imagebuilder-ibp-traj-cleanup-in-dependency-order#task#17 (GetInfrastructureConfiguration): status expected 404, got 200; $.__type expected 'ResourceNotFoundException', got None; $.message expected "The resource ARN 'arn:aws:imagebuilder:us-west-2:000000000000:infrastructure-configuration/<name>' was not found.", got None.
- imagebuilder-ibp-traj-infra-config-before-instance-profile#task#5 (CreateInfrastructureConfiguration): status expected 200, got 400; $.requestId, $.clientToken and $.infrastructureConfigurationArn all missing.
- imagebuilder-ibp-traj-recipe-reference-mistakes#task#8 (CreateImageRecipe): status expected 200, got 400; $.clientToken, $.imageRecipeArn, $.latestVersionReferences and $.requestId all missing.
- imagebuilder-ibp-traj-component-request-mistakes#task#6 (CreateComponent): $.componentBuildVersionArn expected '.../sim2real-probe-ibp-comp-84c39d61/1.0.0/1', got '.../sim2real-probe-ibp-comp-84c39d61/1.0.0/2'; $.latestVersionReferences missing.
Model (smithy_query): DeleteImageRecipe/DeleteComponent http code 200 with output members requestId + the echoed ARN; ResourceNotFoundException is client/httpError 404.

## Steps to Reproduce
1. CreateComponent(name=A, semanticVersion='1.0.0', platform='Linux', data=<valid document>, clientToken=<uuid>) -> expect componentBuildVersionArn ending '/1.0.0/1'. Precede it with one request the service rejects (e.g. a document with no phases) and observe the emulator returns '/1.0.0/2'.
2. CreateImageRecipe(name=B, semanticVersion='1.0.0', parentImage=<valid AMI>, components=[A], clientToken=<uuid>) and CreateImagePipeline referencing B; then delete the pipeline, then DeleteImageRecipe(<B arn>) — expect 200 echoing imageRecipeArn and requestId; the emulator answers 400.
3. DeleteComponent(<A arn>) once nothing references it — expect 200 echoing componentBuildVersionArn and requestId; the emulator answers 400.
4. CreateInfrastructureConfiguration(name=C, instanceProfileName=<existing profile>, clientToken=<uuid>) after a prior attempt with the same name was rejected — expect 200 with infrastructureConfigurationArn/clientToken/requestId; the emulator answers 400.
5. DeleteInfrastructureConfiguration(<C arn>) then GetInfrastructureConfiguration(<C arn>) — expect 404 ResourceNotFoundException "The resource ARN '<arn>' was not found."; the emulator answers 200 with the configuration.
6. CreateImageRecipe(name=B, semanticVersion='1.0.0', parentImage=<valid AMI>) after a prior attempt with an invalid parentImage was rejected — expect 200; the emulator answers 400.
7. Re-replay the fixture steps listed in Traces.
