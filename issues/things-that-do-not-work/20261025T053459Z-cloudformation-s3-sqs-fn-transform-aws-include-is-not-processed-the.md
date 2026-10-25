Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__s3__sqs.deploy-cloudformation-3-include-transform-missing-s3-key-fails
Issue-Key: cloudformation__s3__sqs.deploy-cloudformation-3-include-transform-missing-s3-key-fails|run#17|AssertionError|assert expected in reasons, (
Issue-Commit: 35f0669fbed72946ad7349f4657cbc8fac9f7c2b

# Fn::Transform / AWS::Include is not processed: the intrinsic is forwarded to the target service as a resource property instead of failing the stack with the transform's error

> _Note: this issue was identified against the code base at commit `35f0669fbed72946ad7349f4657cbc8fac9f7c2b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: a resource-level Fn::Transform with AWS::Include is left in the property bag instead of being expanded, so the missing-snippet failure never surfaces as a transform error.
AWS: CloudFormation processes Fn::Transform / AWS::Include during stack operations by reading the object named by Location from S3 and splicing it into the template. CreateStack ACCEPTS the template and returns a StackId (the recording confirms this), and when the key is absent the stack fails and rolls back with CloudFormation's own stack-level reason, recorded as 'Transform AWS::Include failed with: S3 key [snippets/absent.yaml] does not exist.' Because the transform fails before provisioning, the included resource gets no events and the stack owns no resources.
Emulator: the Fn::Transform key is handed to the resource provider as if it were a queue property, so SQS refuses it and the stack's only reasons are 'The following resource(s) failed to create: [Q]. InvalidAttributeName: Unknown Attribute Fn::Transform.' (repeated per attempt). The S3 object is never read, no transform error is produced, and the failure is attributed to the resource rather than to template processing.
Why: a client gets a misleading, service-specific refusal for a template-processing defect, and cannot tell a missing include snippet from a bad queue attribute. Worse, the same path means an AWS::Include whose snippet DOES exist is ignored rather than merged, so the properties the snippet contributes are silently absent from the deployed resource.
Fix: the cloudformation service crate's template-processing stage — expand Fn::Transform/AWS::Include (fetching Location from S3) before resource provisioning, strip the intrinsic from the property bag, and on a failed fetch fail the stack with a stack-level reason naming the transform and the missing key, provisioning nothing.
Traces: cloudformation__s3__sqs.deploy-cloudformation-3-include-transform-missing-s3-key-fails. The open issue 20261025T015306Z-cloudformation-template-transforms-and-intrinsics-are-not-processed.md owns the sibling positive check (deploy-cloudformation-3-include-transform-injects-queue-properties) but does not carry this replay key or this negative/refusal behaviour.

## Evidence
AssertionError: the stack must record the AWS::Include transform error 'Transform AWS::Include failed with: S3 key [snippets/absent.yaml] does not exist.'; its stack-level reasons were ['User Initiated', 'The following resource(s) failed to create: [Q]. InvalidAttributeName: Unknown Attribute Fn::Transform.', 'The following resource(s) failed to create: [Q]. InvalidAttributeName: Unknown Attribute Fn::Transform.', 'The following resource(s) failed to create: [Q]. InvalidAttributeName: Unknown Attribute Fn::Transform.']

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the behavioral suite.
2. Create a bucket and put one snippet object in it; CreateStack a template whose AWS::SQS::Queue properties contain Fn::Transform with Name AWS::Include and Location s3://<bucket>/snippets/absent.yaml (a key that does not exist).
3. Wait for the terminal status and call DescribeStackEvents — observe stack reasons naming SQS's InvalidAttributeName for 'Fn::Transform' instead of 'Transform AWS::Include failed with: S3 key [snippets/absent.yaml] does not exist.'
4. Re-replay cloudformation__s3__sqs.deploy-cloudformation-3-include-transform-missing-s3-key-fails to confirm.
