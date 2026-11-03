Issue-Source: trace-sweep-divergence
Issue-Trace: sfn-express-failed-execution-lookups
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sfn-express-failed-execution-lookups
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: states|StartSyncExecution|2xx->2xx|$.outputDetails
Issue-Key: states|StartSyncExecution|2xx->2xx|$.cause
Issue-Key: states|StartSyncExecution|2xx->2xx|$.error
Issue-Key: states|StartSyncExecution|2xx->2xx|$.input
Issue-Commit: e331167544f8c61f25de403c8943054d68a37733

# StartSyncExecution response omits outputDetails and leaks failure members

> _Note: this issue was identified against the code base at commit `e331167544f8c61f25de403c8943054d68a37733`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartSyncExecution

## Description
StartSyncExecution builds its response body from the wrong member set: the data-details members AWS always sends are dropped, members that belong only to a failed run are emitted on a run that the recording shows succeeding, and the cause text for a Fail state's unresolvable ErrorPath is the emulator's own wording. 3 causes.

Cause 1 of 3 — outputDetails is never serialized.
AWS: Smithy: StartSyncExecution.output member outputDetails -> CloudWatchEventsExecutionDataDetails, whose `included` member is documented "Indicates whether input or output was included in the response. Always true for API calls." AWS therefore always sends outputDetails {included: true} on a StartSyncExecution response.
Emulator: outputDetails is absent.
Why: a client that reads outputDetails.included to decide whether `output` was redacted (the documented purpose of the member) gets a null dereference, and the response does not round-trip through SDK shapes that expect it.
Fix: the StartSyncExecution serializer — populate inputDetails/outputDetails whenever the corresponding data member is emitted.
Traces: sfn-express-failed-execution-lookups#task#5

Cause 2 of 3 — error, cause and input are emitted on a response where AWS sends none of them.
AWS: Smithy: StartSyncExecution.output members error (SensitiveError) and cause (SensitiveCause) are optional and are sent only for a non-SUCCEEDED SyncExecutionStatus; `input` is likewise omitted for this call in the recording.
Emulator: error 'ConfigError', cause "Configuration validation failed: missing required field 'targetBucket'" and input '{}' are all present where AWS sent nothing.
Why: a client that tests `if response.error` to decide the run failed treats a run AWS reported without an error as a failure, and takes the error branch.
Fix: the StartSyncExecution serializer's conditional member emission, keyed on the run's SyncExecutionStatus.
Traces: sfn-express-failed-execution-lookups#task#6

Cause 3 of 3 — the cause for a Fail state whose ErrorPath cannot be resolved is the emulator's own sentence instead of AWS's.
AWS: for a JSONPath that selects no value in a named field of a named state, AWS words the cause as a fixed sentence with the state name, the entering event id, the JSONPath, the field name and the state's input substituted in: "An error occurred while executing the state '<StateName>' (entered at the event id #<id>). The JSONPath '<path>' specified for the field '<FieldName>' could not be found in the input '<input JSON>'". Fixed text: everything outside the angle brackets, including the trailing "could not be found in the input"; variable: the state name and field name from the definition, the entering event's id, the path from the definition and the state's input document. outputDetails is also absent here (same defect as cause 1).
Emulator: cause is "the path '$.err' selected no value".
Why: a client that matches the cause text to classify a definition error (or that shows it to an operator) sees a string that appears in no AWS runbook and carries neither the state name nor the field name, so the operator cannot locate the broken field.
Fix: the interpreter's path-resolution error constructor — it must build AWS's sentence from the state, field, event id, path and input rather than a bare path message.
Traces: sfn-express-failed-execution-lookups#task#8

## Evidence
All three causes are mismatch clusters from the replay of simparity-wire-aws-v1; all returned HTTP 200 as expected, with diverging bodies.

Cause 1 (sfn-express-failed-execution-lookups#task#5, cluster also covers #task#7): $.outputDetails: expected {'__type': 'com.amazonaws.swf.base.model#CloudWatchEventsExecutionDataDetails', 'included': True}, got None.

Cause 2 (#task#6): $.error: expected None, got 'ConfigError'; $.cause: expected None, got "Configuration validation failed: missing required field 'targetBucket'"; $.input: expected None, got '{}'.

Cause 3 (#task#8): $.cause: expected "An error occurred while executing the state 'Reject' (entered at the event id #2). The JSONPath '$.err' specified for the field 'ErrorPath' could not be found in the input '{\"field\": \"targetBucket\"}'", got "the path '$.err' selected no value"; $.outputDetails: expected {'__type': 'com.amazonaws.swf.base.model#CloudWatchEventsExecutionDataDetails', 'included': True}, got None.

## Steps to Reproduce
1. Replay fixture sfn-express-failed-execution-lookups against the emulator.
2. Step task#5 (the representative) calls StartSyncExecution on an EXPRESS state machine; inspect the response body for the outputDetails member — the emulator omits it.
3. Step task#6 calls StartSyncExecution on a definition whose run AWS reported with no error/cause/input members; the emulator adds all three.
4. Step task#8 calls StartSyncExecution on a definition with a Fail state whose ErrorPath JSONPath selects nothing; compare $.cause against the recorded AWS sentence.
