Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: imagebuilder.create-component-get-roundtrip imagebuilder.create-component-with-kms-encryption-roundtrip imagebuilder.delete-component-referenced-by-recipe-rejected imagebuilder.create-image-recipe-get-roundtrip
Issue-Shape-Id: com.amazonaws.imagebuilder#imagebuilder
Issue-Key: imagebuilder.create-component-get-roundtrip|run#15|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException after delete, got {code!r}"
Issue-Key: imagebuilder.create-component-with-kms-encryption-roundtrip|run#23|AssertionError|assert code == "ResourceNotFoundException", (
Issue-Key: imagebuilder.delete-component-referenced-by-recipe-rejected|run#5|AssertionError|assert code_after == "ResourceNotFoundException", (
Issue-Key: imagebuilder.create-image-recipe-get-roundtrip|run#16|AssertionError|assert code == "ResourceNotFoundException", (
Issue-Commit: e3579db6a1835cfcd1e7033eb2fb9f64dc458cca

# Image Builder not-found lookups refuse 400 InvalidRequestException instead of 404 ResourceNotFoundException

> _Note: this issue was identified against the code base at commit `e3579db6a1835cfcd1e7033eb2fb9f64dc458cca`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetComponent

## Description
ONE root cause in the imagebuilder emulator's resource-lookup paths: when the ARN a request names resolves to nothing (because it was deleted, or was never created), the handler refuses with the operation's modeled InvalidRequestException (HTTP 400) instead of Image Builder's not-found error. Every read-after-delete and mutate-after-delete assertion in the suite fails on this one branch.

Cause 1 of 1: a resolved-to-nothing ARN is answered 400 InvalidRequestException rather than 404 ResourceNotFoundException.
AWS: Smithy: com.amazonaws.imagebuilder#ResourceNotFoundException is a client error with @httpError 404, documented "At least one of the resources referenced by your request does not exist.", carrying a single `message` member. The operation error lists for GetComponent, GetImageRecipe and DeleteComponent do NOT include it (they list CallRateLimitExceededException 429, ClientException 400, ForbiddenException 403, InvalidRequestException 400, ServiceException 500, ServiceUnavailableException 503; DeleteComponent additionally ResourceDependencyException 400), so the 404 for a missing resource is recorded behavior only. The recorded message is "The following required resource '<ResourceType>' cannot be found: '<arn>'." — its fixed text is "The following required resource '", "' cannot be found: '" and the trailing "'."; its variable parts are the resource-type token (Component, ImageRecipe) and the ARN taken verbatim from the request member (componentBuildVersionArn / imageRecipeArn). The same 404 ResourceNotFoundException is what a mutation on an absent resource gets (a repeat DeleteComponent on an already-deleted component).
Emulator: GetComponent returns the modeled InvalidRequestException — botocore reports Code 'InvalidRequestException' at HTTP 400 — for a component the store cannot resolve; the sibling GetImageRecipe/DeleteImageRecipe and the repeat DeleteComponent paths behave the same way. The open wire issue 20261031T232326Z-imagebuilder-image-builder-error-responses-use-the-wrong-status.md records the recipe side of the same branch answering 400 with the emulator-authored sentence 'The image recipe <arn> does not exist.'; that issue is bound to GetImageRecipe only and does not cover the component paths or the delete-on-absent paths named here.
Why: every SDK and every caller discriminates this condition by status and error type. A client that catches ResourceNotFoundException (or polls a delete to completion by treating 404 as 'gone', or makes teardown idempotent by swallowing ResourceNotFoundException) instead receives a 400 InvalidRequestException, which it classifies as a malformed request and surfaces as a hard failure; retries and cleanup loops abort. The message text users and assertions see is also not AWS's.
Fix: the lookup/resolve branch of the imagebuilder component and image-recipe handlers (the `let Some(...) = view else` arm in services/service-com-amazonaws-imagebuilder-imagebuilder/src/ops/get_component.rs, and the equivalent arms in get_image_recipe.rs, delete_component.rs and delete_image_recipe.rs) must return the not-found error (404, ResourceNotFoundException error type) with AWS's message template, instead of InvalidRequestException; the shared message builder in src/state.rs must produce that template.
Traces: imagebuilder.create-component-get-roundtrip (GetComponent after DeleteComponent), imagebuilder.create-component-with-kms-encryption-roundtrip (GetComponent and repeat DeleteComponent after delete), imagebuilder.delete-component-referenced-by-recipe-rejected (GetComponent on a deleted unreferenced component), imagebuilder.create-image-recipe-get-roundtrip (GetImageRecipe on a deleted recipe).

## Evidence
imagebuilder.create-component-get-roundtrip: after DeleteComponent, GetComponent(componentBuildVersionArn=<arn>) — expected Code 'ResourceNotFoundException' (404) with message "The following required resource 'Component' cannot be found: '<arn>'.", got Code 'InvalidRequestException' (AssertionError: expected ResourceNotFoundException after delete, got 'InvalidRequestException').
imagebuilder.create-component-with-kms-encryption-roundtrip: AssertionError: expected ResourceNotFoundException reading a deleted component, got 'InvalidRequestException'.
imagebuilder.delete-component-referenced-by-recipe-rejected: AssertionError: expected ResourceNotFoundException reading a deleted component, got 'InvalidRequestException'.
imagebuilder.create-image-recipe-get-roundtrip: AssertionError: expected GetImageRecipe on deleted arn to be not-found, got 'InvalidRequestException'.
Source: services/service-com-amazonaws-imagebuilder-imagebuilder/src/ops/get_component.rs lines 182-191 return GetComponentErrorModeled::InvalidRequestException with crate::state::component_not_found_message(..) when resolve_component yields None.

## Steps to Reproduce
1. CreateComponent(name=<unique>, semanticVersion='1.0.0', platform='Linux', data=<valid YAML document>, clientToken=<uuid>) and keep componentBuildVersionArn.
2. DeleteComponent(componentBuildVersionArn=<arn>).
3. GetComponent(componentBuildVersionArn=<arn>) — observe HTTP status, the x-amzn-errortype/Code and the message. Expect 404 ResourceNotFoundException with "The following required resource 'Component' cannot be found: '<arn>'."; the emulator answers 400 InvalidRequestException.
4. DeleteComponent(componentBuildVersionArn=<arn>) a second time — same expectation, same divergence.
5. Repeat with CreateImageRecipe/DeleteImageRecipe then GetImageRecipe(imageRecipeArn=<arn>).
