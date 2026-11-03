Issue-Source: trace-sweep-divergence
Issue-Trace: sfn-failing-lambda-task-execution-diagnosis
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sfn-failing-lambda-task-execution-diagnosis sfn-legacy-lambda-arn-task-failure-events sfn-retry-catch-error-name-matching sfn-runtime-error-bypasses-catch-and-redrive
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: states|DescribeExecution|2xx->2xx|$.cause
Issue-Key: states|DescribeExecution|2xx->2xx|$.error
Issue-Key: states|DescribeExecution|2xx->2xx|$.redriveStatus
Issue-Key: states|DescribeExecution|2xx->2xx|$.redriveStatusReason
Issue-Key: states|DescribeExecution|2xx->2xx|$.status
Issue-Related: 20261029T171911Z-states-step-functions-response-construction-map-run-arns-carry.md
Issue-Commit: e331167544f8c61f25de403c8943054d68a37733

# Execution outcome, error name and cause text diverge on failed and caught runs

> _Note: this issue was identified against the code base at commit `e331167544f8c61f25de403c8943054d68a37733`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeExecution

## Description
DescribeExecution reports the wrong terminal outcome for runs whose Task state fails: the error name is the emulator's own, the cause is emulator-authored text, a run AWS reports SUCCEEDED (because its Catch recovered) is reported FAILED, and failure members are emitted on responses where AWS sends none. 4 causes.

Cause 1 of 4 — the error name and cause of a failed Lambda Task are the emulator's rather than the function's.
AWS: Smithy: DescribeExecution.output.error -> SensitiveError (@length 0..256), .cause -> SensitiveCause (@length 0..32768). For an optimized lambda:invoke Task whose function raised, AWS surfaces the function's own error name in `error` (here RuntimeError, the Python exception class the recorded function raised) and the function's message/stack in `cause`. The error name is the function's unhandled-error type, not a Step Functions error.
Emulator: error is 'Lambda.AWSLambdaException' and cause is an IAM-authorization sentence ("User: <role arn> is not authorized to perform: lambda:InvokeFunction ...") that no part of the recording produced.
Why: a Catch/Retry ErrorEquals on the function's error name (the documented way to branch on a function failure) never matches, and an operator reading `cause` is pointed at a non-existent IAM problem instead of the function's exception. Related open issue 20261027T173205Z-sfn-step-functions-never-dispatches-the-optimized-lambda-invoke.md covers the dispatch; it does not cover DescribeExecution's reported error/cause asserted here.
Fix: the lambda:invoke Task integration's failure mapping, and the execution record's error/cause fields.
Traces: sfn-failing-lambda-task-execution-diagnosis#task#7

Cause 2 of 4 — input, error and cause are emitted on a DescribeExecution response where AWS sends none of them.
AWS: DescribeExecution.output members input, error and cause are optional; for this call (a describe with includedData restricting the returned data) AWS returned none of them.
Emulator: input '{"orderId": "o-1"}', error 'Lambda.AWSLambdaException' and the IAM cause sentence are all present.
Why: a client that treats a present `error` as proof the execution failed, or that relies on includedData to keep execution data out of the response, sees data AWS withheld.
Fix: the DescribeExecution serializer's conditional member emission (includedData handling and the failed-only members).
Traces: sfn-failing-lambda-task-execution-diagnosis#task#18

Cause 3 of 4 — an execution whose Catch recovers the task is reported FAILED and REDRIVABLE instead of SUCCEEDED and NOT_REDRIVABLE.
AWS: Smithy: ExecutionStatus = [RUNNING, SUCCEEDED, FAILED, TIMED_OUT, ABORTED, PENDING_REDRIVE]; ExecutionRedriveStatus = [REDRIVABLE, NOT_REDRIVABLE, REDRIVABLE_BY_MAP_RUN]. When a Catch handles the task error and the execution reaches a terminal success, status is SUCCEEDED, output/outputDetails are populated, redriveStatus is NOT_REDRIVABLE and redriveStatusReason is the fixed sentence "Execution is SUCCEEDED and cannot be redriven".
Emulator: status FAILED (the fixture polls and the emulator stays FAILED), no output or outputDetails, redriveStatus REDRIVABLE and no redriveStatusReason.
Why: this inverts the outcome of the run: a client waiting for SUCCEEDED waits forever or takes its failure path, and a redrive tool offers to redrive an execution AWS considers finished.
Fix: the Catch handling in the states execution interpreter, and the redriveStatus/redriveStatusReason derivation in the execution record.
Traces: sfn-retry-catch-error-name-matching#task#12

