Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__config__s3.deploy-config-delivery-channel-by-cloudformation-delivers-a-snapshot-containing-the-recorded-item
Issue-Key: cloudformation__config__s3.deploy-config-delivery-channel-by-cloudformation-delivers-a-snapshot-containing-the-recorded-item|run#13|AssertionError|assert status == "CREATE_COMPLETE", (
Issue-Commit: a59989a9f63c403b66485a4e93f54ab6df22c2a6

# CloudFormation generates an AWS::IAM::Role physical name longer than IAM's 64-character limit, so stacks declaring an unnamed role fail to create

> _Note: this issue was identified against the code base at commit `a59989a9f63c403b66485a4e93f54ab6df22c2a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
ONE CAUSE must be closed.

Cause 1 of 1: the physical name CloudFormation generates for a resource whose name is left unspecified is not fitted to the target service's name constraint, so creating the resource fails and the stack rolls back.
AWS: recorded behavior only — when a template declares an AWS::IAM::Role without RoleName, CloudFormation generates a physical name from the stack name, the logical id and a random suffix AND keeps it within the target resource's limit, so the stack reaches CREATE_COMPLETE. IAM's own contract is the constraint being violated: iam:CreateRole RoleName is @length max 64 (Smithy: IAM CreateRole.RoleName), and IAM's refusal for a longer name is the 400 ValidationError seen in the evidence.
Emulator: it concatenates the full stack name, the full logical id and a 12-character suffix (observed: a 69-character name of the form "<stack-name>-<LogicalId>-<12 hex>"), IAM rejects it with ValidationError ("Value '<name>' at 'roleName' failed to satisfy constraint: Member must have length less than or equal to 64"), and the stack ends CREATE_FAILED with "The following resource(s) failed to create: [ConfigRecorderRole]".
Why: any template that declares an unnamed IAM role under a stack name of ordinary length cannot be deployed at all — the failure is in the emulator's name generation, not in the client's template, and it rolls back every other resource in the stack. The constraint-driven truncation is the requirement; the exact generated name is never part of it.
Fix: CloudFormation's physical-resource-name generator — truncate the stack-name/logical-id components so the generated name fits the target resource type's name length limit before calling the owning service (services/service-com-amazonaws-cloudformation-cloudformation/src/ — the stack-execution/physical-id generation path).
Traces: cloudformation__config__s3.deploy-config-delivery-channel-by-cloudformation-delivers-a-snapshot-containing-the-recorded-item.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator built from mainline (emulator serving; 30/179 passed).

cloudformation__config__s3.deploy-config-delivery-channel-by-cloudformation-delivers-a-snapshot-containing-the-recorded-item:
  AssertionError: the delivery infrastructure stack did not deploy: 'CREATE_FAILED' ([('sim2real-behav-cfg-stack-eed8edb87ce1', "The following resource(s) failed to create: [ConfigRecorderRole]. ValidationError: 1 validation error detected: Value 'sim2real-behav-cfg-stack-eed8edb87ce1-ConfigRecorderRole-F6EF37E2F5D4' at 'roleName' failed to satisfy constraint: Member must have length less than or equal to 64")])

The generated name is 69 characters; expected: a generated name within 64 characters and a stack that reaches CREATE_COMPLETE. The IAM refusal itself is correct (RoleName is capped at 64) — the defect is the name CloudFormation produced.

## Steps to Reproduce
1. git clone the repo, build and boot the emulator from mainline.
2. cloudformation create-stack --stack-name sim2real-behav-cfg-stack-<12 hex> --template-body <template declaring an AWS::IAM::Role with logical id ConfigRecorderRole and no RoleName> --capabilities CAPABILITY_IAM.
3. cloudformation describe-stacks / describe-stack-events: the stack ends CREATE_FAILED with the IAM ValidationError above, because the generated roleName is 69 characters.
4. Replay simparity-behavioral-aws-v1 and re-replay the check id in trace_ids.
