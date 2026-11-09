Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__dsql.deploy-dsql-cluster-round-trip-and-delete cloudformation__dsql.deploy-dsql-cluster-invalid-kms-key-rejected-and-rolled-back
Issue-Key: cloudformation__dsql|CreateStack|ValidationError
Issue-Commit: e85d2009e41cfda8a7867f84cab4a050d23db70a

# cloudformation__dsql: CloudFormation does not register AWS::DSQL::Cluster, so every DSQL stack is refused at CreateStack

> _Note: this issue was identified against the code base at commit `e85d2009e41cfda8a7867f84cab4a050d23db70a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: AWS::DSQL::Cluster is not a recognized resource type.
AWS: AWS::DSQL::Cluster is a public CloudFormation resource type, so CreateStack accepts a template declaring it and provisions the cluster; an invalid KmsEncryptionKey then fails the resource and rolls the stack back.
Emulator: CreateStack answers HTTP 400 `ValidationError` with `Template format error: Unrecognized resource types: [AWS::DSQL::Cluster]`. The fixed text is "Template format error: Unrecognized resource types: " followed by the bracketed type list taken from the request.
Why: DSQL clusters cannot be deployed through CloudFormation at all, and the negative path (a bad KMS key must roll the stack back) can never be observed.
Fix: register AWS::DSQL::Cluster in the CloudFormation resource-type registry and add its resource handler.
Traces: the two failing checks below.

## Evidence
botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::DSQL::Cluster]. Expected: CreateStack accepted (HTTP 200 with a StackId) and the cluster provisioned.

## Steps to Reproduce
1. Build and boot the emulator, then replay the behavioral suite.
2. CreateStack with a template declaring an AWS::DSQL::Cluster resource.
3. Observe the ValidationError instead of a StackId.
