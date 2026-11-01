Issue-Source: trace-sweep-divergence
Issue-Trace: swf-deprecated-domain-blocks-operations
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: swf-deprecated-domain-blocks-operations swf-request-cancel-workflow-execution swf-start-describe-signal-history-terminate swf-workflow-execution-full-cycle swf-workflow-signal-and-cancel swf-activity-task-canceled-path swf-activity-task-failure-path swf-workflow-decision-and-activity-lifecycle swf-terminate-workflow-execution-lifecycle swf-domain-lifecycle-and-tags swf-tag-untag-full-clear swf-terminate-workflow-execution swf-workflow-terminate-and-list swf-closed-workflow-count-and-list-after-terminate swf-list-open-workflow-executions-by-tag swf-open-workflow-count-and-list swf-open-executions-visibility-counts swf-workflow-execution-history-pagination swf-domain-tag-lifecycle swf-domain-tag-isolation-across-resources swf-activity-task-failure-and-cancellation
Issue-Shape-Id: com.amazonaws.swf#SimpleWorkflowService
Issue-Key: swf|DescribeWorkflowExecution|2xx->2xx|$.executionInfo.lastClosedEventId
Issue-Key: swf|DescribeWorkflowExecution|2xx->2xx|$.executionInfo.reopenGenerationId
Issue-Key: swf|DescribeWorkflowExecution|2xx->2xx|$.executionInfo.reopenable
Issue-Key: swf|DescribeWorkflowExecution|2xx->2xx|$.executionInfo.cancelRequested
Issue-Key: swf|DescribeWorkflowExecution|2xx->2xx|$.executionInfo.lastClosedTimestamp
Issue-Key: swf|PollForDecisionTask|2xx->2xx|$.events[*].decisionTaskScheduledEventAttributes.scheduleToStartTimeout
Issue-Key: swf|PollForDecisionTask|2xx->2xx|$.events[*].decisionTaskStartedEventAttributes.clientToken
Issue-Key: swf|PollForDecisionTask|2xx->2xx|$.events[*].decisionTaskStartedEventAttributes.startedOnPreviousTaskCompletion
Issue-Key: swf|GetWorkflowExecutionHistory|2xx->2xx|$.events[*]
Issue-Key: swf|ListTagsForResource|2xx->2xx|$.tags[key="env"].__type
Issue-Key: swf|ListTagsForResource|2xx->2xx|$.tags[*]
Issue-Key: swf|DescribeWorkflowExecution|2xx->2xx|$.executionInfo.closeStatus
Issue-Key: swf|DescribeWorkflowExecution|2xx->2xx|$.executionInfo.closeTimestamp
Issue-Key: swf|DescribeWorkflowExecution|2xx->2xx|$.executionInfo.executionStatus
Issue-Key: swf|DescribeWorkflowExecution|2xx->2xx|$.openCounts.openActivityTasks
Issue-Key: swf|DescribeWorkflowExecution|2xx->2xx|$.openCounts.openDecisionTasks
Issue-Key: swf|ListClosedWorkflowExecutions|2xx->2xx|$.executionInfos[*].lastClosedEventId
Issue-Key: swf|ListClosedWorkflowExecutions|2xx->2xx|$.executionInfos[*].lastClosedTimestamp
Issue-Key: swf|ListClosedWorkflowExecutions|2xx->2xx|$.executionInfos[*].reopenGenerationId
Issue-Key: swf|ListClosedWorkflowExecutions|2xx->2xx|$.executionInfos[*].reopenable
Issue-Key: swf|ListOpenWorkflowExecutions|2xx->2xx|$.executionInfos[*].lastClosedEventId
Issue-Key: swf|ListOpenWorkflowExecutions|2xx->2xx|$.executionInfos[*].reopenGenerationId
Issue-Key: swf|ListOpenWorkflowExecutions|2xx->2xx|$.executionInfos[*].reopenable
Issue-Key: swf|ListOpenWorkflowExecutions|2xx->2xx|$.executionInfos[*]
Issue-Key: swf|ListTagsForResource|2xx->2xx|$.tags[key="owner"]
Issue-Key: swf|ListTagsForResource|2xx->2xx|$.tags[key="stage"].__type
Issue-Key: swf|ListTagsForResource|2xx->2xx|$.tags[key="team"]
Issue-Key: swf|ListTagsForResource|2xx->2xx|$.tags[key="a"].__type
Issue-Key: swf|ListTagsForResource|2xx->2xx|$.tags[key="b"].__type
Issue-Key: swf|ListTagsForResource|2xx->2xx|$.tags[key="c"].__type
Issue-Key: swf|ListClosedWorkflowExecutions|2xx->2xx|$.executionInfos[*]
Issue-Key: swf|ListClosedWorkflowExecutions|2xx->2xx|$.executionInfos[*].closeStatus
Issue-Key: swf|ListTagsForResource|2xx->2xx|$.tags[key="extra"].__type
Issue-Key: swf|ListTagsForResource|2xx->2xx|$.tags[key="project"].__type
Issue-Commit: dbd8b6da40b3283b718ba9768f41bda1f397bd18

