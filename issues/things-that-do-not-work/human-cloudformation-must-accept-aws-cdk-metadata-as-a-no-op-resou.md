# CloudFormation must accept AWS::CDK::Metadata as a no-op resource

## Problem
CloudFormation refuses any template that contains a resource of type `AWS::CDK::Metadata`: CreateChangeSet and CreateStack fail with `Template format error: Unrecognized resource types: [AWS::CDK::Metadata]`. The AWS CDK adds this resource to every synthesized stack (parent and nested, default and legacy synthesizer) unless version reporting is turned off. So a default `cdk deploy` never deploys anything against the emulator.

On AWS this is a built-in, inert resource type. It needs no registration, the change set is created, the stack reaches CREATE_COMPLETE and the resource is listed on the stack.

## What to add
In the CloudFormation template resource-type handling, add `AWS::CDK::Metadata` as a recognised no-op resource:
1. **Validation:** accept the type in CreateChangeSet, CreateStack, UpdateStack, ValidateTemplate and nested-stack templates. Its only property is `Analytics` (string, e.g. `v2:deflate64:...`). It may also carry a `Condition` (CDK emits `CDKMetadataAvailable`, an `Fn::Or` of `Fn::Equals` on `AWS::Region`), and the Condition must be honoured.
2. **Provisioning:** creating it does nothing outside the stack. The resource goes straight to CREATE_COMPLETE and gets a stable physical resource id.
3. **Reporting:** DescribeStackResources, DescribeStackResource, ListStackResources and stack events list it with `LogicalResourceId=CDKMetadata`, `ResourceType=AWS::CDK::Metadata`, `ResourceStatus=CREATE_COMPLETE`. GetTemplate (Original) returns the template unchanged, including `Properties.Analytics`.
4. **Update and delete:** an `Analytics` change updates in place (UPDATE_COMPLETE). Stack delete removes it with no error, and it never blocks delete or rollback.
5. **Guard:** other unknown types must still be refused with the same `Unrecognized resource types: [...]` message.

## Done when
These fidelity tests pass, then the linked issues are re-replayed:
- cloudformation__dynamodb__lambda__logs__s3.cdk-default-deploy-function-writes-granted-table-and-carries-cdk-metadata
- cloudformation__s3__sqs.cdk-default-deploy-queue-with-dlq-redrives-and-carries-cdk-metadata
- cloudformation__s3__sqs__ssm.cdk-default-deploy-nested-stack-queue-url-reaches-parent-output
- cloudformation__sqs__ssm.cdk-default-deploy-legacy-synthesizer-carries-cdk-metadata-without-bootstrap
- cloudformation__dynamodb__s3__ssm.cdk-default-deploy-all-two-stacks-share-an-export

All are in simparity-behavioral-aws-v1. Each asserts `[("AWS::CDK::Metadata", "CREATE_COMPLETE")]` for `CDKMetadata` in DescribeStackResources, and GetTemplate properties `["Analytics"]`.

Quick repro:
`aws cloudformation create-change-set --stack-name t --change-set-name c --template-body '{"Resources":{"CDKMetadata":{"Type":"AWS::CDK::Metadata","Properties":{"Analytics":"v2:deflate64:abc"}}}}'` is accepted on AWS and refused by the emulator.

## Existing issues this covers (same cause)
- issues/things-that-do-not-work/20261102T090835Z-cloudformation-dynamodb-lambda-logs-s3-cloudformation-rejects-the-aws-cdk-metadata-resource-type.md
- issues/things-that-do-not-work/20261102T094631Z-cloudformation-s3-sqs-3-net-new-causes-aws.md (cause 1 of 3)
- issues/things-that-do-not-work/20261102T094631Z-cloudformation-s3-sqs-ssm-a-nested-stack-cdk.md
- issues/things-that-do-not-work/20261102T094631Z-cloudformation-sqs-ssm-2-net-new-causes-aws.md (cause 1 of 2)

Scope is only AWS::CDK::Metadata. The other causes in the two multi-cause issues (Parameter AllowedValues, TemplateBody+TemplateURL message, failed-resource event reason) stay with those issues.

Reference: https://docs.aws.amazon.com/cdk/v2/guide/usage-data.html
