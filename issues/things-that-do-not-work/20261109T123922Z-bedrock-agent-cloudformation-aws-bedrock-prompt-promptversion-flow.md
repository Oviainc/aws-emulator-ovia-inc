Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agent__cloudformation.deploy-bedrock-agent-prompt-and-version-round-trip-and-delete bedrock-agent__cloudformation.deploy-bedrock-agent-prompt-default-variant-not-in-variants-rejected-and-rolled-back bedrock-agent__cloudformation.deploy-flow-node-type-enum-rejected bedrock-agent__cloudformation.deploy-flow-version-alias-round-trip-then-delete
Issue-Key: bedrock-agent__cloudformation|CreateStack|ValidationError
Issue-Commit: 86c25e3173c48b1c4d4a9173324e183693bfe876

# bedrock-agent__cloudformation: AWS::Bedrock::{Prompt,PromptVersion,Flow,FlowVersion,FlowAlias} are unregistered resource types, so no prompt or flow can be deployed from a template

> _Note: this issue was identified against the code base at commit `86c25e3173c48b1c4d4a9173324e183693bfe876`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: the CloudFormation template resource-type registry does not contain the Bedrock Agent authoring types AWS::Bedrock::Prompt, ::PromptVersion, ::Flow, ::FlowVersion and ::FlowAlias.

AWS: recorded behavior only — CreateStack accepts these types, provisions them through the Bedrock Agent build-time control plane, and reaches CREATE_COMPLETE; a prompt whose DefaultVariant is not among its Variants, or a flow node with an out-of-enum type, instead rolls the stack back with the service's own refusal as the resource failure reason.
Emulator: CreateStack answers 400 ValidationError `Template format error: Unrecognized resource types: [...]` naming exactly those types. No prompt, prompt version, flow, flow version or flow alias is created, so neither the round-trips nor the two validation-refusal legs are reached.
Why: the entire prompt/flow authoring surface is undeployable by template; a user is told these AWS resource types do not exist.
Fix: register the five types in the CloudFormation template resource-type registry and add a resource handler per type over the bedrock-agent crate, including the Ref/GetAtt attributes (prompt id/ARN/version, flow id/ARN/version, alias id/ARN) each documents.
Traces: the 4 checks in trace_ids.

## Evidence
Expected: CreateStack 200 + StackId; CREATE_COMPLETE for the round-trips; ROLLBACK_COMPLETE citing the Bedrock Agent refusal for the two negative legs.
Actual: botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [...], observed as:
  [AWS::Bedrock::Prompt, AWS::Bedrock::PromptVersion]
  [AWS::Bedrock::Prompt]
  [AWS::Bedrock::Flow]
  [AWS::Bedrock::Flow, AWS::Bedrock::FlowVersion, AWS::Bedrock::FlowAlias]

## Steps to Reproduce
1. CreateStack with a template declaring AWS::Bedrock::Prompt (+ ::PromptVersion) or AWS::Bedrock::Flow (+ ::FlowVersion, ::FlowAlias).
2. Observe the 400 ValidationError "Unrecognized resource types".
3. Re-replay the 4 checks in trace_ids.
