Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__serverlessrepo.deploy-cfn-serverlessrepo-application-round-trip-and-delete cloudformation__serverlessrepo.deploy-cfn-serverlessrepo-application-invalid-name-rejected
Issue-Key: cloudformation__serverlessrepo|CreateStack|ValidationError
Issue-Commit: e85d2009e41cfda8a7867f84cab4a050d23db70a

# cloudformation__serverlessrepo: CloudFormation does not register AWS::ServerlessRepo::Application

> _Note: this issue was identified against the code base at commit `e85d2009e41cfda8a7867f84cab4a050d23db70a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: AWS::ServerlessRepo::Application is not a recognized resource type.
AWS: it is a public CloudFormation resource type, so CreateStack accepts a template declaring it and publishes the Serverless Application Repository application; a Name that breaks the application-name constraint then fails the resource and rolls the stack back.
Emulator: CreateStack answers HTTP 400 `ValidationError` with `Template format error: Unrecognized resource types: [AWS::ServerlessRepo::Application]` — fixed text plus the bracketed type list from the request.
Why: SAR applications cannot be published through CloudFormation at all, and the name-constraint refusal can never be observed.
Fix: register AWS::ServerlessRepo::Application in the CloudFormation resource-type registry and add its resource handler.
Traces: the two failing checks below.

## Evidence
botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::ServerlessRepo::Application]. Expected CreateStack accepted with a StackId.

## Steps to Reproduce
1. Build and boot the emulator, then replay the behavioral suite.
2. CreateStack with a template declaring AWS::ServerlessRepo::Application.
3. Observe the ValidationError.
