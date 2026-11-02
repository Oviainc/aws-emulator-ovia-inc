Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: stepfunctions.jsonata-output-and-states-input-transform stepfunctions.choice-jsonata-condition-reads-assigned-variables stepfunctions.choice-jsonata-assign-is-scoped-to-the-selected-rule stepfunctions.deep-sfn-2-sfn-jsonata-catch-output-from-states-error-output stepfunctions.deep-sfn-2-sfn-jsonata-map-items-expression-and-item-context stepfunctions.deep-sfn-2-sfn-jsonata-mixed-query-language-state-override stepfunctions.intrinsic-function-library-evaluates-exactly stepfunctions.deep-sfn-language-hash-unsupported-algorithm-rejected stepfunctions.deep-sfn-language-choice-nested-and-or-not-path-comparators stepfunctions.execution-output-numeric-precision-preserved stepfunctions.sfn-retry-with-backoff stepfunctions.sfn-task-timeout stepfunctions.deep-sfn-1-sfn-sync-child-fail-surfaces-as-task-failed-in-parent stepfunctions.stop-execution-aborted-event-details-with-and-without-cause stepfunctions.stop-execution-oversized-error-rejected-execution-keeps-running stepfunctions.reject-update-state-machine-on-version-arn stepfunctions.list-executions-missing-required-filter-rejected
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: stepfunctions.jsonata-output-and-states-input-transform|run#5|AssertionError|assert big["status"] == "SUCCEEDED", (
Issue-Key: stepfunctions.choice-jsonata-condition-reads-assigned-variables|run#6|AssertionError|assert desc["status"] == "SUCCEEDED", (
Issue-Key: stepfunctions.choice-jsonata-assign-is-scoped-to-the-selected-rule|run#5|AssertionError|assert desc["status"] == "SUCCEEDED", (
Issue-Key: stepfunctions.deep-sfn-2-sfn-jsonata-catch-output-from-states-error-output|run#8|AssertionError|assert exe["status"] == "SUCCEEDED", (
Issue-Key: stepfunctions.deep-sfn-2-sfn-jsonata-map-items-expression-and-item-context|run#6|AssertionError|assert exe["status"] == "SUCCEEDED", (
Issue-Key: stepfunctions.deep-sfn-2-sfn-jsonata-mixed-query-language-state-override|run#5|AssertionError|assert exe["status"] == "SUCCEEDED", (
Issue-Key: stepfunctions.intrinsic-function-library-evaluates-exactly|run#4|AssertionError|assert desc["status"] == "SUCCEEDED", (
Issue-Key: stepfunctions.deep-sfn-language-hash-unsupported-algorithm-rejected|run#4|AssertionError|assert good["status"] == "SUCCEEDED", (
Issue-Key: stepfunctions.deep-sfn-language-choice-nested-and-or-not-path-comparators|run#5|AssertionError|assert desc["status"] == "SUCCEEDED", (
Issue-Key: stepfunctions.execution-output-numeric-precision-preserved|run#14|AssertionError|assert output["bigInt"] == big_int, f"{output['bigInt']} != {big_int}"
Issue-Key: stepfunctions.sfn-retry-with-backoff|run#7|AssertionError|assert desc["status"] == "SUCCEEDED", (
Issue-Key: stepfunctions.sfn-task-timeout|run#8|AssertionError|assert desc.get("error") == "States.Timeout", (
Issue-Key: stepfunctions.deep-sfn-1-sfn-sync-child-fail-surfaces-as-task-failed-in-parent|run#19|JSONDecodeError|inner = json.loads(d["cause"])
Issue-Key: stepfunctions.stop-execution-aborted-event-details-with-and-without-cause|_wait_entered#6|Timeout|time.sleep(1)
Issue-Key: stepfunctions.stop-execution-oversized-error-rejected-execution-keeps-running|_wait_entered#5|Timeout|time.sleep(1)
Issue-Key: stepfunctions.reject-update-state-machine-on-version-arn|run#18|AssertionError|assert code == "StateMachineDoesNotExist", (
Issue-Key: stepfunctions.list-executions-missing-required-filter-rejected|run#11|AssertionError|assert message == "Must provide a StateMachine ARN or MapRun ARN", f"unexpected ValidationException message {message!r}"
Issue-Commit: dfc2bec1feea4e1eda9981ef982ae4c91760421c

