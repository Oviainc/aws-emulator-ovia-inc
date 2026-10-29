Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot__lambda.cfn-deploy-iot-thing-custom-resource-lambda-positive
Issue-Key: iot|DescribeThing|ResourceNotFoundException
Issue-Commit: 221dd3602e5d0e66d71e70da1fb8509b07c948c5

# IoT: a CloudFormation custom resource that creates a thing reports success but the thing does not exist (1 cause)

> _Note: this issue was identified against the code base at commit `221dd3602e5d0e66d71e70da1fb8509b07c948c5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeThing

## Description
One net-new root cause in the iot__lambda group; the replay report marks it neither ALREADY FILED nor OWNED.

Cause 1 of 1 - After a stack with a Lambda-backed custom resource that calls iot:CreateThing reaches its terminal state, DescribeThing for the thing the custom resource was supposed to create answers ResourceNotFoundException.
AWS: the custom resource's provider Lambda runs during stack creation, creates the IoT thing, and signals SUCCESS; DescribeThing then returns the thing (thingName/thingArn/version). The thing exists as long as the stack does.
Emulator: `DescribeThing` is answered `ResourceNotFoundException` with the message `Thing <account-id>:<thing-name> not found.` - fixed text `Thing ` ... ` not found.`, with the account id and thing name taken from the request/state. The thing was never created, because the custom resource's provider was never really executed (the same emulator gap that refuses Invoke), yet the stack outcome did not surface that failure.
Why: a client that provisions IoT resources through a custom resource believes provisioning succeeded and then every downstream call against the thing fails with a 404 - the stack and the resource state disagree.
Fix: the CloudFormation custom-resource provider path (actually invoke the provider Lambda and fail the stack when it does not signal SUCCESS) and the IoT thing state it is expected to write; the DescribeThing handler itself behaves correctly for an absent thing.
Traces: iot__lambda.cfn-deploy-iot-thing-custom-resource-lambda-positive.

## Evidence
`botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the DescribeThing operation: Thing 000000000000:sim2real_xsvc_thing_<id> not found.` after the stack had been deployed; AWS returns the thing's description instead.

## Steps to Reproduce
1. git_clone(); 2. run_behavioral_suite(); 3. replay iot__lambda.cfn-deploy-iot-thing-custom-resource-lambda-positive: deploy a stack whose Custom::Resource ServiceToken is a Lambda that calls iot:CreateThing, wait for the stack to settle, then call DescribeThing with the thing name the custom resource reports.
