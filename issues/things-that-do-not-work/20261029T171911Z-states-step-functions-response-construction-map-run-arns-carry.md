Issue-Source: trace-sweep-divergence
Issue-Trace: sfn-distributed-map-terminal-map-run
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sfn-distributed-map-terminal-map-run sfn-validation-reason-labeled-arn-and-completed-map-run sfn-execution-included-data-and-history-paging states-activity-task-worker-flow stepfunctions-activity-task-success-lifecycle
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: states|ListMapRuns|2xx->2xx|$.mapRuns[*]
Issue-Key: states|ListMapRuns|2xx->2xx|$.mapRuns[*].mapRunArn
Issue-Key: states|ListMapRuns|2xx->2xx|$.mapRuns[*].stateMachineArn
Issue-Key: states|DescribeStateMachine|2xx->2xx|$.definition
Issue-Key: states|DescribeStateMachine|2xx->2xx|$.label
Issue-Key: states|DescribeStateMachine|2xx->2xx|$.type
Issue-Key: states|DescribeExecution|2xx->2xx|$.input
Issue-Key: states|DescribeExecution|2xx->2xx|$.inputDetails
Issue-Key: states|DescribeExecution|2xx->2xx|$.output
Issue-Key: states|DescribeExecution|2xx->2xx|$.outputDetails
Issue-Key: states|GetActivityTask|2xx->2xx|$.input
Issue-Commit: a9006feb831fdd789c02a15722f93d2f2e52852a

# Step Functions response construction: Map Run ARNs carry no map-state label, the Map Run's child state machine is misdescribed, includedData is ignored and payloads are re-serialized (5 causes)

