Issue-Severity: HIGH
Issue-Commit: 86c25e3173c48b1c4d4a9173324e183693bfe876

# cleanroomsml__cloudformation__s3: AWS::CleanRoomsML::TrainingDataset is not a registered resource type, so UpdateStack refuses every training-dataset template

> _Note: this issue was identified against the code base at commit `86c25e3173c48b1c4d4a9173324e183693bfe876`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack

## Description
Cause 1 of 1: AWS::CleanRoomsML::TrainingDataset is missing from the CloudFormation template resource-type registry.

AWS: recorded behavior only — a template declaring AWS::CleanRoomsML::TrainingDataset is accepted by CreateStack/UpdateStack, the dataset is created against its S3 location, and the stack reaches a COMPLETE status; a training dataset whose S3 location is empty instead fails the resource with Clean Rooms ML's own refusal as the failure reason.
Emulator: UpdateStack answers 400 ValidationError `Template format error: Unrecognized resource types: [AWS::CleanRoomsML::TrainingDataset]` for both legs, so neither the round-trip nor the empty-S3-location refusal is reached.
Why: Clean Rooms ML training datasets cannot be declared in a template at all; the user is told the resource type does not exist.
Fix: register AWS::CleanRoomsML::TrainingDataset in the CloudFormation template resource-type registry and add its resource handler over the cleanroomsml crate, including the S3-location validation on create.
Traces: the 2 checks in trace_ids.

## Evidence
Expected: UpdateStack 200 and the stack reaching UPDATE_COMPLETE (resource failure citing the empty-S3-location refusal for the negative leg).
Actual: botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the UpdateStack operation: Template format error: Unrecognized resource types: [AWS::CleanRoomsML::TrainingDataset]

## Steps to Reproduce
1. Submit a template declaring AWS::CleanRoomsML::TrainingDataset over an S3 location and call UpdateStack (or CreateStack).
2. Observe the 400 ValidationError "Unrecognized resource types: [AWS::CleanRoomsML::TrainingDataset]".
3. Re-replay the 2 checks in trace_ids.
