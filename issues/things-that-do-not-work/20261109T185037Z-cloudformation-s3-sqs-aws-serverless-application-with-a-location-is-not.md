Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__s3__sqs.sam-serverless-application-s3-location-creates-real-nested-stack
Issue-Key: cloudformation__s3__sqs.sam-serverless-application-s3-location-creates-real-nested-stack|run#19|AssertionError|assert rows == [("App", "AWS::CloudFormation::Stack", "CREATE_COMPLETE")], (
Issue-Commit: b4090e4eff86c6645a745aa23fbd4b95671a37a2

# AWS::Serverless::Application with a Location is not expanded into a real nested stack

> _Note: this issue was identified against the code base at commit `b4090e4eff86c6645a745aa23fbd4b95671a37a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: a SAM AWS::Serverless::Application whose Location points at a template in S3 reaches CREATE_COMPLETE as a leaf resource; no AWS::CloudFormation::Stack child is created.

AWS: the SAM transform rewrites AWS::Serverless::Application into an AWS::CloudFormation::Stack whose TemplateURL is the Location, so after the deploy the parent's resources contain exactly one resource of type AWS::CloudFormation::Stack (and ListStackResources/DescribeStackResources report the child stack id as its PhysicalResourceId). Recorded behavior: real AWS expands the application into exactly one nested stack.
Emulator: the resource stays ('App', 'AWS::Serverless::Application', 'CREATE_COMPLETE') — the transform leaves it unexpanded, so nothing in the nested template is provisioned while the stack reports success.
Why: a client deploying a SAM app composed of nested applications gets a green stack with none of the child resources; every downstream read of the child's outputs or resources fails, and the failure is silent at deploy time.
Fix: the SAM transform stage for AWS::Serverless::Application (expand to AWS::CloudFormation::Stack with the Location as TemplateURL and provision the child) in the CloudFormation template-transform component.
Traces: cloudformation__s3__sqs.sam-serverless-application-s3-location-creates-real-nested-stack.

## Evidence
[mismatch] cloudformation__s3__sqs.sam-serverless-application-s3-location-creates-real-nested-stack
AssertionError: the application must expand into exactly one nested stack; got [('App', 'AWS::Serverless::Application', 'CREATE_COMPLETE')]

## Steps to Reproduce
1. Upload a child template to S3.
2. CreateStack with Transform: AWS::Serverless-2016-10-31 and a resource of type AWS::Serverless::Application whose Location is that S3 template.
3. After CREATE_COMPLETE, list the stack's resources — expect one AWS::CloudFormation::Stack child; the emulator reports the unexpanded AWS::Serverless::Application instead.
