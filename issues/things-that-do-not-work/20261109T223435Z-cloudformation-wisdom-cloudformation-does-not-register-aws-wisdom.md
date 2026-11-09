Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__wisdom.deploy-wisdom-assistant-knowledgebase-and-assistantassociation-round-trip-and-delete cloudformation__wisdom.deploy-wisdom-assistantassociation-unknown-knowledgebase-rejected
Issue-Key: cloudformation__wisdom|CreateStack|ValidationError
Issue-Commit: e85d2009e41cfda8a7867f84cab4a050d23db70a

# cloudformation__wisdom: CloudFormation does not register AWS::Wisdom::Assistant, KnowledgeBase or AssistantAssociation

> _Note: this issue was identified against the code base at commit `e85d2009e41cfda8a7867f84cab4a050d23db70a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: AWS::Wisdom::Assistant, AWS::Wisdom::KnowledgeBase and AWS::Wisdom::AssistantAssociation are not recognized resource types.
AWS: all three are public CloudFormation resource types, so CreateStack accepts a template declaring them and provisions the assistant, knowledge base and their association; an association naming a knowledge base that does not exist is then refused and rolls the stack back.
Emulator: CreateStack answers HTTP 400 `ValidationError` with `Template format error: Unrecognized resource types: [AWS::Wisdom::Assistant, AWS::Wisdom::KnowledgeBase, AWS::Wisdom::AssistantAssociation]` — fixed text plus the bracketed type list from the request.
Why: the Wisdom assistant/knowledge-base estate cannot be deployed through CloudFormation at all, and the unknown-knowledge-base refusal can never be observed.
Fix: register those AWS::Wisdom::* types in the CloudFormation resource-type registry and add their resource handlers.
Traces: the two failing checks below.

## Evidence
botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::Wisdom::Assistant, AWS::Wisdom::KnowledgeBase, AWS::Wisdom::AssistantAssociation]. Expected CreateStack accepted with a StackId.

## Steps to Reproduce
1. Build and boot the emulator, then replay the behavioral suite.
2. CreateStack with a template declaring AWS::Wisdom::Assistant, AWS::Wisdom::KnowledgeBase and AWS::Wisdom::AssistantAssociation.
3. Observe the ValidationError.
