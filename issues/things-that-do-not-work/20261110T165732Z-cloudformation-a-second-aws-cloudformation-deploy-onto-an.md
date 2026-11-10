Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.redeploy-with-changed-template-applies-the-change cloudformation.redeploy-broken-importvalue-rolls-back-preserving-original
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation.redeploy-with-changed-template-applies-the-change|_deploy#15|Timeout|result = subprocess.run(
Issue-Key: cloudformation.redeploy-broken-importvalue-rolls-back-preserving-original|_deploy_allow_failure#16|Timeout|result = subprocess.run(
Issue-Commit: 0c065617979d1ee76fa77b423bd58183fc04c861

# cloudformation: a second `aws cloudformation deploy` onto an existing stack never settles — the CLI blocks until the check times out

> _Note: this issue was identified against the code base at commit `0c065617979d1ee76fa77b423bd58183fc04c861`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteChangeSet

## Description
One net-new root cause, grounded in two failing checks.

Cause 1 of 1: the update path `aws cloudformation deploy` takes against a stack that already exists (create change set, execute it, wait for a terminal stack status) never reaches a terminal state, so the CLI never returns.

AWS: recorded behavior only (the CLI's `deploy` is a composite of CreateChangeSet, ExecuteChangeSet and DescribeStacks/DescribeStackEvents; nothing in the Smithy model governs the composite). The recording shows a re-`deploy` of a changed template against the stack the first `deploy` created reaching UPDATE_COMPLETE, with the added resource really usable; and a re-`deploy` whose new resource imports a never-declared export exiting non-zero, rolling the stack back to UPDATE_ROLLBACK_COMPLETE with a reason that names the missing export, leaving the original resource untouched and the new one never created, after which a third valid `deploy` still succeeds. In both cases the command terminates.

Emulator: neither check reaches any AWS-level assertion — the harness reports `Error: Check timed out after 60.0s` for both. The first `deploy` is not what hangs: other CLI-driven CloudFormation checks in this suite do reach assertions (cloudformation.cli-script-cfn-deploy-and-verify fails on a real comparison, not a timeout). It is specifically the second `deploy` — the change-set-driven UPDATE of an existing stack — that never settles. Hypothesis: the executed UPDATE change set leaves the stack parked in UPDATE_IN_PROGRESS (and, in the broken-ImportValue case, never transitions to UPDATE_ROLLBACK_COMPLETE), so the CLI's built-in waiter polls forever.

Why: a client's deployment pipeline blocks indefinitely on every stack update and never learns the outcome. Because no terminal status is ever published, the caller cannot distinguish success from failure, cannot roll back, and cannot retry without risking a double update. A failed update also never surfaces the missing-export reason a client needs to fix its template.

Fix: the CloudFormation stack-update state machine behind ExecuteChangeSet — an executed UPDATE change set must drive the stack to UPDATE_COMPLETE, and an update whose resource cannot be resolved (an Fn::ImportValue of an export that does not exist) must drive it to UPDATE_ROLLBACK_COMPLETE with the failing resource's reason naming the missing export, in both cases publishing the terminal status DescribeStacks reports.

Traces: cloudformation.redeploy-with-changed-template-applies-the-change, cloudformation.redeploy-broken-importvalue-rolls-back-preserving-original.

## Evidence
Expected (happy path): the second `aws cloudformation deploy` returns 0 and DescribeStacks reports UPDATE_COMPLETE, with the added queue able to send and receive.
Expected (rejection path): the second `aws cloudformation deploy` exits non-zero, DescribeStacks reports UPDATE_ROLLBACK_COMPLETE, the failing resource's ResourceStatusReason names the never-declared export, the pre-existing resource is untouched and the new one was never created.

Actual (suite report, both checks, verdict mismatch):
```
cloudformation.redeploy-with-changed-template-applies-the-change
Error: Check timed out after 60.0s

cloudformation.redeploy-broken-importvalue-rolls-back-preserving-original
Error: Check timed out after 60.0s
```
No stack status, no exit code and no reason text were ever observed — the commands did not terminate.

## Steps to Reproduce
1. `aws cloudformation deploy` a template with one SQS queue; it reaches CREATE_COMPLETE.
2. `aws cloudformation deploy` the same stack again with a template that adds a second queue. The CLI never returns; DescribeStacks stays in UPDATE_IN_PROGRESS.
3. Repeat step 2 with a template whose added resource has a tag value of `Fn::ImportValue` naming an export that was never declared. The CLI never returns and the stack never reaches UPDATE_ROLLBACK_COMPLETE.
