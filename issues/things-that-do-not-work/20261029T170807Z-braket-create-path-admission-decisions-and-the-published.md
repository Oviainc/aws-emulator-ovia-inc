Issue-Source: trace-sweep-divergence
Issue-Trace: braket-hybrid-job-lifecycle
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: braket-hybrid-job-lifecycle braket-spending-limit-lifecycle braket-search-and-inspect-devices
Issue-Shape-Id: com.amazonaws.braket#Braket
Issue-Key: braket|CreateJob|4xx->2xx|$.__type
Issue-Key: braket|CreateJob|4xx->2xx|$.message
Issue-Key: braket|CreateJob|4xx->2xx|status
Issue-Key: braket|CreateSpendingLimit|2xx->4xx|$.spendingLimitArn
Issue-Key: braket|CreateSpendingLimit|2xx->4xx|status
Issue-Key: braket|GetDevice|2xx->2xx|$.deviceCapabilities
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# Braket create-path admission decisions and the published device-capabilities document diverge (3 causes)

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateJob

## Description
Three net-new wire divergences in the braket crate share ONE fault: the emulator's create/read doors do not model the admission decisions and the published device data the real service answers with, so a request AWS refuses is accepted, a request AWS accepts is refused, and the device document served to clients is not the one AWS publishes. Unlike the message-wording bundle filed alongside this one, each of these three diverges in STATUS or in a response member, so clients break outright. braket is bootstrapped and answering, so these are fidelity defects, not bootstrap gaps. A fixer must close all THREE before submitting. (The open braket issue 20261022T134201Z-braket-quantum-task-lifecycle-createquantumtask-validation-and-not is behavioral and covers the quantum-task execution state machine, CreateQuantumTask's output-bucket/action/shots validation, GetJob's scaffold and SearchDevices' filter message — not the three causes below. The resolved issues 20261022T061137Z-braket-getdevice-devicecapabilities-omits-the-published-device-schema and 20261022T134201Z-braket-createspendinglimit-stub touch the same surfaces but are closed, and a resolved issue never covers a live failure: Cause 3 is still diverging and Cause 2 is a NEW refusal on a now-implemented operation.)

=== Cause 1 of 3: CreateJob never resolves roleArn, so a job whose IAM role Braket cannot assume is accepted 201 ===
AWS: Smithy: CreateJob POST /job, httpResponseCode 201, input members clientToken (@idempotencyToken, @required), algorithmSpecification/outputDataConfig/jobName/roleArn/instanceConfig/deviceConfig all @required, roleArn (RoleArn, pattern ^arn:aws[a-z\-]*:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$); modeled errors include ValidationException (httpError 400). AWS resolves the named execution role BEFORE creating the job: a syntactically valid roleArn that Braket cannot assume (the role does not exist, or its trust policy does not admit braket.amazonaws.com) is refused 400 ValidationException with the message template "Braket cannot assume the IAM role: '<roleArn>'" — fixed text plus one variable part, the roleArn taken verbatim from the request.
Emulator: the request is accepted with HTTP 201 and a jobArn; no ValidationException is raised and the response carries neither $.message nor $.__type. The role is never resolved against IAM at all.
Why: a hybrid-job client's whole failure path for a mis-configured execution role disappears — the caller believes the job was created, then polls a job that can never run, instead of being told at submit time which role to fix. Any workflow that asserts the refusal (role bootstrapping, trust-policy checks, error surfacing) silently passes against the emulator and fails against AWS.
Fix: CreateJob's handler, in its pre-create validation, must resolve roleArn against the account's IAM roles (existence plus a trust relationship admitting the Braket service principal) and refuse with the modeled ValidationException before any job record is stored.
Traces: braket-hybrid-job-lifecycle#task#1

=== Cause 2 of 3: CreateSpendingLimit refuses a spending limit AWS creates, so the whole spending-limit lifecycle is unreachable ===
AWS: Smithy: CreateSpendingLimit POST /spending-limit, input clientToken (@idempotencyToken, @required), deviceArn (DeviceArn, @required), spendingLimit (String, @required, pattern ^\\d+(\\.\\d{1,2})?$), optional timePeriod/tags; output spendingLimitArn (SpendingLimitArn, @required, pattern ^arn:aws[a-z\-]*:braket:[a-z0-9\-]+:[0-9]{12}:spending-limit/.*$); modeled errors ValidationException (400), DeviceRetiredException (410). Recorded behavior: the create SUCCEEDS — status 200 on the wire (the model declares httpResponseCode 201, so the RECORDING is the authority for the status a fixer must serve) with a spendingLimitArn in the body, which the rest of the fixture then reads, updates and deletes.
Emulator: the create is refused with HTTP 400 and no spendingLimitArn, so every later step of the lifecycle has no ARN to work with. In the checkout, create_spending_limit.rs refuses with ValidationException before storing anything in four places — a deviceArn that is not a Braket device ARN, a device absent from the emulator's own published inventory ("Device <arn> not found"), a SIMULATOR device, and an inverted timePeriod — so the emulator is applying its own admission rule to a request the service admits; the fixture's device is one AWS accepts a limit for.
Why: the spending-limit feature is unusable end to end: a client cannot create a limit, so it can never update, search or delete one, and cost-control code paths cannot be exercised at all.
Fix: CreateSpendingLimit's handler and the crate's device inventory it consults — the device-resolution and device-type gates must admit the devices AWS admits a spending limit for, and the success path must answer with the recorded status and the minted spendingLimitArn.
Traces: braket-spending-limit-lifecycle#task#1

