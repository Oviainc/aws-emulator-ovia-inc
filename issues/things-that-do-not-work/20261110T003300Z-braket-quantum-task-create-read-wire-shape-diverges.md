Issue-Source: trace-sweep-divergence
Issue-Trace: braket-sv1-deterministic-circuit-results-in-s3
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: braket-sv1-deterministic-circuit-results-in-s3
Issue-Shape-Id: com.amazonaws.braket#Braket
Issue-Key: braket|CreateQuantumTask|2xx->2xx|$.status
Issue-Key: braket|CreateQuantumTask|2xx->2xx|status
Issue-Key: braket|GetQuantumTask|2xx->2xx|$.createdAt
Issue-Key: braket|GetQuantumTask|2xx->2xx|$.deviceParameters
Issue-Key: braket|GetQuantumTask|2xx->2xx|$.endedAt
Issue-Key: braket|GetQuantumTask|2xx->2xx|$.failureReason
Issue-Key: braket|GetQuantumTask|2xx->2xx|$.status
Issue-Key: braket|GetQuantumTask|2xx->2xx|$.tags
Issue-Commit: afd3b182fafbbe8cee1c009b5b1f0d78bd466684

# Braket quantum-task create/read wire shape diverges: 201 without status, tasks never execute, epoch timestamps, dropped members

> _Note: this issue was identified against the code base at commit `afd3b182fafbbe8cee1c009b5b1f0d78bd466684`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateQuantumTask

## Description
Four distinct root causes on the braket quantum-task create/read path, all found by replaying the wire corpus (fixture braket-sv1-deterministic-circuit-results-in-s3). Braket is answering (no bootstrap gap), so these are fidelity defects. A fixer must close ALL FOUR before submitting.

Note on already-open issues: the behavioral issue 20261022T134201Z-braket-quantum-task-lifecycle-createquantumtask-validation-and-not.md describes the 'tasks are never executed' symptom from the behavioral corpus, and 20261103T044954Z-braket-read-paths-serialize-response-bodies-wrongly-spending.md describes wrong timestamp/number serialization on the spending-limit and device read paths. NEITHER carries the wire replay key for these steps (the replay reported owned_by empty for both of them), so they do not cover the divergences below; they are named here only so a fixer can fix them together.

=== Cause 1 of 4: CreateQuantumTask answers 201 and omits the task's initial status ===
AWS: recorded behavior only for the wire shape. The recorded CreateQuantumTask response is HTTP 200 and its JSON body carries the created task's initial status member alongside the task ARN (recorded value 'INITIALIZED'). The on-disk Smithy model CONFLICTS with the recording and must be reconciled by the fixer rather than trusted blindly: smithy_query(operation=CreateQuantumTask) gives http {method: POST, uri: /quantum-task, code: 201} and an output shape CreateQuantumTaskResponse whose only member is quantumTaskArn (required); 'INITIALIZED' is also not a member of the modelled QuantumTaskStatus enum (CANCELLED|CANCELLING|COMPLETED|CREATED|FAILED|QUEUED|RUNNING). The recorded wire response is the requirement for the replay: status code 200, body containing the task ARN plus the task's status member. The ARN value itself is per-run and is not a requirement.
Emulator: answers HTTP 201 (the model-derived success code) and a body with quantumTaskArn only, no status member — replay: 'status: expected 200, got 201' and '$.status: missing (expected INITIALIZED, got None)'.
Why: a client that reads the create response's status to decide whether to start polling, or that treats a non-200 2xx as an unexpected response, gets a different code and a missing member than against AWS; SDK response-shape parsing that expects the status member sees nothing.
Fix: CreateQuantumTask's handler/serializer — its success status constant and the output members it assembles (SUCCESS_STATUS and the CreateQuantumTaskOutput construction), together with the service's Smithy output shape if the model is what is stale.
Traces: braket-sv1-deterministic-circuit-results-in-s3#task#3 (CreateQuantumTask). Code: services/service-com-amazonaws-braket-braket/src/ops/create_quantum_task.rs:205 (const SUCCESS_STATUS = 201) and :358 (output carries only quantum_task_arn).

