Issue-Source: trace-sweep-divergence
Issue-Trace: cloudformation-register-type-error
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudformation-register-type-error cloudformation-type-registry-reads
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation|RegisterType|4xx->2xx|$.ErrorResponse
Issue-Key: cloudformation|RegisterType|4xx->2xx|status
Issue-Key: cloudformation|DescribeTypeRegistration|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Commit: 8de6682f074d8bd25eca3d1022f82b7ad69da0d3

# cloudformation extension type-registry refusals diverge: RegisterType accepts a request AWS refuses, and the unknown-registration-token message is reworded (2 causes)

> _Note: this issue was identified against the code base at commit `8de6682f074d8bd25eca3d1022f82b7ad69da0d3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterType

## Description
Two net-new root causes of service `cloudformation`, both in the private-extension (type) registry's refusal surface: a registration request AWS rejects synchronously is accepted with a 200 and a token, and the refusal for a registration token the account holds nothing under is worded in the emulator's own words rather than AWS's. Each cause is independent and must be closed separately.

Related open issues cover OTHER parts of this surface and do NOT cover either cause here: 20261029T214321Z-cloudformation-type-registry-testtype-s-publisher-gate-refuses.md (TestType publisher gate, SetTypeConfiguration pattern text, enum-filter ValidationError text) and 20261102T023139Z-cloudformation-stack-refactor-execution-stackset-import-validation-and.md (type registry LISTING). Neither names these fixture steps.

=== Cause 1 of 2: RegisterType answers 200 with a RegistrationToken for a request AWS refuses with 400 CFNRegistryException ===
AWS: Smithy models RegisterType with exactly one error, `CFNRegistryException` (errorKind client, httpError 400, awsQueryError CFNRegistryException/400); its input members are TypeName (@required, @length 10..204, @pattern ^[A-Za-z0-9]{2,64}::[A-Za-z0-9]{2,64}::[A-Za-z0-9]{2,64}(::MODULE){0,1}$), SchemaHandlerPackage (@required, @length 1..4096), Type (enum RegistryType), LoggingConfig, ExecutionRoleArn, ClientRequestToken; the operation is @idempotent and its success output is a RegistrationToken. For this request the recorded AWS response is a REFUSAL of the call itself: HTTP 400 with the awsQuery error envelope ErrorResponse/Error = {Type: `Sender`, Code: `CFNRegistryException`, Message: <not asserted by the fixture>} plus a RequestId. So AWS decides this registration cannot be accepted at request time and emits no RegistrationToken at all; the registry's error class for every such rejection is CFNRegistryException with Error.Type `Sender` (the class and the status are the requirement here — the fixture does not assert the message, and the RequestId is a per-run value).
Emulator: the same call is answered HTTP 200 with a successful RegisterTypeResponse carrying a RegistrationToken, and no ErrorResponse element is emitted at all (`$.ErrorResponse: missing`). Hypothesis: the handler deliberately classifies this condition as a failure of the asynchronous REGISTRATION rather than of the request — services/service-com-amazonaws-cloudformation-cloudformation/src/ops/register_type.rs documents "Validating that package is part of REGISTERING rather than of accepting the registration, so an object Amazon S3 will not hand over, one that is not a ZIP archive, and an archive carrying no schema of this extension each leave the registration with nothing to register: the request is accepted, and it concludes FAILED with the reason instead of registering a version". Only three conditions are refused up front (reserved namespace, text that addresses no S3 object at all, and the 50-version quota), so every other rejection AWS makes synchronously becomes a 200 plus a later ProgressStatus=FAILED.
Why: a client cannot tell a rejected registration from an accepted one. The SDK returns success, so error handling never runs; the caller stores a RegistrationToken that stands for a registration that was never accepted, and only discovers the failure if it polls DescribeTypeRegistration and inspects ProgressStatus/Description. Retry and error-taxonomy layers that branch on HTTP status or on the CFNRegistryException code see a 2xx and treat the extension as registered, so templates referencing that private type are then deployed against a registry that holds no version of it.
Fix: the RegisterType handler and its request-level validation in the cloudformation service crate (services/service-com-amazonaws-cloudformation-cloudformation/src/ops/register_type.rs) — the conditions AWS rejects at request time must be refused from `handle` as the modelled CFNRegistryException (400, Error.Type Sender) instead of being deferred onto the asynchronous StoredTypeRegistration record.
Traces: cloudformation-register-type-error#task#1.

=== Cause 2 of 2: DescribeTypeRegistration's unknown-token refusal uses the emulator's own wording, not AWS's ===
AWS: Smithy models DescribeTypeRegistration with input RegistrationToken (@required, @length 1..128, @pattern ^[a-zA-Z0-9][-a-zA-Z0-9]*$) and the single error `CFNRegistryException` (client, httpError 400, awsQueryError CFNRegistryException/400). Recorded behavior for a token this account and Region holds no registration request under: HTTP 400, Error.Code `CFNRegistryException`, Error.Message `No registration token matches for account.` — that sentence is entirely FIXED text; it has no variable parts, and in particular AWS does not echo the token from the request back into the message.
Emulator: same status and same error code, but Error.Message is `Extension registration request [<token>] does not exist` — different fixed text, and it interpolates the request's token into the message where AWS interpolates nothing. (The token value in the recording, ffffffff-ffff-ffff-ffff-ffffffffffff, is the fixture's probe value, not part of the requirement.)
Why: the message is the only description of the failure a caller gets, since AWS gives every registry failure the same code (CFNRegistryException) — so clients, CLI output and taxonomy tests distinguish "no such registration token" from any other registry error by the message text alone. The emulator's wording defeats that: a client matching AWS's sentence does not recognise the condition, and tooling that asserts the refusal text fails on a difference that is not a per-run value.
Fix: the DescribeTypeRegistration handler's refusal wording in the cloudformation service crate — `no_such_registration` in services/service-com-amazonaws-cloudformation-cloudformation/src/ops/describe_type_registration.rs (line 223) must emit AWS's fixed sentence with no interpolation rather than the bracketed-id house style used by the service's other "this id resolves to nothing" messages.
Traces: cloudformation-type-registry-reads#task#6.

## Evidence
Cause 1 — fixture cloudformation-register-type-error, step task#1, operation RegisterType [mismatch], 1 affected step:
  status: expected 400, got 200
  $.ErrorResponse: missing (expected {'Error': {'Type': 'Sender', 'Code': 'CFNRegistryException', 'Message': '<shaped-not-asserted>'}, 'RequestId': '<per-run value>'}, got None)
  divergence_key: service=cloudformation|RegisterType|status=400->200|keys=$.ErrorResponse,status
  Smithy: RegisterType errors = CFNRegistryException (errorKind client, httpError 400, awsQueryError CFNRegistryException/400); input TypeName @required @length 10..204 @pattern ^[A-Za-z0-9]{2,64}::[A-Za-z0-9]{2,64}::[A-Za-z0-9]{2,64}(::MODULE){0,1}$, SchemaHandlerPackage @required @length 1..4096; output RegistrationToken @length 1..128. (The recorded RequestId is a per-run value and is not asserted; the fixture asserts the 400 status and the Sender/CFNRegistryException envelope.)

Cause 2 — fixture cloudformation-type-registry-reads, step task#6, operation DescribeTypeRegistration [mismatch], 1 affected step:
  $.ErrorResponse.Error.Message: value_mismatch (expected 'No registration token matches for account.', got 'Extension registration request [ffffffff-ffff-ffff-ffff-ffffffffffff] does not exist')
  divergence_key: service=cloudformation|DescribeTypeRegistration|status=400->400|keys=$.ErrorResponse.Error.Message
  Status (400) and Error.Code (CFNRegistryException) already agree; only the message text diverges. AWS's text is wholly fixed; the emulator's interpolates the request's token (the probe token shown is the fixture's value, not a requirement).
  Smithy: DescribeTypeRegistration input RegistrationToken @required @length 1..128 @pattern ^[a-zA-Z0-9][-a-zA-Z0-9]*$; errors = CFNRegistryException (client, 400).
  Emulator source: services/service-com-amazonaws-cloudformation-cloudformation/src/ops/describe_type_registration.rs lines 182-187 and 223-225.

## Steps to Reproduce
Cause 1:
1. Replay fixture cloudformation-register-type-error, step task#1 (a RegisterType call the recording shows AWS refusing).
2. Observe HTTP 200 with a RegisterTypeResponse/RegistrationToken and no ErrorResponse element.
3. AWS answers HTTP 400 with ErrorResponse/Error Type=Sender, Code=CFNRegistryException and no RegistrationToken.
4. Control that must keep working: a RegisterType request AWS DOES accept must still answer 200 with a RegistrationToken, and the registry's three already-enforced request-level refusals (reserved namespace, SchemaHandlerPackage that addresses no S3 object, 50-version quota) must keep answering 400 CFNRegistryException with their current messages.

Cause 2:
1. Call DescribeTypeRegistration with a RegistrationToken this account and Region holds no registration request under (e.g. the fixture's ffffffff-ffff-ffff-ffff-ffffffffffff), as in cloudformation-type-registry-reads step task#6.
2. Observe 400 CFNRegistryException with Error.Message 'Extension registration request [<token>] does not exist'.
3. AWS answers 400 CFNRegistryException with the fixed message 'No registration token matches for account.' — no token interpolated.
4. Control: DescribeTypeRegistration for a token that DOES exist must keep reporting ProgressStatus, and Description only for FAILED, TypeArn/TypeVersionArn only for COMPLETE.

Re-replay to confirm: cloudformation-register-type-error#task#1, cloudformation-type-registry-reads#task#6.
