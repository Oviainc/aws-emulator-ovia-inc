Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: braket.braket-create-spending-limit-idempotency-under-concurrent-retries braket.braket-resource-not-found-error-semantics-consistent
Issue-Shape-Id: com.amazonaws.braket#Braket
Issue-Key: braket.braket-create-spending-limit-idempotency-under-concurrent-retries|run#4|AssertionError|assert err["Message"] == "Spending limits can not be created for simulator devices.", (
Issue-Key: braket.braket-resource-not-found-error-semantics-consistent|run#13|AssertionError|assert _get_device_message == f"Braket device '{fake_device}' not found in {ctx.region}. You can find a list of all supported device ARNs and the regions in whi
Issue-Commit: 02f6d4f25c27220a14c0924662b79788aefc3700

# Braket refusal messages diverge from AWS's wording on CreateSpendingLimit's simulator gate and GetDevice's not-found (2 causes)

> _Note: this issue was identified against the code base at commit `02f6d4f25c27220a14c0924662b79788aefc3700`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSpendingLimit

## Description
TWO net-new root causes in the braket crate, both in the same family: the refusal the emulator serves carries the right HTTP status and the right error code, but the message is the crate's own interpolated prose instead of the sentence AWS words for that error class. An error message is a requirement, so each is a defect. braket is bootstrapped and answering (bootstrap issue 20261021T211836Z-bootstrap-service-com-amazonaws-braket-braket is resolved), so these are fidelity defects, not bootstrap gaps. A fixer must close BOTH causes before submitting.

Related-but-not-covering open issues: 20261022T134201Z-braket-quantum-task-lifecycle-createquantumtask-validation-and-not owns the quantum-task execution state machine, CreateQuantumTask input validation and SearchDevices' filter message — neither cause below. 20261029T170807Z-braket-create-path-admission-decisions-and-the-published owns CreateSpendingLimit's *admission decision* (that the device gate refuses a limit AWS creates) but says nothing about the wording of the simulator refusal, which is Cause 1 here. 20261029T170807Z-braket-client-error-messages-are-the-emulator-s is the wire-corpus sibling of this family and its Cause 5 is the SAME root cause as Cause 2 below, reached here from the behavioral corpus with a different replay key; if that issue's Cause 5 is fixed, Cause 2 goes with it, and Cause 1 is not in that bundle at all.

=== Cause 1 of 2: CreateSpendingLimit's simulator-device refusal is the emulator's own interpolated prose ===
AWS: Smithy (services/service-com-amazonaws-braket-braket/smithy-model/com-amazonaws-braket-braket.json): CreateSpendingLimit POST /spending-limit, input deviceArn (DeviceArn, @required, len 1..256), spendingLimit (@required, pattern ^\d+(\.\d{1,2})?$), clientToken (@idempotencyToken, @required); modeled errors ValidationException (httpError 400), DeviceRetiredException (410), AccessDeniedException (403), ThrottlingException (429). The model does not carry message text — recorded behavior only: spending limits are a QPU-only cost control, and a simulator deviceArn is refused 400 ValidationException with the message "Spending limits can not be created for simulator devices." That sentence is entirely FIXED text: it interpolates nothing from the request — no deviceArn, no device type. (The same recorded corpus shows the retired-QPU class drawing its OWN distinct refusal, DeviceRetiredException with the fixed message "Device is RETIRED. Try another device.", so the two negative classes must stay distinguishable.)
Emulator: status 400 and code ValidationException match, but the message is "Device <deviceArn> is a SIMULATOR and simulators do not support spending limits" — the handler's inline prose, which echoes the request's deviceArn (AWS does not), names the internal device-type enum value, and drops AWS's sentence and its terminating period. Observed value in replay: 'Device arn:aws:braket:::device/quantum-simulator/amazon/sv1 is a SIMULATOR and simulators do not support spending limits' — the ARN in that string is the request's value, not part of the requirement; the requirement is the fixed sentence with nothing interpolated.
Why: a client that classifies cost-control failures by message text (or that asserts the documented sentence) cannot tell the simulator-unsupported class from any other ValidationException, and code that distinguishes "simulator not supported" from "device retired" sees neither service sentence. Message-matching integration tests pass against AWS and fail against the emulator. The divergence is deterministic and reproduces on every one of 8 concurrent racers, so it is not a race artifact.
Fix: the simulator/device-type gate in CreateSpendingLimit's handler (services/service-com-amazonaws-braket-braket/src/ops/create_spending_limit.rs), whose refusal branch must emit the service's fixed sentence rather than an interpolated one; the wording belongs in that operation's validation, not in a shared single-homed prose helper, since neighbouring operations word the same classes differently.
Traces: braket.braket-create-spending-limit-idempotency-under-concurrent-retries

