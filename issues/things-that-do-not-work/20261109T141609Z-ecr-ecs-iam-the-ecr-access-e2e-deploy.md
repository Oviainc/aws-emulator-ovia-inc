Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecr__ecs__iam.e2e-ecracc-ecs-services-ecr-access-check-lifecycle
Issue-Key: ecr__ecs__iam|GetLifecyclePolicy|LifecyclePolicyNotFoundException
Issue-Commit: 5252be52e0cc9f5d73bee5aa197523afb271707f

# ecr__ecs__iam: the ECR-access e2e deploy phase dies because a stack-declared repository LifecyclePolicy is never stored (GetLifecyclePolicy answers LifecyclePolicyNotFoundException)

> _Note: this issue was identified against the code base at commit `5252be52e0cc9f5d73bee5aa197523afb271707f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetLifecyclePolicy

## Description
Cause 1 of 1: the CloudFormation AWS::ECR::Repository handler does not apply the declared LifecyclePolicy, so the e2e scenario cannot get past its deploy phase.
AWS: a stack-declared AWS::ECR::Repository with LifecyclePolicy.LifecyclePolicyText has that policy put on the repository during create, so a post-deploy ecr:GetLifecyclePolicy returns the policy text. Smithy (ECR): GetLifecyclePolicy returns lifecyclePolicyText and raises LifecyclePolicyNotFoundException only when the repository truly has no policy.
Emulator: phase 1/4 (deploy) of the ECR-access lifecycle aborts with 400 LifecyclePolicyNotFoundException 'Lifecycle policy does not exist for the repository with name \'<repositoryName>\' in the registry with id \'<registryId>\'' — fixed text with the requested repository name and the registry id from state — so none of the later ECS/IAM access phases are reached.
Why: the whole multi-service scenario a client would use to verify ECR access from ECS task roles is unreachable, and the repository silently has no retention despite the template declaring one.
Fix: the AWS::ECR::Repository resource handler's create/update path (call PutLifecyclePolicy for the declared LifecyclePolicy). Same root cause as the ecr-service issue filed in this sweep for ecr.deploy-ecr-repository-with-lifecycle-policy-behaves / ecr.deploy-repository-lifecycle-policy-enforced; filed separately because the failing check is listed under this service.
Traces: ecr__ecs__iam.e2e-ecracc-ecs-services-ecr-access-check-lifecycle.

## Evidence
_PhaseError: [phase 1/4 deploy] LifecyclePolicyNotFoundException: An error occurred (LifecyclePolicyNotFoundException) when calling the GetLifecyclePolicy operation: Lifecycle policy does not exist for the repository with name '<stack>-repository-<suffix>' in the registry with id '000000000000'

Expected: the deploy phase completes — GetLifecyclePolicy returns the lifecyclePolicyText declared by the stack — and the scenario proceeds to its ECS/IAM access phases.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. Re-replay ecr__ecs__iam.e2e-ecracc-ecs-services-ecr-access-check-lifecycle.
3. Or: deploy a stack whose AWS::ECR::Repository declares LifecyclePolicy and call GetLifecyclePolicy on the created repository.
