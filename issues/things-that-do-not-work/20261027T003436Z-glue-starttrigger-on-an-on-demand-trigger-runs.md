Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue.trigger-on-demand-lifecycle
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue.trigger-on-demand-lifecycle|run#9|AssertionError|assert len(runs) == 1, f"ON_DEMAND start_trigger should produce exactly one job run, got {len(runs)}"
Issue-Commit: cf7df418f843e0a7b878589d1abafe23db84fa21

# glue: StartTrigger on an ON_DEMAND trigger runs none of its Actions (GetJobRuns stays empty) and leaves the trigger ACTIVATED instead of CREATED (2 causes)

> _Note: this issue was identified against the code base at commit `cf7df418f843e0a7b878589d1abafe23db84fa21`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartTrigger

## Description
Two root causes, both in the glue StartTrigger handler. Cause count: 2.

NOTE ON AN EXISTING ISSUE: the open issue 20261026T234618Z-glue-starttrigger-stub.md claims `glue.trigger-on-demand-lifecycle` fails because StartTrigger is an unimplemented 501 scaffold stub. That no longer reproduces: services/service-com-amazonaws-glue-awsglue/src/ops/start_trigger.rs is implemented, carries no `//! SCAFFOLDED, NOT IMPLEMENTED:` header, and the call now answers 200 (the replay verdict is a `mismatch` on an assertion, not a ClientError). A fixer who closes that stub issue alone will re-replay this check and still find it failing; the surviving defects are the two below.

