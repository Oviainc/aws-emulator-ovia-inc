Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__stepfunctions.e2e-diagnose-step-functions-failing-executions-lifecycle
Issue-Key: lambda__stepfunctions.e2e-diagnose-step-functions-failing-executions-lifecycle|phase#6|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Commit: 04fdd45ea890c13ea5d31cc41f99075331e6a703

# lambda__stepfunctions: a log group declared by a deployed stack does not exist, so the stack-owned group cannot survive a cleanup phase

> _Note: this issue was identified against the code base at commit `04fdd45ea890c13ea5d31cc41f99075331e6a703`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1 — an AWS::Logs::LogGroup declared in a deployed template is never created, so nothing of that prefix exists after the stack reports success.
AWS: recorded behavior: a stack that declares a log group with an explicit LogGroupName reaches CREATE_COMPLETE only once that group exists; DescribeLogGroups(logGroupNamePrefix=<the declared name>) returns exactly that group for the rest of the stack's life, and only DeleteStack releases it. A cleanup step that reaps the run's OWN ad-hoc groups therefore leaves the stack-declared one standing.
Emulator: after the stack deploys and the scenario runs, DescribeLogGroups on the declared prefix lists [] — the stack-owned group was never created, so the cleanup phase cannot distinguish "released by destroy" from "never existed".
Why: a client that reads a deployed workflow's logs through the group its template declares finds no group at all, and stack ownership of a log group (created by create, released by delete) is not modelled.
Fix: the AWS::Logs::LogGroup resource provider's create path and its ownership/deletion bookkeeping in the stack.
Traces: lambda__stepfunctions.e2e-diagnose-step-functions-failing-executions-lifecycle

The same missing-resource behavior is reported for the cloudformation__stepfunctions suite in this sweep; it is filed separately there because an issue carries one service token.

## Evidence
From this replay of simparity-behavioral-aws-v1: AssertionError: [phase 4/5 cleanup] exactly the stack-owned group '/sim2real/scn/declared-<id>' must survive cleanup — it is destroy's to release, not cleanup's; the prefix now lists []. The stack had deployed successfully earlier in the same replay, and the group name is the one its template declares.

## Steps to Reproduce
1. Deploy the check's CDK/CFN template (deployments/sfn-processing) which declares an AWS::Logs::LogGroup with an explicit name under /sim2real/scn/, a failing Lambda and a Step Functions state machine; wait for CREATE_COMPLETE.
2. Call logs:DescribeLogGroups(logGroupNamePrefix='/sim2real/scn/declared-') — the declared group must be listed.
3. Run the replay's cleanup phase (which deletes only the groups the run created ad hoc) and list the prefix again: the stack-declared group must still be there, and must disappear only after DeleteStack.
