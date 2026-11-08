Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ec2.cdk-destroy-removes-every-resource-the-stack-created
Issue-Key: cloudformation__ec2.cdk-destroy-removes-every-resource-the-stack-created|run#50|AssertionError|assert (code, message) == ("InvalidSubnetID.NotFound", want_message), (
Issue-Commit: f4d58d8ba640bd38bd7af13aafdf9baa78485a99

# CreateTags against a deleted subnet id answers InvalidID (malformed) instead of InvalidSubnetID.NotFound, so a destroyed CDK/CFN estate cannot be proven gone

> _Note: this issue was identified against the code base at commit `f4d58d8ba640bd38bd7af13aafdf9baa78485a99`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTags

## Description
Cause 1 of 1: EC2 classifies a well-formed but no-longer-existing subnet id as malformed.

AWS: recorded behavior only for the message (EC2's Smithy model declares no modelled errors on CreateTags; refusals travel as ec2-query errors). A write naming a subnet id that is syntactically valid but does not exist — including one that existed and was deleted — is refused with error code `InvalidSubnetID.NotFound` and EC2's not-found message template for that resource kind: fixed text `The subnet ID '` ... `' does not exist`, with the id taken verbatim from the request. `InvalidID` with `The ID '<id>' is not valid` is EC2's distinct refusal for an id whose SYNTAX is wrong (bad prefix or bad length), not for an unknown one; a `subnet-` id of the correct length never earns it.

Emulator: refuses CreateTags on a destroyed subnet with error code `InvalidID` and message `The ID '<subnet-id>' is not valid`. The id in the replay (`subnet-00000000000000055`) is a per-run value; the requirement is the code/message pair for the unknown-but-well-formed class.

Why: a client cannot distinguish "you sent me garbage" from "that subnet is gone". Teardown verification, idempotent delete paths and drift detection all key on the resource-specific `Invalid<Resource>ID.NotFound` codes; collapsing them into `InvalidID` makes a successful destroy indistinguishable from a client bug, and retry logic that treats malformed input as non-retryable permanently mis-handles a deleted resource.

Fix: the EC2 id-resolution/validation step shared by the tagging and write paths — syntax validation must be separated from existence lookup, so a well-formed id that misses raises the per-resource NotFound code (here CreateTags on `subnet-*` -> InvalidSubnetID.NotFound) rather than the malformed-id refusal.

Traces: cloudformation__ec2.cdk-destroy-removes-every-resource-the-stack-created.

## Evidence
Expected: CreateTags(Resources=[<deleted subnet id>], Tags=[...]) -> error code 'InvalidSubnetID.NotFound', message "The subnet ID '<subnet-id>' does not exist".
Actual: error code 'InvalidID', message "The ID '<subnet-id>' is not valid".
Replay assertion: "CreateTags on the destroyed subnet 'subnet-00000000000000055' failed with ('InvalidID', \"The ID 'subnet-00000000000000055' is not valid\"), expected ('InvalidSubnetID.NotFound', \"The subnet ID 'subnet-00000000000000055' does not exist\")".

## Steps to Reproduce
1. Deploy a CDK/CloudFormation VPC stack that creates a subnet, note the subnet id, then destroy the stack.
2. Call ec2:CreateTags with Resources=[<that subnet id>] and any tag.
3. Observe error code InvalidID with "The ID '<id>' is not valid" instead of InvalidSubnetID.NotFound with "The subnet ID '<id>' does not exist".
4. Re-replay check cloudformation__ec2.cdk-destroy-removes-every-resource-the-stack-created to confirm.