# sfn interpreter and data fidelity: nine net-new causes — JSONata never evaluated, States.Hash missing, Choice path comparators error on type mismatch, large integers lose precision, lambda:invoke never dispatched, StartSyncExecution returns a non-JSON body, StopExecution contracts never converge, and two control-plane classifications diverge (9 causes)

> _Note: this issue was identified against the code base at commit `dfc2bec1feea4e1eda9981ef982ae4c91760421c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
Nine net-new root causes in the Step Functions interpreter, data representation and two control-plane classifications.

Cause 1 of 9: JSONata definitions are never evaluated at run time. Any state machine with QueryLanguage=JSONata (whole-machine or per-state override) fails immediately with `States.Runtime` and cause "the definition is written in 'JSONata', and this emulator evaluates only 'JSONPath' definitions, so nothing was run for it"; a per-state JSONata override instead fails with "state '<name>' carries the member 'Output', which this emulator does not interpret".
  AWS: recorded behavior only for the evaluator (ASL query language is not in the Smithy model). AWS evaluates JSONata in Arguments/Output/Assign/Condition/Items, exposes `$states.input`, `$states.result`, `$states.errorOutput` and assigned variables, scopes a Choice rule's Assign to the selected rule, and recovers through Catch using `$states.errorOutput`.
  Emulator: nothing is evaluated; every JSONata execution fails before the first state.
  Why: the entire JSONata query language — the current default for new ASL authoring — is unusable, and the failure names an emulator limitation rather than any AWS error.
  Fix: the ASL evaluator in the sfn service crate (JSONata expression evaluation and the `$states` context). The open issue 20261028T095020Z-sfn-step-functions-jsonata-definitions-are-never-evaluated-at.md records the same gap but does not carry these checks.
  Checks: .jsonata-output-and-states-input-transform, .choice-jsonata-condition-reads-assigned-variables, .choice-jsonata-assign-is-scoped-to-the-selected-rule, .deep-sfn-2-sfn-jsonata-catch-output-from-states-error-output, .deep-sfn-2-sfn-jsonata-map-items-expression-and-item-context, .deep-sfn-2-sfn-jsonata-mixed-query-language-state-override.

Cause 2 of 9: the `States.Hash` intrinsic is not evaluated — any state using it fails with `States.Runtime` cause "'States.Hash($.s, 'MD5')': 'States.Hash' is an intrinsic function this emulator does not evaluate, so nothing was run for it".
  AWS: recorded behavior only. AWS evaluates States.Hash(data, algorithm) for MD5, SHA-1, SHA-256, SHA-384 and SHA-512, returning the lowercase hex digest, and refuses an unsupported algorithm with `States.IntrinsicFailure` naming the algorithm from the request.
  Why: hashing-dependent workflows cannot run, and the unsupported-algorithm negative is unreachable because the supported control already fails.
  Fix: the intrinsic-function library in the sfn service crate. The open issue 20261024T043150Z-sfn-step-functions-intrinsic-functions-diverge-wrong-error-name.md records this family but does not carry these checks.
  Checks: .intrinsic-function-library-evaluates-exactly, .deep-sfn-language-hash-unsupported-algorithm-rejected.

Cause 3 of 9: a Choice `*Path` comparator whose two operands have different JSON types fails the execution instead of evaluating to false.
  AWS: recorded behavior only. In AWS, a comparator whose operands are type-incompatible simply does not match, so the enclosing And/Or/Not evaluates normally and the Default (or the next rule) is taken; the execution succeeds.
  Emulator: `status='FAILED'`, `error='States.Runtime'`, `cause="the choice rule on '$.n' compares 3 with \"zeta\" through 'NumericEqualsPath', which compares numbers"`.
  Why: a Choice whose arms deliberately test for absence/mismatch — the normal way to write a defensive router — aborts the execution instead of routing.
  Fix: Choice-rule comparator evaluation in the sfn service crate (a type mismatch is a non-match, not a runtime error).
  Check: .deep-sfn-language-choice-nested-and-or-not-path-comparators.

Cause 4 of 9: integers in execution output lose precision — they are round-tripped through a double.
  AWS: recorded behavior only for the JSON representation. AWS preserves the JSON number as written, so a 30-digit integer in a Pass Result comes back from DescribeExecution byte-identical.
  Emulator: `1.2345678901234568e+29 != 123456789012345678901234567890` — the value is reserialized in exponential float form.
  Why: any workflow carrying ids, amounts or counters larger than 2^53 silently corrupts them, which no client can detect from the response.
  Fix: the JSON number representation used by the sfn service crate's execution state and its DescribeExecution/GetExecutionHistory serializer (preserve arbitrary-precision numbers verbatim rather than parsing to f64).
  Check: .execution-output-numeric-precision-preserved.

Cause 5 of 9: the optimized `arn:aws:states:::lambda:invoke` integration is never dispatched, so Retry/backoff and TimeoutSeconds contracts cannot be observed.
  AWS: recorded behavior only. AWS invokes the function under the execution role; repeated handler failures drive the Retry policy's backoff and then the Catch, and a Task exceeding TimeoutSeconds fails with `States.Timeout`.
  Emulator: `error='States.Runtime'`, cause "state '<name>': 'arn:aws:states:::lambda:invoke' names an integration with 'lambda', which is not one of the services this emulator makes a 'Task' state's integration call to, so nothing was run for it".
  Fix: Task-state integration dispatch in the sfn service crate (see open issue 20261027T173205Z-sfn-step-functions-never-dispatches-the-optimized-lambda-invoke.md, which does not carry these checks).
  Checks: .sfn-retry-with-backoff, .sfn-task-timeout.

Cause 6 of 9: a synchronous Step Functions call returns a body that is not JSON, so the SDK cannot decode the response at all.
  AWS: Smithy models StartSyncExecution (host prefix `sync-`) with a JSON output document carrying executionArn, stateMachineArn, name, startDate, stopDate, status, input/output (or error/cause), billingDetails. Every response is a well-formed JSON document.
  Emulator: the response body is empty or non-JSON — the client raises `json.decoder.JSONDecodeError: Expecting value: line 1 column 1 (char 0)` before any field can be read.
  Why: a client cannot get any answer at all from the synchronous path — not even an error it could classify.
  Fix: the response serializer of the sync execution handler in the sfn service crate (it must always emit the modelled JSON document, including on the failure path).
  Check: .deep-sfn-1-sfn-sync-child-fail-surfaces-as-task-failed-in-parent.

Cause 7 of 9: StopExecution's observable contracts never converge — neither the ABORTED history event's details (with and without `cause`) nor the oversized-`error` refusal is ever reached; both checks run to the 60s cap.
  AWS: Smithy StopExecution input `error` (@length max 256) and `cause` (@length max 32768); a value over the limit is refused with ValidationException and the execution keeps running, while a successful stop writes an `ExecutionAborted` history event whose executionAbortedEventDetails carry the submitted error and cause (and omit cause when it was not given).
  Emulator: neither arm produces an observable result within the budget.
  Fix: the StopExecution handler and the execution-history writer in the sfn service crate (emit ExecutionAborted with the submitted details, and enforce the modelled @length on `error`/`cause` before mutating the execution).
  Checks: .stop-execution-aborted-event-details-with-and-without-cause, .stop-execution-oversized-error-rejected-execution-keeps-running.

Cause 8 of 9: UpdateStateMachine against a published VERSION ARN is refused with the wrong error class.
  AWS: Smithy UpdateStateMachine models StateMachineDoesNotExist (400); AWS answers that code with the message `State Machine Does Not Exist: '<version arn>'` — fixed text plus the submitted ARN.
  Emulator: `ValidationException`.
  Why: a client distinguishing "you aimed at an immutable version" from "your parameters were wrong" cannot, and retry logic keyed on StateMachineDoesNotExist never fires.
  Fix: the ARN-kind check in the UpdateStateMachine handler of the sfn service crate.
  Check: .reject-update-state-machine-on-version-arn.

Cause 9 of 9: ListExecutions with neither filter uses the wrong message.
  AWS: Smithy ListExecutions has both `stateMachineArn` and `mapRunArn` optional, and ValidationException (400) is modelled; AWS's message is the fixed text `Must provide a StateMachine ARN or MapRun ARN`.
  Emulator: `one of stateMachineArn or mapRunArn must be supplied`.
  Fix: the required-filter validation in the ListExecutions handler of the sfn service crate.
  Check: .list-executions-missing-required-filter-rejected.

## Evidence
Cause 1: `AssertionError: JSONata state machine did not succeed: FAILED error=States.Runtime cause=the definition is written in 'JSONata', and this emulator evaluates only 'JSONPath' definitions, so nothing was run for it` (and, for the per-state override, cause="state 'Nata' carries the member 'Output', which this emulator does not interpret, so nothing was run for it").
Cause 2: `AssertionError: intrinsic evaluation must succeed, got FAILED error=States.Runtime cause='States.Hash($.a, 'SHA-256')': 'States.Hash' is an intrinsic function this emulator does not evaluate, so nothing was run for it`.
Cause 3: `AssertionError: [R1 blocked: both Or arms false] execution must succeed, got 'FAILED' error='States.Runtime' cause='the choice rule on \'$.n\' compares 3 with "zeta" through \'NumericEqualsPath\', which compares numbers'`.
Cause 4: `AssertionError: 1.2345678901234568e+29 != 123456789012345678901234567890`.
Cause 5: `AssertionError: Expected SUCCEEDED via catch-after-retries, got FAILED: state 'InvokeWithRetry': 'arn:aws:states:::lambda:invoke' names an integration with 'lambda', which is not one of the services this emulator makes a 'Task' state's integration call to, so nothing was run for it`; `AssertionError: Expected error States.Timeout, got error=States.Runtime ...`.
Cause 6: `json.decoder.JSONDecodeError: Expecting value: line 1 column 1 (char 0)` on the synchronous execution response.
Cause 7: both checks end `Check timed out after 60.0s`.
Cause 8: `AssertionError: expected StateMachineDoesNotExist, got ValidationException`.
Cause 9: `AssertionError: unexpected ValidationException message 'one of stateMachineArn or mapRunArn must be supplied'` — expected `Must provide a StateMachine ARN or MapRun ARN`.

## Steps to Reproduce
Cause 1: CreateStateMachine with QueryLanguage=JSONata (and a variant with a per-state QueryLanguage override) using only Pass states, then StartExecution — FAILED before the first state.
Cause 2: StartExecution on a Pass state whose Parameters use States.Hash($.s, 'SHA-256').
Cause 3: StartExecution on a Choice whose Or arms use NumericEqualsPath against a string-valued path, with a Default Pass.
Cause 4: StartExecution on a Pass whose Result is the integer 123456789012345678901234567890 and read DescribeExecution.output.
Cause 5: StartExecution on a Task Resource=arn:aws:states:::lambda:invoke with Retry and TimeoutSeconds.
Cause 6: StartSyncExecution on an EXPRESS state machine and decode the response body.
Cause 7: StartExecution, then StopExecution with error+cause (and once with error only); read GetExecutionHistory for ExecutionAborted; separately StopExecution with an error longer than 256 characters.
Cause 8: PublishStateMachineVersion, then UpdateStateMachine on the version ARN.
Cause 9: ListExecutions with no stateMachineArn and no mapRunArn.