=== Cause 3 of 3: GetDevice serves a deviceCapabilities document that is not the device's published device-schema document ===
AWS: Smithy: GetDevice GET /device/{deviceArn}, output member deviceCapabilities (JsonValue, @required, mediaType application/json). The value is the device's published Braket device-schema document: a `service` block (braketSchemaHeader name braket.device_schema.device_service_properties, executionWindows, shotsRange, deviceCost, deviceDocumentation, updatedAt), an `action` block keyed by the action type the device accepts (e.g. braket.ir.openqasm.program, with version, actionType, supportedOperations, supportedPragmas, forbiddenPragmas and the array/operation limits) and the device's paradigm/provider properties. Clients (including the Amazon Braket SDK) parse this document to decide which operations, pragmas and shot counts a device accepts.
Emulator: GetDevice answers 200 with the stored inventory value (get_device.rs returns `device.device_capabilities` straight from the crate's seeded device records), and that document does not match the published one — the replay reports $.deviceCapabilities as a value_mismatch against the expected service/action schema document for the simulator under test.
Why: an SDK that reads deviceCapabilities to validate a program before submitting it (supported operations, allowed pragmas, shotsRange bounds, execution windows, cost) cannot do so, and any code that keys off braketSchemaHeader finds no schema it recognises. Cause 2 of the open behavioral braket issue also depends on shotsRange coming from this document.
Fix: the crate's device inventory in its state module (the seeded device records' capabilities documents), which must carry each published device's full device-schema document; GetDevice/SearchDevices then serve it unchanged. The expected document's concrete numbers, URLs and updatedAt in one recording are example values of a per-device published document, not a literal requirement — the requirement is the schema'd structure and each device's own properties.
Traces: braket-search-and-inspect-devices#task#2

## Evidence
Wire replay of simparity-wire-aws-v1 against the emulator: 8/31 steps passed (25.8%), 10 divergence clusters, all braket, none stub-blocked and none owned by an open issue.

Cause 1 — braket-hybrid-job-lifecycle#task#1, CreateJob, status expected 400, got 201:
  $.message: missing (expected "Braket cannot assume the IAM role: '<roleArn>'", got None)
  $.__type: missing (expected 'ValidationException', got None)
  The recorded roleArn (arn:aws:iam::000000000000:role/<generated-role-name>) and the account id are per-run values; the requirement is the refusal, its ValidationException code, the 400 status and the message template with the request's roleArn interpolated.

Cause 2 — braket-spending-limit-lifecycle#task#1, CreateSpendingLimit, status expected 200, got 400:
  $.spendingLimitArn: missing (expected the minted spending-limit ARN, got None)
  The emulator's 400 comes from create_spending_limit.rs's pre-store ValidationException branches (services/service-com-amazonaws-braket-braket/src/ops/create_spending_limit.rs: malformed deviceArn, 'Device {} not found' when the ARN is absent from `guard.devices`, the SIMULATOR refusal, and the inverted-timePeriod refusal). Note the handler's SUCCESS_STATUS is 201 while the recording shows 200.

Cause 3 — braket-search-and-inspect-devices#task#2, GetDevice, status 200 -> 200:
  $.deviceCapabilities: value_mismatch. Expected a device-schema document carrying {"service": {"braketSchemaHeader": {"name": "braket.device_schema.device_service_properties", "version": "1"}, "executionWindows": [...], "shotsRange": [0, 100000], "deviceCost": {...}, "deviceDocumentation": {...}, "updatedAt": ...}, "action": {"braket.ir.openqasm.program": {"version": [...], "actionType": ..., "supportedOperations": [...], "supportedPragmas": [...], "forbiddenPragmas": [...], "maximumQubitArrays": 1, "maximumClassicalArrays": 1, ...}}, ...}; the emulator's stored capabilities value does not carry it. GetDevice returns `device.device_capabilities` verbatim (services/service-com-amazonaws-braket-braket/src/ops/get_device.rs), so the gap is in the seeded inventory, not the handler.

## Steps to Reproduce
Boot the emulator and point a botocore braket client at it in us-west-2.

Cause 1: create an S3 output bucket, then braket.create_job(clientToken=<uniq>, jobName=<uniq>, roleArn='arn:aws:iam::<account>:role/<role-that-was-never-created>', algorithmSpecification={'scriptModeConfig': {...}}, outputDataConfig={'s3Path': 's3://<bucket>/jobs'}, instanceConfig={'instanceType': 'ml.m5.large', 'volumeSizeInGb': 30}, deviceConfig={'device': 'arn:aws:braket:::device/quantum-simulator/amazon/sv1'}) -> expect 400 ValidationException "Braket cannot assume the IAM role: '<roleArn>'"; observe HTTP 201 with a jobArn (fixture braket-hybrid-job-lifecycle, step task#1).

Cause 2: braket.create_spending_limit(clientToken=<uniq>, deviceArn=<the QPU device ARN the fixture uses>, spendingLimit='100.00') -> expect the recorded success (status 200, body carrying spendingLimitArn) and then update/search/delete it; observe HTTP 400 with no spendingLimitArn, so the rest of the lifecycle cannot run (fixture braket-spending-limit-lifecycle, step task#1).

Cause 3: braket.get_device(deviceArn='arn:aws:braket:::device/quantum-simulator/amazon/sv1') and json.loads(resp['deviceCapabilities']) -> expect the published device-schema document (service block with braketSchemaHeader braket.device_schema.device_service_properties, executionWindows, shotsRange, deviceCost, deviceDocumentation; action block keyed by braket.ir.openqasm.program with supportedOperations/supportedPragmas/forbiddenPragmas); observe a document that does not match it (fixture braket-search-and-inspect-devices, step task#2).

Then re-run the wire replay: all three steps above must pass.