> _Note: this issue was identified against the code base at commit `a9006feb831fdd789c02a15722f93d2f2e52852a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListMapRuns

## Description
This issue bundles FIVE net-new wire divergence clusters in which the emulator builds a response from its own internal values instead of the values AWS derives: three on the Distributed Map resource identity, one on DescribeExecution's includedData contract, and one on payload echo. All five must be closed before submitting; re-replay every fixture#step listed in trace_ids.

=== Cause 1 of 5: a Map Run's stateMachineArn is reported as the PARENT state machine ARN instead of the labeled child ARN ===
AWS: Smithy models ListMapRuns output.mapRuns as a list of MapRunListItem {executionArn (required), mapRunArn (required, LongArn), stateMachineArn (required, Arn, "The Amazon Resource Name (ARN) of the executed state machine"), startDate (required), stopDate}. Recorded behavior: for a Distributed Map, the "executed state machine" is the Map Run's own child state machine, whose ARN is the LABELED form `arn:aws:states:<region>:<account>:stateMachine:<parent name>/<label>`, and the mapRunArn is `arn:aws:states:<region>:<account>:mapRun:<parent name>/<label>:<uuid>` carrying the SAME label. In one recording the label is the Map state's name (`fanout`); in another (Cause 2) it is a generated 36-character label where the state name is not usable as one — Hypothesis: AWS derives the label from the Map state and substitutes a generated label when the state name cannot serve. The requirement that both recordings share: the stateMachineArn must be the labeled child form, and its label must equal the label embedded in the mapRunArn.
Emulator: stateMachineArn is the bare parent ARN with no `/<label>` suffix, while its mapRunArn does carry a label — so the two members disagree and the child state machine the Map Run executed is unnameable from the listing.
Why: the labeled ARN is the handle used to describe the Map Run's child state machine and to correlate child executions and CloudWatch dimensions with their Map Run; a client that takes stateMachineArn from the listing addresses the parent instead and gets the wrong definition (Cause 3).
Fix: the Map Run projection in the sfn state / ListMapRuns' serializer — derive and store the Map Run label once and render both the mapRun ARN and the child stateMachineArn from it.
Traces: sfn-distributed-map-terminal-map-run#task#6.

=== Cause 2 of 5: the Map Run label is the raw Map state name where AWS uses a generated label, and the stateMachineArn again carries none ===
AWS: recorded behavior — the same ListMapRuns item reports mapRunArn `...:mapRun:<parent name>/<label>:<uuid>` and stateMachineArn `...:stateMachine:<parent name>/<label>`, where for this definition the label is a generated 36-character identifier rather than the Map state's name.
Emulator: it interpolates the Map state's name directly as the label (`.../<state name>:<uuid>`) and again omits the label from stateMachineArn, so both members diverge.
Why: as Cause 1, plus a client that parses the label out of a mapRunArn to recover the Map state sees a value AWS would not have produced, and the correlation across mapRunArn/stateMachineArn breaks.
Fix: as Cause 1 — the label-derivation rule, applied consistently to both ARNs.
Traces: sfn-validation-reason-labeled-arn-and-completed-map-run#task#6.

=== Cause 3 of 5: DescribeStateMachine on a labeled Map Run ARN returns the PARENT state machine's definition, type and no label ===
AWS: Smithy models DescribeStateMachine output with required definition, roleArn, type and name plus the optional `label` (MapRunLabel). Recorded behavior: describing the labeled child ARN of a Distributed Map returns the ItemProcessor's OWN sub-definition (the inner StartAt/States of the Map state), `type` taken from the ItemProcessor's ProcessorConfig.ExecutionType (EXPRESS here), and a populated `label` member (in this recording the parent state machine ARN).
Emulator: it resolves the labeled ARN to the parent, returning the parent's whole definition (the outer Map machine), `type` STANDARD (the parent's type) and no `label` at all.
Why: the labeled describe is how a caller inspects what a Distributed Map actually runs per item and at which execution type; answering with the parent makes the child indistinguishable from its parent, so tooling that renders a Map Run shows the wrong workflow and the wrong execution semantics.
Fix: DescribeStateMachine's handler/state lookup — resolve a labeled state-machine ARN to the Map Run's child machine (ItemProcessor definition, ItemProcessor ExecutionType) and emit the `label` member.
Traces: sfn-validation-reason-labeled-arn-and-completed-map-run#task#9.

=== Cause 4 of 5: DescribeExecution ignores includedData and returns execution data when METADATA_ONLY was requested ===
AWS: Smithy models DescribeExecution input {executionArn (required), includedData (IncludedData enum: ALL_DATA | METADATA_ONLY)} and output members input, inputDetails, output and outputDetails (input/output are @sensitive, @length 0..262144). Recorded behavior: with includedData other than ALL_DATA the response omits input, inputDetails, output and outputDetails entirely, returning metadata only.
Emulator: it returns all four regardless — the execution input, the output, and both CloudWatchEventsExecutionDataDetails blocks with included=true.
Why: METADATA_ONLY exists so a caller can poll execution status without handling the execution's payload, which is sensitive data that may be subject to size and confidentiality limits. Ignoring it leaks the payload to every status poller, inflates responses, and makes `included` report true when the caller asked for no data.
Fix: DescribeExecution's handler/serializer — honour includedData and suppress input/inputDetails/output/outputDetails (and the `included` flag) when ALL_DATA was not requested.
Traces: sfn-execution-included-data-and-history-paging#task#4.

=== Cause 5 of 5: GetActivityTask re-serializes the task input instead of echoing the execution's payload bytes ===
AWS: Smithy models GetActivityTask output {taskToken (@length 1..2048, sensitive), input (SensitiveDataJobInput, @length 0..1048576, sensitive)}. Recorded behavior: `input` is the state's input JSON as produced for the Task state, preserving the whitespace of the payload the caller supplied to StartExecution.
Emulator: it parses and re-emits the payload compactly, so `{"work": 1}` reaches the worker as `{"work":1}`.
Why: an activity worker that verifies a signature or checksum over the task input, or that compares the bytes against what the client submitted, rejects a payload the emulator silently rewrote.
Fix: the raw-payload echo in the activity-task scheduling path shared by the interpreter and GetActivityTask — carry the input bytes rather than round-tripping them through a JSON writer.
Traces: states-activity-task-worker-flow#task#8 (cluster also covers states-activity-task-worker-flow#task#12 and stepfunctions-activity-task-success-lifecycle#task#6).

## Evidence
Replay of trace set simparity-wire-aws-v1; all clusters verdict=mismatch, service=states, status 200 on both sides.

Cause 1 — sfn-distributed-map-terminal-map-run#task#6, ListMapRuns: `$.mapRuns[0]` differs — expected `{'executionArn': <exec arn>, 'mapRunArn': '<shaped-not-asserted>', 'stateMachineArn': 'arn:aws:states:<region>:<account>:stateMachine:<parent name>/fanout', 'startDate': ..., 'stopDate': ...}`, got stateMachineArn `arn:aws:states:<region>:<account>:stateMachine:<parent name>` (no `/fanout` label) while its mapRunArn was `...:mapRun:<parent name>/fanout:<uuid>`.
Cause 2 — sfn-validation-reason-labeled-arn-and-completed-map-run#task#6, ListMapRuns: `$.mapRuns[0].mapRunArn: value_mismatch (expected '...:mapRun:<parent name>/69691c7b-dcc3-3e6d-9d8a-1361f22d04ac:<uuid>', got '...:mapRun:<parent name>/M:<uuid>')`; `$.mapRuns[0].stateMachineArn: value_mismatch (expected '...:stateMachine:<parent name>/69691c7b-dcc3-3e6d-9d8a-1361f22d04ac', got '...:stateMachine:<parent name>')`. `M` is the Map state's name in this fixture's definition and the 36-char label is this run's recorded value — neither is part of the requirement; the requirement is that stateMachineArn carries a label and that it equals the mapRunArn's label.
Cause 3 — sfn-validation-reason-labeled-arn-and-completed-map-run#task#9, DescribeStateMachine: `$.definition: value_mismatch (expected '{"StartAt":"P","States":{"P":{"Type":"Pass","End":true}}}' — the ItemProcessor sub-definition — got the full parent Map definition)`; `$.type: value_mismatch (expected 'EXPRESS', got 'STANDARD')`; `$.label: missing (expected '<parent state machine arn>', got None)`.
Cause 4 — sfn-execution-included-data-and-history-paging#task#4, DescribeExecution: `$.input: unexpected_present (expected None, got '{"order": "o-1"}')`; `$.inputDetails: unexpected_present (expected None, got {'__type': 'com.amazonaws.swf.base.model#CloudWatchEventsExecutionDataDetails', 'included': True})`; `$.output: unexpected_present (expected None, got '{"order":"o-1",...}')`; `$.outputDetails: unexpected_present (expected None, got {... 'included': True})`.
Cause 5 — states-activity-task-worker-flow#task#8, GetActivityTask: `$.input: value_mismatch (expected '{"work": 1}', got '{"work":1}')` (also #task#12 and stepfunctions-activity-task-success-lifecycle#task#6).

## Steps to Reproduce
Cause 1/2: 1) sfn:CreateStateMachine(type=STANDARD) whose single state is a Map with ProcessorConfig {Mode: DISTRIBUTED, ExecutionType: EXPRESS} and an ItemProcessor '{"StartAt":"P","States":{"P":{"Type":"Pass","End":true}}}'; 2) sfn:StartExecution with items and wait for the Map Run to finish; 3) sfn:ListMapRuns(executionArn=<exec>). Expect mapRuns[0].stateMachineArn == '<parent sm arn>/<label>' with the SAME label embedded in mapRuns[0].mapRunArn; observed the bare parent ARN in stateMachineArn.
Cause 3: 4) sfn:DescribeStateMachine(stateMachineArn=<the labeled ARN from step 3>). Expect definition == the ItemProcessor sub-definition, type == 'EXPRESS' (the ItemProcessor's ExecutionType) and a populated `label`; observed the parent's definition, 'STANDARD' and no label.
Cause 4: 1) StartExecution(input='{"order": "o-1"}') and wait for SUCCEEDED; 2) sfn:DescribeExecution(executionArn=<exec>, includedData='METADATA_ONLY'). Expect no input, inputDetails, output or outputDetails in the response; observed all four present with included=true. Positive control: includedData='ALL_DATA' must return them.
Cause 5: 1) sfn:CreateActivity; 2) create a state machine whose Task Resource is that activity ARN; 3) StartExecution(input='{"work": 1}'); 4) sfn:GetActivityTask(activityArn=<act>, workerName=<w>). Expect `input` to echo '{"work": 1}' byte-for-byte; observed '{"work":1}'.
Replay: the fixture#step tokens in Issue-Trace/Issue-Step and trace_ids.
