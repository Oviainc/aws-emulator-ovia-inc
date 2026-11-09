Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: swf.poll-decision-task-unknown-domain-rejected
Issue-Shape-Id: com.amazonaws.swf#SimpleWorkflowService
Issue-Key: swf|PollForDecisionTask|UnknownResourceFault
Issue-Commit: b3c3b361898dc8345b8f70886f10e9172e43f421

# swf PollForDecisionTask raises UnknownResourceFault for a never-registered domain instead of returning an empty long-poll response

> _Note: this issue was identified against the code base at commit `b3c3b361898dc8345b8f70886f10e9172e43f421`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PollForDecisionTask

## Description
One net-new root cause.

Cause 1 of 1: PollForDecisionTask against a domain name that was never registered is refused with an error instead of completing the long poll empty. Failing check id: swf.poll-decision-task-unknown-domain-rejected.

AWS: recorded behavior only. Smithy (com-amazonaws-swf-simpleworkflowservice, PollForDecisionTask) does model UnknownResourceFault (client error, HTTP 400) on this operation, but the recording this check was captured from shows that a poll naming a domain that was never registered is NOT refused: the call behaves as an ordinary server-side long poll and returns HTTP 200 with a DecisionTask body carrying no taskToken (the empty/timed-out poll shape), exactly as a poll on a real domain whose task list has nothing assignable does. The check's own statement of the recorded contract is: "PollForDecisionTask on a never-registered domain returns an empty long-poll response (no taskToken), not UnknownResourceFault, while a poll on a registered domain with a scheduled task returns its taskToken." The positive half of the check — polling the registered shared domain and task list of a freshly started execution — does return a taskToken against the emulator, so only the unknown-domain half diverges.

Emulator: the same call is refused with HTTP 400 and error code UnknownResourceFault, message "Unknown domain: <domainName>". In the replay, botocore raised botocore.errorfactory.UnknownResourceFault: An error occurred (UnknownResourceFault) when calling the PollForDecisionTask operation: Unknown domain: sim2real-behav-nodomain-0a79ead5ce06. In that message "Unknown domain: " is the fixed text and the trailing token is the `domain` value copied from the request (here a per-run generated name — the generated name itself is not part of the requirement); the whole response is the divergence, not its wording.

Why: a client that long-polls for decision tasks treats an empty response as "nothing to do, poll again" and an UnknownResourceFault as a fatal configuration error. A decider loop pointed at a domain the emulator does not know (a typo, a not-yet-created domain, or a domain created by another component that has not landed yet) crashes or aborts its poll loop against the emulator where against AWS it would simply keep polling and pick up work once the domain/tasks exist. Conversely, test suites that assert "unknown domain poll returns empty" see an exception, and code paths that distinguish a genuinely bad domain from an idle task list cannot be exercised at all.

Fix: the PollForDecisionTask handler in the swf service crate — specifically its domain-existence precondition. The unknown-domain check must not short-circuit this operation into UnknownResourceFault; the poll should fall through to the normal long-poll path and serialize the empty DecisionTask response (no taskToken, 200) when no task is assignable, including when the domain does not exist. Other SWF operations that legitimately raise UnknownResourceFault for a missing domain (DescribeDomain, StartWorkflowExecution, TerminateWorkflowExecution — all relied on elsewhere in this same check and passing) must keep doing so, so the precondition should be relaxed only for this long-poll operation.

Traces: swf.poll-decision-task-unknown-domain-rejected (error verdict; failure raised on the bogus-domain poll after the positive-control poll succeeded).

## Evidence
Expected (recorded AWS): HTTP 200, DecisionTask body with no taskToken, e.g. the empty long-poll response; assertion `not bogus_poll.get("taskToken")` holds.

Actual (emulator): HTTP 400, Error.Code=UnknownResourceFault, message "Unknown domain: <domain from request>".
Replay output:
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
  botocore.errorfactory.UnknownResourceFault: An error occurred (UnknownResourceFault) when calling the PollForDecisionTask operation: Unknown domain: sim2real-behav-nodomain-0a79ead5ce06

The positive control in the same check (PollForDecisionTask on the registered shared domain `sim2real-swf-shared` with a freshly started execution) returned a taskToken from the emulator, so the divergence is isolated to the never-registered-domain poll.

## Steps to Reproduce
1. Boot the emulator and point an SWF client at it.
2. Ensure a registered domain exists (RegisterDomain name=sim2real-swf-shared, workflowExecutionRetentionPeriodInDays=1) and register a workflow type with a defaultTaskList, then StartWorkflowExecution so a decision task is scheduled.
3. PollForDecisionTask(domain=<registered domain>, taskList={name: <that task list>}) -> emulator correctly returns a taskToken.
4. PollForDecisionTask(domain=<a name never registered>, taskList={name: <same task list>}) with a read timeout longer than the long-poll window (e.g. botocore Config(read_timeout=75, retries={'max_attempts': 1})).
5. Observe UnknownResourceFault (HTTP 400, "Unknown domain: <name>") instead of a 200 response with no taskToken.

Replay: run the behavioral suite check swf.poll-decision-task-unknown-domain-rejected.
