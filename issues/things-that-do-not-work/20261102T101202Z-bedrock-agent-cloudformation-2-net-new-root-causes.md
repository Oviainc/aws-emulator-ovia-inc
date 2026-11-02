Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agent__cloudformation.kbcfn-cfn-created-kb-round-trips-config-and-stack-tags bedrock-agent__cloudformation.kbcfn-same-name-kb-replacement-rejected-already-exists
Issue-Key: bedrock-agent__cloudformation|GetKnowledgeBase|ValidationException
Issue-Key: bedrock-agent__cloudformation.kbcfn-same-name-kb-replacement-rejected-already-exists|run#4|AssertionError|assert status == "UPDATE_ROLLBACK_COMPLETE", (
Issue-Commit: 765add48a71cdb59193040bf215c1324536972c5

# bedrock-agent/cloudformation: 2 net-new root causes — a CFN-created knowledge base gets a non-conforming id, and a same-name replacement is accepted

> _Note: this issue was identified against the code base at commit `765add48a71cdb59193040bf215c1324536972c5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetKnowledgeBase

## Description
Two distinct net-new root causes for CloudFormation-provisioned AWS::Bedrock::KnowledgeBase resources. Open issue 20261029T214321Z-bedrock-agent-cloudformation-ssm-a-cloudformation-created-aws.md covers neighbouring checks but carries neither replay key below.

Cause 1 of 2: the knowledge base's physical resource id is its template name, not a knowledge-base id.
AWS: Smithy/recorded: knowledgeBaseId must satisfy the pattern ^[0-9a-zA-Z]{10}$; a CloudFormation-created knowledge base therefore gets a 10-character alphanumeric id as its PhysicalResourceId, and GetKnowledgeBase with that id returns the knowledge base (with its configuration and the stack's tags).
Emulator: the PhysicalResourceId is the resource's name (e.g. '<stack>-metadata-kb-g1'), so GetKnowledgeBase answers ValidationException "1 validation error detected: Value '<physical id>' at 'knowledgeBaseId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9a-zA-Z]{10}$".
Why: nothing created through CloudFormation can be read or used through the bedrock-agent API — every call that takes a knowledgeBaseId is refused.
Fix: AWS::Bedrock::KnowledgeBase resource provisioning — mint a conforming knowledge-base id and return it as the physical resource id.
Traces: bedrock-agent__cloudformation.kbcfn-cfn-created-kb-round-trips-config-and-stack-tags.

Cause 2 of 2: a replacing update that would create a second knowledge base with an existing name is accepted.
AWS: recorded behavior only — the replacement fails because the name is already taken and the stack rolls back (the failure reason names the already-existing knowledge base).
Emulator: the update settles UPDATE_COMPLETE with reasons ['User Initiated', 'User Initiated'] — no conflict is detected.
Why: a client ends up with a stack claiming to own a knowledge base whose name collides, and loses the duplicate-name refusal it relies on.
Fix: name-uniqueness check in the AWS::Bedrock::KnowledgeBase create/replace handler, plus rollback on the failure.
Traces: bedrock-agent__cloudformation.kbcfn-same-name-kb-replacement-rejected-already-exists.

## Evidence
C1: botocore ValidationException on GetKnowledgeBase — "1 validation error detected: Value 'sim2real-kbcfn-<run>-metadata-kb-g1' at 'knowledgeBaseId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9a-zA-Z]{10}$". The stack/resource name is a per-run value; the pattern is the requirement.
C2: "a same-name KB replacement ended 'UPDATE_COMPLETE': ['User Initiated', 'User Initiated']" — expected a failed replacement and a rollback.

## Steps to Reproduce
C1: deploy a stack with an AWS::Bedrock::KnowledgeBase, take its PhysicalResourceId and call GetKnowledgeBase with it. C2: update the stack so the knowledge base is replaced while another knowledge base already holds the target name, then poll DescribeStacks.
