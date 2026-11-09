Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appflow__cloudformation__s3.deploy-appflow-flow-missing-bucket-policy-rejected-and-rolled-back appflow__cloudformation__s3.deploy-appflow-flow-s3-to-s3-round-trip-and-delete
Issue-Key: appflow__cloudformation__s3|CreateStack|ValidationError
Issue-Commit: 86c25e3173c48b1c4d4a9173324e183693bfe876

# appflow__cloudformation__s3: AWS::AppFlow::Flow is not a registered resource type

> _Note: this issue was identified against the code base at commit `86c25e3173c48b1c4d4a9173324e183693bfe876`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: AWS::AppFlow::Flow is missing from the CloudFormation template resource-type registry.

AWS: recorded behavior only — CreateStack accepts AWS::AppFlow::Flow, provisions the S3-to-S3 flow, and reaches CREATE_COMPLETE; a flow whose destination bucket carries no policy permitting AppFlow to write instead rolls the stack back with AppFlow's own bucket-policy refusal as the resource failure reason.
Emulator: CreateStack answers 400 ValidationError `Template format error: Unrecognized resource types: [AWS::AppFlow::Flow]` for both legs, so neither the round-trip nor the missing-bucket-policy refusal is reached.
Why: AppFlow flows cannot be declared in a template at all; the user is told the resource type does not exist.
Fix: register AWS::AppFlow::Flow in the CloudFormation template resource-type registry and add its resource handler over the appflow crate, including the destination-bucket policy pre-flight on create.
Traces: the 2 checks in trace_ids.

## Evidence
Expected: CreateStack 200 + StackId and CREATE_COMPLETE (ROLLBACK_COMPLETE citing AppFlow's bucket-policy refusal for the negative leg).
Actual: botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::AppFlow::Flow]

## Steps to Reproduce
1. CreateStack with a template declaring AWS::AppFlow::Flow over two S3 buckets.
2. Observe the 400 ValidationError "Unrecognized resource types: [AWS::AppFlow::Flow]".
3. Re-replay the 2 checks in trace_ids.
