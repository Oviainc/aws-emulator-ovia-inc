Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__iam__sqs__ssm.cdk-deploy-role-arn-stays-the-stacks-role-for-later-deploys
Issue-Key: cloudformation__iam__sqs__ssm.cdk-deploy-role-arn-stays-the-stacks-role-for-later-deploys|run#21|AssertionError|assert rc == 0, f"`cdk deploy --role-arn` exited {rc}; output tail {out[-800:]!r}"
Issue-Commit: 81fe993c0fc493a00f42b69c764d54e4324ad80b

# cloudformation__iam__sqs__ssm: `cdk deploy --role-arn` cannot deploy — the executed CREATE change set leaves the stack in REVIEW_IN_PROGRESS, so the stack's RoleARN is never recorded or re-used

> _Note: this issue was identified against the code base at commit `81fe993c0fc493a00f42b69c764d54e4324ad80b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteChangeSet

## Description
Cause 1 of 1: ExecuteChangeSet on a CREATE change set does not advance the stack out of REVIEW_IN_PROGRESS, so a deploy that passes an explicit service role never provisions anything and the stack never records the acting role for later deploys.

AWS: CreateChangeSet(ChangeSetType=CREATE, RoleARN=<role>) parks a new stack in REVIEW_IN_PROGRESS (Smithy: cloudformation DescribeStacks StackStatus enum includes REVIEW_IN_PROGRESS, CREATE_IN_PROGRESS, CREATE_COMPLETE); ExecuteChangeSet then provisions the stack as that role and the stack keeps the RoleARN, so a later deploy with no --role-arn still acts as it (and a resource the role may not create fails and rolls back, naming the role). Smithy models ExecuteChangeSet's refusals as ChangeSetNotFoundException (404), InsufficientCapabilitiesException (400), InvalidChangeSetStatusException (400) and TokenAlreadyExistsException (400); none describes a stack left in review. The RoleARN stickiness is recorded behavior only.

Emulator: the execute leaves the stack in REVIEW_IN_PROGRESS, so the deploy tool's follow-up template read is refused and `cdk deploy --role-arn` exits 1 with 'ValidationError: Template does not exist for stack [<stack>] in REVIEW_IN_PROGRESS state'. Neither the SSM parameter nor the role binding is ever created.

Why: a client that deploys through a least-privilege CloudFormation service role cannot deploy at all, and nothing can be learned about whether the emulator honours a stack's recorded RoleARN on subsequent deploys.

Fix: the ExecuteChangeSet handler and the stack-state machine/provisioner it drives in the CloudFormation service (the execute must leave REVIEW_IN_PROGRESS, provision as the change set's RoleARN and persist that RoleARN on the stack).

Traces: cloudformation__iam__sqs__ssm.cdk-deploy-role-arn-stays-the-stacks-role-for-later-deploys. The same root cause is filed separately under cloudformation__sqs, cloudformation__sqs__ssm and cloudformation__sns__sqs; the open issue 20261102T091623Z-cloudformation-stack-change-set-rolearn-is-not-the.md covers the change-set RoleARN principal, not this execute-path stall.

## Evidence
Expected (AWS): `cdk deploy --role-arn <role>` exits 0, the SSM parameter exists, and DescribeStacks reports the stack's RoleARN.
Actual (emulator): 'AssertionError: `cdk deploy --role-arn` exited 1; output tail ... <stack>: creating CloudFormation changeset...\n<stack>: deploying... [1/1]\n failed to deploy\n ValidationError: Template does not exist for stack [<stack>] in REVIEW_IN_PROGRESS state'.

## Steps to Reproduce
1. Create an SSM-only IAM role CloudFormation may assume.
2. CreateChangeSet(StackName=<new stack>, ChangeSetType=CREATE, RoleARN=<that role>, TemplateBody=<an AWS::SSM::Parameter>) then ExecuteChangeSet.
3. Poll DescribeStacks: the stack stays REVIEW_IN_PROGRESS, no parameter is created, and GetTemplate is refused ValidationError 'Template does not exist for stack [<new stack>] in REVIEW_IN_PROGRESS state'.
Replay check cloudformation__iam__sqs__ssm.cdk-deploy-role-arn-stays-the-stacks-role-for-later-deploys.
