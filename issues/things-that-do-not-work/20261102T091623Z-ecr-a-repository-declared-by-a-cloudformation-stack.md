Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecr__ecs__iam.e2e-ecracc-ecs-services-ecr-access-check-lifecycle
Issue-Key: ecr__ecs__iam|GetLifecyclePolicy|RepositoryNotFoundException
Issue-Commit: 3879dd05c1f9087ace2750498c91b002c8086d49

# ecr: a repository declared by a CloudFormation stack does not exist after the stack completes (GetLifecyclePolicy answers RepositoryNotFoundException)

> _Note: this issue was identified against the code base at commit `3879dd05c1f9087ace2750498c91b002c8086d49`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetLifecyclePolicy

## Description
Cause 1 of 1: after a stack that declares an ECR repository deploys, the repository is absent from the registry.
AWS: once the deploy completes, the repository the stack created exists in the registry, so GetLifecyclePolicy on it answers either the policy or LifecyclePolicyNotFoundException ('Lifecycle policy does not exist for the repository with name <name> in the registry with id <account>') — RepositoryNotFoundException is reserved for a repository that does not exist.
Emulator: GetLifecyclePolicy answers RepositoryNotFoundException "The repository with name '<stack-generated-repository-name>' does not exist in the registry with id '<account>'" for the repository the deploy phase just created, so the resource was never materialized under the name the deployment reports.
Why: every later step of an ECR access review (lifecycle policy, repository policy, image scan, task-definition pull) fails on a repository the deployment claims to have created, and the error misattributes the gap to the caller's repository name.
Fix: the AWS::ECR::Repository resource provider must create the repository (and register the physical resource id the stack reports) before the deploy completes; failing that, GetLifecyclePolicy must distinguish a missing policy from a missing repository. See the related open issue 20261022T234922Z-cloudwatch-ecr-ecs-a-cloudformation-stack-declaring-aws-ecr-repository-aws.md, which carries a different check's replay key.
Traces: ecr__ecs__iam.e2e-ecracc-ecs-services-ecr-access-check-lifecycle.

## Evidence
'[phase 1/4 deploy] RepositoryNotFoundException: An error occurred (RepositoryNotFoundException) when calling the GetLifecyclePolicy operation: The repository with name \'sim2real-ecracc-e2e-...-repository-...\' does not exist in the registry with id \'000000000000\''.

## Steps to Reproduce
1. Deploy a stack declaring an AWS::ECR::Repository and wait for it to complete.
2. Call ecr GetLifecyclePolicy with the repository name the stack reports.
3. Or replay ecr__ecs__iam.e2e-ecracc-ecs-services-ecr-access-check-lifecycle.