=== Cause 2 of 4: the task is never executed — GetQuantumTask reports FAILED with an emulator-authored failureReason instead of reaching COMPLETED ===
AWS: GetQuantumTask reports the task's real lifecycle (Smithy: GetQuantumTask output members status (QuantumTaskStatus, required), failureReason (String), endedAt, numSuccessfulShots — 'The number of successful shots for the quantum task. This is available after a successfully completed quantum task'). A task submitted to the managed SV1 simulator runs and reaches the terminal status COMPLETED, with failureReason absent. The recorded trace polls GetQuantumTask and expects status COMPLETED.
Emulator: the task transitions to FAILED and sets failureReason to the emulator's own sentence 'This emulator does not execute quantum programs: no device or simulator ran this quantum task, so no result was written to the output S3 location.' — replay: '$.status: value_mismatch (expected COMPLETED, got FAILED)', '$.failureReason: type_mismatch (expected None, got <that sentence>)', and the poller aborted because FAILED is in its fail_on set. failureReason is a modelled member, but AWS only populates it for a task that actually failed, and never with emulator-implementation prose.
Why: every Braket workflow terminates in failure against the emulator. No client can submit a circuit and read results, no code path downstream of COMPLETED (results fetch, numSuccessfulShots, cancel-after-terminal conflict) is reachable, and the failure text is not any error AWS produces, so client-side failure handling keyed on AWS's reasons never matches.
Fix: the quantum-task state machine behind CreateQuantumTask/GetQuantumTask (the braket task state's due-transition path) plus the simulator execution step it is missing, so a task on a simulator device reaches COMPLETED with no failureReason and with the results document written to its outputS3Directory (see the companion s3 issue filed for braket-sv1-deterministic-circuit-results-in-s3#task#5).
Traces: braket-sv1-deterministic-circuit-results-in-s3#task#4 (GetQuantumTask). Code: services/service-com-amazonaws-braket-braket/src/ops/get_quantum_task.rs:260 (apply_due_transitions) and :295-297 (num_successful_shots hard-coded None because 'nothing executed this task').

=== Cause 3 of 4: GetQuantumTask serializes createdAt/endedAt as epoch numbers instead of date-time strings ===
AWS: GetQuantumTask's createdAt (required) and endedAt are Timestamp members and this service's model pins their wire form (Smithy: GetQuantumTask output createdAt target Timestamp with timestampFormat date-time; endedAt likewise), i.e. an ISO-8601/RFC-3339 date-time STRING. The instant itself is per-run and is not a requirement — only the type and format are.
Emulator: both come back as JSON numbers of epoch seconds with a fractional part — replay: '$.createdAt: value_mismatch (expected <shaped-not-asserted>, got 1794270608.1937099)' and '$.endedAt: value_mismatch (expected <shaped-not-asserted>, got 1794270609.39371)'. The same defect class is already filed for the spending-limit read path in 20261103T044954Z-braket-read-paths-serialize-response-bodies-wrongly-spending.md, which does not cover this step; a single shared timestamp serializer fix likely closes both.
Why: an SDK parsing a date-time-formatted member receives a number and either fails to deserialize the response or produces a wrong instant, so every consumer of the task's creation/end time breaks.
Fix: the timestamp serializer used by braket's read paths / the helper that renders stored SystemTime values for date-time members, so these members emit a date-time string.
Traces: braket-sv1-deterministic-circuit-results-in-s3#task#4 (GetQuantumTask). Code: services/service-com-amazonaws-braket-braket/src/ops/get_quantum_task.rs:288-289 (crate::state::to_date_time).

=== Cause 4 of 4: GetQuantumTask drops response members — empty tags omitted, and deviceParameters is not round-tripped ===
AWS: GetQuantumTask returns deviceParameters (JsonValue, REQUIRED, mediaType application/json) — 'The parameters for the device on which the quantum task ran' — i.e. the device-parameters document the task was created with, echoed back verbatim; the recording shows the braket.device_schema.simulators.gate_model_simulator_device_parameters document that was submitted on CreateQuantumTask. It also returns tags (TagsMap); the recording shows tags present as an EMPTY MAP for a task created with no tags, rather than the member being absent. Contents of the device-parameters document are whatever the request sent, not a fixed value.
Emulator: deviceParameters comes back as the empty object '{}' — the document supplied at create time is lost — and tags is absent entirely. Replay: '$.deviceParameters: value_mismatch (expected <the submitted gate_model_simulator_device_parameters document>, got {})' and '$.tags: missing (expected {}, got None)'.
Why: a client that reads back the parameters its task ran under sees an empty document, so it cannot verify or re-submit the same configuration; a client that indexes response['tags'] on a task with no tags gets a KeyError/None against the emulator where AWS gives an empty map.
Fix: CreateQuantumTask's record construction (store the request's deviceParameters instead of defaulting to '{}') and GetQuantumTask's response assembly (emit tags as an empty map rather than omitting the member when the task carries none).
Traces: braket-sv1-deterministic-circuit-results-in-s3#task#4 (GetQuantumTask). Code: services/service-com-amazonaws-braket-braket/src/ops/create_quantum_task.rs:320-323 (device_parameters unwrap_or '{}') and services/service-com-amazonaws-braket-braket/src/ops/get_quantum_task.rs:290-291 (tags emitted only when non-empty).

