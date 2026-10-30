Issue-Source: trace-sweep-divergence
Issue-Trace: braket-hybrid-job-error-paths
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: braket-hybrid-job-error-paths braket-quantum-task-error-paths braket-search-and-inspect-devices braket-spending-limit-error-paths
Issue-Shape-Id: com.amazonaws.braket#Braket
Issue-Key: braket|CancelJob|4xx->4xx|$.message
Issue-Key: braket|CreateQuantumTask|4xx->4xx|$.message
Issue-Key: braket|GetQuantumTask|4xx->4xx|$.message
Issue-Key: braket|CancelQuantumTask|4xx->4xx|$.message
Issue-Key: braket|GetDevice|4xx->4xx|$.message
Issue-Key: braket|DeleteSpendingLimit|4xx->4xx|$.message
Issue-Key: braket|UpdateSpendingLimit|4xx->4xx|$.message
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# Braket client-error messages are the emulator's own prose, not AWS's per-operation wording (7 causes)

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CancelJob

## Description
Seven net-new wire divergences in the braket crate share ONE fault: every client-error body this service raises is worded by the crate's own single-homed prose helpers (src/state.rs `invalid_arn_message`, and inline `format!("Device {} not found", ...)` in the device handlers) instead of the wording AWS uses for that error class. In all seven the HTTP status and the error code (`__type`) match the recording exactly; only `$.message` diverges. AWS does NOT single-home these strings: the same malformed-ARN class is worded differently per operation (GetQuantumTask says "quantumTaskArn is invalid." where CancelQuantumTask says "quantumTaskArn is not valid."), and the not-found refusals carry documentation pointers that the emulator's prose drops entirely. An error message is a requirement, so each of these is a defect. braket is bootstrapped and answering, so these are fidelity defects, not bootstrap gaps. A fixer must close all SEVEN before submitting. (The one open braket issue, 20261022T134201Z-braket-quantum-task-lifecycle-createquantumtask-validation-and-not, is from the behavioral corpus and covers the quantum-task execution state machine, CreateQuantumTask input validation, GetJob's scaffold and SearchDevices' filter message — none of the seven causes below. The resolved issues 20261022T061137Z-braket-quantum-task-arn-syntax-not-validated-404 and ...-braket-request-body-constraint-violations-... fixed the status/code of these refusals; they did not fix their wording, and a resolved issue never covers a live failure.)

=== Cause 1 of 7: CancelJob's malformed-jobArn refusal drops AWS's wording and the rejected value ===
AWS: Smithy: CancelJob PUT /job/{jobArn}/cancel, input member jobArn (JobArn, @httpLabel, @required, pattern ^arn:aws[a-z\-]*:braket:[a-z0-9\-]+:[0-9]{12}:job/.*$); modeled error ValidationException, httpError 400. Recorded response: 400 ValidationException whose message is the fixed text "jobArn " + the rejected jobArn value taken verbatim from the request + " is not a valid ARN." (recorded example value: not-a-valid-job-arn).
Emulator: 400 ValidationException with message "jobArn parameter is not valid." — the crate's shared `invalid_arn_message` prose, which names the member but neither AWS's sentence nor the value that was rejected.
Why: a client that branches on the message text, or that surfaces which input it must fix, gets neither AWS's words nor the offending value; message-matching integration tests pass against AWS and fail against the emulator.
Fix: CancelJob's jobArn validation branch and the shared refusal-wording helper it calls in the crate's state module, which must render each operation's message as AWS words it rather than one generic sentence.
Traces: braket-hybrid-job-error-paths#task#3

=== Cause 2 of 7: CreateQuantumTask's unknown-device refusal echoes the ARN instead of pointing at SearchDevices ===
AWS: Smithy: CreateQuantumTask POST /quantum-task, input deviceArn (DeviceArn, @required, len 1..256); modeled errors ValidationException (400), DeviceOfflineException (424), DeviceRetiredException (410) — no ResourceNotFoundException, so an unresolvable device is a ValidationException. Recorded response: 400 ValidationException with the FIXED message "Device ARN not found. Please use SearchDevices operation to get a list of supported devices." — the whole string is fixed text; it interpolates nothing from the request.
Emulator: 400 ValidationException with "Device <deviceArn> not found" — the handler's inline prose, which echoes the request value AWS deliberately does not echo and omits the SearchDevices pointer.
Why: a caller that cannot resolve a device ARN loses the only instruction AWS gives for recovering (enumerate devices with SearchDevices), and clients keying on the fixed text do not recognise the error.
Fix: the unknown-device branch of CreateQuantumTask's handler (its device-inventory lookup), which must emit the service's fixed not-found sentence rather than an interpolated one.
Traces: braket-quantum-task-error-paths#task#1

