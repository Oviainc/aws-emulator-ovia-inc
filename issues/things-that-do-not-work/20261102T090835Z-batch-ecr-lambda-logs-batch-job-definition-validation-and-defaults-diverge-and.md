Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: batch__ecr__lambda__logs.refarch-batch-fargate-job-definition-with-ec2-vcpus-rejected batch__ecr__lambda__logs.refarch-batch-job-definition-preserves-untagged-image batch__ecr__lambda__logs.refarch-batch-repository-name-collision-rejected
Issue-Key: batch__ecr__lambda__logs.refarch-batch-fargate-job-definition-with-ec2-vcpus-rejected|_error#6|AssertionError|raise AssertionError(f'the call SUCCEEDED and returned {result!r}; real AWS rejects it')
Issue-Key: batch__ecr__lambda__logs.refarch-batch-job-definition-preserves-untagged-image|run#21|AssertionError|assert props.get('fargatePlatformConfiguration') == {'platformVersion': 'LATEST'}, (
Issue-Key: batch__ecr__lambda__logs|CreateRepository|RepositoryAlreadyExistsException
Issue-Commit: 86c9f1e4438a61fbe37b19dfcd3d77efce3d0679

# Batch job-definition validation and defaults diverge, and the ECR repository namespace is not region-scoped (3 causes)

> _Note: this issue was identified against the code base at commit `86c9f1e4438a61fbe37b19dfcd3d77efce3d0679`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterJobDefinition

## Description
Three net-new root causes. Each must be closed before submitting.

Cause 1 of 3 — RegisterJobDefinition accepts a FARGATE job definition that carries the EC2-style vcpus/memory container properties AWS refuses.
AWS: Smithy (com.amazonaws.batch) RegisterJobDefinition POST /v1/registerjobdefinition, success 200, models ClientException with httpError 400. The model carries no message text for this rule, so this is recorded behavior only: a job definition whose platformCapabilities include FARGATE must express compute as resourceRequirements (VCPU/MEMORY); supplying the legacy containerProperties.vcpus/memory members instead is refused with HTTP 400 / ClientException and nothing is registered.
Emulator: answers HTTP 200 and registers the definition, returning jobDefinitionName / jobDefinitionArn `arn:aws:batch:<region>:<account>:job-definition/<name>:1` / revision 1.
Why: the refusal is the only thing that tells a caller its Fargate job definition is malformed. Accepted, it registers a revision that can never run, and submit-time failures appear far from the request that caused them; IaC that relies on the register call failing reports success for a broken stack.
Fix: the platformCapabilities/containerProperties cross-validation in RegisterJobDefinition's validation layer in the batch crate.
Traces: batch__ecr__lambda__logs.refarch-batch-fargate-job-definition-with-ec2-vcpus-rejected

Cause 2 of 3 — a registered FARGATE job definition does not carry the applied fargatePlatformConfiguration default.
AWS: RegisterJobDefinition/DescribeJobDefinitions; recorded behavior only. When a job definition declares FARGATE platform capabilities and does not set containerProperties.fargatePlatformConfiguration, AWS applies and then reports the default `{"platformVersion": "LATEST"}` on reads of that revision.
Emulator: reports fargatePlatformConfiguration as absent (None) on the registered revision.
Why: a client reading back the definition to learn the platform version it will run on (a deploy diff, a drift detector, a provider's refresh) sees no value, so it either re-applies the field on every refresh or concludes the definition is incomplete.
Fix: the defaulting applied in RegisterJobDefinition and the projection of containerProperties in DescribeJobDefinitions.
Traces: batch__ecr__lambda__logs.refarch-batch-job-definition-preserves-untagged-image

Cause 3 of 3 — ECR's repository-name collision guard is account-wide instead of per account per region.
AWS: Smithy (com.amazonaws.ecr) CreateRepository, models RepositoryAlreadyExistsException. An ECR registry is scoped to one account IN ONE REGION: a repository name taken in us-west-2 is free in us-east-1, and CreateRepository there succeeds, returning a repositoryArn/repositoryUri naming that other region. The duplicate create in the SAME region is refused with RepositoryAlreadyExistsException and the message `The repository with name '<name>' already exists in the registry with id '<accountId>'`.
Emulator: emits that same correct refusal for a CreateRepository issued against a DIFFERENT region's endpoint, so the second region can never hold a repository with that name.
Why: the region is part of a repository's identity (it is in the ARN and in the pull URI). Collapsing all regions onto one registry makes multi-region image promotion impossible and makes a legitimate create fail with an error that says the name is taken when, in the region addressed, it is not.
Fix: key the repository store by (accountId, region) in the ecr crate's state, and scope CreateRepository's collision check and DescribeRepositories/ListImages lookups to the request's region.
Traces: batch__ecr__lambda__logs.refarch-batch-repository-name-collision-rejected

## Evidence
All from the simparity-behavioral-aws-v1 replay against this checkout.

Cause 1 — refarch-batch-fargate-job-definition-with-ec2-vcpus-rejected:
  AssertionError: the call SUCCEEDED and returned {... 'jobDefinitionName': '<name>', 'jobDefinitionArn': 'arn:aws:batch:<region>:<account>:job-definition/<name>:1', 'revision': 1}; real AWS rejects it
  Status 200 where AWS answers 400 ClientException.

Cause 2 — refarch-batch-job-definition-preserves-untagged-image:
  AssertionError: fargatePlatformConfiguration came back None, expected the applied default {"platformVersion": "LATEST"}

Cause 3 — refarch-batch-repository-name-collision-rejected (verdict: error; the exception escaped the check body):
  botocore.errorfactory.RepositoryAlreadyExistsException: An error occurred (RepositoryAlreadyExistsException) when calling the CreateRepository operation: The repository with name '<name>' already exists in the registry with id '<accountId>'
  This was raised by the control step `ecr_other.create_repository(repositoryName=name)` against a SECOND region's client, after the same-region duplicate had already been correctly refused. The repository name and account id above are this run's values filling the message template, not requirements.

## Steps to Reproduce
Boot the emulator from this checkout.

Cause 1: aws batch register-job-definition --job-definition-name t --type container --platform-capabilities FARGATE --container-properties '{"image":"<repo uri>","vcpus":1,"memory":2048,"executionRoleArn":"<role>"}' -> 200 with revision 1; AWS answers 400 ClientException and registers nothing.

Cause 2: register a FARGATE job definition with resourceRequirements and no fargatePlatformConfiguration, then aws batch describe-job-definitions --job-definitions <arn> -> containerProperties has no fargatePlatformConfiguration; AWS reports {"platformVersion":"LATEST"}.

Cause 3: aws --region us-west-2 ecr create-repository --repository-name r1 ; aws --region us-west-2 ecr create-repository --repository-name r1 (correctly refused) ; aws --region us-east-1 ecr create-repository --repository-name r1 -> RepositoryAlreadyExistsException; AWS creates the repository and returns an ARN containing :us-east-1:.

Then re-replay: batch__ecr__lambda__logs.refarch-batch-fargate-job-definition-with-ec2-vcpus-rejected, .refarch-batch-job-definition-preserves-untagged-image, .refarch-batch-repository-name-collision-rejected
