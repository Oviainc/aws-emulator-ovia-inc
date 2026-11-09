Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agent__cloudformation__ssm.kbcfn-remove-imports-then-replace-kb-swaps-embedding-model
Issue-Key: bedrock-agent__cloudformation__ssm|GetKnowledgeBase|ValidationException
Issue-Commit: 95ee1f215215607c18ab877072afcef7ad3742b1

# bedrock-agent__cloudformation__ssm: a CloudFormation-created AWS::Bedrock::KnowledgeBase gets a stack-derived name as its physical id, so bedrock-agent refuses it against the knowledgeBaseId pattern (1 cause)

> _Note: this issue was identified against the code base at commit `95ee1f215215607c18ab877072afcef7ad3742b1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetKnowledgeBase

## Description
One net-new behavioral root cause of service `bedrock-agent__cloudformation__ssm`, grounded in one failing behavioral check. A fixer must close it before submitting.

Cause 1 of 1: the physical resource id CloudFormation assigns to an AWS::Bedrock::KnowledgeBase is a stack/logical-name-derived string, not a Bedrock knowledge base id, so the value the stack publishes (via Ref, GetAtt, stack Outputs and an SSM parameter) is rejected by bedrock-agent's own API the moment a client uses it.

AWS: the bedrock-agent Smithy model constrains the identifier precisely — GetKnowledgeBase's `knowledgeBaseId` must satisf`^[0-9a-zA-Z]{10}$`, i.e. exactly ten alphanumeric characters. CloudFormation's physical resource id for AWS::Bedrock::KnowledgeBase is therefore that ten-character id (recorded behavior for the CFN side: the model does not describe resource types), and `Ref`/`GetAtt KnowledgeBaseId` on the resource resolve to a value GetKnowledgeBase accepts. A client is expected to be able to take the stack's output straight to bedrock-agent.

Emulator: the physical id is the generated name `<stack>-<kb>-<generation>` (shape: the stack name, a resource token and a generation suffix; the exact string differs every run and is not the requirement). Passing it to bedrock-agent GetKnowledgeBase is refused with HTTP 400, Error.Code `ValidationException`, message `1 validation error detected: Value '<that value>' at 'knowledgeBaseId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9a-zA-Z]{10}$` — the constraint refusal is correct; what is wrong is that CloudFormation published a value that can never satisfy it.

Why: the whole point of the CFN↔Bedrock contract is that the stack's knowledge base id is usable. Against the emulator every cross-service hop off a CFN-provisioned knowledge base — GetKnowledgeBase, Update/DeleteKnowledgeBase, data-source and retrieval calls, and anything reading the id out of the SSM parameter or stack Output this check wires up — is refused at the first call, so a replacement that swaps the embedding model can never be verified.

Fix: the AWS::Bedrock::KnowledgeBase provisioner in the cloudformation service's resource-provider layer — it must create the knowledge base through bedrock-agent (or mint an id of the modelled shape) and store THAT id as the resource's physical id, so Ref/GetAtt, DescribeStackResource(s), ListStackResources and stack Outputs all surface an id bedrock-agent accepts.

Traces: bedrock-agent__cloudformation__ssm.kbcfn-remove-imports-then-replace-kb-swaps-embedding-model.

## Evidence
bedrock-agent__cloudformation__ssm.kbcfn-remove-imports-then-replace-kb-swaps-embedding-model [error]:
  botocore.errorfactory.ValidationException: An error occurred (ValidationException) when calling the GetKnowledgeBase operation: 1 validation error detected: Value 'sim2real-kbcfn-4e48c833e862-escalation-kb-g2' at 'knowledgeBaseId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9a-zA-Z]{10}$

The value 'sim2real-kbcfn-4e48c833e862-escalation-kb-g2' is this run's generated stack-scoped name and is not itself the requirement; the requirement is that CloudFormation's physical resource id for AWS::Bedrock::KnowledgeBase satisfies the modelled knowledgeBaseId pattern ^[0-9a-zA-Z]{10}$ so that bedrock-agent accepts it. The 400 ValidationException and its message are bedrock-agent behaving correctly — the defect is the id CloudFormation handed out.

## Steps to Reproduce
1. CreateStack with an AWS::Bedrock::KnowledgeBase, an Output publishing Ref/GetAtt of it, and an AWS::SSM::Parameter carrying that value; wait for CREATE_COMPLETE.
2. Read DescribeStackResource(LogicalResourceId=<the knowledge base>).PhysicalResourceId (and the stack Output / SSM parameter). Observe a value of the form '<stack>-<kb>-<generation>'.
3. Call bedrock-agent GetKnowledgeBase(knowledgeBaseId=<that value>). Observe HTTP 400 ValidationException against the pattern ^[0-9a-zA-Z]{10}$.
4. Control: any id of the modelled shape must be accepted by GetKnowledgeBase, so the refusal is attributable to the published physical id alone.
5. Re-replay: bedrock-agent__cloudformation__ssm.kbcfn-remove-imports-then-replace-kb-swaps-embedding-model.