=== Cause 3 of 7: GetQuantumTask's malformed-ARN refusal explains the ARN grammar AWS never states ===
AWS: Smithy: GetQuantumTask GET /quantum-task/{quantumTaskArn}, input quantumTaskArn (QuantumTaskArn, @httpLabel, @required, len 0..256); modeled errors ValidationException (400), ResourceNotFoundException (404). Recorded response: 400 ValidationException with the FIXED message "quantumTaskArn is invalid." — member name plus that fixed sentence, with no echo of the rejected value and no grammar description.
Emulator: 400 ValidationException with "quantumTaskArn is invalid: expected an Amazon Braket quantum-task ARN of the form arn:aws:braket:<region>:<account>:quantum-task/<id>, got \"<value>\"" — the crate's shared ARN-refusal builder, which appends an explanatory clause and echoes the request value.
Why: the message is not AWS's, and because it is built from one shared helper it also echoes caller-supplied input of unbounded length back into an error body where AWS returns a constant.
Fix: GetQuantumTask's quantumTaskArn validation branch and the shared ARN-refusal wording helper in the crate's state module.
Traces: braket-quantum-task-error-paths#task#2

=== Cause 4 of 7: CancelQuantumTask words the same class differently from GetQuantumTask, and the emulator collapses both ===
AWS: Smithy: CancelQuantumTask PUT /quantum-task/{quantumTaskArn}/cancel, input quantumTaskArn (QuantumTaskArn, @httpLabel, @required); modeled errors ValidationException (400), ResourceNotFoundException (404), ConflictException (409). Recorded response: 400 ValidationException with the FIXED message "quantumTaskArn is not valid." — note this is NOT the sentence GetQuantumTask uses for the identical input defect ("quantumTaskArn is invalid."); AWS words it per operation.
Emulator: the same shared builder output as Cause 3: "quantumTaskArn is invalid: expected an Amazon Braket quantum-task ARN of the form arn:aws:braket:<region>:<account>:quantum-task/<id>, got \"<value>\"".
Why: one shared sentence cannot satisfy both operations, so any fix that single-homes the wording keeps one of them diverging; clients that distinguish the two operations' messages see the wrong one.
Fix: CancelQuantumTask's quantumTaskArn validation branch, worded for THIS operation, with the shared helper reduced to a per-operation template rather than one sentence for all of them.
Traces: braket-quantum-task-error-paths#task#3

