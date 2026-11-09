Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: transfer.delete-webapp-customization-conflict-while-in-use
Issue-Shape-Id: com.amazonaws.transfer#TransferService
Issue-Key: transfer.delete-webapp-customization-conflict-while-in-use|run#15|AssertionError|assert conflict_messages <= {
Issue-Commit: 3ef9cf67ecef4fca23b47bd048af294813d54223

# Transfer DeleteWebAppCustomization losing a race against UpdateWebAppCustomization answers ConflictException with emulator-authored prose instead of AWS's 'Unable to delete WebApp customization'

> _Note: this issue was identified against the code base at commit `3ef9cf67ecef4fca23b47bd048af294813d54223`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteWebAppCustomization

## Description
ONE root cause in the transfer emulator, grounded in a failing behavioral check of this suite.

=== Cause 1 of 1: the ConflictException a DeleteWebAppCustomization racer gets, when it overlaps writes of the same web app's customization, carries the emulator's own sentence instead of AWS's fixed message ===

AWS: Smithy (com.amazonaws.transfer#TransferService, services/service-com-amazonaws-transfer-transferservice/smithy-model/com-amazonaws-transfer-transferservice.json) DeleteWebAppCustomization: input WebAppId (@required, @length 24..24, pattern ^webapp-[0-9a-f]{17}$), output Unit, @idempotent, awsJson RPC (POST /deleteWebAppCustomization, dispatched by X-Amz-Target); modeled errors AccessDeniedException 403, ConflictException 409, InternalServiceError 503, InvalidRequestException 400, ResourceNotFoundException 404, ThrottlingException 429. The model does not carry message text, so for the text this is recorded behavior only: when DeleteWebAppCustomization and UpdateWebAppCustomization calls on the SAME web app's single customization overlap, a racer either succeeds or is refused HTTP 409 ConflictException, and the Message for the delete path is the service's fixed text for that condition, 'Unable to delete WebApp customization' (the update path's counterpart being 'Unable to update WebApp customization'). The whole message is FIXED text for the error class: it names the operation's resource kind only and interpolates nothing — no WebAppId, no title, no timestamp, no request id — so there is no variable part to fill from the request or from state.

Emulator: status and code are right (409, ConflictException) but the Message of the delete path's refusal is emulator-authored prose: "Another update of this web app's customization is in progress; wait for it to complete and try again." It is a crate constant, src/state/web_app_customization.rs::CUSTOMIZATION_WRITE_IN_PROGRESS, returned by write_conflict() and raised from src/ops/delete_web_app_customization.rs on Refusal::Conflict. Replay of transfer.delete-webapp-customization-conflict-while-in-use (4 concurrent UpdateWebAppCustomization + 2 concurrent DeleteWebAppCustomization on one web app) collected the ConflictException (kind, message) pairs and saw {('delete', "Another update of this web app's customization is in progress; wait for it to complete and try again.")} where only ('delete', 'Unable to delete WebApp customization') / ('update', 'Unable to update WebApp customization') are legal.

Why: the message is part of the requirement for this error class. A client, assertion or operator runbook that matches AWS's sentence for a lost customization write never matches, and the prose additionally mis-describes the condition — it announces 'another update' for a DELETE that lost the claim, so a caller reading the message concludes an update is in flight when it is a competing delete. The emulator's wording also leaks its own internal claim model into the public API surface.

Fix: the refusal text for the DELETE path of the web-app customization write claim in the transfer crate — the conflict payload produced for Refusal::Conflict in src/state/web_app_customization.rs and raised by src/ops/delete_web_app_customization.rs — must be AWS's fixed text for this condition, and the delete path must be distinguishable from the update path (one shared write_conflict() helper currently serves both operations, so the two paths need their own per-operation text; the update path must keep answering AWS's update wording and not regress).

Related already-open issue: 20261109T050426Z-transfer-web-app-and-agreement-refusals-diverge-deletewebappcustomization.md describes the same constant under its Cause 1, but it carries only the replay keys of transfer.delete-webapp-customization-conflict-blocked and transfer.delete-server-referenced-by-agreement-succeeds-and-orphans-agreement; it does not carry this check's key, so this check (the delete-racing-update variant, which also constrains the update path's wording and the coherence of the surviving customization) must be re-replayed in its own right.

Traces: transfer.delete-webapp-customization-conflict-while-in-use.

## Evidence
Check transfer.delete-webapp-customization-conflict-while-in-use, check_transfer_delete_webapp_customization_conflict_while_in_use.py line 54:

  assert conflict_messages <= {
      ("update", "Unable to update WebApp customization"),
      ("delete", "Unable to delete WebApp customization"),
  }

Expected: every ConflictException raised by a racer among 4x UpdateWebAppCustomization + 2x DeleteWebAppCustomization on one web app's customization carries HTTP 409, code ConflictException, and the fixed message for its path — 'Unable to delete WebApp customization' for a delete, 'Unable to update WebApp customization' for an update (fixed text, no interpolated values).

Actual: AssertionError: unexpected ConflictException message(s): {('delete', "Another update of this web app's customization is in progress; wait for it to complete and try again.")} — HTTP 409 / ConflictException are correct, the Message is not.

Source of the diverging text: services/service-com-amazonaws-transfer-transferservice/src/state/web_app_customization.rs lines 38-47 (const CUSTOMIZATION_WRITE_IN_PROGRESS, fn write_conflict), raised from src/ops/delete_web_app_customization.rs on Refusal::Conflict.

## Steps to Reproduce
1. Boot the emulator. sso-admin.list_instances (or create_instance) to obtain an IAM Identity Center InstanceArn; iam.create_role with a trust policy for transfer.amazonaws.com.
2. transfer.create_web_app(IdentityProviderDetails={'IdentityCenterConfig': {'InstanceArn': <arn>, 'Role': <role arn>}}) -> WebAppId.
3. transfer.update_web_app_customization(WebAppId=<id>, Title='sim2real-initial').
4. From a thread pool, issue concurrently: 4x transfer.update_web_app_customization(WebAppId=<id>, Title='sim2real-inuse-<i>') and 2x transfer.delete_web_app_customization(WebAppId=<id>).
5. Collect the (operation kind, Error.Message) of every call refused with Error.Code == 'ConflictException'.
6. Observe a delete racer refused HTTP 409 ConflictException with Message "Another update of this web app's customization is in progress; wait for it to complete and try again." instead of 'Unable to delete WebApp customization'.
