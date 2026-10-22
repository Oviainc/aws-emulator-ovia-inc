Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: marketplace-deployment.list-tags-for-resource-nonexistent-arn-not-found
Issue-Shape-Id: com.amazonaws.marketplacedeployment#AWSMPDeploymentParametersService
Issue-Key: marketplace-deployment.list-tags-for-resource-nonexistent-arn-not-found|run#5|AssertionError|assert code == "NotFoundException", f"expected NotFoundException, got {code}"
Issue-Commit: f2a9c91c50f7e55687472cf5b456a197c6c3ca83

# ListTagsForResource rejects an unknown deployment-parameter ARN with ResourceNotFoundException instead of AWS's NotFoundException

> _Note: this issue was identified against the code base at commit `f2a9c91c50f7e55687472cf5b456a197c6c3ca83`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTagsForResource

## Description
Cause 1 of 1: the not-found error CODE on ListTagsForResource is wrong.
AWS: Smithy (com.amazonaws.marketplacedeployment#ListTagsForResource) binds GET /tags/{resourceArn} and lists ResourceNotFoundException (httpError 404) among its errors, but the replayed recording — confirmed against real AWS by the check — shows that an ARN matching this service's own DeploymentParameter resource-ARN template ("DeploymentParameter:catalogs/{catalog}/products/{productId}/{deploymentParameterId}", the model's own example shape) that was never created is rejected with error code NotFoundException, not the modeled ResourceNotFoundException. The recorded message is "Resource was not found" (an example; the requirement here is the error code, since the rule behind that fixed text is not documented). The one thing AWS never does is answer 200 with an empty tag map, which would mask a bad ARN as "no tags yet".
Emulator: returns error code ResourceNotFoundException for exactly that request. src/ops/list_tags_for_resource.rs handle() looks the ARN up in store.parameters and, on miss, constructs ListTagsForResourceErrorModeled::ResourceNotFoundException via crate::state::parameter_not_found(&input.resource_arn) — the modeled shape rather than the code AWS actually emits on this path.
Why: clients match on the error code. Botocore/SDK error-handling branches (and retry/catch blocks written against the real service, e.g. `except md.exceptions.NotFoundException`) do not fire, so a bad or stale deployment-parameter ARN surfaces as an unhandled exception instead of the expected not-found branch.
Fix: ListTagsForResource's error path in services/service-com-amazonaws-marketplacedeployment-awsmpdeploymentparametersservice/src/ops/list_tags_for_resource.rs (the store.parameters miss arm, ~line 154) together with the error constructor in src/state.rs (parameter_not_found): emit the NotFoundException code this service's recorded responses use for an unknown resource ARN.
Traces: marketplace-deployment.list-tags-for-resource-nonexistent-arn-not-found.

## Evidence
Expected (recorded real-AWS behavior): Error.Code == 'NotFoundException' (message example: "Resource was not found").
Actual: Error.Code == 'ResourceNotFoundException'.
Replay: AssertionError: expected NotFoundException, got ResourceNotFoundException (check_marketplace_deployment_list_tags_for_resource_nonexistent_arn_not_found.py line 34).
Source: src/ops/list_tags_for_resource.rs returns ListTagsForResourceErrorModeled::ResourceNotFoundException(crate::state::parameter_not_found(&input.resource_arn)) on a store miss.

## Steps to Reproduce
1. Against the running emulator, region us-east-1, client 'marketplace-deployment'.
2. Build an ARN in this service's own DeploymentParameter shape for a resource that was never created: arn:aws:aws-marketplace:us-east-1:<account>:DeploymentParameter:catalogs/AWSMarketplace/products/prod-<random>/dp-<random>
3. Call ListTagsForResource(resourceArn=<that ARN>).
4. Expected ClientError with Code 'NotFoundException'; observed Code 'ResourceNotFoundException'.
Replay: marketplace-deployment.list-tags-for-resource-nonexistent-arn-not-found
