Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: stepfunctions.alias-list-ordering-consistent stepfunctions.deep-sfn-2-sfn-jsonata-context-redrive-count-joins-describe-execution
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: stepfunctions.alias-list-ordering-consistent|run#16|AssertionError|assert tracked1 == [alias_arns["a"], alias_arns["b"], alias_arns["c"]], (
Issue-Key: stepfunctions.deep-sfn-2-sfn-jsonata-context-redrive-count-joins-describe-execution|_poll_activity_task#8|Timeout|resp = sfn.get_activity_task(activityArn=activity_arn, workerName="sim2real-behav")
Issue-Commit: e5b04c00ada8052fad6ddec319a50738ff12651d

# Step Functions aliases and redrive context diverge: ListStateMachineAliases ordering is reversed, and $$.Execution.RedriveCount is not available to the interpreter (2 causes)

> _Note: this issue was identified against the code base at commit `e5b04c00ada8052fad6ddec319a50738ff12651d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListStateMachineAliases

## Description
Two net-new root causes in the sfn service, bundled. A fixer must close BOTH before submitting.

=== Cause 1 of 2: ListStateMachineAliases returns the aliases of a state machine in the opposite order to the service ===

AWS: Smithy model, ListStateMachineAliases -> output stateMachineAliases (StateMachineAliasList, required); operation doc: "Lists aliases for a specified state machine ARN. Results are sorted by time... If nextToken is returned, there are more results available." Recorded behavior: for three aliases created in the order a, then b, then c on one state machine, the service's listing presents them in CREATION order - a, b, c - and that relative order is stable across an unrelated UpdateStateMachineAlias, an unrelated CreateStateMachineAlias and an unrelated DeleteStateMachineAlias, and across repeated calls. The recorded order is the requirement here; where the prose summary and the recording disagree, the recording is what a client observes.

Emulator: the listing comes back exactly reversed - newest-created first. The replayed check read ['...:sim2real-behav-alias-c-...', '...:sim2real-behav-alias-b-...', '...:sim2real-behav-alias-a-...'] where a, b, c was owed, and failed on its first assertion ("unexpected initial alias order") before it could even grade the stability-under-unrelated-mutation part. The call itself answered 200 with the right membership and a usable nextToken, so only the ORDER of the list diverges.

Why: a client that walks ListStateMachineAliases to find "the first alias created" or that pages through aliases and reconciles them positionally against its own creation ledger gets the list inverted. Pagination makes it worse than a cosmetic difference: with maxResults smaller than the alias count, the first page contains the wrong aliases entirely, so a paged consumer that stops early sees a different set, not merely a different order.

Fix: the ordering the ListStateMachineAliases handler asks the store for - services/service-com-amazonaws-sfn-awsstepfunctions/src/ops/list_state_machine_aliases.rs selects store.aliases_newest_first(&state_machine_arn) and then pages with skip(offset)/take(page), so the reversal is applied before pagination and the fix belongs in that selection (and in the store helper it names), not in the serializer.

Traces: sfn.alias-list-ordering-consistent.

=== Cause 2 of 2: the context object exposes no Execution.RedriveCount, so a definition that reads $$.Execution.RedriveCount cannot run and the redrive path is unobservable ===

AWS: Smithy model, DescribeExecution -> output redriveCount (RedriveCount, integer), redriveDate (Timestamp), redriveStatus (ExecutionRedriveStatus enum); RedriveExecution doc: "When you redrive an execution, it continues the failed execution from the unsuccessful step and uses the same input. Step Functions preserves the results and execution history of the successful steps, and doesn't rerun these steps when you redrive an execution. Redriven executions use the same state machine definition and execution ARN as the original execution attempt... Redriven executions append their event history to the existing event history." Recorded behavior: the interpreter re-evaluates the context object on every (re-)entry to a state, so a state entered on the original run reads $$.Execution.RedriveCount = 0 and a state re-entered on the redriven run reads 1; the same value is readable in a JSONata-mode state as $states.context.Execution.RedriveCount; it agrees with DescribeExecution's redriveCount; a state that already succeeded is NOT re-entered (its stamped 0 survives into the final output and exactly one PassStateEntered event exists for it); and the redrive appends exactly one ExecutionRedriven history event carrying redriveCount 1.

Emulator: the check never gets past its first wait. It starts an execution of StartAt "Stamp" (a Pass state whose Parameters are {"rc.$": "$$.Execution.RedriveCount"}) followed by an activity Task, then long-polls GetActivityTask for the task token; no activity task is ever handed out and the check is killed at the suite's 60 s cap ("Check timed out after 60.0s"), so nothing downstream - SendTaskFailure, RedriveExecution, the redriven run's context value, DescribeExecution's redriveCount, the ExecutionRedriven event - was ever reached. Hypothesis for the mechanism: the interpreter's context object carries only the execution/state-machine identity fields (crate::state::asl::Context is built with execution_arn, execution_name, state_machine_arn, state_machine_name, role_arn - see the Context literal in src/ops/redrive_execution.rs), with no Execution.RedriveCount member, so resolving "$$.Execution.RedriveCount" in the very first Pass state's payload template fails and the execution ends before the activity Task can schedule a task, leaving GetActivityTask with nothing to return forever.

Why: any workflow that routes on, logs or propagates its own redrive attempt count - the documented way a redriven run tells itself apart from the original - cannot run at all here, and it fails at the first state rather than with a diagnosable refusal at create time. Because the execution dies before the activity Task is scheduled, a worker polling GetActivityTask hangs indefinitely instead of receiving work, which is the worst failure mode for a callback-style client: no error, no progress.

Why this is not covered by an open issue: 20261024T190039Z-sfn-step-functions-activities-and-teststate-diverge-task-states.md covers activity Task states not executing and 20261024T043150Z-sfn-step-functions-interpreter-does-not-run-the-workflow.md covers Task/JSONata/Distributed-Map/callback execution generally; neither names the context object's Execution.RedriveCount member, the JSONata $states.context.Execution.RedriveCount spelling, the agreement of that value with DescribeExecution's redriveCount, or the ExecutionRedriven history event's redriveCount - and RedriveExecution itself is implemented in this checkout, so this is a distinct gap in the context object rather than a restatement of either.

Fix: the interpreter's context object - the Context type and its construction/serialisation in services/service-com-amazonaws-sfn-awsstepfunctions/src/state/asl.rs, plus the two call sites that build it (src/ops/start_execution.rs and src/ops/redrive_execution.rs, which passes the same identity on a redrive) - must expose Execution.RedriveCount, resolved per state entry, under both $$ and $states.context; the value must be the same one DescribeExecution projects and the one the ExecutionRedriven event carries (src/ops/describe_execution.rs / the store's redrive bookkeeping).

Traces: sfn.deep-sfn-2-sfn-jsonata-context-redrive-count-joins-describe-execution.

## Evidence
Cause 1 - sfn.alias-list-ordering-consistent [mismatch]:
  AssertionError: unexpected initial alias order
  ['arn:aws:states:us-west-2:<acct>:stateMachine:sim2real-behav-aliasord-<id>:sim2real-behav-alias-c-<id>',
   'arn:aws:states:us-west-2:<acct>:stateMachine:sim2real-behav-aliasord-<id>:sim2real-behav-alias-b-<id>',
   'arn:aws:states:us-west-2:<acct>:stateMachine:sim2real-behav-aliasord-<id>:sim2real-behav-alias-a-<id>']
  Expected (real AWS): [alias a, alias b, alias c] - the order they were created in.
  Actual (emulator):   [alias c, alias b, alias a] - exactly reversed.
  Status/code: no refusal on either side; ListStateMachineAliases answered 200 with the correct three ARNs. Only the list order diverges.

Cause 2 - sfn.deep-sfn-2-sfn-jsonata-context-redrive-count-joins-describe-execution [mismatch]:
  Error: Check timed out after 60.0s
  Expected (real AWS), in order: GetActivityTask hands out the "Work" task with input {"rc": 0, "before": 0}; after SendTaskFailure the execution is (FAILED, REDRIVABLE, redriveCount 0); after RedriveExecution the re-entered task's input is {"rc": 1, "before": 0}; the run ends SUCCEEDED with redriveCount 1 and output {"orig": true, "before": {"rc": 0}, "work": {"ok": true}, "after": {"rc": 1}, "nata": 1}; history carries exactly one ExecutionRedriven with redriveCount 1 and exactly one PassStateEntered for "Stamp".
  Actual (emulator): GetActivityTask returns no task token for the whole 90 s poll budget, so the check is killed at the 60 s cap having observed none of the above. The first state of the definition is a Pass whose Parameters read "$$.Execution.RedriveCount".
  Supporting code evidence: services/service-com-amazonaws-sfn-awsstepfunctions/src/ops/redrive_execution.rs builds crate::state::asl::Context with only execution_arn, execution_name, state_machine_arn, state_machine_name, role_arn - there is no RedriveCount member for $$.Execution.RedriveCount to resolve against.
  Smithy: DescribeExecution models redriveCount/redriveDate/redriveStatus on its output, so the value the context object must agree with is part of the modelled contract.

## Steps to Reproduce
Cause 1 (ListStateMachineAliases ordering):
1. CreateStateMachine with a trivial definition {"StartAt":"Done","States":{"Done":{"Type":"Succeed"}}} and a states.amazonaws.com-trusted role.
2. PublishStateMachineVersion on it.
3. CreateStateMachineAlias three times in order - name them a, then b, then c - each routing 100% to that version.
4. ListStateMachineAliases(stateMachineArn=<the state machine ARN>), paging with nextToken until exhausted.
   Expected: the three alias ARNs appear in creation order a, b, c.
   Actual: they appear c, b, a.
5. (Still ungraded because step 4 fails first) UpdateStateMachineAlias on b, CreateStateMachineAlias d, DeleteStateMachineAlias d, and re-list: the relative order of a/b/c must not change, and two back-to-back listings must agree.
Replay: sfn.alias-list-ordering-consistent

Cause 2 ($$.Execution.RedriveCount):
1. CreateActivity, and CreateStateMachine (STANDARD) with the definition:
   Stamp (Pass, Parameters {"rc.$": "$$.Execution.RedriveCount"}, ResultPath "$.before") -> Work (Task, Resource = the activity ARN, Parameters {"rc.$": "$$.Execution.RedriveCount", "before.$": "$.before.rc"}, ResultPath "$.work") -> After (Pass, Parameters {"rc.$": "$$.Execution.RedriveCount"}, ResultPath "$.after") -> AfterNata (Pass, QueryLanguage JSONata, Output "{% $merge([$states.input, {'nata': $states.context.Execution.RedriveCount}]) %}").
2. StartExecution with input {"orig": true}.
3. Poll GetActivityTask on the activity ARN.
   Expected: a task token with input {"rc": 0, "before": 0}.
   Actual: no task token ever arrives; the poll runs out.
4. Once step 3 yields: SendTaskFailure(error="SimRedriveProbe"), confirm DescribeExecution is (FAILED, REDRIVABLE, redriveCount 0), RedriveExecution, re-poll GetActivityTask and confirm the input is {"rc": 1, "before": 0}, SendTaskSuccess({"ok": true}), then confirm DescribeExecution redriveCount == 1, the final output's after.rc == nata == 1, one ExecutionRedriven event with redriveCount 1, and exactly one PassStateEntered for "Stamp".
Replay: sfn.deep-sfn-2-sfn-jsonata-context-redrive-count-joins-describe-execution
