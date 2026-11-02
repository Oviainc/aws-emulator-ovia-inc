Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__lambda__s3.sam-layerversion-serves-function-and-outlives-stack cloudformation__lambda__s3.stack-lambda-code-s3key-update-serves-new-code-and-missing-key-rolls-back
Issue-Key: cloudformation__lambda__s3|CreateStack|ValidationError
Issue-Key: cloudformation__lambda__s3|GetFunction|ResourceNotFoundException
Issue-Commit: 765add48a71cdb59193040bf215c1324536972c5

# cloudformation/lambda/s3: 2 net-new root causes — the SAM LayerVersion transform leaves an unresolved dependency, and a CFN-created function is invisible to the Lambda API

> _Note: this issue was identified against the code base at commit `765add48a71cdb59193040bf215c1324536972c5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Two distinct net-new root causes.

Cause 1 of 2: a SAM template with AWS::Serverless::LayerVersion cannot be created.
AWS: recorded behavior only — the AWS::Serverless transform renames the layer into a hashed AWS::Lambda::LayerVersion logical id and rewrites every Ref to it, so CreateStack succeeds, the function serves with the layer, and the layer outlives the stack (its default RetentionPolicy).
Emulator: CreateStack is refused with ValidationError 'Template format error: Unresolved resource dependencies [Shared] in the Resources block of the template' — the transform leaves references to the serverless layer's logical id unresolved.
Why: no SAM application using a layer can deploy.
Fix: the AWS::Serverless transform's LayerVersion expansion (logical-id rewrite of Refs) in the template transform stage.
Traces: cloudformation__lambda__s3.sam-layerversion-serves-function-and-outlives-stack.

Cause 2 of 2: a function created by CloudFormation is not registered in Lambda.
AWS: recorded behavior only — once the stack reports the function CREATE_COMPLETE, GetFunction on its name/ARN returns the function, so an update of Code.S3Key serves the new code and a missing key rolls the stack back.
Emulator: GetFunction answers ResourceNotFoundException 'Function not found: arn:aws:lambda:<region>:<account>:function:<name>'.
Why: the deployed function cannot be read or invoked through the Lambda API, so code-update behaviour cannot be observed at all.
Fix: the AWS::Lambda::Function resource handler must create the function in Lambda state.
Traces: cloudformation__lambda__s3.stack-lambda-code-s3key-update-serves-new-code-and-missing-key-rolls-back.

## Evidence
C1: botocore ClientError (ValidationError) on CreateStack: 'Template format error: Unresolved resource dependencies [Shared] in the Resources block of the template'.
C2: botocore ResourceNotFoundException on GetFunction: 'Function not found: arn:aws:lambda:us-west-2:000000000000:function:sim2real-cfn-code-<run>'. Region, account and name are per-run fillers.

## Steps to Reproduce
C1: CreateStack with a SAM template declaring AWS::Serverless::LayerVersion and a function that Refs it. C2: deploy a stack with an AWS::Lambda::Function whose Code points at an S3 key, then call GetFunction on the created function.
