Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecr.deploy-ecr-repository-with-lifecycle-policy-behaves ecr.deploy-repository-lifecycle-policy-enforced
Issue-Shape-Id: com.amazonaws.ecr#AmazonEC2ContainerRegistry_V20150921
Issue-Key: ecr|GetLifecyclePolicy|LifecyclePolicyNotFoundException
Issue-Key: ecr|StartLifecyclePolicyPreview|LifecyclePolicyNotFoundException
Issue-Commit: 5252be52e0cc9f5d73bee5aa197523afb271707f

# ecr: a CloudFormation AWS::ECR::Repository that declares a LifecyclePolicy stores no lifecycle policy — GetLifecyclePolicy and StartLifecyclePolicyPreview answer LifecyclePolicyNotFoundException

> _Note: this issue was identified against the code base at commit `5252be52e0cc9f5d73bee5aa197523afb271707f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetLifecyclePolicy

## Description
Cause 1 of 1: the AWS::ECR::Repository resource handler ignores the LifecyclePolicy property.
AWS: when a stack declares AWS::ECR::Repository with LifecyclePolicy.LifecyclePolicyText, the handler puts that policy on the repository, so after CREATE_COMPLETE ecr:GetLifecyclePolicy returns the policy text (and lastEvaluatedAt) and ecr:StartLifecyclePolicyPreview runs a preview against it. Smithy (ECR): GetLifecyclePolicy returns lifecyclePolicyText (@length 100..30720) and raises LifecyclePolicyNotFoundException only when no policy is set; StartLifecyclePolicyPreview raises the same error only when the repository has no policy and none is supplied; PutLifecyclePolicy is the operation the handler must call.
Emulator: the stack reports the repository created, but the policy was never stored: GetLifecyclePolicy answers 400 LifecyclePolicyNotFoundException 'Lifecycle policy does not exist for the repository with name \'<repositoryName>\' in the registry with id \'<registryId>\'' and StartLifecyclePolicyPreview answers the same error. Fixed text 'Lifecycle policy does not exist for the repository with name ' + the requested repository name + ' in the registry with id ' + the registry id from state.
Why: a client that declares retention in its template believes expiry is configured; nothing expires, the policy cannot be read back or previewed, and the refusal wrongly says the repository has no policy.
Fix: the AWS::ECR::Repository resource handler's create/update path (apply LifecyclePolicy by calling PutLifecyclePolicy and keeping it in ECR repository state).
Traces: ecr.deploy-ecr-repository-with-lifecycle-policy-behaves, ecr.deploy-repository-lifecycle-policy-enforced.

## Evidence
1) deploy-ecr-repository-with-lifecycle-policy-behaves — botocore.errorfactory.LifecyclePolicyNotFoundException: An error occurred (LifecyclePolicyNotFoundException) when calling the GetLifecyclePolicy operation: Lifecycle policy does not exist for the repository with name '<repo>' in the registry with id '000000000000'
2) deploy-repository-lifecycle-policy-enforced — botocore.errorfactory.LifecyclePolicyNotFoundException: ... when calling the StartLifecyclePolicyPreview operation: Lifecycle policy does not exist for the repository with name '<repo>' in the registry with id '000000000000'

The stacks declare:
  Type: AWS::ECR::Repository
  Properties:
    RepositoryName: !Ref RepoName
    LifecyclePolicy:
      LifecyclePolicyText: '{"rules":[{"rulePriority":1,...,"action":{"type":"expire"}}]}'

Expected: after CREATE_COMPLETE, GetLifecyclePolicy returns that lifecyclePolicyText and StartLifecyclePolicyPreview returns a preview status instead of a not-found refusal.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. Re-replay the two checks in trace_ids (service ecr).
3. Or: CreateStack with an AWS::ECR::Repository carrying LifecyclePolicy.LifecyclePolicyText, wait for CREATE_COMPLETE, then call GetLifecyclePolicy and StartLifecyclePolicyPreview on that repository.
