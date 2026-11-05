Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__iam__imagebuilder.ibp-delete-resources-referenced-by-a-pipeline-rejected
Issue-Key: ec2__iam__imagebuilder.ibp-delete-resources-referenced-by-a-pipeline-rejected|_refusal#12|AssertionError|raise AssertionError(f"the call succeeded; real AWS rejects it. Response: {resp!r}")
Issue-Commit: 4f520f1edeaf87046d1e59f82118bd1eb9149075

# Image Builder DeleteImageRecipe ignores the image-pipeline dependency: it deletes the recipe and answers HTTP 200 where AWS refuses 400 ResourceDependencyException

> _Note: this issue was identified against the code base at commit `4f520f1edeaf87046d1e59f82118bd1eb9149075`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteImageRecipe

## Description
ONE net-new root cause, grounded in one failing behavioral check.

Scope note on already-open issues (neither carries this check's replay key, so this check is net-new): 20261102T085510Z-imagebuilder-image-builder-accepts-creates-and-deletes-that-aws.md (wire corpus) describes Image Builder accepting creates and deletes AWS refuses with HTTP 200 in general, and 20261103T091826Z-imagebuilder-2-net-new-delete-path-divergences-deleteinfrastructureconfiguration.md covers DeleteComponent's dependency-message wording and DeleteInfrastructureConfiguration's 501 stub. Neither covers DeleteImageRecipe's missing dependency guard, which is what this check reaches.

Cause 1 of 1: DeleteImageRecipe performs no reverse-reference check, so an image recipe that an image pipeline still references is deleted and the call is answered 200 instead of being refused ResourceDependencyException.
AWS: Smithy (services/service-com-amazonaws-imagebuilder-imagebuilder/smithy-model/com-amazonaws-imagebuilder-imagebuilder.json): DeleteImageRecipe is DELETE /DeleteImageRecipe, input imageRecipeArn (@httpQuery, required, ImageRecipeArn pattern ^arn:aws[^:]*:imagebuilder:[^:]+:(?:[0-9]{12}|aws):image-recipe/[a-z0-9-_]+/(?:[0-9]+|x)\.(?:[0-9]+|x)\.(?:[0-9]+|x)$), success 200 with {imageRecipeArn, requestId}, and it models ResourceDependencyException (client, @httpError 400, documented "You have attempted to mutate or delete a resource with a dependency that prohibits this action. See the error message for more details."). The operation doc says only "Deletes an image recipe.", so the dependency rule itself is recorded behavior only: while an image pipeline references the recipe, the delete is refused HTTP 400 with error code ResourceDependencyException and the message template "Resource dependency error: The resource ARN '<imageRecipeArn>' has other resources depended on it." — the fixed text is "Resource dependency error: The resource ARN '" and "' has other resources depended on it."; the single variable part is the image-recipe ARN taken verbatim from the request (AWS does not name the referencing pipeline). The refused delete has no effect: GetImageRecipe on that ARN still answers 200. After the referencing pipeline is deleted (DeleteImagePipeline), the same DeleteImageRecipe succeeds, and GetImageRecipe on the dead ARN is then refused ResourceNotFoundException with "The following required resource 'ImageRecipe' cannot be found: '<imageRecipeArn>'." (fixed text "The following required resource 'ImageRecipe' cannot be found: '" and "'.", ARN from the request). This is the same dependency-error template the already-filed DeleteComponent cause documents.
Emulator: the call is accepted while the pipeline still references the recipe — HTTP 200 with a body carrying requestId and imageRecipeArn, and the recipe record is removed from the store, so the referencing pipeline is left pointing at a recipe that no longer exists. No ResourceDependencyException is ever produced by this operation.
Why: a client cannot distinguish "still referenced by a pipeline" from "deleted". Dependency-ordered teardown appears to succeed out of order, so cleanup code that relies on the refusal to sequence DeleteImagePipeline before DeleteImageRecipe never exercises that ordering, and provisioning code that probes the refusal to discover live references sees none. Worse, the emulator's own state is then inconsistent: a pipeline survives whose imageRecipeArn resolves to nothing, which no real account can reach.
Fix: the dependency guard belongs in the DeleteImageRecipe handler (services/service-com-amazonaws-imagebuilder-imagebuilder/src/ops/delete_image_recipe.rs), which today goes straight from the store lookup to remove_image_recipe with no reference check, together with the reverse-reference lookup in the service state (image pipelines whose imageRecipeArn is the requested recipe). It must return the modeled ResourceDependencyException (400) with AWS's message template above, leaving the recipe intact, and share the dependency-message helper with DeleteComponent/DeleteInfrastructureConfiguration rather than inventing a second wording.
Traces: ec2__iam__imagebuilder.ibp-delete-resources-referenced-by-a-pipeline-rejected.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator; service heading in the suite report: ### ec2__iam__imagebuilder.

[mismatch] ec2__iam__imagebuilder.ibp-delete-resources-referenced-by-a-pipeline-rejected:
The check creates a component, an image recipe, an infrastructure configuration, a distribution configuration and an image pipeline referencing all three, then asserts each of the three deletes is refused ResourceDependencyException while the pipeline lives. The infrastructure-configuration and distribution-configuration deletes each raised a client error (so the run got past them); the image-recipe delete SUCCEEDED:

  AssertionError: the call succeeded; real AWS rejects it. Response: {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/json', ...}}, 'requestId': '<per-run id>', 'imageRecipeArn': 'arn:aws:imagebuilder:<region>:<account>:image-recipe/<generated-name>/1.0.0'}

Expected for that step (template; the ARN is the one from the request, the name/version are per-run values):
  HTTP 400, Error.Code = 'ResourceDependencyException',
  Error.Message = "Resource dependency error: The resource ARN '<imageRecipeArn>' has other resources depended on it."
and GetImageRecipe(<imageRecipeArn>) still 200 afterwards.

Handler evidence: services/service-com-amazonaws-imagebuilder-imagebuilder/src/ops/delete_image_recipe.rs handle() reads the store, calls state.lock().remove_image_recipe(&input.image_recipe_arn, &region), returns ResourceNotFoundException only when nothing was stored, and otherwise returns DeleteImageRecipeOutput{request_id, image_recipe_arn} — there is no check for image pipelines referencing the recipe anywhere on the path.

Model evidence (smithy_query, query="operation", operation="DeleteImageRecipe", model_path=services/service-com-amazonaws-imagebuilder-imagebuilder/smithy-model/com-amazonaws-imagebuilder-imagebuilder.json): errors include ResourceDependencyException (errorKind client, httpError 400) alongside ClientException, InvalidRequestException, ForbiddenException, CallRateLimitExceededException, ServiceException, ServiceUnavailableException; http {method: DELETE, uri: /DeleteImageRecipe, code: 200}.

## Steps to Reproduce
1. iam.CreateRole + CreateInstanceProfile + AddRoleToInstanceProfile for a unique name (the infrastructure configuration needs a profile).
2. imagebuilder.CreateComponent(name=<unique>, semanticVersion='1.0.0', platform='Linux', data=<valid document with build and validate phases>).
3. imagebuilder.CreateImageRecipe(name=<unique>, semanticVersion='1.0.0', parentImage='arn:aws:imagebuilder:<region>:aws:image/amazon-linux-2-x86/x.x.x', components=[{componentArn: <step 2>}]) -> note imageRecipeArn.
4. imagebuilder.CreateInfrastructureConfiguration(name=<unique>, instanceProfileName=<step 1>, clientToken=<uuid>).
5. imagebuilder.CreateDistributionConfiguration(name=<unique>, distributions=[{region: <a region other than the build region>, ...}], clientToken=<uuid>).
6. imagebuilder.CreateImagePipeline(name=<unique>, imageRecipeArn=<step 3>, infrastructureConfigurationArn=<step 4>, distributionConfigurationArn=<step 5>, status='DISABLED', clientToken=<uuid>).
7. imagebuilder.DeleteImageRecipe(imageRecipeArn=<step 3>) — expect HTTP 400 ResourceDependencyException with message "Resource dependency error: The resource ARN '<step 3 arn>' has other resources depended on it.". The emulator answers HTTP 200 and removes the recipe.
8. imagebuilder.GetImageRecipe(imageRecipeArn=<step 3>) — expect 200 (the refused delete had no effect); after the emulator's accepted delete this is instead a not-found.
9. imagebuilder.DeleteImagePipeline(imagePipelineArn=<step 6>), then DeleteImageRecipe(<step 3>) — expect 200, and GetImageRecipe(<step 3>) then refused ResourceNotFoundException "The following required resource 'ImageRecipe' cannot be found: '<step 3 arn>'.".
10. Re-replay ec2__iam__imagebuilder.ibp-delete-resources-referenced-by-a-pipeline-rejected to confirm the fix.
