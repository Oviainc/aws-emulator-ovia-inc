Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: config.concurrent-put-connector-name-conflict config.concurrent-put-connector-name-race-single-winner config.connector-configuration-payload-round-trip config.connector-lifecycle-create-active-delete config.connector-lifecycle-create-get-delete-404 config.connector-listing-reflects-created-and-deleted-connectors config.connector-put-nonexistent-azure-tenant-rejected config.duplicate-connector-configuration-rejected config.e2e-connector-write-read-delete-flow config.list-connectors-pagination-completeness config.put-connector-duplicate-configuration-conflict-rejected config.put-connector-duplicate-name-conflict-rejected config.put-connector-max-number-exceeded-blocked config.delete-connector-nonexistent-rejected config.e2e-connector-malformed-arn-rejected config.connector-missing-provider-config-rejected config.concurrent-put-connector-name-race
Issue-Shape-Id: com.amazonaws.configservice#StarlingDoveService
Issue-Key: config.concurrent-put-connector-name-conflict|run#13|AssertionError|assert not created_arns, (
Issue-Key: config.concurrent-put-connector-name-race-single-winner|run#10|AssertionError|assert not winners, (
Issue-Key: config.connector-configuration-payload-round-trip|_error_code#3|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: config.connector-lifecycle-create-active-delete|run#4|AssertionError|raise AssertionError(
Issue-Key: config.connector-lifecycle-create-get-delete-404|run#5|AssertionError|raise AssertionError(
Issue-Key: config.connector-listing-reflects-created-and-deleted-connectors|run#8|AssertionError|raise AssertionError(
Issue-Key: config.connector-put-nonexistent-azure-tenant-rejected|run#8|AssertionError|raise AssertionError(
Issue-Key: config.duplicate-connector-configuration-rejected|_error_code_and_message#4|AssertionError|raise AssertionError("expected PutConnector to raise on an unreachable Azure tenant, but it succeeded")
Issue-Key: config.e2e-connector-write-read-delete-flow|_error_code_and_message#5|AssertionError|raise AssertionError(f"expected PutConnector to be rejected, got a response: {result!r}")
Issue-Key: config.list-connectors-pagination-completeness|run#5|AssertionError|raise AssertionError(
Issue-Key: config.put-connector-duplicate-configuration-conflict-rejected|_put_connector_error#3|AssertionError|raise AssertionError(
Issue-Key: config.put-connector-duplicate-name-conflict-rejected|_error#4|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Key: config.put-connector-max-number-exceeded-blocked|run#5|AssertionError|raise AssertionError(
Issue-Key: config.delete-connector-nonexistent-rejected|run#8|AssertionError|assert code == "ValidationException", f"expected a rejection, got {code}"
Issue-Key: config.e2e-connector-malformed-arn-rejected|run#9|AssertionError|assert message == _INVALID_ARN, f"expected the exact invalid-ARN message from Get, got {message!r}"
Issue-Key: config.connector-missing-provider-config-rejected|run#6|AssertionError|assert empty_message == "ConnectorConfiguration must contain at least one provider", (
Issue-Key: config.concurrent-put-connector-name-race|run#5|AssertionError|assert structural_message == "ConnectorConfiguration must contain at least one provider", (
Issue-Commit: a59989a9f63c403b66485a4e93f54ab6df22c2a6

# Config connector surface: PutConnector accepts unverifiable/malformed Azure credentials, and connector-ARN refusals use the wrong error class and message (3 causes)