=== Cause 5 of 7: GetDevice's 404 drops the region and the documentation URL AWS returns ===
AWS: Smithy: GetDevice GET /device/{deviceArn}; modeled error ResourceNotFoundException, httpError 404. Recorded response: 404 ResourceNotFoundException whose message is the fixed template "Braket device '<deviceArn>' not found in <region>. You can find a list of all supported device ARNs and the regions in which they are available in the documentation: https://docs.aws.amazon.com/braket/latest/developerguide/braket-devices.html" — fixed text plus two variable parts: the deviceArn from the request and the region the request was signed for.
Emulator: 404 ResourceNotFoundException with "Device <deviceArn> not found" (get_device.rs's inline not-found branch) — right status and code, none of AWS's wording, no region, no documentation URL.
Why: Braket device availability is region-scoped, so AWS's message is how a caller learns the device may exist in another region; the emulator's message hides that and breaks message-matching clients.
Fix: the unknown-device branch of GetDevice's handler, which must render the service's not-found template from the requested deviceArn and the request's region.
Traces: braket-search-and-inspect-devices#task#3

=== Cause 6 of 7: DeleteSpendingLimit's malformed-ARN refusal is the generic "parameter is not valid" sentence ===
AWS: Smithy: DeleteSpendingLimit DELETE /spending-limit/{spendingLimitArn}/delete, input spendingLimitArn (SpendingLimitArn, @httpLabel, @required, pattern ^arn:aws[a-z\-]*:braket:[a-z0-9\-]+:[0-9]{12}:spending-limit/.*$); modeled errors ValidationException (400), ResourceNotFoundException (404). Recorded response: 400 ValidationException with the FIXED message "spendingLimitArn is not valid."
Emulator: 400 ValidationException with "spendingLimitArn parameter is not valid." — the shared builder's sentence, which inserts the word "parameter" AWS does not use.
Why: byte-for-byte message assertions and client-side error classification keyed on the service's sentence fail, for a refusal whose status and code are otherwise correct.
Fix: DeleteSpendingLimit's spendingLimitArn validation branch and the shared ARN-refusal wording helper in the crate's state module.
Traces: braket-spending-limit-error-paths#task#2

=== Cause 7 of 7: UpdateSpendingLimit repeats the same mis-worded refusal ===
AWS: Smithy: UpdateSpendingLimit PATCH /spending-limit/{spendingLimitArn}/update, input spendingLimitArn (SpendingLimitArn, @httpLabel, @required, same pattern); modeled errors ValidationException (400), ResourceNotFoundException (404). Recorded response: 400 ValidationException with the FIXED message "spendingLimitArn is not valid."
Emulator: 400 ValidationException with "spendingLimitArn parameter is not valid."
Why: same as Cause 6 — the message is a requirement and this one differs from AWS's for every caller of the update door.
Fix: UpdateSpendingLimit's spendingLimitArn validation branch, sharing the corrected per-operation wording rather than the generic sentence.
Traces: braket-spending-limit-error-paths#task#3

## Evidence
Wire replay of simparity-wire-aws-v1 against the emulator: 8/31 steps passed (25.8%), 10 divergence clusters, all braket, none stub-blocked and none owned by an open issue. The seven clusters below all report matching status and matching __type with a value_mismatch on $.message only.

Cause 1 — braket-hybrid-job-error-paths#task#3, CancelJob, status 400 -> 400:
  $.message: expected 'jobArn not-a-valid-job-arn is not a valid ARN.', got 'jobArn parameter is not valid.'

Cause 2 — braket-quantum-task-error-paths#task#1, CreateQuantumTask, status 400 -> 400:
  $.message: expected 'Device ARN not found. Please use SearchDevices operation to get a list of supported devices.', got 'Device arn:aws:braket:us-west-2::device/qpu/rigetti/<generated-name> not found'

Cause 3 — braket-quantum-task-error-paths#task#2, GetQuantumTask, status 400 -> 400:
  $.message: expected 'quantumTaskArn is invalid.', got 'quantumTaskArn is invalid: expected an Amazon Braket quantum-task ARN of the form arn:aws:braket:<region>:<account>:quantum-task/<id>, got "not-a-valid-quantum-task-arn"'

Cause 4 — braket-quantum-task-error-paths#task#3, CancelQuantumTask, status 400 -> 400:
  $.message: expected 'quantumTaskArn is not valid.', got the same verbose builder output as Cause 3.

Cause 5 — braket-search-and-inspect-devices#task#3, GetDevice, status 404 -> 404:
  $.message: expected "Braket device '<deviceArn>' not found in us-west-2. You can find a list of all supported device ARNs and the regions in which they are available in the documentation: https://docs.aws.amazon.com/braket/latest/developerguide/braket-devices.html", got 'Device <deviceArn> not found'

Cause 6 — braket-spending-limit-error-paths#task#2, DeleteSpendingLimit, status 400 -> 400:
  $.message: expected 'spendingLimitArn is not valid.', got 'spendingLimitArn parameter is not valid.'

Cause 7 — braket-spending-limit-error-paths#task#3, UpdateSpendingLimit, status 400 -> 400:
  $.message: expected 'spendingLimitArn is not valid.', got 'spendingLimitArn parameter is not valid.'

Code sites observed in the checkout: the shared sentence comes from `crate::state::invalid_arn_message(member, resource, value)` (called e.g. from services/service-com-amazonaws-braket-braket/src/ops/create_spending_limit.rs and .../src/ops/get_device.rs), and the device not-found sentences are inline `format!("Device {} not found", ...)` in .../src/ops/get_device.rs and the CreateQuantumTask handler. Generated ARNs/device names and the account id in the quoted strings are per-run values, not requirements; the fixed text and the member names are.

## Steps to Reproduce
Boot the emulator and point a botocore braket client at it in us-west-2, then replay each fixture step:

Cause 1: braket.cancel_job(jobArn='not-a-valid-job-arn') -> expect 400 ValidationException 'jobArn not-a-valid-job-arn is not a valid ARN.'; observe 'jobArn parameter is not valid.' (fixture braket-hybrid-job-error-paths, step task#3).
Cause 2: braket.create_quantum_task(clientToken=<uniq>, deviceArn='arn:aws:braket:us-west-2::device/qpu/rigetti/<never-published>', shots=10, action='{...}', outputS3Bucket=<bucket>, outputS3KeyPrefix='r') -> expect 400 ValidationException 'Device ARN not found. Please use SearchDevices operation to get a list of supported devices.'; observe the interpolated 'Device <arn> not found' (fixture braket-quantum-task-error-paths, step task#1).
Cause 3: braket.get_quantum_task(quantumTaskArn='not-a-valid-quantum-task-arn') -> expect 400 ValidationException 'quantumTaskArn is invalid.' (fixture braket-quantum-task-error-paths, step task#2).
Cause 4: braket.cancel_quantum_task(quantumTaskArn='not-a-valid-quantum-task-arn', clientToken=<uniq>) -> expect 400 ValidationException 'quantumTaskArn is not valid.' — different sentence from Cause 3 (fixture braket-quantum-task-error-paths, step task#3).
Cause 5: braket.get_device(deviceArn='arn:aws:braket:::device/quantum-simulator/amazon/<never-published>') -> expect 404 ResourceNotFoundException "Braket device '<arn>' not found in <region>. You can find a list of all supported device ARNs and the regions in which they are available in the documentation: https://docs.aws.amazon.com/braket/latest/developerguide/braket-devices.html" (fixture braket-search-and-inspect-devices, step task#3).
Cause 6: braket.delete_spending_limit(spendingLimitArn='not-a-valid-spending-limit-arn') -> expect 400 ValidationException 'spendingLimitArn is not valid.' (fixture braket-spending-limit-error-paths, step task#2).
Cause 7: braket.update_spending_limit(spendingLimitArn='not-a-valid-spending-limit-arn', clientToken=<uniq>, spendingLimit='10') -> expect 400 ValidationException 'spendingLimitArn is not valid.' (fixture braket-spending-limit-error-paths, step task#3).

Then re-run the wire replay: all seven steps above must pass.
