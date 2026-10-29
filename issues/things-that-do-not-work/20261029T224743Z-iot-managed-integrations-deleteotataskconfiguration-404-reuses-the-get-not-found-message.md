Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot-managed-integrations.ota-task-configuration-lifecycle
Issue-Shape-Id: com.amazonaws.iotmanagedintegrations#IotManagedIntegrations
Issue-Key: iot-managed-integrations.ota-task-configuration-lifecycle|run#15|AssertionError|assert second_delete_code_msg == 'Failed to delete Ota task configuration, resource does not exist.', second_delete_code_msg
Issue-Commit: 42112e68a35449969ef0708992459a3b7df02e34

# DeleteOtaTaskConfiguration 404 reuses the Get not-found message instead of AWS's delete-specific wording

> _Note: this issue was identified against the code base at commit `42112e68a35449969ef0708992459a3b7df02e34`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteOtaTaskConfiguration

## Description
Cause 1 of 1: DeleteOtaTaskConfiguration's ResourceNotFoundException body carries the GET resource's not-found sentence (which quotes the identifier) instead of the distinct, identifier-free sentence AWS returns for a failed delete. Failing check: iot-managed-integrations.ota-task-configuration-lifecycle.

AWS: Smithy (services/service-com-amazonaws-iotmanagedintegrations-iotmanagedintegrations/smithy-model/com-amazonaws-iotmanagedintegrations-iotmanagedintegrations.json) models DeleteOtaTaskConfiguration as DELETE /ota-task-configurations/{Identifier} -> 204 with errors AccessDeniedException(403), InternalServerException(500), ResourceNotFoundException(404), ThrottlingException(429), ValidationException(400). The model does not state the message text, so for the text this is recorded behavior only: the recording shows that deleting an OTA task configuration id that does not exist (here: the id of a configuration deleted by a preceding successful DeleteOtaTaskConfiguration) answers HTTP 404, Code 'ResourceNotFoundException', Message exactly 'Failed to delete Ota task configuration, resource does not exist.' That whole sentence is FIXED text — it has no variable part, and notably does NOT quote the requested Identifier. AWS words the READ path differently and the emulator already matches it there: GetOtaTaskConfiguration on a missing id returns Code 'ResourceNotFoundException' with Message 'OTA task configuration not found for id: <Identifier>', whose fixed text is 'OTA task configuration not found for id: ' and whose only variable part is the Identifier taken from the request. So the two paths are NOT the same message in AWS.

Emulator: the second (repeated) DeleteOtaTaskConfiguration returns Code 'ResourceNotFoundException' with Message 'OTA task configuration not found for id: <Identifier>' (replay output: "OTA task configuration not found for id: 18e3218cfb0b179200000000" — the id is the per-run generated value, not part of the requirement). The handler builds its 404 from the shared read-path helper crate::state::ota_task_configuration_not_found(&input.identifier) and its own doc comment states the intent explicitly: "The one sentence this resource's 404s carry, read and mutation alike". That collapsing of read and mutation wording is the defect.

Why: a client that branches on, logs, or asserts the refusal text of a delete (idempotent-delete wrappers, retry/cleanup helpers, and test suites that match the documented delete message) sees the read-path sentence instead, and any matcher on 'Failed to delete Ota task configuration, resource does not exist.' never fires. Clients that scrape ids out of error text also get an id where AWS emits none.

Fix: the DeleteOtaTaskConfiguration handler's not-found branch — it must construct a delete-specific ResourceNotFoundException message rather than reuse the shared read-path not-found helper; the read path's wording must stay as it is (it already matches AWS).

Traces: iot-managed-integrations.ota-task-configuration-lifecycle (check asserts the second delete's code and message after a successful delete; the Get assertions in the same check pass). Code citations: services/service-com-amazonaws-iotmanagedintegrations-iotmanagedintegrations/src/ops/delete_ota_task_configuration.rs lines 194-204 (the not-found branch calling crate::state::ota_task_configuration_not_found). Related but NOT covering this: the open issue 20261029T193935Z-iot-managed-integrations-deleteotataskconfiguration-stub.md still claims DeleteOtaTaskConfiguration is a 501 scaffold stub, but the handler is now implemented and the call is served — this divergence is the surviving message defect, not a stub; the open issue 20261029T171839Z-iot-managed-integrations-error-messages-for-not-found-and-invalid-reference.md covers other not-found wording paths and does not name this delete message.

## Evidence
Check iot-managed-integrations.ota-task-configuration-lifecycle, line 42:
  assert second_delete_code_msg == 'Failed to delete Ota task configuration, resource does not exist.'
AssertionError: OTA task configuration not found for id: 18e3218cfb0b179200000000

Expected (AWS, recorded): HTTP 404, Code 'ResourceNotFoundException', Message 'Failed to delete Ota task configuration, resource does not exist.' (entirely fixed text, no Identifier interpolated).
Actual (emulator): HTTP 404, Code 'ResourceNotFoundException', Message 'OTA task configuration not found for id: <Identifier>' — i.e. the GetOtaTaskConfiguration not-found template, with the requested Identifier interpolated.

The preceding assertions in the same check pass: the first DeleteOtaTaskConfiguration succeeds (204) and the following GetOtaTaskConfiguration returns ResourceNotFoundException with exactly 'OTA task configuration not found for id: <Identifier>', so only the delete-path wording diverges.

Source: services/service-com-amazonaws-iotmanagedintegrations-iotmanagedintegrations/src/ops/delete_ota_task_configuration.rs:198-203 returns DeleteOtaTaskConfigurationErrorModeled::ResourceNotFoundException(crate::state::ota_task_configuration_not_found(&input.identifier)).

## Steps to Reproduce
1. Boot the emulator and point a client at iot-managed-integrations (region eu-west-1).
2. CreateOtaTaskConfiguration(Name=<unique>, Description='ota rollout config', PushConfig={RolloutConfig:{MaximumPerMinute:5}, TimeoutConfig:{InProgressTimeoutInMinutes:60}}) and keep TaskConfigurationId.
3. DeleteOtaTaskConfiguration(Identifier=<TaskConfigurationId>) — succeeds with 204.
4. GetOtaTaskConfiguration(Identifier=<TaskConfigurationId>) — observe 404 ResourceNotFoundException 'OTA task configuration not found for id: <Identifier>' (correct).
5. Call DeleteOtaTaskConfiguration(Identifier=<TaskConfigurationId>) again. Observe 404 ResourceNotFoundException whose Message is 'OTA task configuration not found for id: <Identifier>' instead of AWS's 'Failed to delete Ota task configuration, resource does not exist.'
