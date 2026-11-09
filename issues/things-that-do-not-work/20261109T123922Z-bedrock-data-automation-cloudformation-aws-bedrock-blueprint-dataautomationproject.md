Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-data-automation__cloudformation.deploy-blueprint-and-project-round-trip-then-delete bedrock-data-automation__cloudformation.deploy-blueprint-invalid-type-enum-rejected bedrock-data-automation__cloudformation.deploy-data-automation-library-round-trip-then-delete
Issue-Key: bedrock-data-automation__cloudformation|CreateStack|ValidationError
Issue-Commit: 86c25e3173c48b1c4d4a9173324e183693bfe876

# bedrock-data-automation__cloudformation: AWS::Bedrock::{Blueprint,DataAutomationProject,DataAutomationLibrary} are unregistered resource types

> _Note: this issue was identified against the code base at commit `86c25e3173c48b1c4d4a9173324e183693bfe876`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: the CloudFormation template resource-type registry is missing the Bedrock Data Automation types AWS::Bedrock::Blueprint, AWS::Bedrock::DataAutomationProject and AWS::Bedrock::DataAutomationLibrary.

AWS: recorded behavior only — CreateStack accepts them, provisions each through the Bedrock Data Automation build-time service, and reaches CREATE_COMPLETE; a Blueprint whose Type is outside the documented enum instead rolls the stack back with the service's refusal as the resource failure reason.
Emulator: CreateStack answers 400 ValidationError `Template format error: Unrecognized resource types: [...]` naming those types, so nothing is created and the enum-refusal leg is never reached.
Why: no Data Automation blueprint, project or library can be provisioned from a template.
Fix: register the three types in the CloudFormation template resource-type registry and add resource handlers over the bedrock-data-automation crate.
Traces: the 3 checks in trace_ids.

## Evidence
Expected: CreateStack 200 + StackId and CREATE_COMPLETE (ROLLBACK_COMPLETE for the invalid-enum leg).
Actual: botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [...], observed as:
  [AWS::Bedrock::DataAutomationProject, AWS::Bedrock::Blueprint]
  [AWS::Bedrock::Blueprint]
  [AWS::Bedrock::DataAutomationLibrary]

## Steps to Reproduce
1. CreateStack with a template declaring AWS::Bedrock::Blueprint, AWS::Bedrock::DataAutomationProject or AWS::Bedrock::DataAutomationLibrary.
2. Observe the 400 ValidationError "Unrecognized resource types".
3. Re-replay the 3 checks in trace_ids.
