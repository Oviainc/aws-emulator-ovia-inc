Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: aiops__cloudformation.cfn-investigation-group-round-trip-then-invalid-rolearn-rejected
Issue-Key: aiops__cloudformation|CreateStack|ValidationError
Issue-Commit: 86c25e3173c48b1c4d4a9173324e183693bfe876

# aiops__cloudformation: AWS::AIOps::InvestigationGroup is not a registered resource type

> _Note: this issue was identified against the code base at commit `86c25e3173c48b1c4d4a9173324e183693bfe876`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: AWS::AIOps::InvestigationGroup is missing from the CloudFormation template resource-type registry.

AWS: recorded behavior only — CreateStack accepts a template declaring AWS::AIOps::InvestigationGroup, provisions the investigation group through the AIOps control plane, and reaches CREATE_COMPLETE; a later update to an invalid RoleArn is refused with the service's own refusal as the resource failure reason.
Emulator: CreateStack answers 400 ValidationError `Template format error: Unrecognized resource types: [AWS::AIOps::InvestigationGroup]`, so neither the round-trip nor the invalid-RoleArn refusal is reached.
Why: AIOps investigation groups cannot be provisioned from a template; the user is told the resource type does not exist.
Fix: register AWS::AIOps::InvestigationGroup in the CloudFormation template resource-type registry and add its resource handler over the aiops crate, including RoleArn validation on create/update.
Traces: aiops__cloudformation.cfn-investigation-group-round-trip-then-invalid-rolearn-rejected

## Evidence
Expected: CreateStack 200 + StackId, CREATE_COMPLETE, then a refused update for the invalid RoleArn.
Actual: botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::AIOps::InvestigationGroup]

## Steps to Reproduce
1. CreateStack with a template declaring AWS::AIOps::InvestigationGroup.
2. Observe the 400 ValidationError "Unrecognized resource types: [AWS::AIOps::InvestigationGroup]".
3. Re-replay aiops__cloudformation.cfn-investigation-group-round-trip-then-invalid-rolearn-rejected.
