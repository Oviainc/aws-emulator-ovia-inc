Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__qconnect.deploy-wisdom-aiprompt-aiguardrail-aiagent-and-their-versions-round-trip-and-delete cloudformation__qconnect.deploy-wisdom-aiprompt-mismatched-apiformat-rejected cloudformation__qconnect.deploy-wisdom-message-template-and-version-round-trip-and-delete cloudformation__qconnect.deploy-wisdom-quick-response-round-trip-and-delete cloudformation__qconnect.deploy-wisdom-quick-response-on-message-templates-knowledge-base-rejected
Issue-Key: cloudformation__qconnect|CreateStack|ValidationError
Issue-Commit: e85d2009e41cfda8a7867f84cab4a050d23db70a

# cloudformation__qconnect: CloudFormation does not register any AWS::Wisdom::* resource type, so no Q in Connect estate can be deployed

> _Note: this issue was identified against the code base at commit `e85d2009e41cfda8a7867f84cab4a050d23db70a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: AWS::Wisdom::Assistant, KnowledgeBase, AIPrompt, AIPromptVersion, AIGuardrail, AIGuardrailVersion, AIAgent, AIAgentVersion, MessageTemplate, MessageTemplateVersion and QuickResponse are not recognized resource types.
AWS: all of these are public CloudFormation resource types, so CreateStack accepts templates declaring them and provisions the assistant/knowledge base and their children; a mismatched AIPrompt ApiFormat, and a quick response created on a message-templates knowledge base, are then refused by the service and roll the stack back.
Emulator: CreateStack answers HTTP 400 `ValidationError` with `Template format error: Unrecognized resource types: [<the bracketed list of declared AWS::Wisdom::* types>]`. The fixed text is "Template format error: Unrecognized resource types: "; the list comes from the request.
Why: the entire Amazon Q in Connect / Wisdom surface is unreachable from CloudFormation, so neither the positive round trips nor the two modelled refusals can ever be exercised.
Fix: register the AWS::Wisdom::* types in the CloudFormation resource-type registry and add their resource handlers.
Traces: the five failing checks below.

## Evidence
botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::Wisdom::Assistant, AWS::Wisdom::AIPrompt, AWS::Wisdom::AIPromptVersion, AWS::Wisdom::AIGuardrail, AWS::Wisdom::AIGuardrailVersion, AWS::Wisdom::AIAgent, AWS::Wisdom::AIAgentVersion] (and the narrower lists for the message-template and quick-response checks). Expected CreateStack accepted with a StackId.

## Steps to Reproduce
1. Build and boot the emulator, then replay the behavioral suite.
2. CreateStack with a template declaring AWS::Wisdom::Assistant / KnowledgeBase plus any of the AI* or MessageTemplate/QuickResponse children.
3. Observe the ValidationError.
