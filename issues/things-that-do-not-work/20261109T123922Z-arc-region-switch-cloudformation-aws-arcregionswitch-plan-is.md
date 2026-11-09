Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: arc-region-switch__cloudformation.deploy-arcregionswitch-plan-duplicate-regions-rejected arc-region-switch__cloudformation.deploy-arcregionswitch-plan-round-trip-then-delete
Issue-Key: arc-region-switch__cloudformation|CreateStack|ValidationError
Issue-Commit: 86c25e3173c48b1c4d4a9173324e183693bfe876

# arc-region-switch__cloudformation: AWS::ARCRegionSwitch::Plan is not a registered resource type

> _Note: this issue was identified against the code base at commit `86c25e3173c48b1c4d4a9173324e183693bfe876`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: AWS::ARCRegionSwitch::Plan is missing from the CloudFormation template resource-type registry.

AWS: recorded behavior only — CreateStack accepts AWS::ARCRegionSwitch::Plan, provisions the region-switch plan, and reaches CREATE_COMPLETE; a plan that lists the same Region twice instead rolls the stack back with ARC Region Switch's own duplicate-region refusal as the resource failure reason.
Emulator: CreateStack answers 400 ValidationError `Template format error: Unrecognized resource types: [AWS::ARCRegionSwitch::Plan]` for both legs, so neither the round-trip nor the duplicate-region refusal is reached.
Why: ARC Region Switch plans cannot be declared in a template at all; the user is told the resource type does not exist.
Fix: register AWS::ARCRegionSwitch::Plan in the CloudFormation template resource-type registry and add its resource handler over the arc-region-switch crate, including the duplicate-region check on create.
Traces: the 2 checks in trace_ids.

## Evidence
Expected: CreateStack 200 + StackId and CREATE_COMPLETE (ROLLBACK_COMPLETE citing the duplicate-region refusal for the negative leg).
Actual: botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::ARCRegionSwitch::Plan]

## Steps to Reproduce
1. CreateStack with a template declaring AWS::ARCRegionSwitch::Plan.
2. Observe the 400 ValidationError "Unrecognized resource types: [AWS::ARCRegionSwitch::Plan]".
3. Re-replay the 2 checks in trace_ids.