=== Cause 1 of 2: an ON_DEMAND trigger that is started initiates none of its Actions, so the job it names gets no job run ===
AWS: `StartTrigger` "Starts an existing trigger", and a trigger's `Actions` are "The actions initiated by this trigger" (Smithy: com.amazonaws.glue#Trigger$Actions -> ActionList of com.amazonaws.glue#Action, whose `JobName` is "The name of a job to be run"). An ON_DEMAND trigger is the one type that fires on the caller alone — the model states "Set to true to start SCHEDULED and CONDITIONAL triggers when created. True is not supported for ON_DEMAND triggers" (CreateTriggerRequest$StartOnCreation) — so the start IS the fire. Firing it initiates each action, which for an `Action` carrying a `JobName` is a run of that job: afterwards `GetJobRuns(JobName=<the action's job>)` reports exactly one `JobRun` for the one fire (Smithy: GetJobRuns output `JobRuns`), with no `StartJobRun` call from the client. A trigger with no `WorkflowName` is fully legal — `Trigger$WorkflowName` is optional — and its actions are initiated just the same.
Emulator: the handler derives `fires_on_start` from `trigger_type == ON_DEMAND` and then begins a run ONLY of the workflow the trigger names (start_trigger.rs lines 193-233: `workflow_name.as_deref().filter(|_| fires_on_start)` -> `workflow.begin_run(...)`). It reads `trigger.actions.len()` purely as a count (`total_actions`) and never initiates a single action, and its own module doc says so: "Nothing here runs the jobs and crawlers a trigger's actions initiate". For a trigger with no `WorkflowName` the fire is therefore a complete no-op beyond a state write: the start answers 200 with the trigger name, and `GetJobRuns(JobName=J)` keeps reporting an empty `JobRuns` list. The check polls that list for 30 s and fails: "AssertionError: ON_DEMAND start_trigger should produce exactly one job run, got 0".
Why: ON_DEMAND triggers are how clients and orchestrators kick off Glue work without reimplementing `StartJobRun` bookkeeping. A client that starts one sees a 200 and then no run at all: nothing in `GetJobRuns`, no run id to poll, no failure to surface. Logic that waits for the triggered run hangs forever, and logic that counts runs to enforce "fire once" can never be exercised, so an emulator that later fires zero, one or ten runs is indistinguishable from a correct one.
Fix: the StartTrigger handler — on a fire, initiate the trigger's `Actions`, creating one job run per `Action.JobName` (and the crawler equivalent for `Action.CrawlerName`) in the job-run state the store already holds for `StartJobRun`, independently of whether the trigger carries a `WorkflowName`; apply the action's `Arguments`/`Timeout`/`NotificationProperty` overrides the model documents.
Traces: glue.trigger-on-demand-lifecycle (services/service-com-amazonaws-glue-awsglue/src/ops/start_trigger.rs lines 193-249).

=== Cause 2 of 2: after an ON_DEMAND trigger is started, GetTrigger reports State ACTIVATED instead of CREATED ===
AWS: `Trigger$State` is the trigger's current state out of the modelled `TriggerState` enum (Smithy: CREATING, CREATED, ACTIVATING, ACTIVATED, DEACTIVATING, DEACTIVATED, DELETING, UPDATING). ACTIVATED is the state of a trigger that has been switched ON and is now waiting for its firing condition — which only the types that wait for something outside the caller have (SCHEDULED, CONDITIONAL, EVENT); the model marks ON_DEMAND as the type that cannot be started on creation at all. An ON_DEMAND trigger has nothing to wait in: starting it fires it once and it settles back to CREATED, which is what a `GetTrigger` after the start reports. Recorded behavior (this check's own assertion) pins exactly that: `glue.get_trigger(Name=T)['Trigger']['State'] == 'CREATED'` after `start_trigger`.
Emulator: the handler writes `trigger.state = crate::state::TRIGGER_STATE_ACTIVATED` unconditionally for every trigger type, including ON_DEMAND (start_trigger.rs lines 235-242), and that write is a steady state nothing later clears. Creation is correct (create_trigger.rs lines 266-273 store CREATED when `StartOnCreation` is absent), so the divergence is introduced by the start. The check does not reach this assertion today because Cause 1 aborts it first; it fails the moment Cause 1 is fixed.
Why: clients and IaC drift detection read `Trigger$State` to decide whether a trigger needs starting or stopping. A permanently ACTIVATED ON_DEMAND trigger reads as "armed and waiting", so reconcilers skip the start it actually needs, and `StopTrigger` is offered on a trigger AWS would refuse to stop.
Fix: the StartTrigger handler's state write — derive the post-fire state from the trigger's `Type`: a type that waits for an external condition becomes ACTIVATED, while an ON_DEMAND trigger that fired here stays CREATED.
Traces: glue.trigger-on-demand-lifecycle (start_trigger.rs line 241; compare create_trigger.rs lines 269-273).

## Evidence
glue.trigger-on-demand-lifecycle [mismatch]

Actual (emulator):
  File "check_glue_trigger_on_demand_lifecycle.py", line 133, in run
    assert len(runs) == 1, f"ON_DEMAND start_trigger should produce exactly one job run, got {len(runs)}"
  AssertionError: ON_DEMAND start_trigger should produce exactly one job run, got 0

The sequence the check drives, and what each call answers:
- iam.create_role (trusting glue.amazonaws.com) + attach AWSGlueServiceRole -> OK
- glue.create_job(Name=J, Role=<role>, Command={'Name':'pythonshell','PythonVersion':'3.9','ScriptLocation':'s3://.../scripts/etl.py'}, MaxCapacity=0.0625, Timeout=10) -> OK
- glue.create_trigger(Name=T, Type='ON_DEMAND', Actions=[{'JobName': J}]) -> OK, and the trigger is stored with State CREATED (correct)
- glue.start_trigger(Name=T) -> HTTP 200, {'Name': T}. No error, nothing refused.
- glue.get_job_runs(JobName=J) polled every 2 s for 30 s -> {'JobRuns': []} every time.

Expected (AWS, per the Smithy model cited in the description):
- glue.get_job_runs(JobName=J) reports EXACTLY ONE JobRun after the single start_trigger — the run the trigger's one Action initiated, carrying a JobRunId minted by the service and the job's name. (Its generated run id, timestamps and terminal state are per-run values, not requirements; the requirement is that exactly one run exists.)
- glue.get_trigger(Name=T)['Trigger']['State'] == 'CREATED' after the fire. Emulator reports 'ACTIVATED' (written at start_trigger.rs:241); the check cannot reach this assertion until the first one passes.

The emulator's own handler documents the gap: start_trigger.rs lines 15-19 — "Nothing here runs the jobs and crawlers a trigger's actions initiate" — and lines 193-233 only ever call `workflow.begin_run(...)`, and only when the trigger carries a WorkflowName (this trigger carries none). Generated role/job/trigger names and the 30 s poll budget are per-run values, not requirements.

## Steps to Reproduce
1. iam.create_role(RoleName=R, AssumeRolePolicyDocument=<trust glue.amazonaws.com>); iam.attach_role_policy(RoleName=R, PolicyArn='arn:aws:iam::aws:policy/service-role/AWSGlueServiceRole').
2. glue.create_job(Name=J, Role=<R's arn>, Command={'Name':'pythonshell','PythonVersion':'3.9','ScriptLocation':'s3://<bucket>/scripts/etl.py'}, MaxCapacity=0.0625, Timeout=10) -> 200.
3. glue.create_trigger(Name=T, Type='ON_DEMAND', Actions=[{'JobName': J}]) -> 200; glue.get_trigger(Name=T)['Trigger']['State'] -> 'CREATED' (already correct). Note: do NOT set a WorkflowName — this trigger belongs to no workflow.
4. glue.start_trigger(Name=T) -> 200 {'Name': T}.
5. Poll glue.get_job_runs(JobName=J) for 30 s.
   -> AWS: exactly one JobRun appears (the action the fire initiated).
   -> Emulator: {'JobRuns': []} forever. This is Cause 1.
6. glue.get_trigger(Name=T)['Trigger']['State'].
   -> AWS: 'CREATED' (an ON_DEMAND trigger settles back; it never sits in ACTIVATED).
   -> Emulator: 'ACTIVATED'. This is Cause 2.
7. Replay glue.trigger-on-demand-lifecycle to confirm both causes are closed.