## Evidence
Replay of trace set simparity-wire-aws-v1: 39 steps, 30 passed, 9 mismatched, score 76.9%. Divergences below are the two clusters on this path that no open issue owns (owned_by empty).

Cause 1 — fixture braket-sv1-deterministic-circuit-results-in-s3, step task#3, operation CreateQuantumTask, verdict mismatch, divergence_key 'service=braket|CreateQuantumTask|status=200->201|keys=$.status,status':
  status: expected 200, got 201
  $.status: missing (expected 'INITIALIZED', got None)

Causes 2-4 — fixture braket-sv1-deterministic-circuit-results-in-s3, step task#4, operation GetQuantumTask, verdict mismatch, expected 200 / actual 200, divergence_key 'service=braket|GetQuantumTask|status=200->200|keys=$.createdAt,$.deviceParameters,$.endedAt,$.failureReason,$.status,$.tags':
  $.status: value_mismatch (expected 'COMPLETED', got 'FAILED')
  $.status: poll: 'FAILED' is in fail_on after 2 attempt(s) (expected 'COMPLETED', got 'FAILED')
  $.failureReason: type_mismatch (expected None, got 'This emulator does not execute quantum programs: no device or simulator ran this quantum task, so no result was written to the output S3 location.')
  $.deviceParameters: value_mismatch (expected '{"braketSchemaHeader": {"name": "braket.device_schema.simulators.gate_model_simulator_device_parameters", "version": "1"}, "paradigmParameters": {"braketSchemaHeader": {"name": "braket.device_schema.gate_model_parameters", "version": "1"}, "qubitCount": 2, "disableQubitRewiring": false}}', got '{}')
  $.createdAt: value_mismatch (expected '<shaped-not-asserted>', got 1794270608.1937099)
  $.endedAt: value_mismatch (expected '<shaped-not-asserted>', got 1794270609.39371)
  $.tags: missing (expected {}, got None)

Model consulted: smithy_query(query='operation', operation='CreateQuantumTask,GetQuantumTask', model_path='services/service-com-amazonaws-braket-braket/smithy-model/com-amazonaws-braket-braket.json') — CreateQuantumTask http {POST /quantum-task, code 201}, output {quantumTaskArn required}; GetQuantumTask output {status required (QuantumTaskStatus: CANCELLED|CANCELLING|COMPLETED|CREATED|FAILED|QUEUED|RUNNING), deviceParameters required JsonValue, createdAt required Timestamp timestampFormat date-time, endedAt Timestamp date-time, tags TagsMap, failureReason String, numSuccessfulShots Long}.

## Steps to Reproduce
Boot the emulator and replay fixture braket-sv1-deterministic-circuit-results-in-s3 (replay_wire_corpus), or by hand with a botocore braket client in us-west-2:

1. s3.create_bucket(Bucket='amazon-braket-<uniq>').
2. braket.create_quantum_task(clientToken=<uniq>, deviceArn='arn:aws:braket:::device/quantum-simulator/amazon/sv1', shots=10, outputS3Bucket=<bucket>, outputS3KeyPrefix='results', action=json.dumps({'braketSchemaHeader': {'name': 'braket.ir.openqasm.program', 'version': '1'}, 'source': 'OPENQASM 3.0; bit[2] b; qubit[2] q; x q[0]; b[0] = measure q[0]; b[1] = measure q[1];'}), deviceParameters=json.dumps({'braketSchemaHeader': {'name': 'braket.device_schema.simulators.gate_model_simulator_device_parameters', 'version': '1'}, 'paradigmParameters': {'braketSchemaHeader': {'name': 'braket.device_schema.gate_model_parameters', 'version': '1'}, 'qubitCount': 2, 'disableQubitRewiring': False}})).
   -> Cause 1: observe HTTP 201 and a body with no status member; the recorded wire response is 200 with the task's status member present.
3. Poll braket.get_quantum_task(quantumTaskArn=<arn from step 2>) until the status is terminal.
   -> Cause 2: observe status FAILED plus failureReason 'This emulator does not execute quantum programs: ...' instead of COMPLETED with no failureReason.
   -> Cause 3: inspect the raw JSON body (not the SDK-parsed shape): createdAt/endedAt are epoch-second numbers instead of date-time strings.
   -> Cause 4: deviceParameters is '{}' rather than the document sent in step 2, and the tags member is absent rather than an empty map.
