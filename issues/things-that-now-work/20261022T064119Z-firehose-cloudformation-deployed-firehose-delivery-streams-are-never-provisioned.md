Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: firehose.deploy-cfn-delivery-stream-behaves-after-create firehose.deploy-cfn-firehose-stream-becomes-active firehose.deploy-cfn-stack-deletion-removes-delivery-stream firehose.deploy-cfn-stream-accepts-put-record firehose.deploy-cfn-stream-s3-destination-config-applied firehose.deploy-cfn-stream-delivers-to-s3 firehose.deploy-cfn-stream-active-and-delivers-records-to-s3 firehose.deploy-cfn-stream-tags-and-encryption-applied
Issue-Shape-Id: com.amazonaws.firehose#Firehose_20150804
Issue-Key: firehose|DescribeDeliveryStream|ResourceNotFoundException
Issue-Key: firehose|PutRecord|ResourceNotFoundException
Issue-Key: firehose.deploy-cfn-stream-delivers-to-s3|run#12|KeyError|for o in cfn.describe_stacks(StackName=stack_name)["Stacks"][0]["Outputs"]
Issue-Key: firehose.deploy-cfn-stream-active-and-delivers-records-to-s3|run#10|KeyError|bucket_name = outputs["BucketName"]
Issue-Key: firehose.deploy-cfn-stream-tags-and-encryption-applied|run#11|KeyError|bucket_name = outputs["BucketName"]
Issue-Commit: 360c4c1618594432a64f84ba94d0fa368ed8f619

# CloudFormation-deployed Firehose delivery streams are never provisioned and the stack exposes no Outputs (2 causes)

> _Note: this issue was identified against the code base at commit `360c4c1618594432a64f84ba94d0fa368ed8f619`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Every CloudFormation-deployed Firehose check fails before it can assert anything about Firehose: the stack completes but the delivery stream it declares does not exist, and the stack's Outputs are not returned. Two root causes. The failing checks are firehose.* (the firehose service is bootstrapped as service-com-amazonaws-firehose-firehose-20150804), so they are filed here; note that the open CloudFormation issues "DescribeStacks Stack serialization drops Outputs, Parameters, Tags and tracked flag state" (20261022T043551Z-cloudformation-describestacks-stack-serialization-drops-outputs-parameters-tags-and.md) and "DescribeStacks omits the Outputs member for a stack whose template declares Outputs, breaking every CloudFormation-deployed FIS workflow" (20261022T043627Z-fis-describestacks-omits-the-outputs-member-for-a-stack.md) describe a similar Outputs symptom for other services and do not carry these replay keys.

Cause 1 of 2: an AWS::KinesisFirehose::DeliveryStream resource in a stack template is not materialized into Firehose state.
AWS: recorded behavior only (CloudFormation resource provisioning is not expressed in the Firehose Smithy model). The recordings show that after CreateStack on a template declaring an AWS::KinesisFirehose::DeliveryStream and the stack reaching its completed state, DescribeDeliveryStream on the stream name returns the stream (reaching ACTIVE), PutRecord against it succeeds, its S3 destination configuration reflects the template, and deleting the stack removes the stream.
Emulator: DescribeDeliveryStream and PutRecord on the template-declared stream name both answer error code ResourceNotFoundException (HTTP 400; recorded message example "Firehose stream <name> not found"), i.e. the stack never created the resource, so the stream's lifecycle, config application and stack-deletion cleanup can none of them be exercised.
Why: any client that provisions Firehose through infrastructure-as-code gets a stack that reports success while the stream it asked for does not exist — the deployment path is entirely non-functional, and stack-delete cleanup semantics are untestable.
Fix: the CloudFormation stack-create/stack-delete resource provisioner must handle the AWS::KinesisFirehose::DeliveryStream resource type and drive the Firehose service state (create on stack create, delete on stack delete).
Traces: firehose.deploy-cfn-delivery-stream-behaves-after-create, firehose.deploy-cfn-firehose-stream-becomes-active, firehose.deploy-cfn-stack-deletion-removes-delivery-stream, firehose.deploy-cfn-stream-accepts-put-record, firehose.deploy-cfn-stream-s3-destination-config-applied.

Cause 2 of 2: DescribeStacks returns a Stack with no Outputs member for a template that declares Outputs.
AWS: recorded behavior only — for a stack whose template declares an Outputs section (here exporting the Firehose stream name and the destination BucketName), DescribeStacks returns Stacks[0].Outputs as a list of OutputKey/OutputValue pairs containing every declared output.
Emulator: Stacks[0] has no 'Outputs' key at all (KeyError: 'Outputs'), and where the harness reaches further the specific declared key is missing (KeyError: 'BucketName').
Why: the standard way to discover the names a stack generated is to read its Outputs; with the member absent every consumer of a deployed stack fails immediately and cannot address the resources it just created.
Fix: the CloudFormation stack state must record the template's resolved Outputs at create time and the DescribeStacks serializer must emit the Outputs member.
Traces: firehose.deploy-cfn-stream-delivers-to-s3, firehose.deploy-cfn-stream-active-and-delivers-records-to-s3, firehose.deploy-cfn-stream-tags-and-encryption-applied.

## Evidence
Cause 1 — expected: DescribeDeliveryStream/PutRecord succeed on the stack-declared stream after the stack completes. Actual: firehose.deploy-cfn-delivery-stream-behaves-after-create, firehose.deploy-cfn-firehose-stream-becomes-active, firehose.deploy-cfn-stack-deletion-removes-delivery-stream, firehose.deploy-cfn-stream-accepts-put-record — "botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the DescribeDeliveryStream operation: Firehose stream <name> not found"; firehose.deploy-cfn-stream-s3-destination-config-applied — the same ResourceNotFoundException from PutRecord.

Cause 2 — expected: describe_stacks(StackName=...)['Stacks'][0]['Outputs'] is the list of declared outputs. Actual: firehose.deploy-cfn-stream-delivers-to-s3 — "KeyError: 'Outputs'" at `for o in cfn.describe_stacks(StackName=stack_name)["Stacks"][0]["Outputs"]`; firehose.deploy-cfn-stream-active-and-delivers-records-to-s3 and firehose.deploy-cfn-stream-tags-and-encryption-applied — "KeyError: 'BucketName'" (the declared output key is absent).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the behavioral suite.
2. Cause 1: CreateStack with a template declaring an AWS::KinesisFirehose::DeliveryStream (ExtendedS3 destination) plus its bucket and role; wait for the stack to complete; call DescribeDeliveryStream on the declared stream name — observe ResourceNotFoundException. Call PutRecord on it — same. Then DeleteStack and observe there is nothing to clean up.
3. Cause 2: use a template with an Outputs section exporting the stream name and BucketName, CreateStack, then call DescribeStacks and read Stacks[0]['Outputs'] — observe the key is absent.
