Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ecr__iam__lambda__s3__ssm.cdk-bootstrap-qualifier-provisions-real-resources-used-by-deploy
Issue-Key: cloudformation__ecr__iam__lambda__s3__ssm|DescribeRepositories|RepositoryNotFoundException
Issue-Commit: 5bc50a64f180637d452285d750c7aafc588926d7

# `cdk bootstrap` with a custom qualifier does not provision the container-assets ECR repository, so DescribeRepositories answers RepositoryNotFoundException

> _Note: this issue was identified against the code base at commit `5bc50a64f180637d452285d750c7aafc588926d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeRepositories

## Description
Cause 1 of 1: the CDK toolkit bootstrap stack does not create its ECR container-assets repository, so the repository the bootstrap template declares does not exist afterwards.

AWS: `cdk bootstrap --qualifier <q>` deploys the CDKToolkit stack, which provisions a staging S3 bucket, an ECR repository named `cdk-<qualifier>-container-assets-<account>-<region>` and five IAM roles (FilePublishingRole, ImagePublishingRole, LookupRole, DeploymentActionRole, CloudFormationExecutionRole). After the stack reaches CREATE_COMPLETE each of those resources is readable through its owning service's API — in particular ecr DescribeRepositories(repositoryNames=[the container-assets repository]) returns it. Smithy (ecr): DescribeRepositories errors include RepositoryNotFoundException, which AWS returns only for a repository that does not exist; its message is `The repository with name '<name>' does not exist in the registry with id '<registryId>'` (fixed text with the requested name and the registry/account id as the variable parts).

Emulator: after a successful bootstrap, ecr DescribeRepositories for the bootstrap repository raises `RepositoryNotFoundException: The repository with name 'cdk-<qualifier>-container-assets-<account>-<region>' does not exist in the registry with id '<account>'`. The message wording matches AWS's not-found text; the defect is that the repository was never created by the bootstrap stack, so the not-found is returned where the resource should exist.

Why: a client that bootstraps with a custom qualifier cannot publish container image assets — every `cdk deploy` of a Docker-image Lambda, an ECS image asset or any DockerImageAsset fails at the asset-publishing step, and a client inspecting the bootstrap result concludes the bootstrap did not run even though it reported success.

Fix: the AWS::ECR::Repository resource handler in the CloudFormation provisioning path for the bootstrap template (the resource reaches a completed state without being created in ECR's registry state).

Traces: check cloudformation__ecr__iam__lambda__s3__ssm.cdk-bootstrap-qualifier-provisions-real-resources-used-by-deploy.

## Evidence
Expected: after `cdk bootstrap --qualifier <q>` reports success, ecr DescribeRepositories(repositoryNames=['cdk-<q>-container-assets-<account>-<region>']) returns that repository (200), as do the S3 bucket and the five bootstrap roles.

Actual: `botocore.errorfactory.RepositoryNotFoundException: An error occurred (RepositoryNotFoundException) when calling the DescribeRepositories operation: The repository with name 'cdk-q4ef2afb3b-container-assets-000000000000-us-west-2' does not exist in the registry with id '000000000000'`.
Fixed text of the message: `The repository with name '` ... `' does not exist in the registry with id '` ... `'`; the variable parts are the repository name from the request and the registry/account id — both correct here. The divergence is the missing repository, not the wording.

## Steps to Reproduce
1. Run `cdk bootstrap` against the emulator with a custom qualifier (e.g. --qualifier q4ef2afb3b); wait for the CDKToolkit stack to reach CREATE_COMPLETE.
2. List the stack's resources — the AWS::ECR::Repository is reported as created.
3. Call ecr DescribeRepositories(repositoryNames=['cdk-<qualifier>-container-assets-<account>-<region>']) — it raises RepositoryNotFoundException.
