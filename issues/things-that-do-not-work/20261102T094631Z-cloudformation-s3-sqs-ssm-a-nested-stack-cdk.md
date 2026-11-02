Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__s3__sqs__ssm.cdk-default-deploy-nested-stack-queue-url-reaches-parent-output
Issue-Key: cloudformation__s3__sqs__ssm.cdk-default-deploy-nested-stack-queue-url-reaches-parent-output|run#15|AssertionError|assert rc == 0, f"`cdk deploy` exited {rc}; output tail {out[-800:]!r}"
Issue-Commit: 977af42ea79b884a07e35b887b5e587a14d941a9

# cloudformation__s3__sqs__ssm: a nested-stack CDK deploy cannot be created — AWS::CDK::Metadata is refused as an unrecognized resource type

> _Note: this issue was identified against the code base at commit `977af42ea79b884a07e35b887b5e587a14d941a9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateChangeSet

## Description
Cause 1 of 1: CloudFormation refuses the AWS::CDK::Metadata resource type that every default-synthesized CDK template (parent and nested) carries, so the nested-stack deploy never reaches a change set.

AWS: AWS::CDK::Metadata is a known CloudFormation resource type provisioned as an inert no-op; a parent template and its nested-stack template both carry it and CreateChangeSet/CreateStack accept them. The refusal class ValidationError 'Template format error: Unrecognized resource types: [...]' is reserved for types CloudFormation does not know. Smithy (cloudformation CreateChangeSet) models no dedicated error for a known type — recorded behavior only.

Emulator: `cdk deploy` of the nested stack fails at 'creating CloudFormation changeset...' with 'Template format error: Unrecognized resource types: [AWS::CDK::Metadata]', after both templates were published, so the nested queue URL never reaches the parent's Outputs.

Why: no default CDK application — least of all one using nested stacks — can be deployed against the emulator; the whole nested Outputs/Ref chain is unreachable.

Fix: register AWS::CDK::Metadata in the CloudFormation resource-type registry / template validator so it validates and provisions as a no-op (the same registry gap reported for the single-stack and legacy-synthesizer deploys).

Traces: cloudformation__s3__sqs__ssm.cdk-default-deploy-nested-stack-queue-url-reaches-parent-output.

## Evidence
Expected: the parent stack reaches CREATE_COMPLETE and its Output carries the nested stack's queue URL.
Actual: 'AssertionError: `cdk deploy` exited 1; output tail ... success: Published Nest Nested Stack Template ... creating CloudFormation changeset...\nTemplate format error: Unrecognized resource types: [AWS::CDK::Metadata]'.

## Steps to Reproduce
1. `cdk deploy` an app whose parent stack declares a NestedStack containing an SQS queue and an Output exporting its URL (default synthesizer, so both templates include AWS::CDK::Metadata). 2. Observe the change-set creation refusal. Replay check cloudformation__s3__sqs__ssm.cdk-default-deploy-nested-stack-queue-url-reaches-parent-output.