Cause 4 of 4 — the cause for an unresolvable Parameters JSONPath drops AWS's state/event prefix and re-serializes the input.
AWS: the cause is a fixed sentence with values substituted from the definition and the run: "An error occurred while executing the state '<StateName>' (entered at the event id #<id>). The JSONPath '<path>' specified for the field '<FieldName>' could not be found in the input '<the state's input JSON, as received>'". Fixed text: everything outside the brackets; variable: state name and field name from the definition, the entering event id, the path, and the input document.
Emulator: cause is only the second sentence ("The JSONPath '...' specified for the field '...' could not be found in the input '...'") with the leading "An error occurred while executing the state ... (entered at the event id #N). " clause missing, and the embedded input re-serialized without the separator spacing it was received with.
Why: the operator loses the state name and the event id, which are the only pointers back to where in the definition the bad path lives; a client matching AWS's wording does not match.
Fix: the interpreter's path-resolution error constructor — prepend AWS's state/event clause and embed the input verbatim rather than re-serializing it.
Traces: sfn-runtime-error-bypasses-catch-and-redrive#task#11

## Evidence
All four causes are mismatch clusters from the replay of simparity-wire-aws-v1; all returned HTTP 200 as expected, with diverging bodies.

Cause 1 (sfn-failing-lambda-task-execution-diagnosis#task#7; cluster also covers #task#16, sfn-legacy-lambda-arn-task-failure-events#task#9, sfn-retry-catch-error-name-matching#task#16 and sfn-runtime-error-bypasses-catch-and-redrive#task#15): $.error: expected 'RuntimeError', got 'Lambda.AWSLambdaException'; $.cause: expected a shaped value, got 'User: arn:aws:iam::<account>:role/<sfn role> is not authorized to perform: lambda:InvokeFunction on resource: <function arn> because no identity-based policy allows the lambda:InvokeFunction action (Service: AWSLambda; Status Code: 400; Error Code: AccessDeniedException; Proxy: null)'.

Cause 2 (#task#18): $.input: expected None, got '{"orderId": "o-1"}'; $.error: expected None, got 'Lambda.AWSLambdaException'; $.cause: expected None, got the same IAM sentence.

Cause 3 (sfn-retry-catch-error-name-matching#task#12; cluster also covers #task#14 and sfn-runtime-error-bypasses-catch-and-redrive#task#13): $.status: expected 'SUCCEEDED', got 'FAILED' (the step's poll also reports "'FAILED' is in fail_on after 2 attempt(s)"); $.output: expected present, got None; $.outputDetails: expected {'__type': 'com.amazonaws.swf.base.model#CloudWatchEventsExecutionDataDetails', 'included': True}, got None; $.redriveStatus: expected 'NOT_REDRIVABLE', got 'REDRIVABLE'; $.redriveStatusReason: expected 'Execution is SUCCEEDED and cannot be redriven', got None.

Cause 4 (sfn-runtime-error-bypasses-catch-and-redrive#task#11): $.cause: expected "An error occurred while executing the state 'InvokeProcessingLambda' (entered at the event id #2). The JSONPath '$.targetBucket' specified for the field 'bucket.$' could not be found in the input '{\"orderId\": \"o-1\"}'", got "The JSONPath '$.targetBucket' specified for the field 'bucket.$' could not be found in the input '{\"orderId\":\"o-1\"}'".

## Steps to Reproduce
1. Replay fixture sfn-failing-lambda-task-execution-diagnosis against the emulator; at step task#7 (the representative) the fixture waits for the execution to reach a terminal status and calls DescribeExecution. Compare $.error and $.cause against the recording.
2. Step task#18 of the same fixture calls DescribeExecution after a redrive with includedData restricted; check that input/error/cause are absent in the recording and present in the emulator's answer.
3. Replay sfn-retry-catch-error-name-matching and compare step task#12: the definition's Catch recovers the failing task, so AWS's DescribeExecution reports status SUCCEEDED with redriveStatus NOT_REDRIVABLE; the emulator reports FAILED/REDRIVABLE.
4. Replay sfn-runtime-error-bypasses-catch-and-redrive and compare step task#11's $.cause string against the recorded sentence.
