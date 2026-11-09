Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock__cloudformation.deploy-bedrock-automated-reasoning-policy-and-version-round-trip-and-delete bedrock__cloudformation.deploy-bedrock-automated-reasoning-policy-invalid-variable-type-rejected-and-rolled-back bedrock__cloudformation.deploy-enforced-guardrail-configuration-cfn-lifecycle bedrock__cloudformation.deploy-enforced-guardrail-configuration-cfn-rejects-unknown-guardrail bedrock__cloudformation.deploy-guardrail-version-resource-policy-cfn-lifecycle bedrock__cloudformation.deploy-intelligent-prompt-router-response-quality-difference-rejected bedrock__cloudformation.deploy-intelligent-prompt-router-round-trip-then-delete
Issue-Key: bedrock__cloudformation|CreateStack|ValidationError
Issue-Commit: 86c25e3173c48b1c4d4a9173324e183693bfe876

# bedrock__cloudformation: AWS::Bedrock::{Session,AutomatedReasoningPolicy,AutomatedReasoningPolicyVersion,GuardrailVersion,EnforcedGuardrailConfiguration,ResourcePolicy,IntelligentPromptRouter} are unregistered resource types

> _Note: this issue was identified against the code base at commit `86c25e3173c48b1c4d4a9173324e183693bfe876`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: the CloudFormation template resource-type registry is missing a whole set of real AWS::Bedrock::* types, so CreateStack refuses every template that declares one.

AWS: recorded behavior only — AWS CloudFormation recognizes AWS::Bedrock::Session, ::AutomatedReasoningPolicy, ::AutomatedReasoningPolicyVersion, ::GuardrailVersion, ::EnforcedGuardrailConfiguration, ::ResourcePolicy and ::IntelligentPromptRouter. CreateStack answers 200 with a StackId, the resources are provisioned through the Bedrock control plane, and the stack reaches CREATE_COMPLETE; a template with a genuinely bad property instead reaches ROLLBACK_COMPLETE with the service's own refusal as the resource failure reason.
Emulator: CreateStack answers 400 ValidationError `Template format error: Unrecognized resource types: [<declared AWS::Bedrock::* types>]`. Nothing is created, so neither the lifecycle round-trips nor the negative legs (an automated-reasoning policy with an invalid variable type, an EnforcedGuardrailConfiguration naming an unknown guardrail, an IntelligentPromptRouter with a response-quality difference outside range) are ever reached.
Why: no Bedrock guardrail-version, resource-policy, automated-reasoning-policy, session or prompt-router resource can be deployed from a template; the user is told the resource type does not exist, which is false.
Fix: register each of these types in the CloudFormation template resource-type registry and add a resource handler per type that drives the bedrock control-plane crate's create/read/update/delete, mints the PhysicalResourceId AWS mints, and exposes the documented Ref/GetAtt attributes.
Traces: the 7 checks in trace_ids.

## Evidence
Expected: CreateStack 200 + StackId; CREATE_COMPLETE for the positive legs; ROLLBACK_COMPLETE with the service's refusal for the negative legs.
Actual: botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [...], with these lists observed:
  [AWS::Bedrock::AutomatedReasoningPolicy, AWS::Bedrock::AutomatedReasoningPolicyVersion]
  [AWS::Bedrock::AutomatedReasoningPolicy]
  [AWS::Bedrock::GuardrailVersion, AWS::Bedrock::EnforcedGuardrailConfiguration]
  [AWS::Bedrock::EnforcedGuardrailConfiguration]
  [AWS::Bedrock::GuardrailVersion, AWS::Bedrock::ResourcePolicy]
  [AWS::Bedrock::IntelligentPromptRouter]

## Steps to Reproduce
1. CreateStack with a template declaring any of AWS::Bedrock::Session, ::AutomatedReasoningPolicy, ::AutomatedReasoningPolicyVersion, ::GuardrailVersion, ::EnforcedGuardrailConfiguration, ::ResourcePolicy, ::IntelligentPromptRouter.
2. Observe the 400 ValidationError "Template format error: Unrecognized resource types: [...]".
3. Re-replay the 7 checks in trace_ids.
