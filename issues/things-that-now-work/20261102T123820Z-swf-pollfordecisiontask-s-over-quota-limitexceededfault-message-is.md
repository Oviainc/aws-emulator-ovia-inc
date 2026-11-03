Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: swf.poll-for-decision-task-limit-exceeded-error
Issue-Shape-Id: com.amazonaws.swf#SimpleWorkflowService
Issue-Key: swf.poll-for-decision-task-limit-exceeded-error|run#29|AssertionError|assert limit_seen and all(
Issue-Commit: 42974e275418b1ef9bb74f16d34e8312c05bbaa6

# swf PollForDecisionTask's over-quota LimitExceededFault message is emulator wording, not AWS's "Limit on maximum concurrent poll requests per task list reached"

> _Note: this issue was identified against the code base at commit `42974e275418b1ef9bb74f16d34e8312c05bbaa6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PollForDecisionTask

## Description
One net-new root cause found replaying the behavioral suite against the SWF emulator (service token `swf`, bootstrapped crate services/service-com-amazonaws-swf-simpleworkflowservice). It must be closed before this issue is submitted.

Context: cause 7 of the resolved issue 20261022T233359Z-swf-executionstarttoclosetimeout-never-fires-server-side-undeprecate-tagging.md reported that the per-task-list poller cap was absent entirely. That part is now fixed — the cap IS enforced and the refusal IS the modelled LimitExceededFault, which is the correct response class for this request, so this is not a reopened refusal. What remains is the error MESSAGE, which that issue did not state and which still diverges.

=== Cause 1 of 1: the LimitExceededFault message for an over-quota PollForDecisionTask is the emulator's own phrasing ===
AWS: Smithy: PollForDecisionTask models LimitExceededFault (errorKind client, HTTP 400), documented "Returned by any operation if a system imposed limitation has been reached. To address this fault you should either clean up unused resources or increase the limit by contacting AWS." The model does not fix the message text, so for the message this is recorded behavior only: when more simultaneous pollers are opened against a single task list than the Amazon SWF Developer Guide's "Quotas on task executions" cap of 1,000, the excess calls are refused with HTTP 400, error code `LimitExceededFault`, and the message `Limit on maximum concurrent poll requests per task list reached`. That message is entirely FIXED text for this error class — it has no request-derived or state-derived variable part: no task-list name, no domain, no numeric limit is interpolated into it. The code must be LimitExceededFault and never OperationNotPermittedFault (which the model reserves for permission failures).
Emulator: status and error code are correct (400 / `LimitExceededFault`), but the message is the emulator's own sentence with the configured cap interpolated into it: `a task list admits at most 1000 simultaneous pollers` (the `1000` is `crate::state::MAX_POLLERS_PER_TASK_LIST` formatted into the string — AWS's message contains no such number at all).
Why: a client that branches on, logs, or asserts over-quota poller rejections by the fault's message — the only text SWF gives for this condition, since the fault code is the generic "any operation" limit fault shared with unrelated quotas — cannot recognise the emulator's rejection as the concurrent-poller cap. Decider fleets and test suites that match AWS's documented wording pass against AWS and fail against the emulator, and a reader of the emulator's message is told a limit value AWS never reports, which can be mistaken for an account-specific quota readout.
Fix: the LimitExceededFault construction in the PollForDecisionTask handler's poller-admission guard in the SWF crate — emit the fixed AWS text for this error class instead of formatting the internal cap constant into a bespoke sentence. Leave the status (400), the error code (LimitExceededFault, not OperationNotPermittedFault) and the admission accounting itself unchanged; only the message is wrong.
Traces: swf.poll-for-decision-task-limit-exceeded-error.

## Evidence
Behavioral suite, 50/65 passed. Evidence below is the emulator's replayed response.

swf.poll-for-decision-task-limit-exceeded-error — [mismatch]
AssertionError: [{'Message': 'a task list admits at most 1000 simultaneous pollers', 'Code': 'LimitExceededFault'}]
raised by the check's assertion
    assert limit_seen and all(
        err.get("Message") == "Limit on maximum concurrent poll requests per task list reached"
        for err in limit_seen
    ), limit_seen

Expected (AWS): HTTP 400, Error.Code = `LimitExceededFault`, Error.Message = `Limit on maximum concurrent poll requests per task list reached` (fixed text, no variable parts).
Actual (emulator): HTTP 400, Error.Code = `LimitExceededFault` (correct), Error.Message = `a task list admits at most 1000 simultaneous pollers`.

The check's earlier assertions passed: at least one of the 1,001 concurrent pollers was rejected with LimitExceededFault, and OperationNotPermittedFault was never seen. The sole divergence is the message text.

Source of the diverging text: services/service-com-amazonaws-swf-simpleworkflowservice/src/ops/poll_for_decision_task.rs lines 284-295, where the poller-admission slot failure builds `LimitExceededFault { message: Some(format!("a task list admits at most {} simultaneous pollers", crate::state::MAX_POLLERS_PER_TASK_LIST)) }`.

## Steps to Reproduce
Boot the emulator and replay swf.poll-for-decision-task-limit-exceeded-error.

1. RegisterDomain(name=D, workflowExecutionRetentionPeriodInDays="1").
2. Pick one task-list name TL (never registered state is fine; the cap is per (domain, task list)).
3. From a single client whose connection pool can hold them all, open 1,001 concurrent PollForDecisionTask(domain=D, taskList={name: TL}, identity=poller-<i>) long polls simultaneously (one more than the documented 1,000-simultaneous-poller cap), releasing them together from a barrier.
4. Collect the Error dicts of every call rejected with Code == "LimitExceededFault".

Observed: the rejection carries Message "a task list admits at most 1000 simultaneous pollers".
Expected: Message "Limit on maximum concurrent poll requests per task list reached" — identical fixed text on every such rejection, with no task list name, domain or limit number interpolated. Code must remain LimitExceededFault at HTTP 400, never OperationNotPermittedFault.