# swf: workflow history, visibility and tag responses omit recorded fields and lose lifecycle state

> _Note: this issue was identified against the code base at commit `dbd8b6da40b3283b718ba9768f41bda1f397bd18`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeWorkflowExecution

## Description
This issue bundles TEN distinct root causes in the Amazon SWF emulator, all grounded in wire-fixture divergences of the `swf` service replayed from simparity-wire-aws-v1. A fixer must close ALL TEN before submitting; each is numbered below and carries its own fixture/step. Causes 1-7 are response-shape omissions (the emulator answers 200 with a body that is missing fields AWS always sends); causes 8-10 are state-tracking defects (the emulator's recorded state for an execution or a resource's tags is wrong).

=== Cause 1 of 10: WorkflowExecutionInfo is serialized without lastClosedEventId, lastClosedTimestamp, reopenGenerationId and reopenable ===
AWS: recorded behavior only. The on-disk Smithy model's com.amazonaws.swf#WorkflowExecutionInfo declares only execution/workflowType/startTimestamp/closeTimestamp/executionStatus/closeStatus/parent/tagList/cancelRequested, so these four members are not modeled; the recordings show the live service nevertheless emits them on EVERY WorkflowExecutionInfo it returns, from DescribeWorkflowExecution ($.executionInfo), ListOpenWorkflowExecutions and ListClosedWorkflowExecutions ($.executionInfos[*]). Their recorded values follow a rule, not a constant: `reopenGenerationId` is 0 and `reopenable` is false for an execution that has never been reopened; `lastClosedEventId` is 0 while the execution is OPEN and otherwise the eventId of the history event that closed the run (3 for a run closed by TerminateWorkflowExecution at event 3, 11 for a run completed at event 11, 7 for a run closed at event 7); `lastClosedTimestamp` is absent while the run is OPEN and is the close time of the run once it is CLOSED.
Emulator: all four members are absent from every WorkflowExecutionInfo it serializes, for open and closed executions alike and in all three operations.
Why: a client that reads executionInfo.lastClosedEventId to find the history event that closed a run, or reopenable/reopenGenerationId to decide whether a run may be reopened, gets null for a field AWS always populates; SDKs that model these as non-optional (or that round-trip the struct) see a truncated record for every execution the emulator reports.
Fix: the shared WorkflowExecutionInfo projection/serializer used by the DescribeWorkflowExecution, ListOpenWorkflowExecutions and ListClosedWorkflowExecutions handlers (services/service-com-amazonaws-swf-simpleworkflowservice/src/ops/) must emit all four members, deriving lastClosedEventId/lastClosedTimestamp from the run's closing history event and defaulting reopenGenerationId=0 / reopenable=false; the execution state needs to retain the closing eventId for that.
Traces: swf-deprecated-domain-blocks-operations#task#5 (DescribeWorkflowExecution, open run: lastClosedEventId/reopenGenerationId/reopenable missing; same cluster covers swf-request-cancel-workflow-execution#task#5, swf-start-describe-signal-history-terminate#task#4, swf-workflow-execution-full-cycle#task#5, swf-workflow-signal-and-cancel#task#7), swf-closed-workflow-count-and-list-after-terminate#task#5 (ListClosedWorkflowExecutions), swf-list-open-workflow-executions-by-tag#task#4 and swf-open-executions-visibility-counts#task#5 (ListOpenWorkflowExecutions), swf-workflow-terminate-and-list#task#6.

