Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__sqs.cdk-import-adopts-an-existing-queue-and-refuses-a-missing-one cloudformation__sqs.cdk-rollback-force-completes-past-the-resource-whose-rollback-fails cloudformation__sqs.cdk-rollback-orphan-skips-the-resource-whose-rollback-fails
Issue-Key: cloudformation__sqs.cdk-import-adopts-an-existing-queue-and-refuses-a-missing-one|run#15|AssertionError|assert rc == 0, f"the first deploy exited {rc}; output tail {out[-800:]!r}"
Issue-Key: cloudformation__sqs.cdk-rollback-force-completes-past-the-resource-whose-rollback-fails|run#11|AssertionError|assert rc == 0, f"the first deploy exited {rc}; output tail {out[-800:]!r}"
Issue-Key: cloudformation__sqs.cdk-rollback-orphan-skips-the-resource-whose-rollback-fails|run#11|AssertionError|assert rc == 0, f"the first deploy exited {rc}; output tail {out[-800:]!r}"
Issue-Commit: 81fe993c0fc493a00f42b69c764d54e4324ad80b

# cloudformation__sqs: ExecuteChangeSet on a CREATE change set never advances the stack out of REVIEW_IN_PROGRESS, so every `cdk deploy` fails with ValidationError "Template does not exist for stack [...] in REVIEW_IN_PROGRESS state"

> _Note: this issue was identified against the code base at commit `81fe993c0fc493a00f42b69c764d54e4324ad80b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteChangeSet

## Description
Cause 1 of 1: a stack created through the change-set path (CreateChangeSet ChangeSetType=CREATE, then ExecuteChangeSet) is left in REVIEW_IN_PROGRESS forever — the execute never provisions the resources and never moves the stack on — so the very next read of the stack's template is refused and the CDK deploy aborts.

AWS: CreateChangeSet with ChangeSetType=CREATE puts a brand-new stack in REVIEW_IN_PROGRESS (Smithy: cloudformation DescribeStacks, StackStatus enum contains REVIEW_IN_PROGRESS, CREATE_IN_PROGRESS, CREATE_COMPLETE, CREATE_FAILED, ROLLBACK_COMPLETE). A subsequent ExecuteChangeSet is accepted and the stack then leaves REVIEW_IN_PROGRESS: it goes CREATE_IN_PROGRESS and settles in a terminal create status, after which GetTemplate serves the stack's template. Smithy models ExecuteChangeSet's only refusals as ChangeSetNotFoundException (404, awsQueryError ChangeSetNotFound), InsufficientCapabilitiesException (400), InvalidChangeSetStatusException (400) and TokenAlreadyExistsException (400); none of them describes a stack that is left in REVIEW_IN_PROGRESS by a successful execute. The ValidationError sentence 'Template does not exist for stack [<stack name>] in REVIEW_IN_PROGRESS state' is AWS's own GetTemplate refusal, correct only for a stack that still has no executed template — its fixed text is 'Template does not exist for stack [' ... '] in REVIEW_IN_PROGRESS state' with the stack name taken from the request. Receiving it after ExecuteChangeSet is the symptom, not the defect.

Emulator: after the change set is created and executed, the stack is still REVIEW_IN_PROGRESS, so the deploy tool's follow-up template read is refused and `cdk deploy` exits 1 with '<stack>: creating CloudFormation changeset... <stack>: deploying... [1/1] ... failed to deploy ValidationError: Template does not exist for stack [<stack>] in REVIEW_IN_PROGRESS state'. No resource of the stack is ever created.

Why: no CDK-driven (change-set-based) deployment works at all against the emulator. A client that uses the CreateChangeSet/ExecuteChangeSet pair — the default path of the AWS CDK and of `aws cloudformation deploy` — gets a stack that exists only as a review stub, with no resources, no outputs and no terminal status to wait on, while AWS would have provisioned it. Every behaviour layered on top (queue import/adoption, rollback, rollback --force, --orphan) is unreachable.

Fix: the ExecuteChangeSet handler and the stack-state machine it drives in the CloudFormation service (the execute must take the stack out of REVIEW_IN_PROGRESS into CREATE_IN_PROGRESS and run the provisioner to a terminal create status, so the executed template becomes the stack's template).

Traces: cloudformation__sqs.cdk-import-adopts-an-existing-queue-and-refuses-a-missing-one, cloudformation__sqs.cdk-rollback-force-completes-past-the-resource-whose-rollback-fails, cloudformation__sqs.cdk-rollback-orphan-skips-the-resource-whose-rollback-fails. The same cause shows under three further service headings and is filed there separately (cloudformation__sqs__ssm, cloudformation__sns__sqs, cloudformation__iam__sqs__ssm), because an issue carries one service token.

## Evidence
Expected (AWS): `cdk deploy` exits 0; DescribeStacks reports the stack in a terminal create status (CREATE_COMPLETE, or CREATE_FAILED/ROLLBACK_COMPLETE when a resource fails) and GetTemplate serves the executed template.
Actual (emulator): 'AssertionError: the first deploy exited 1; output tail ... <stack>: creating CloudFormation changeset...\n<stack>: deploying... [1/1]\n failed to deploy\n ValidationError: Template does not exist for stack [<stack>] in REVIEW_IN_PROGRESS state' — observed identically for the import, rollback --force and rollback --orphan legs (only the generated stack name differs; that name is a per-run value, not part of the requirement).

## Steps to Reproduce
1. CreateChangeSet(StackName=<new stack>, ChangeSetType=CREATE, TemplateBody=<a template declaring one AWS::SQS::Queue>); DescribeStacks shows REVIEW_IN_PROGRESS.
2. ExecuteChangeSet(ChangeSetName=<that change set>, StackName=<new stack>).
3. Poll DescribeStacks: the stack stays REVIEW_IN_PROGRESS and no queue is created; GetTemplate(StackName=<new stack>) is refused ValidationError 'Template does not exist for stack [<new stack>] in REVIEW_IN_PROGRESS state'.
Equivalently, run `cdk deploy` for any app. Replay checks cloudformation__sqs.cdk-import-adopts-an-existing-queue-and-refuses-a-missing-one, cloudformation__sqs.cdk-rollback-force-completes-past-the-resource-whose-rollback-fails, cloudformation__sqs.cdk-rollback-orphan-skips-the-resource-whose-rollback-fails.
