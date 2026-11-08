Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr__events__sqs.emr-step-status-change-delivers-failed-and-withholds-it-from-the-completed-rule
Issue-Key: emr__events__sqs.emr-step-status-change-delivers-failed-and-withholds-it-from-the-completed-rule|run#24|AssertionError|assert states[pass_step_id] == "COMPLETED", (
Issue-Commit: 582ed21b383f102b650a2b551c0e79525419c00c

# emr__events__sqs: every EMR step settles FAILED regardless of its command's exit code, so an `exit 0` step never reaches COMPLETED and the COMPLETED-patterned EventBridge rule has nothing to deliver

> _Note: this issue was identified against the code base at commit `582ed21b383f102b650a2b551c0e79525419c00c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AddJobFlowSteps

## Description
1 net-new root cause, on a check the replay report did NOT mark ALREADY FILED or OWNED. The open issue 20261105T074141Z-emr-events-sqs-a-step-submitted-to-a.md was filed from the SAME check against an EARLIER emulator, where a step never left PENDING and the check died on a TimeoutError; its Issue-Key is bound to that `_poll`/TimeoutError signature, which is why this replay is not marked OWNED. The step state machine has since moved: the steps now DO reach a terminal state, and the defect is now the terminal state they reach. The open issue 20261029T184212Z-emr-clusters-accept-steps-but-never-execute-them.md describes the same inert step executor from the `emr`/`emr__glue__s3` groups and likewise does not carry this replay key. This report grounds the surviving defect in the emr__events__sqs check id below.

--- Cause 1 of 1: a step's outcome is not derived from its command, so every step that ends, ends FAILED ---
AWS: Smithy: StepState = PENDING | CANCEL_PENDING | RUNNING | COMPLETED | CANCELLED | FAILED | INTERRUPTED; Step.Status (StepStatus) carries State, StateChangeReason, FailureDetails and Timeline (StepTimeline: CreationDateTime, StartDateTime, EndDateTime). AddJobFlowSteps's own documentation states the rule verbatim: "Amazon EMR executes each step in the order listed. For a step to be considered complete, the main function must exit with a zero exit code and all Hadoop jobs started while the step was running must have completed and run successfully." So a `command-runner.jar` step whose command exits 0 must settle COMPLETED (with no StateChangeReason and no FailureDetails), and one whose command exits non-zero must settle FAILED with FailureDetails populated. Recorded behavior only for the eventing leg: each terminal transition publishes an EventBridge event with source `aws.emr`, detail-type `EMR Step Status Change` and detail.{clusterId, stepId, name, state, actionOnFailure}, which a rule patterned on that detail.state delivers to its SQS target while a rule patterned on the other state receives nothing.
Emulator: the cluster reaches WAITING, AddJobFlowSteps returns two step ids, both steps leave PENDING and reach a terminal state — and BOTH are FAILED. The outcome is hard-coded, not computed: services/service-com-amazonaws-emr-elasticmapreduce/src/state.rs, StepRecord::run_between sets `self.state = step_state::FAILED` unconditionally once the scheduled end instant has passed (no branch on the step's Jar/Args or any exit status), and StepRecord::to_status then attaches StateChangeReason.Message and FailureDetails.Reason = STEP_NOT_EXECUTED_REASON ("This emulator runs the step lifecycle but executes no Hadoop jar, so the step ended without running its application.") with FailureDetails.Message = STEP_NOT_EXECUTED_MESSAGE. The cluster's own drain reason (CLUSTER_QUEUE_DRAINED_REASON) is written the same way. The check's assertion is on the `exit 0` step: DescribeStep reports 'FAILED' where COMPLETED is required.
Why: a client cannot distinguish a step that failed from a step that succeeded — every branch on Status.State takes the failure path, every retry/alarm/reaper keyed on FAILED fires for healthy work, and no consumer can ever observe the COMPLETED half of the EMR step lifecycle. Downstream, the `EMR Step Status Change` event for a COMPLETED step can never exist, so an EventBridge rule patterned detail.state=["COMPLETED"] is dead and the FAILED-patterned rule receives events for every step, destroying the routing distinction the service is used for.
Fix: the EMR crate's step execution/state machine — services/service-com-amazonaws-emr-elasticmapreduce/src/state.rs (StepRecord::run_between and StepRecord::to_status, scheduled from ClusterRecord::advance_to, projected by src/ops/describe_step.rs and src/ops/list_steps.rs, fed by src/ops/add_job_flow_steps.rs): derive the terminal state from the submitted step's command (a zero exit => COMPLETED with no StateChangeReason/FailureDetails; non-zero => FAILED with FailureDetails), keep StepTimeline populated, and publish one `aws.emr` / `EMR Step Status Change` event carrying clusterId, stepId, name and the new state into the EventBridge service state on each transition so a state-patterned rule delivers exactly its own step's event.
Traces: emr__events__sqs.emr-step-status-change-delivers-failed-and-withholds-it-from-the-completed-rule.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the emulator; per-service pass rate emr__events__sqs 0/1 (0%).

[mismatch] emr__events__sqs.emr-step-status-change-delivers-failed-and-withholds-it-from-the-completed-rule
  File check_emr__events__sqs_emr_step_status_change_delivers_failed_and_withholds_it_from_the_completed_rule.py, line 454, in run
    assert states[pass_step_id] == "COMPLETED", (...)
  AssertionError: the `exit 0` step must COMPLETE so the positive control has something to deliver; got 'FAILED'

Everything before that assertion PASSED, so the emulator got the whole setup right: the IAM roles, the two SQS queues, the two EventBridge rules (detail.state=["COMPLETED"] and detail.state=["FAILED"]) with exactly one SQS target each, RunJobFlow, DescribeCluster reaching 'WAITING', AddJobFlowSteps returning two step ids, and BOTH steps reaching a state in {COMPLETED, FAILED, CANCELLED, INTERRUPTED} within the 600 s poll.

The two submitted steps are identical apart from their command:
  step 1: HadoopJarStep{Jar: 'command-runner.jar', Args: ['bash','-c','exit 0']}, ActionOnFailure CONTINUE
  step 2: HadoopJarStep{Jar: 'command-runner.jar', Args: ['bash','-c','exit 7']}, ActionOnFailure CONTINUE
Expected: DescribeStep(step 1).Status.State == 'COMPLETED' and DescribeStep(step 2).Status.State == 'FAILED'.
Actual: both report 'FAILED'. The assertion on step 2 ('the `exit 7` step must FAIL') is never reached, and it would pass only by coincidence — the state does not depend on the command at all.

Source confirmation (emulator side), services/service-com-amazonaws-emr-elasticmapreduce/src/state.rs:
  fn run_between(&mut self, start, end, now) { ... if now >= end { self.state = step_state::FAILED.to_string(); self.start_date_time = Some(start); self.end_date_time = Some(end); return; } if now >= start { self.state = step_state::RUNNING...
and in StepRecord::to_status: `let failed = self.state == step_state::FAILED;` attaching StateChangeReason.Message / FailureDetails.Reason = STEP_NOT_EXECUTED_REASON and FailureDetails.Message = STEP_NOT_EXECUTED_MESSAGE. There is no code path that can produce step_state::COMPLETED for a submitted step. The doc comment above run_between states the choice deliberately ("That end is the modelled FAILURE state ... COMPLETED ... would be a success for work nothing performed") — it is nonetheless a divergence: AWS's contract is that a zero-exit step is COMPLETED, and the fixed refusal text STEP_NOT_EXECUTED_REASON / STEP_NOT_EXECUTED_MESSAGE is emulator-authored wording AWS never produces.

Follow-on, not separately gradeable in this run: because no step ever becomes COMPLETED, the COMPLETED-patterned rule's queue stays empty and the check's positive control (exactly one aws.emr / 'EMR Step Status Change' envelope with detail.stepId = the passing step and detail.state = 'COMPLETED') can never be satisfied. (The cluster id and step ids of this run are generated values, not part of the requirement.)

## Steps to Reproduce
Boot the emulator and replay emr__events__sqs.emr-step-status-change-delivers-failed-and-withholds-it-from-the-completed-rule from suite simparity-behavioral-aws-v1 (report heading emr__events__sqs):
1. Create the EMR service role and the EC2 instance profile in IAM (trusting elasticmapreduce.amazonaws.com and ec2.amazonaws.com), a VPC/subnet with an S3 gateway endpoint, two SQS queues, and two EventBridge rules on source ['aws.emr'] / detail-type ['EMR Step Status Change'], one patterned detail.state=['COMPLETED'] and one detail.state=['FAILED'], each with its own queue as its single target and a queue policy allowing events.amazonaws.com to SendMessage.
2. RunJobFlow with ReleaseLabel emr-6.15.0, one m5.xlarge MASTER group, KeepJobFlowAliveWhenNoSteps=true, StepConcurrencyLevel=1; poll DescribeCluster until Status.State == 'WAITING'.
3. AddJobFlowSteps with two command-runner.jar steps, ActionOnFailure=CONTINUE: the first running `bash -c 'exit 0'`, the second `bash -c 'exit 7'`.
4. Poll DescribeStep for both returned step ids until each Status.State is terminal. Expect the FIRST step 'COMPLETED' (with no StateChangeReason and no FailureDetails) and the SECOND 'FAILED' (with FailureDetails populated), each with a populated Status.Timeline. Against the emulator BOTH report 'FAILED', with StateChangeReason.Message / FailureDetails.Reason set to the emulator-authored sentence "This emulator runs the step lifecycle but executes no Hadoop jar, so the step ended without running its application."
5. ReceiveMessage on each queue: expect exactly one aws.emr / 'EMR Step Status Change' envelope per step on the rule matching that step's own state (detail.stepId = that step, detail.state = COMPLETED / FAILED, detail.clusterId = the cluster) and nothing on the other rule's queue.
