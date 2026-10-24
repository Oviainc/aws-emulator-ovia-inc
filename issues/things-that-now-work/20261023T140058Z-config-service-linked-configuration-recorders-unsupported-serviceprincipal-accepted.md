Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: config.duplicate-service-linked-recorder-rejected config.put-service-linked-recorder-limit-exceeded-blocked config.put-service-linked-recorder-limit-exceeded-rejected config.service-linked-configuration-recorder-duplicate-conflict-rejected config.server-side-third-party-recorder-registers-managed-resource-types config.delete-service-linked-recorder-while-recording-conflict-blocked config.delete-service-linked-recorder-conflict-while-active-blocked config.server-side-service-linked-recorder-policy-enforced
Issue-Shape-Id: com.amazonaws.configservice#StarlingDoveService
Issue-Key: config.duplicate-service-linked-recorder-rejected|run#9|AssertionError|assert message == (
Issue-Key: config.put-service-linked-recorder-limit-exceeded-blocked|run#9|AssertionError|assert message == _CONFLICT_MESSAGE, f"expected {_CONFLICT_MESSAGE!r}, got {message!r}"
Issue-Key: config.put-service-linked-recorder-limit-exceeded-rejected|run#10|AssertionError|assert message == _CONFLICT_MESSAGE, f"expected {_CONFLICT_MESSAGE!r}, got {message!r}"
Issue-Key: config.service-linked-configuration-recorder-duplicate-conflict-rejected|run#12|AssertionError|assert message == expected_conflict, f"expected {expected_conflict!r}, got {message!r}"
Issue-Key: config.server-side-third-party-recorder-registers-managed-resource-types|run#7|AssertionError|assert code is not None, (
Issue-Key: config.delete-service-linked-recorder-while-recording-conflict-blocked|run#6|AssertionError|assert status.get("servicePrincipal") == _SERVICE_PRINCIPAL, (
Issue-Key: config.delete-service-linked-recorder-conflict-while-active-blocked|run#7|AssertionError|assert "always recording" in message, (
Issue-Key: config.server-side-service-linked-recorder-policy-enforced|run#16|AssertionError|assert message == (
Issue-Commit: a59989a9f63c403b66485a4e93f54ab6df22c2a6

# Config service-linked configuration recorders: unsupported ServicePrincipal accepted, servicePrincipal missing from recorder status, and three refusal messages diverge (5 causes)