=== Cause 2 of 2: GetDevice's unknown-device 404 drops the region and the documentation pointer AWS returns ===
AWS: Smithy: GetDevice GET /device/{deviceArn}, deviceArn (@httpLabel, @required, len 1..256); modeled error ResourceNotFoundException, httpError 404. Recorded behavior: 404 ResourceNotFoundException whose message is the template "Braket device '<deviceArn>' not found in <region>. You can find a list of all supported device ARNs and the regions in which they are available in the documentation: https://docs.aws.amazon.com/braket/latest/developerguide/braket-devices.html" — fixed text (including the single quotes around the ARN and the documentation URL) plus exactly two variable parts: the deviceArn taken verbatim from the request and the region the request was signed for.
Emulator: status 404 and code ResourceNotFoundException match, but the message is "Device <deviceArn> not found" — an inline not-found sentence with no quoting, no region and no documentation URL.
Why: Braket device availability is region-scoped, and AWS's message is the only place a caller is told that the device may simply live in another region, plus where to look up the supported ARNs. The emulator's message hides both, so a caller that hits a cross-region device ARN gets no recovery instruction, and message-matching clients do not recognise the error.
Fix: the unknown-device branch of GetDevice's handler (services/service-com-amazonaws-braket-braket/src/ops/get_device.rs), which must render the service's not-found template from the requested deviceArn and the request's signing region instead of its own sentence.
Traces: braket.braket-resource-not-found-error-semantics-consistent

## Evidence
Behavioral replay of simparity-behavioral-aws-v1 against the emulator: 5/16 checks passed (31.2%), all 16 in braket. Of the 9 braket failures, 4 are OWNED by the open issue 20261022T134201Z-braket-quantum-task-lifecycle-... and 2 are ALREADY FILED stub blocks (UpdateSpendingLimit, ListTagsForResource); the two below are the surviving net-new causes.

Cause 1 — check braket.braket-create-spending-limit-idempotency-under-concurrent-retries (mismatch). The check fires 8 concurrent CreateSpendingLimit calls sharing one clientToken at deviceArn arn:aws:braket:::device/quantum-simulator/amazon/sv1 with spendingLimit '7.00'. The Code assertion PASSES (ValidationException), so status/code are correct; the Message assertion fails on every racer:
  AssertionError: simulator device: expected the simulator-specific message, got 'Device arn:aws:braket:::device/quantum-simulator/amazon/sv1 is a SIMULATOR and simulators do not support spending limits'
  expected (fixed text, nothing interpolated): 'Spending limits can not be created for simulator devices.'
The check aborted at that first assertion, so its later assertions (the retired-QPU class drawing DeviceRetiredException 'Device is RETIRED. Try another device.', and SearchSpendingLimits returning [] for both devices, i.e. no racer persisting a limit) were never reached and are not reported here as separate findings — they may or may not hold once Cause 1 is fixed.

Cause 2 — check braket.braket-resource-not-found-error-semantics-consistent (mismatch). braket.get_device(deviceArn='arn:aws:braket:::device/quantum-simulator/sim2real/ghost-device-does-not-exist'). The Code assertion PASSES (ResourceNotFoundException, 404); the Message assertion fails:
  AssertionError: expected the exact ResourceNotFoundException message AWS returns, got 'Device arn:aws:braket:::device/quantum-simulator/sim2real/ghost-device-does-not-exist not found'
  expected: "Braket device '<deviceArn>' not found in <region>. You can find a list of all supported device ARNs and the regions in which they are available in the documentation: https://docs.aws.amazon.com/braket/latest/developerguide/braket-devices.html"
The ghost ARN in both quoted strings is the request's own value and the region is the client's region — per-request values that fill the template, not literal requirements. This check also aborted at its first assertion, so its downstream expectations (GetJob/CancelJob 'Job <account>#<name> not found', GetQuantumTask/CancelQuantumTask 'Quantum task not found', DeleteSpendingLimit 'Spending limit <arn> not found', and ListTagsForResource answering the distinct NotFoundException 'Resource was not found') were never exercised and are NOT filed here.

This cause 2 is the behavioral-corpus sighting of the same fault the open wire issue 20261029T170807Z-braket-client-error-messages-are-the-emulator-s records as its Cause 5 (replay key braket|GetDevice|4xx->4xx|$.message); it is filed here because that issue does not carry this check id, so a fixer closing it from the wire corpus alone would never re-replay this check.

## Steps to Reproduce
Boot the emulator and point a botocore braket client at it in us-west-2.

Cause 1 (check braket.braket-create-spending-limit-idempotency-under-concurrent-retries):
  braket.create_spending_limit(clientToken=<uniq>, deviceArn='arn:aws:braket:::device/quantum-simulator/amazon/sv1', spendingLimit='7.00')
  Expect: 400 ValidationException, message exactly 'Spending limits can not be created for simulator devices.' (no part of the request interpolated).
  Observe: 400 ValidationException, message 'Device arn:aws:braket:::device/quantum-simulator/amazon/sv1 is a SIMULATOR and simulators do not support spending limits'.
  Repeat the call 8 times concurrently with one shared clientToken: every response must carry the same code and the same fixed message.

Cause 2 (check braket.braket-resource-not-found-error-semantics-consistent):
  braket.get_device(deviceArn='arn:aws:braket:::device/quantum-simulator/sim2real/ghost-device-does-not-exist')
  Expect: 404 ResourceNotFoundException, message "Braket device '<the deviceArn just passed>' not found in <the client's region>. You can find a list of all supported device ARNs and the regions in which they are available in the documentation: https://docs.aws.amazon.com/braket/latest/developerguide/braket-devices.html"
  Observe: 404 ResourceNotFoundException, message 'Device <deviceArn> not found'.

Then re-replay exactly these two checks: braket.braket-create-spending-limit-idempotency-under-concurrent-retries and braket.braket-resource-not-found-error-semantics-consistent. Note each check continues past the fixed assertion into further not-found/negative-class expectations that this run never reached, so both checks must be run to completion.
