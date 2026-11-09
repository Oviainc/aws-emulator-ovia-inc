Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agent-runtime__cloudformation.deploy-bedrock-agent-runtime-session-nonexistent-kms-key-rejected-and-rolled-back bedrock-agent-runtime__cloudformation.deploy-bedrock-agent-runtime-session-round-trip-and-delete
Issue-Key: bedrock-agent-runtime__cloudformation|CreateStack|ValidationError
Issue-Commit: 86c25e3173c48b1c4d4a9173324e183693bfe876

# bedrock-agent-runtime__cloudformation: AWS::Bedrock::Session is not a registered resource type

> _Note: this issue was identified against the code base at commit `86c25e3173c48b1c4d4a9173324e183693bfe876`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: AWS::Bedrock::Session is missing from the CloudFormation template resource-type registry.

AWS: recorded behavior only — CreateStack accepts a template declaring AWS::Bedrock::Session, creates the session through the Bedrock Agent runtime, and reaches CREATE_COMPLETE; a session naming a nonexistent KMS key instead rolls the stack back with the service's refusal as the resource failure reason.
Emulator: CreateStack answers 400 ValidationError `Template format error: Unrecognized resource types: [AWS::Bedrock::Session]` for both the positive and the negative leg, so neither the session round-trip nor the KMS-key refusal is exercised.
Why: Bedrock Agent runtime sessions cannot be provisioned from a template at all.
Fix: register AWS::Bedrock::Session in the CloudFormation template resource-type registry and add its resource handler over the bedrock-agent-runtime crate (including KMS key validation on create).
Traces: the 2 checks in trace_ids.

## Evidence
Expected: CreateStack 200 + StackId; CREATE_COMPLETE for the round-trip and ROLLBACK_COMPLETE citing the KMS refusal for the negative leg.
Actual: botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::Bedrock::Session]

## Steps to Reproduce
1. CreateStack with a template declaring AWS::Bedrock::Session.
2. Observe the 400 ValidationError "Unrecognized resource types: [AWS::Bedrock::Session]".
3. Re-replay the 2 checks in trace_ids.