=== Cause 2 of 10: cancelRequested stays true on a CLOSED execution ===
AWS: Smithy: WorkflowExecutionInfo.cancelRequested (Canceled, default false) — "Set to true if a cancellation is requested for this workflow execution." Recorded behavior: once the run has closed (the same response reports lastClosedEventId=7 and a lastClosedTimestamp), DescribeWorkflowExecution reports cancelRequested=false — the pending-cancellation flag describes an outstanding request against an open run and is not retained once the run is no longer open.
Emulator: DescribeWorkflowExecution still reports cancelRequested=true for the same execution after the cancel request has been consumed and the run has closed.
Why: a client polling an execution to decide whether a cancellation is still outstanding sees a permanently latched true, so cancellation-tracking loops never observe the request being settled.
Fix: the cancelRequested projection in the DescribeWorkflowExecution handler / the execution state in crate::state must clear (or stop reporting) the cancel-requested latch when the run leaves the OPEN status.
Traces: swf-workflow-signal-and-cancel#task#10 ($.executionInfo.cancelRequested: expected False, got True).

=== Cause 3 of 10: DecisionTaskScheduledEventAttributes omits scheduleToStartTimeout ===
AWS: Smithy: com.amazonaws.swf#DecisionTaskScheduledEventAttributes.scheduleToStartTimeout (DurationInSecondsOptional, @length 0..8) — "The maximum amount of time the decision task can wait to be assigned to a worker." Recorded behavior: every DecisionTaskScheduled event AWS returns carries this member, and for a decision task scheduled without an explicit schedule-to-start bound its value is the string "NONE" (SWF's unlimited-duration sentinel), alongside the taskList and startToCloseTimeout the emulator does emit.
Emulator: the member is absent from every DecisionTaskScheduled event, in PollForDecisionTask's `events` and in GetWorkflowExecutionHistory's `events`.
Why: a decider that reads scheduleToStartTimeout off the scheduled event to compute its own deadlines gets null instead of the service's effective value, and cannot distinguish "unlimited" from "not reported".
Fix: the DecisionTaskScheduled event builder in the decision-task scheduling path (crate state / the StartWorkflowExecution and RespondDecisionTaskCompleted handlers) must record scheduleToStartTimeout, defaulting it to "NONE" when the request and the workflow-type defaults set no bound, and the history serializer must emit it.
Traces: swf-activity-task-canceled-path#task#5 (PollForDecisionTask; cluster also covers swf-activity-task-failure-path#task#5, swf-workflow-decision-and-activity-lifecycle#task#5, swf-workflow-execution-full-cycle#task#7), swf-activity-task-failure-and-cancellation#task#5, swf-request-cancel-workflow-execution#task#6 (GetWorkflowExecutionHistory; cluster also covers swf-start-describe-signal-history-terminate#task#6, swf-workflow-execution-history-pagination#task#6, swf-workflow-signal-and-cancel#task#5), swf-workflow-execution-history-pagination#task#8.

