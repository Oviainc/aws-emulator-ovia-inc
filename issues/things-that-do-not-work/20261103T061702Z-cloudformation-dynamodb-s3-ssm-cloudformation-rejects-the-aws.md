Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__dynamodb__s3__ssm.cdk-default-deploy-all-two-stacks-share-an-export
Issue-Key: cloudformation__dynamodb__s3__ssm.cdk-default-deploy-all-two-stacks-share-an-export|run#16|AssertionError|assert rc == 0, f"`cdk deploy --all` exited {rc}; output tail {out[-800:]!r}"
Issue-Commit: 39390cfa3000e408bc0d1db242197466d1e66569

# cloudformation__dynamodb__s3__ssm: CloudFormation rejects the AWS::CDK::Metadata resource type, so a default `cdk deploy --all` of two stacks sharing an export cannot be deployed

> _Note: this issue was identified against the code base at commit `39390cfa3000e408bc0d1db242197466d1e66569`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateChangeSet

## Description
=== Cause 1 of 1: the AWS::CDK::Metadata resource type is refused as unrecognized ===
AWS: recorded behavior only — the Smithy model does not enumerate resource types. `AWS::CDK::Metadata` is the analytics resource the CDK adds to every default-synthesizer stack; real CloudFormation accepts it and provisions it as a no-op resource, so the stack reaches CREATE_COMPLETE with an AWS::CDK::Metadata resource among its StackResources. The recording shows the whole `cdk deploy --all` of a producer stack and a consumer stack that imports the producer's export completing, with the consumer writing the imported table name into an SSM parameter and the table serving a write.
Emulator: change-set creation for the producer stack is refused with `Template format error: Unrecognized resource types: [AWS::CDK::Metadata]`, so `cdk deploy --all` exits 1 and neither stack is created. Fixed text of that refusal is `Template format error: Unrecognized resource types: [` ... `]`, with the bracketed list holding the resource types from the submitted template — here the type the emulator's resource-type registry does not know.
Why: every CDK app synthesized with the default synthesizer (the default for `cdk deploy`) carries this resource, so the emulator refuses essentially all default CDK deployments up front — the deploy never reaches any of the resources the user actually declared.
Fix: the template-validation stage's resource-type registry in the CreateChangeSet/CreateStack handlers must accept AWS::CDK::Metadata and provision it as a no-op resource that appears in DescribeStackResources. Note: open issue 20261102T090835Z-cloudformation-dynamodb-lambda-logs-s3-cloudformation-rejects-the-aws-cdk-metadata-resource-type.md records this same refusal under the cloudformation__dynamodb__lambda__logs__s3 heading, but it does not carry this check's replay key, so this check would otherwise be left unverified.
Traces: cloudformation__dynamodb__s3__ssm.cdk-default-deploy-all-two-stacks-share-an-export

## Evidence
cloudformation__dynamodb__s3__ssm.cdk-default-deploy-all-two-stacks-share-an-export: expected `cdk deploy --all` to exit 0 with both stacks CREATE_COMPLETE and an AWS::CDK::Metadata resource in each; got AssertionError: `cdk deploy --all` exited 1; output tail '... <producer-stack>: creating CloudFormation changeset... / Template format error: Unrecognized resource types: [AWS::CDK::Metadata]'.

## Steps to Reproduce
Replay the behavioral suite and re-run cloudformation__dynamodb__s3__ssm.cdk-default-deploy-all-two-stacks-share-an-export: `cdk bootstrap` with a qualifier, then `cdk deploy --all` a two-stack app using the DEFAULT synthesizer (producer stack with a DynamoDB table exporting its name, consumer stack importing it into an SSM parameter). The change-set creation for the first stack is refused with 'Template format error: Unrecognized resource types: [AWS::CDK::Metadata]'. Equivalently, CreateChangeSet/CreateStack with a template containing a resource of type AWS::CDK::Metadata.