> _Note: this issue was identified against the code base at commit `a59989a9f63c403b66485a4e93f54ab6df22c2a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutServiceLinkedConfigurationRecorder

## Description
The service-linked configuration recorder surface answers, but it accepts a service principal AWS refuses, drops a modeled output member, and emits its own text for three refusals whose message AWS fixes. FIVE CAUSES must be closed.

Cause 1 of 5: the duplicate service-linked-recorder conflict message.
AWS: PutServiceLinkedConfigurationRecorder takes a required ServicePrincipal (@length 1..128, pattern ^[\w+=,.@-]+$) and models ConflictException (client/400), whose doc states the case: "you cannot create a service-linked recorder because a service-linked recorder already exists for the specified service" (Smithy: PutServiceLinkedConfigurationRecorder, ConflictException). Recorded behavior: the second create for the same principal is refused 400 ConflictException with the fixed text "A service-linked configuration recorder with the specified service principal already exists." — the same text regardless of which principal was named.
Emulator: same code, but the text interpolates the principal: "A service-linked configuration recorder already exists for the service principal '<principal>'." (observed for both observabilityadmin.amazonaws.com and telemetry-enablement.observabilityadmin.amazonaws.com).
Why: the refusal text is a fixed, principal-independent string on AWS; a client that matches it (the code alone does not separate the conflict legs of this operation) never matches here.
Fix: the ConflictException message emitted by the PutServiceLinkedConfigurationRecorder handler (services/service-com-amazonaws-configservice-starlingdoveservice/src/ops/put_service_linked_configuration_recorder.rs).
Traces: config.duplicate-service-linked-recorder-rejected, config.put-service-linked-recorder-limit-exceeded-blocked, config.put-service-linked-recorder-limit-exceeded-rejected, config.service-linked-configuration-recorder-duplicate-conflict-rejected.

Cause 2 of 5: PutServiceLinkedConfigurationRecorder accepts a ServicePrincipal that is not a supported Config-linked service.
AWS: the operation models ValidationException (client/400) and ValidationException's own doc calls out the service-principal case for the recorder surfaces ("You have provided a service principal for service-linked configuration recorder that is not valid"). Recorded behavior: a well-formed but unsupported/unrecognized service principal is refused 400 before any recorder is created.
Emulator: the call succeeds and a service-linked recorder is created for the unsupported principal.
Why: a client can create recorder state AWS would never allow, and the only signal distinguishing a supported linked service from an arbitrary string is gone — downstream reads then describe a recorder that cannot exist in AWS.
Fix: ServicePrincipal validation against the supported linked-service set in the PutServiceLinkedConfigurationRecorder handler (.../src/ops/put_service_linked_configuration_recorder.rs).
Traces: config.server-side-third-party-recorder-registers-managed-resource-types.

Cause 3 of 5: DescribeConfigurationRecorderStatus omits servicePrincipal for a service-linked recorder.
AWS: ConfigurationRecorderStatus models servicePrincipal (@length 1..128, pattern ^[\w+=,.@-]+$) documented as "For service-linked configuration recorders, the service principal of the linked Amazon Web Services service" (Smithy: ConfigurationRecorderStatus). For a service-linked recorder the member is populated.
Emulator: the status entry carries no servicePrincipal (None).
Why: a client cannot tell a service-linked recorder from a customer-managed one from its status, which is exactly the check that gates whether Stop/Delete is allowed.
Fix: the DescribeConfigurationRecorderStatus serializer/state for service-linked recorders (.../src/ops/describe_configuration_recorder_status.rs and the recorder state module).
Traces: config.delete-service-linked-recorder-while-recording-conflict-blocked.

Cause 4 of 5: the StopConfigurationRecorder refusal for a service-linked recorder does not state why.
AWS: recorded behavior — StopConfigurationRecorder against a service-linked recorder is refused with a message explaining that a service-linked configuration recorder is ALWAYS recording (the text contains "always recording"); that is the property the refusal is about.
Emulator: refuses with "'<name>' is a service-linked configuration recorder; StopConfigurationRecorder does not affect service-linked configuration recorders" — the rule it reports (a no-op) is not the rule AWS reports (the recorder cannot be stopped because it always records).
Why: the client is told the call had no effect rather than that the operation is not permitted for this recorder class, so recovery logic (delete via the service-linked API instead) is not indicated.
Fix: the service-linked branch of the StopConfigurationRecorder handler and its refusal message (.../src/ops/stop_configuration_recorder.rs).
Traces: config.delete-service-linked-recorder-conflict-while-active-blocked.

Cause 5 of 5: the reserved-prefix refusal message of the standard recorder API.
AWS: PutConfigurationRecorder refuses a recorder whose name starts with the reserved prefix with InvalidConfigurationRecorderNameException, and the model states the message rule verbatim: "The configuration recorder name is not valid. The prefix \"AWSConfigurationRecorderFor\" is reserved for service-linked configuration recorders." (Smithy: InvalidConfigurationRecorderNameException doc; client/400). The recorded message is that sentence; the error code already matched in replay.
Emulator: same code, different text: "the prefix 'AWSConfigurationRecorderFor' is reserved for service-linked configuration recorders, so '<name>' is not a valid configuration recorder name".
Why: the message rule is stated by the model, so the emitted text is part of the contract; a client matching the documented sentence does not match.
Fix: the InvalidConfigurationRecorderNameException message in the PutConfigurationRecorder handler's name validation (.../src/ops/put_configuration_recorder.rs).
Traces: config.server-side-service-linked-recorder-policy-enforced.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator built from mainline (emulator serving; 30/179 passed).

Cause 1:
  config.put-service-linked-recorder-limit-exceeded-blocked / -rejected: AssertionError: expected 'A service-linked configuration recorder with the specified service principal already exists.', got "A service-linked configuration recorder already exists for the service principal 'observabilityadmin.amazonaws.com'."
  config.service-linked-configuration-recorder-duplicate-conflict-rejected: same, with principal 'telemetry-enablement.observabilityadmin.amazonaws.com'.
  config.duplicate-service-linked-recorder-rejected: AssertionError carrying the same emulator text.

Cause 2:
  config.server-side-third-party-recorder-registers-managed-resource-types: AssertionError: expected an unsupported ServicePrincipal to be rejected, but PutServiceLinkedConfigurationRecorder succeeded   (expected: HTTP 400 ValidationException, no recorder created)

Cause 3:
  config.delete-service-linked-recorder-while-recording-conflict-blocked: AssertionError: expected servicePrincipal 'observabilityadmin.amazonaws.com' on the recorder status, got None

Cause 4:
  config.delete-service-linked-recorder-conflict-while-active-blocked: AssertionError: expected the rejection to explain that a service-linked recorder is always recording, got "'AWSConfigurationRecorderForObservabilityadmin' is a service-linked configuration recorder; StopConfigurationRecorder does not affect service-linked configuration recorders"

Cause 5:
  config.server-side-service-linked-recorder-policy-enforced: the code assertion (InvalidConfigurationRecorderNameException) passed; the message assertion failed — expected "The configuration recorder name is not valid, the prefix AWSConfigurationRecorderFor is reserved for service-linked configuration recorders.", got "the prefix 'AWSConfigurationRecorderFor' is reserved for service-linked configuration recorders, so 'AWSConfigurationRecorderForObservabilityadmin' is not a valid configuration recorder name".

Model grounding: com-amazonaws-configservice-starlingdoveservice.json — PutServiceLinkedConfigurationRecorder errors ConflictException/InsufficientPermissionsException/LimitExceededException/ValidationException (client, httpError 400 defaulted); ConfigurationRecorderStatus.servicePrincipal documented for service-linked recorders; InvalidConfigurationRecorderNameException doc carries the reserved-prefix sentence.

## Steps to Reproduce
1. git clone the repo, build and boot the emulator from mainline; replay simparity-behavioral-aws-v1 and re-replay the check ids in trace_ids.
2. Cause 1: config put-service-linked-configuration-recorder --service-principal observabilityadmin.amazonaws.com twice; compare the second call's ConflictException message to 'A service-linked configuration recorder with the specified service principal already exists.'
3. Cause 2: config put-service-linked-configuration-recorder --service-principal <well-formed but unsupported principal> — the emulator answers 200 and creates a recorder; AWS answers 400 ValidationException.
4. Cause 3: config describe-configuration-recorder-status --service-principal observabilityadmin.amazonaws.com — the status entry has no servicePrincipal member.
5. Cause 4: config stop-configuration-recorder on the service-linked recorder and read the refusal message.
6. Cause 5: config put-configuration-recorder with name AWSConfigurationRecorderForSomething and compare the InvalidConfigurationRecorderNameException message to the model's documented sentence.
