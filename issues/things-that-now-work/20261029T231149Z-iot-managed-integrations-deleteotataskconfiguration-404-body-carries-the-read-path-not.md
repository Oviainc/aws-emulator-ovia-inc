Issue-Source: trace-sweep-divergence
Issue-Trace: iot-managed-integrations-ota-task-configuration-lifecycle
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iot-managed-integrations-ota-task-configuration-lifecycle
Issue-Shape-Id: com.amazonaws.iotmanagedintegrations#IotManagedIntegrations
Issue-Key: iot-managed-integrations|DeleteOtaTaskConfiguration|4xx->4xx|$.Message
Issue-Commit: 4c956511a30d233b8b8a2d2bfa41b4b7e6ed0d12

# DeleteOtaTaskConfiguration 404 body carries the read-path not-found sentence instead of AWS's delete wording

> _Note: this issue was identified against the code base at commit `4c956511a30d233b8b8a2d2bfa41b4b7e6ed0d12`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteOtaTaskConfiguration

## Description
Cause 1 of 1: DeleteOtaTaskConfiguration's ResourceNotFoundException body reuses the GET resource's not-found sentence (which interpolates the requested Identifier) instead of the distinct, identifier-free sentence AWS returns when a delete finds nothing to delete. The HTTP status (404) and the error code (ResourceNotFoundException) already match; only the Message diverges.

AWS: Smithy (services/service-com-amazonaws-iotmanagedintegrations-iotmanagedintegrations/smithy-model/com-amazonaws-iotmanagedintegrations-iotmanagedintegrations.json) models DeleteOtaTaskConfiguration as DELETE /ota-task-configurations/{Identifier} with success code 204, input member Identifier (@httpLabel, @required, @length min 1 max 64, @pattern ^[a-zA-Z0-9]*$), and errors AccessDeniedException(403), InternalServerException(500), ResourceNotFoundException(404), ThrottlingException(429), ValidationException(400); the operation is @idempotent. The model does not state message text, so for the text this is recorded behavior only: the recording shows a DELETE against an OTA task configuration id that does not exist answering HTTP 404 with error code ResourceNotFoundException and Message exactly 'Failed to delete Ota task configuration, resource does not exist.' That whole sentence is FIXED text — it has no variable part and, in particular, does NOT quote the requested Identifier. AWS words the READ path differently, and the emulator already matches it there: GetOtaTaskConfiguration (GET /ota-task-configurations/{Identifier}, same error set) on a missing id returns ResourceNotFoundException with Message 'OTA task configuration not found for id: <Identifier>', whose fixed text is 'OTA task configuration not found for id: ' and whose only variable part is the Identifier taken from the request. The two paths are therefore NOT the same message in AWS.

Emulator: the delete path answers 404 / ResourceNotFoundException with the read-path template 'OTA task configuration not found for id: <Identifier>' — in this replay 'OTA task configuration not found for id: nonexistentotaconfig00001', where the trailing id is just the Identifier echoed from the request and is not part of the requirement. The fixed text of the message is wrong and an identifier is interpolated where AWS emits none.

Why: a client that branches on, logs, or asserts the refusal text of a delete (idempotent-delete wrappers, cleanup/retry helpers, conformance suites that match the documented delete sentence) never matches 'Failed to delete Ota task configuration, resource does not exist.', so its missing-resource branch does not fire; clients that parse ids out of error text also receive an id AWS does not emit, and cannot distinguish the read refusal from the delete refusal.

Fix: the DeleteOtaTaskConfiguration handler's not-found branch — it must build a delete-specific ResourceNotFoundException message instead of reusing the shared read-path not-found helper. The read path's wording must be left alone; it already matches AWS.

Related but NOT covering this: the open behavioral issue 20261029T224743Z-iot-managed-integrations-deleteotataskconfiguration-404-reuses-the-get-not-found-message.md describes the same wording defect but carries no wire replay key (it is a behavioral-corpus issue with no Issue-Trace/Issue-Step), so this wire fixture step is un-gated by it; the replay report confirms no open issue owns this step. The open issue 20261029T193935Z-iot-managed-integrations-deleteotataskconfiguration-stub.md claims the operation is an unimplemented 501 scaffold, but the call is served here (404 with a body), so this is a surviving message defect, not a stub. The open issue 20261029T171839Z-iot-managed-integrations-error-messages-for-not-found-and-invalid-reference.md covers other not-found wording paths and does not name this delete message.

Traces: iot-managed-integrations-ota-task-configuration-lifecycle#task#3.

## Evidence
Fixture iot-managed-integrations-ota-task-configuration-lifecycle, step task#3 (DeleteOtaTaskConfiguration), verdict mismatch:
  expected_status 404, actual_status 404 (status matches)
  $.Message: value_mismatch (expected 'Failed to delete Ota task configuration, resource does not exist.', got 'OTA task configuration not found for id: nonexistentotaconfig00001')

Expected (AWS, recorded): HTTP 404, error code ResourceNotFoundException, Message 'Failed to delete Ota task configuration, resource does not exist.' — entirely fixed text, no Identifier interpolated.
Actual (emulator): HTTP 404, error code ResourceNotFoundException, Message 'OTA task configuration not found for id: <Identifier>' — the GetOtaTaskConfiguration not-found template with the requested Identifier (here the fixture's 'nonexistentotaconfig00001', a per-run request value, not part of the requirement) spliced in.

Fixed vs variable parts: AWS's delete message has no variable part at all; the emulator's message has fixed text 'OTA task configuration not found for id: ' plus the Identifier from the request path. The read path (GetOtaTaskConfiguration) legitimately uses that second form, and its steps in this corpus do not diverge on Message.

## Steps to Reproduce
1. Boot the emulator and point an iot-managed-integrations client at it (region eu-west-1).
2. Replay fixture iot-managed-integrations-ota-task-configuration-lifecycle, step task#3, or by hand: send DELETE /ota-task-configurations/nonexistentotaconfig00001 (any syntactically valid OtaTaskConfigurationId, matching ^[a-zA-Z0-9]*$ and 1..64 chars, that does not exist — e.g. an id whose configuration was already deleted).
3. Observe HTTP 404 with x-amzn-errortype ResourceNotFoundException and Message 'OTA task configuration not found for id: <Identifier>'.
4. AWS answers the same 404/ResourceNotFoundException but with Message 'Failed to delete Ota task configuration, resource does not exist.' and no Identifier in the text.
5. For contrast, GET /ota-task-configurations/<same id> and confirm the read path's 'OTA task configuration not found for id: <Identifier>' is correct and must not change.