> _Note: this issue was identified against the code base at commit `a59989a9f63c403b66485a4e93f54ab6df22c2a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutConnector

## Description
PutConnector is now implemented (it answers 200 with a connector Arn) but performs none of the input/credential validation AWS performs, and the connector read/delete path misclassifies its refusals. THREE CAUSES must be closed.

Note: the open issue 20261022T190340Z-config-ten-operations-answer-notimplemented-instead-of-their.md describes PutConnector as answering NotImplemented (its Cause 3). That is no longer the observed behavior — the operation now answers 200 — so these three causes are net-new and are not closed by that issue's description.

Cause 1 of 3: PutConnector creates a connector from an Azure provider configuration AWS refuses.
AWS: PutConnector takes a required ConnectorConfiguration and returns the connector Arn; it models ConflictException, InsufficientPermissionsException, MaxNumberOfConnectorsExceededException and ValidationException, all errorKind client / httpError 400 (Smithy: PutConnector; ConnectorConfiguration -> AzureConnectorConfiguration with required tenantIdentifier and clientIdentifier, @length 1..128). Recorded behavior: before materializing anything, AWS validates the identifier form (a GUID) and verifies the tenant/client pair against Azure AD, and refuses an unverifiable or non-GUID pair with 400 ValidationException (recorded messages, examples: "Invalid tenant identifier", "Unable to verify Azure credentials for the connector"). No connector exists afterwards and it does not appear in ListConnectors.
Emulator: every such PutConnector succeeds with HTTP 200 and returns an Arn (e.g. arn:aws:config:<region>:<account>:connector/azure-<uuid>), including for a tenantIdentifier/clientIdentifier pair that is not even GUID-shaped (one recorded Arn embedded the literal "azure-not-a-guid " plus a space), so the connector is created and listed.
Why: a client gets a connector that exists in Config but has no working credential behind it; the validation boundary AWS enforces (and every test that depends on it: duplicate-name conflict, duplicate-configuration conflict, max-connectors limit, concurrent-create races) never triggers, so a client cannot distinguish a good credential from a bad one and the ConflictException / MaxNumberOfConnectorsExceededException paths are unreachable.
Fix: the PutConnector handler's ConnectorConfiguration validation and the provider-credential verification step it must run before creating connector state (services/service-com-amazonaws-configservice-starlingdoveservice/src/ops/put_connector.rs).
Traces: config.concurrent-put-connector-name-conflict, config.concurrent-put-connector-name-race-single-winner, config.connector-configuration-payload-round-trip, config.connector-lifecycle-create-active-delete, config.connector-lifecycle-create-get-delete-404, config.connector-listing-reflects-created-and-deleted-connectors, config.connector-put-nonexistent-azure-tenant-rejected, config.duplicate-connector-configuration-rejected, config.e2e-connector-write-read-delete-flow, config.list-connectors-pagination-completeness, config.put-connector-duplicate-configuration-conflict-rejected, config.put-connector-duplicate-name-conflict-rejected, config.put-connector-max-number-exceeded-blocked.

Cause 2 of 3: GetConnector/DeleteConnector answer the wrong error class (and message) for a connector ARN that does not resolve.
AWS: both operations take a required Arn and model ResourceNotFoundException and ValidationException, both client/400 (Smithy: GetConnector, DeleteConnector). Recorded behavior: for BOTH a syntactically malformed ARN and a well-formed ARN that was never created, AWS answers 400 ValidationException with one fixed text, "The connector ARN provided is not valid.", and the connector inventory is unchanged.
Emulator: for a well-formed, never-created ARN it answers ResourceNotFoundException instead of ValidationException; for a malformed ARN it answers its own descriptive text ("The Arn 'not-a-valid-connector-arn' provided in the request is not a valid connector ARN. Please provide an ARN of the form arn:aws:config:<region>:<account>:connector/<name>.").
Why: the error CODE is part of the contract — a client branching on ValidationException vs ResourceNotFoundException takes the wrong branch, and because AWS collapses both ARN failures into one class, code written against AWS cannot tell them apart here either.
Fix: the Arn validation/lookup in the GetConnector and DeleteConnector handlers — one refusal class and one message for both malformed and unresolvable connector ARNs (.../src/ops/get_connector.rs, .../src/ops/delete_connector.rs).
Traces: config.delete-connector-nonexistent-rejected, config.e2e-connector-malformed-arn-rejected.

Cause 3 of 3: the structural refusal for a ConnectorConfiguration with no provider carries the wrong message.
AWS: ConnectorConfiguration's only member is `azure` and its doc states "You must specify exactly one provider configuration" (Smithy: ConnectorConfiguration). Recorded behavior: a PutConnector whose ConnectorConfiguration carries no provider is refused 400 ValidationException with the fixed text "ConnectorConfiguration must contain at least one provider". The error code is the same for the other PutConnector refusal legs, so the text is the only thing that identifies this leg.
Emulator: same code, but the text is "ConnectorConfiguration must specify exactly one provider configuration. The supported provider is 'AZURE'."
Why: the empty-payload leg is indistinguishable from the other ValidationException legs for a client (or an operator reading logs) that keys on AWS's fixed text.
Fix: the message emitted by PutConnector's structural ConnectorConfiguration check (.../src/ops/put_connector.rs validation).
Traces: config.connector-missing-provider-config-rejected, config.concurrent-put-connector-name-race.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator built from mainline (emulator serving; 30/179 checks passed; bootstrap_signal false).

Cause 1 — representative replay output:
  config.e2e-connector-write-read-delete-flow: AssertionError: expected PutConnector to be rejected, got a response: {'Arn': 'arn:aws:config:us-west-2:000000000000:connector/azure-84f07f07-93b2-4349-9c4b-b9895fab9e4a', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}
  config.connector-configuration-payload-round-trip: AssertionError: expected the call to be rejected, but it SUCCEEDED and returned {'Arn': 'arn:aws:config:us-west-2:000000000000:connector/azure-not-a-guid d81757398f894f6590b04b1fa0c06c86', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}
  config.put-connector-max-number-exceeded-blocked: AssertionError: PutConnector(azure) accepted a non-GUID tenantIdentifier/clientIdentifier pair
  config.connector-put-nonexistent-azure-tenant-rejected: AssertionError: PutConnector accepted a fabricated, non-existent Azure tenant '<guid>' without live credential verification
  config.concurrent-put-connector-name-conflict: AssertionError: expected both concurrent PutConnector calls with unverifiable Azure credentials to fail, but 1 succeeded: ['arn:...:connector/azure-<uuid>']
  (expected: HTTP 400, Error.Code ValidationException, and no connector in ListConnectors)

Cause 2:
  config.delete-connector-nonexistent-rejected: AssertionError: expected a rejection, got ResourceNotFoundException   (at `assert code == "ValidationException"`, Arn=arn:aws:config:<region>:<account>:connector/sim2real-does-not-exist-<uuid>; expected message "The connector ARN provided is not valid.")
  config.e2e-connector-malformed-arn-rejected: AssertionError: expected the exact invalid-ARN message from Get, got "The Arn 'not-a-valid-connector-arn' provided in the request is not a valid connector ARN. Please provide an ARN of the form arn:aws:config:<region>:<account>:connector/<name>."

Cause 3:
  config.connector-missing-provider-config-rejected: AssertionError: expected the missing-provider message, got "ConnectorConfiguration must specify exactly one provider configuration. The supported provider is 'AZURE'."
  config.concurrent-put-connector-name-race: AssertionError (at `assert structural_message == "ConnectorConfiguration must contain at least one provider"`) with the same emulator text.

Model grounding: services/service-com-amazonaws-configservice-starlingdoveservice/smithy-model/com-amazonaws-configservice-starlingdoveservice.json — PutConnector errors ConflictException/InsufficientPermissionsException/MaxNumberOfConnectorsExceededException/ValidationException and GetConnector/DeleteConnector errors ResourceNotFoundException/ValidationException are all errorKind client with httpError 400 (defaulted); AzureConnectorConfiguration.tenantIdentifier and .clientIdentifier are required, @length 1..128.

## Steps to Reproduce
1. git clone the repo, build and boot the emulator from mainline.
2. Replay simparity-behavioral-aws-v1 and re-replay the check ids in trace_ids.
3. Cause 1: call config put-connector with ConnectorConfiguration={"azure":{"tenantIdentifier":"not-a-guid","clientIdentifier":"not-a-guid"}} — the emulator answers 200 with an Arn and the connector appears in ListConnectors; AWS answers 400 ValidationException and creates nothing. Repeat with a freshly generated (fabricated) GUID pair: same divergence.
4. Cause 2: call config get-connector / delete-connector with Arn=arn:aws:config:<region>:<account>:connector/does-not-exist-<uuid> — the emulator answers ResourceNotFoundException; AWS answers ValidationException with message "The connector ARN provided is not valid." Repeat with Arn="not-a-valid-connector-arn" — same class, same message on AWS.
5. Cause 3: call config put-connector with ConnectorConfiguration={} — compare the ValidationException message to "ConnectorConfiguration must contain at least one provider".