=== Cause 4 of 10: DecisionTaskStartedEventAttributes omits clientToken and startedOnPreviousTaskCompletion ===
AWS: recorded behavior only. The model's com.amazonaws.swf#DecisionTaskStartedEventAttributes declares only identity and scheduledEventId, but the recordings show the live service emits two further members on every DecisionTaskStarted event: `startedOnPreviousTaskCompletion`, whose recorded value is false for a decision task that was not started as a continuation of the previous task's completion, and `clientToken`, an opaque service-generated UUID-shaped token identifying the start (its value differs per start and is not part of the requirement — its presence and shape are).
Emulator: DecisionTaskStarted events carry only eventTimestamp/eventType/eventId/scheduledEventId; both members are absent in PollForDecisionTask's `events` and in GetWorkflowExecutionHistory's `events`.
Why: a decider replaying history to decide whether the task it holds continues a previous decision (SWF's startedOnPreviousTaskCompletion optimisation) has no field to read, and tooling that correlates a started decision task with its service-side client token loses that handle.
Fix: the DecisionTaskStarted event builder behind PollForDecisionTask (services/service-com-amazonaws-swf-simpleworkflowservice/src/ops/poll_for_decision_task.rs) must populate startedOnPreviousTaskCompletion (false unless the task was started on the previous task's completion) and a per-start generated clientToken, and the history event serializer must emit both.
Traces: swf-workflow-signal-and-cancel#task#8, swf-activity-task-canceled-path#task#5 (clientToken + startedOnPreviousTaskCompletion missing on $.events[2]), swf-workflow-execution-full-cycle#task#16 (GetWorkflowExecutionHistory, $.events[2]).

=== Cause 5 of 10: ActivityTaskScheduledEventAttributes omits immediatelyStart ===
AWS: recorded behavior only. The model's com.amazonaws.swf#ActivityTaskScheduledEventAttributes does not declare it, but every recorded ActivityTaskScheduled event carries `immediatelyStart`, whose recorded value is false for an activity task scheduled by an ordinary ScheduleActivityTask decision, next to the activityType/activityId/input/timeouts/taskList/decisionTaskCompletedEventId members the emulator does emit.
Emulator: the member is absent from ActivityTaskScheduled events in GetWorkflowExecutionHistory.
Why: a client replaying activity history sees a field AWS always sends as null and cannot tell whether the task was scheduled for immediate start.
Fix: the ActivityTaskScheduled event builder in the RespondDecisionTaskCompleted handler's ScheduleActivityTask path must record immediatelyStart (false for a normally scheduled task) and the history serializer must emit it.
Traces: swf-workflow-execution-full-cycle#task#16 ($.events[4].activityTaskScheduledEventAttributes.immediatelyStart expected False, absent).

=== Cause 6 of 10: WorkflowExecutionTerminatedEventAttributes omits cause ===
AWS: Smithy: com.amazonaws.swf#WorkflowExecutionTerminatedEventAttributes.cause (WorkflowExecutionTerminatedCause enum) — "If set, indicates that the workflow execution was automatically terminated, and specifies the cause." Recorded behavior: a run terminated through an explicit TerminateWorkflowExecution call records cause=OPERATOR_INITIATED on its WorkflowExecutionTerminated event, alongside reason/details/childPolicy.
Emulator: the WorkflowExecutionTerminated event carries reason/details/childPolicy but no `cause` at all.
Why: a client auditing why a run was terminated cannot distinguish an operator-initiated termination from a parent-policy or timeout-driven one, which is exactly what this member encodes.
Fix: the TerminateWorkflowExecution handler (services/service-com-amazonaws-swf-simpleworkflowservice/src/ops/terminate_workflow_execution.rs) must set cause on the WorkflowExecutionTerminated event it appends — OPERATOR_INITIATED for a direct API call — and the history serializer must emit it.
Traces: swf-terminate-workflow-execution-lifecycle#task#6 ($.events[0].workflowExecutionTerminatedEventAttributes.cause expected 'OPERATOR_INITIATED', absent).

=== Cause 7 of 10: ListTagsForResource tag entries omit the __type discriminator ===
AWS: Smithy: ListTagsForResource -> ListTagsForResourceOutput.tags (ResourceTagList of com.amazonaws.swf#ResourceTag {key, value}); the service uses awsJson, and the recordings show each element of the returned list carries the JSON type discriminator `"__type": "com.amazonaws.swf.base.model#ResourceTag"` in addition to key and value. The discriminator is fixed text identifying the element's shape, identical for every tag in every response.
Emulator: each element is serialized as {key, value} only; `__type` is absent on every tag of every ListTagsForResource response.
Why: clients and proxies that dispatch on the awsJson `__type` discriminator (or that compare responses byte-for-byte against recorded AWS traffic) see an element AWS never emits in that form, so a shape-driven deserializer cannot resolve the member's type.
Fix: the ListTagsForResource response serializer (services/service-com-amazonaws-swf-simpleworkflowservice/src/ops/list_tags_for_resource.rs) must emit the ResourceTag `__type` discriminator on each list element.
Traces: swf-domain-lifecycle-and-tags#task#3 (cluster also covers swf-domain-lifecycle-and-tags#task#7), swf-domain-lifecycle-and-tags#task#5 (with swf-domain-tag-lifecycle#task#3), swf-domain-tag-isolation-across-resources#task#6, swf-domain-tag-isolation-across-resources#task#7 (with #task#9, #task#10), swf-domain-tag-lifecycle#task#5 (with #task#7), swf-tag-untag-full-clear#task#5.

=== Cause 8 of 10: UntagResource does not remove tags — ListTagsForResource keeps returning the untagged keys ===
AWS: Smithy: UntagResource (input resourceArn + tagKeys, output Unit) removes the named tags from the resource; recorded behavior: after untagging keys the subsequent ListTagsForResource returns only the remaining tags, and untagging every key returns an empty tag set.
Emulator: the untagged keys are still present in the next ListTagsForResource response. Untagging all of a domain's tags leaves the full set in place (the response still carries every key/value that was set), and untagging a subset leaves the removed keys present while the surviving ones are returned too.
Why: tag-based cleanup and re-tagging flows are unobservable — a client cannot remove a tag at all, so tag state grows monotonically and any assertion of "tag removed" fails against the emulator.
Fix: the UntagResource handler (services/service-com-amazonaws-swf-simpleworkflowservice/src/ops/untag_resource.rs) must delete the requested keys from the resource's tag map in crate::state so that ListTagsForResource reads the reduced set; today the mutation is not applied to the stored tags.
Traces: swf-tag-untag-full-clear#task#7 (expected no tags, got the three tags a/b/c as `extra`; cluster also covers swf-tag-untag-full-clear#task#9), swf-domain-tag-lifecycle#task#9 (keys team and owner expected gone, returned as `extra`).

=== Cause 9 of 10: an execution whose activity task closed and whose decider completed it stays OPEN forever, with openActivityTasks stuck at 1 ===
AWS: Smithy: DescribeWorkflowExecution -> executionInfo.executionStatus/closeStatus and openCounts (WorkflowExecutionOpenCounts.openActivityTasks, "The count of activity tasks whose status is OPEN"). Recorded behavior: after the activity task reaches a closed state (completed, failed or canceled) and the decider answers the following decision task with CompleteWorkflowExecution, DescribeWorkflowExecution reports executionStatus=CLOSED with closeStatus=COMPLETED, a closeTimestamp, openCounts.openActivityTasks=0, and the execution then appears in ListClosedWorkflowExecutions with that closeStatus.
Emulator: the same execution is still reported executionStatus=OPEN with openCounts.openActivityTasks=1 (no closeStatus, no closeTimestamp), and it never shows up in ListClosedWorkflowExecutions — the corpus polled that list 59 times waiting for closeStatus=COMPLETED and the execution never appeared.
Why: the whole activity lifecycle is unobservable end-to-end: a client cannot see a run finish, cannot read its close status, and queue-depth/open-task counters never drain, so any orchestration that waits for completion hangs against the emulator.
Fix: the activity-task close handlers (respond_activity_task_completed.rs / _failed.rs / _canceled.rs) must decrement the run's open-activity-task count in crate::state, and the RespondDecisionTaskCompleted handler's CompleteWorkflowExecution decision path must transition the run to CLOSED/COMPLETED (close timestamp, closing eventId) so the visibility handlers see it.
Traces: swf-activity-task-canceled-path#task#11 (executionStatus expected CLOSED got OPEN, openActivityTasks expected 0 got 1; cluster also covers swf-activity-task-failure-path#task#11 and swf-workflow-execution-full-cycle#task#15), swf-workflow-execution-full-cycle#task#17 (ListClosedWorkflowExecutions poll never observes the run).

=== Cause 10 of 10: after TerminateWorkflowExecution the run's outstanding decision task is no longer counted in openCounts.openDecisionTasks ===
AWS: Smithy: WorkflowExecutionOpenCounts.openDecisionTasks (range 0..1) — "The count of decision tasks whose status is OPEN. A workflow execution can have at most one open decision task." Recorded behavior: a run that was started (which schedules its first decision task) and then terminated without that decision task ever being polled or answered still reports openCounts.openDecisionTasks=1 on DescribeWorkflowExecution, in the same response that reports the run closed.
Emulator: the same DescribeWorkflowExecution reports openDecisionTasks=0 — the pending decision task is dropped from the counters when the run is terminated (the emulator reports this counter correctly for a still-open run, so the divergence is specific to the terminated run).
Why: a client reconciling outstanding work against a terminated execution (or asserting that SWF's at-most-one-open-decision-task invariant held at termination) reads zero where AWS reports the task that was never delivered.
Fix: the TerminateWorkflowExecution handler must leave the run's scheduled-but-unanswered decision task in the state the open-counts projection of describe_workflow_execution.rs reads, rather than clearing it when the run closes.
Traces: swf-terminate-workflow-execution#task#5 (openCounts.openDecisionTasks expected 1 got 0; cluster also covers swf-terminate-workflow-execution-lifecycle#task#5 and swf-workflow-terminate-and-list#task#5).

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout: 409 steps, 303 passed, 90 mismatched, 4 errored, 12 blocked, score 74.1%, emulator_up=true, bootstrap_signal=false. All 35 divergence clusters belong to service `swf`; none carries a stub_path or an owned_by, so none is already filed. The ten causes above are the clusters' distinct root causes.

Cause 1 — swf-deprecated-domain-blocks-operations#task#5, DescribeWorkflowExecution, HTTP 200 vs 200:
  $.executionInfo.lastClosedEventId: missing (expected 0, got None)
  $.executionInfo.reopenGenerationId: missing (expected 0, got None)
  $.executionInfo.reopenable: missing (expected False, got None)
  Same omission on ListClosedWorkflowExecutions (swf-closed-workflow-count-and-list-after-terminate#task#5: $.executionInfos[0].lastClosedEventId expected 3, lastClosedTimestamp expected, reopenGenerationId 0, reopenable False — all missing) and on ListOpenWorkflowExecutions (swf-list-open-workflow-executions-by-tag#task#4, swf-open-executions-visibility-counts#task#5/#task#7, where the whole executionInfos element mismatches only on those members).

Cause 2 — swf-workflow-signal-and-cancel#task#10, DescribeWorkflowExecution, 200 vs 200:
  $.executionInfo.cancelRequested: value_mismatch (expected False, got True)
  (reported together with the Cause-1 omissions lastClosedEventId=7 / lastClosedTimestamp, i.e. AWS reports the run closed and cancelRequested false.)

Cause 3 — swf-activity-task-canceled-path#task#5, PollForDecisionTask, 200 vs 200:
  $.events[1].decisionTaskScheduledEventAttributes.scheduleToStartTimeout: missing (expected 'NONE', got None)
  Same on GetWorkflowExecutionHistory: swf-request-cancel-workflow-execution#task#6 reports $.events[1] mismatching only by the absent scheduleToStartTimeout ('NONE') while taskList and startToCloseTimeout match.

Cause 4 — swf-workflow-signal-and-cancel#task#8, PollForDecisionTask, 200 vs 200:
  $.events[4].decisionTaskStartedEventAttributes.clientToken: missing (expected a UUID-shaped token, got None)
  $.events[4].decisionTaskStartedEventAttributes.startedOnPreviousTaskCompletion: missing (expected False, got None)
  Same two members missing on swf-activity-task-canceled-path#task#5 ($.events[2]) and on GetWorkflowExecutionHistory in swf-workflow-execution-full-cycle#task#16 ($.events[2]).

Cause 5 — swf-workflow-execution-full-cycle#task#16, GetWorkflowExecutionHistory, 200 vs 200: $.events[4] expected activityTaskScheduledEventAttributes containing 'immediatelyStart': False; the emulator's event carries every other member but not immediatelyStart.

Cause 6 — swf-terminate-workflow-execution-lifecycle#task#6, GetWorkflowExecutionHistory, 200 vs 200: $.events[0] expected workflowExecutionTerminatedEventAttributes {'cause': 'OPERATOR_INITIATED', 'childPolicy': 'TERMINATE', 'details': ..., 'reason': ...}; got the same object without 'cause'.

Cause 7 — swf-domain-lifecycle-and-tags#task#3, ListTagsForResource, 200 vs 200:
  $.tags[key="env"].__type: missing (expected 'com.amazonaws.swf.base.model#ResourceTag', got None)
  Reported identically for every tag key in swf-domain-lifecycle-and-tags#task#5/#task#7, swf-domain-tag-lifecycle#task#3/#task#5/#task#7/#task#9, swf-domain-tag-isolation-across-resources#task#6/#task#7/#task#9/#task#10 and swf-tag-untag-full-clear#task#5.

Cause 8 — swf-tag-untag-full-clear#task#7, ListTagsForResource, 200 vs 200 (after UntagResource removed every key):
  $.tags[0]: extra (expected None, got {'key': 'a', 'value': '1'})
  $.tags[1]: extra (expected None, got {'key': 'b', 'value': '2'})
  $.tags[2]: extra (expected None, got {'key': 'c', 'value': '3'})
  Partial untag, swf-domain-tag-lifecycle#task#9: $.tags[key="team"]: extra (got {'key': 'team', 'value': 'workflow'}), $.tags[key="owner"]: extra (got {'key': 'owner', 'value': 'probe-suite'}).

Cause 9 — swf-activity-task-canceled-path#task#11, DescribeWorkflowExecution, 200 vs 200:
  $.executionInfo.executionStatus: value_mismatch (expected 'CLOSED', got 'OPEN')
  $.executionInfo.closeStatus: missing (expected 'COMPLETED', got None)
  $.executionInfo.closeTimestamp: missing (expected a close time, got None)
  $.openCounts.openActivityTasks: value_mismatch (expected 0, got 1)
  swf-workflow-execution-full-cycle#task#17, ListClosedWorkflowExecutions: $.executionInfos[0]: missing (expected the CLOSED/COMPLETED execution, got None); $.executionInfos[0].closeStatus: poll: not in ['COMPLETED'] after 59 attempt(s).

Cause 10 — swf-terminate-workflow-execution#task#5, DescribeWorkflowExecution, 200 vs 200:
  $.openCounts.openDecisionTasks: value_mismatch (expected 1, got 0)
  (same response also shows the Cause-1 omissions lastClosedEventId=3 / lastClosedTimestamp.)

## Steps to Reproduce
Cause 1 (WorkflowExecutionInfo omissions):
1. RegisterDomain; RegisterWorkflowType; StartWorkflowExecution.
2. DescribeWorkflowExecution -> AWS: executionInfo carries lastClosedEventId=0, reopenGenerationId=0, reopenable=false; emulator omits all three.
3. TerminateWorkflowExecution, then ListClosedWorkflowExecutions -> AWS: executionInfos[0] also carries lastClosedEventId=<closing eventId> and lastClosedTimestamp; emulator omits them.
4. ListOpenWorkflowExecutions on another open run -> same three members missing.
Replay: swf-deprecated-domain-blocks-operations#task#5, swf-closed-workflow-count-and-list-after-terminate#task#5, swf-list-open-workflow-executions-by-tag#task#4.

Cause 2 (cancelRequested on a closed run):
1. Start an execution, SignalWorkflowExecution, RequestCancelWorkflowExecution, then let/make the run close.
2. DescribeWorkflowExecution -> AWS: cancelRequested=false on the closed run; emulator: true.
Replay: swf-workflow-signal-and-cancel#task#10.

Cause 3 (DecisionTaskScheduled.scheduleToStartTimeout):
1. Start an execution on task list T with no schedule-to-start bound.
2. PollForDecisionTask(domain, T) and GetWorkflowExecutionHistory -> the DecisionTaskScheduled event must carry scheduleToStartTimeout="NONE"; the emulator's event has no such member.
Replay: swf-activity-task-canceled-path#task#5, swf-request-cancel-workflow-execution#task#6.

Cause 4 (DecisionTaskStarted.clientToken / startedOnPreviousTaskCompletion):
1. Start an execution; PollForDecisionTask.
2. Inspect the returned `events` (and GetWorkflowExecutionHistory) -> the DecisionTaskStarted event must carry startedOnPreviousTaskCompletion=false and a service-generated clientToken; the emulator emits neither.
Replay: swf-workflow-signal-and-cancel#task#8, swf-workflow-execution-full-cycle#task#16.

Cause 5 (ActivityTaskScheduled.immediatelyStart):
1. Start an execution; answer its decision task with a ScheduleActivityTask decision.
2. GetWorkflowExecutionHistory -> the ActivityTaskScheduled event must carry immediatelyStart=false; the emulator omits it.
Replay: swf-workflow-execution-full-cycle#task#16.

Cause 6 (WorkflowExecutionTerminated.cause):
1. Start an execution; TerminateWorkflowExecution(reason, details, childPolicy=TERMINATE).
2. GetWorkflowExecutionHistory -> the WorkflowExecutionTerminated event must carry cause=OPERATOR_INITIATED; the emulator omits it.
Replay: swf-terminate-workflow-execution-lifecycle#task#6.

Cause 7 (ListTagsForResource __type):
1. RegisterDomain with tags; DescribeDomain -> arn; ListTagsForResource(arn).
2. Each element must carry "__type": "com.amazonaws.swf.base.model#ResourceTag" beside key/value; the emulator emits key/value only.
Replay: swf-domain-lifecycle-and-tags#task#3, swf-tag-untag-full-clear#task#5.

Cause 8 (UntagResource is a no-op):
1. RegisterDomain; TagResource(arn, tags a=1,b=2,c=3); ListTagsForResource -> three tags.
2. UntagResource(arn, tagKeys=[a,b,c]); ListTagsForResource -> AWS: empty; emulator: still a,b,c.
3. Partial case: tags env/stage/team/owner, UntagResource([team,owner]) -> AWS: env,stage; emulator: all four.
Replay: swf-tag-untag-full-clear#task#7, swf-domain-tag-lifecycle#task#9.

Cause 9 (run never closes, openActivityTasks stuck):
1. RegisterDomain; RegisterWorkflowType; RegisterActivityType; StartWorkflowExecution.
2. PollForDecisionTask; RespondDecisionTaskCompleted with ScheduleActivityTask.
3. PollForActivityTask; close the activity task (RespondActivityTaskCanceled / _Failed / _Completed).
4. PollForDecisionTask; RespondDecisionTaskCompleted with CompleteWorkflowExecution.
5. DescribeWorkflowExecution -> AWS: CLOSED/COMPLETED, closeTimestamp set, openCounts.openActivityTasks=0; emulator: OPEN, no closeStatus, openActivityTasks=1.
6. ListClosedWorkflowExecutions -> AWS lists the run with closeStatus=COMPLETED; the emulator never does.
Replay: swf-activity-task-canceled-path#task#11, swf-workflow-execution-full-cycle#task#17.

Cause 10 (openDecisionTasks after terminate):
1. RegisterDomain; RegisterWorkflowType; StartWorkflowExecution (its first decision task is scheduled and never polled).
2. TerminateWorkflowExecution.
3. DescribeWorkflowExecution -> AWS: openCounts.openDecisionTasks=1; emulator: 0.
Replay: swf-terminate-workflow-execution#task#5.
