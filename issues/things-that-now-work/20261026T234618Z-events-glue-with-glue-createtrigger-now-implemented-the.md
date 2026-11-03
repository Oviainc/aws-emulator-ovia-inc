Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__glue.event-trigger-fires-only-after-its-batch-size-is-reached
Issue-Key: events__glue.event-trigger-fires-only-after-its-batch-size-is-reached|run#17|Timeout|time.sleep(60.0) # IAM role propagation is eventually consistent
Issue-Commit: da7bc09c8c1c199d36cb9fc39c5c5796d768dc3a

# events__glue: with glue CreateTrigger now implemented, the EventBridge->Glue-workflow ingress still never turns a delivery into a workflow run, so the EVENT trigger's EventBatchingCondition contract makes no progress at all

> _Note: this issue was identified against the code base at commit `da7bc09c8c1c199d36cb9fc39c5c5796d768dc3a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
One root cause. Cause count: 1.

=== Cause 1 of 1: an EventBridge rule target that is a Glue workflow ARN never produces a Glue workflow run, so neither the batched nor the unbatched EVENT-trigger arm ever advances ===
AWS: a workflow whose only starting trigger is of Type EVENT is driven from EventBridge. PutTargets accepts a target whose Arn is `arn:aws:glue:<region>:<account>:workflow/<name>` together with a RoleArn granting `glue:notifyEvent`, answering FailedEntryCount 0, and every event the rule matches is then delivered to that workflow and counted against its trigger's EventBatchingCondition. Smithy/docs (com.amazonaws.glue#EventBatchingCondition, API_EventBatchingCondition): BatchSize is @required with @range 1..100 — "Number of events that must be received from Amazon EventBridge before EventBridge event trigger fires" — and BatchWindow is @range 1..900 — "Window of time in seconds after which EventBridge event trigger fires. Window starts when first event is received". So with BatchSize=3 and BatchWindow=900 the workflow must have ZERO runs after deliveries 1 and 2 and EXACTLY ONE after delivery 3 (GetWorkflowRuns), while a BatchSize=1 twin on the SAME rule, role and deliveries must already have one run from delivery 1; the run carries the workflow's Name and a WorkflowRunId. Recorded behavior supplies only the delivery LATENCY (the check calibrates its absence window from the twin's measured latency), never a fixed timing requirement.
Emulator: glue CreateTrigger now accepts the EVENT trigger — services/service-com-amazonaws-glue-awsglue/src/ops/create_trigger.rs is implemented, carries no SCAFFOLDED marker, and stores State CREATED for an EVENT trigger with StartOnCreation absent (lines 254-273) — but the check then makes NO further progress and the harness kills it: "Check timed out after 60.0s". The check issues no StartTrigger (an EVENT trigger has no CREATED->ACTIVATED lifecycle), so the only bounded waits it can be sitting in are the two whose own budgets exceed the 60 s harness window: the retry that requires `events:PutTargets` to report FailedEntryCount == 0 for the two Glue workflow targets (budget 120 s) and the poll waiting for the BatchSize=1 twin workflow to show its first run after delivery 1 (budget 480 s). Both are longer than 60 s, so neither can have completed, and the check is killed before it reaches a single assertion about batching. Hypothesis: there is no EventBridge target-delivery path for a Glue workflow ARN (the `glue:notifyEvent` ingress), so either PutTargets never accepts such a target or a delivered event never becomes a workflow run and GetWorkflowRuns stays empty forever.
Why: the only way AWS lets an event start a Glue workflow is an EVENT trigger behind an EventBridge rule, and the only reason to set EventBatchingCondition is to SUPPRESS work — coalescing N deliveries into one run. A client that builds the documented ingress sees a rule it created, a trigger in state CREATED, PutEvents accepted with FailedEntryCount 0, and then nothing ever happens: no workflow run, no error, no signal distinguishing "the batch has not filled yet" from "deliveries are dropped on the floor". Worse, an emulator that later starts one run per delivery (ignoring BatchSize) would be indistinguishable from a correct one today, because this whole path is ungraded.
Related open issues that no longer describe this failure: 20261026T095655Z-events-glue-glue-createtrigger-is-an-unimplemented-501.md and 20261026T095517Z-glue-createtrigger-501-stub-also-blocks-the-cross.md both diagnose this check as aborting on glue CreateTrigger answering 501 NotImplemented. That no longer reproduces — CreateTrigger is implemented — so a fixer who closes those two on the stub alone will re-replay this check and still find it failing; the surviving defect is the ingress and the batch accounting described above, not the create call.
Fix: the EventBridge target-delivery component for `glue` workflow target ARNs (the `glue:notifyEvent` path invoked with the target's RoleArn), plus the Glue trigger's batch accounting in the trigger/workflow state — count each delivery against the trigger's EventBatchingCondition and start exactly one workflow run when BatchSize is reached (or when BatchWindow expires), recording it so GetWorkflowRuns reports it with the workflow's Name and a WorkflowRunId.
Traces: events__glue.event-trigger-fires-only-after-its-batch-size-is-reached.

## Evidence
events__glue.event-trigger-fires-only-after-its-batch-size-is-reached [mismatch]

Actual (emulator): "Error: Check timed out after 60.0s" — the check produced no assertion failure and no API error; it was killed while blocked in one of its two bounded waits.

Expected (AWS, per the docs cited above):
- events:PutTargets(Rule=r, Targets=[{Id:'batched', Arn:'arn:aws:glue:<region>:<account>:workflow/<W1>', RoleArn:<role with glue:notifyEvent>}, {Id:'twin', Arn:'.../workflow/<W2>', RoleArn:<same>}]) -> FailedEntryCount 0, and ListTargetsByRule echoes both target ARNs.
- after the FIRST events:PutEvents entry matching the rule: glue:GetWorkflowRuns(Name=<W2>) (the BatchSize=1 twin) reports one run.
- throughout a window shorter than BatchWindow=900 s: glue:GetWorkflowRuns(Name=<W1>) (BatchSize=3) reports ZERO runs after deliveries 1 and 2 — GetWorkflowRuns 400 EntityNotFoundException for a never-run workflow is the same zero.
- after the THIRD delivery: glue:GetWorkflowRuns(Name=<W1>) reports EXACTLY one run, whose Name is <W1> and which carries a non-empty WorkflowRunId.

What the emulator demonstrably does do, and why that narrows the stall: glue CreateTrigger(Type='EVENT', EventBatchingCondition={'BatchSize':3,'BatchWindow':900}) is accepted (the handler is implemented: services/service-com-amazonaws-glue-awsglue/src/ops/create_trigger.rs, with State CREATED derived at lines 269-273), and the check's GetTrigger poll for State=='CREATED' is therefore satisfiable. No StartTrigger is called by this check. The remaining waits are the PutTargets-accepts-a-Glue-workflow-target retry (120 s) and the twin-workflow-run poll (480 s); both exceed the 60 s harness window, so the stall is in the EventBridge->Glue delivery path, not in trigger creation. The generated workflow/trigger/job/role names, the account id and the exact 60 s harness budget are per-run values, not requirements.

## Steps to Reproduce
1. iam.create_role trusting glue.amazonaws.com; glue.create_job(Name=J, Command={'Name':'pythonshell',...}, MaxCapacity=0.0625).
2. glue.create_workflow(Name=W1) and glue.create_workflow(Name=W2).
3. glue.create_trigger(Name=W1+'-evt', Type='EVENT', WorkflowName=W1, Actions=[{'JobName': J}], EventBatchingCondition={'BatchSize': 3, 'BatchWindow': 900}) and glue.create_trigger(Name=W2+'-evt', Type='EVENT', WorkflowName=W2, Actions=[{'JobName': J}], EventBatchingCondition={'BatchSize': 1}); glue.get_trigger on each -> Type 'EVENT', State 'CREATED', EventBatchingCondition as submitted (this much already works).
4. iam.create_role trusting events.amazonaws.com with glue:notifyEvent on both workflow ARNs; events.put_rule(Name=r, State='ENABLED', EventPattern='{"source":["<S>"],"detail-type":["<DT>"]}').
5. events.put_targets(Rule=r, Targets=[{'Id':'batched','Arn':'arn:aws:glue:<region>:<acct>:workflow/'+W1,'RoleArn':<events role>}, {'Id':'twin','Arn':'arn:aws:glue:<region>:<acct>:workflow/'+W2,'RoleArn':<events role>}]).
   -> AWS: FailedEntryCount 0 and ListTargetsByRule reports both. Record what the emulator answers here: a non-zero FailedEntryCount (or a FailedEntries list) is the first half of this cause.
6. events.put_events one matching entry (FailedEntryCount must be 0), then poll glue.get_workflow_runs(Name=W2).
   -> AWS: one run appears for the BatchSize=1 twin. Emulator: no run ever appears — this is where the check is killed at 60 s.
7. (What must pass after the fix) assert glue.get_workflow_runs(Name=W1) reports ZERO runs across deliveries 1 and 2 for a window well under BatchWindow=900 s, then put a third event and assert it reports EXACTLY one run whose Name is W1 and which carries a WorkflowRunId.

Replay events__glue.event-trigger-fires-only-after-its-batch-size-is-reached to confirm a fix.
