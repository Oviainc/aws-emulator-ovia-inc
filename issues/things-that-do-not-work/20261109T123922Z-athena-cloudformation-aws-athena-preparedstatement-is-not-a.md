Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena__cloudformation.deploy-prepared-statement-cloudformation-missing-workgroup-rejected athena__cloudformation.deploy-prepared-statement-cloudformation-round-trip
Issue-Key: athena__cloudformation|CreateStack|ValidationError
Issue-Commit: 86c25e3173c48b1c4d4a9173324e183693bfe876

# athena__cloudformation: AWS::Athena::PreparedStatement is not a registered resource type

> _Note: this issue was identified against the code base at commit `86c25e3173c48b1c4d4a9173324e183693bfe876`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: AWS::Athena::PreparedStatement is missing from the CloudFormation template resource-type registry.

AWS: recorded behavior only — CreateStack accepts AWS::Athena::PreparedStatement, creates the prepared statement in the named workgroup, and reaches CREATE_COMPLETE; a statement declared against a workgroup that does not exist instead rolls the stack back with Athena's own not-found refusal as the resource failure reason.
Emulator: CreateStack answers 400 ValidationError `Template format error: Unrecognized resource types: [AWS::Athena::PreparedStatement]` for both legs, so neither the round-trip nor the missing-workgroup refusal is reached.
Why: Athena prepared statements cannot be declared in a template at all; the user is told the resource type does not exist.
Fix: register AWS::Athena::PreparedStatement in the CloudFormation template resource-type registry and add its resource handler over the athena crate, including workgroup-existence validation on create.
Traces: the 2 checks in trace_ids.

## Evidence
Expected: CreateStack 200 + StackId and CREATE_COMPLETE (ROLLBACK_COMPLETE citing Athena's refusal for the missing-workgroup leg).
Actual: botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::Athena::PreparedStatement]

## Steps to Reproduce
1. CreateStack with a template declaring AWS::Athena::PreparedStatement.
2. Observe the 400 ValidationError "Unrecognized resource types: [AWS::Athena::PreparedStatement]".
3. Re-replay the 2 checks in trace_ids.
