Issue-Source: trace-sweep-divergence
Issue-Trace: ssm-parameter-store-default-tier-round-trip
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ssm-parameter-store-default-tier-round-trip ssm-parameter-store-high-throughput-toggle ssm-service-setting-update-and-reset ssm-service-setting-update-and-reset-lifecycle ssm-toggle-high-throughput-parameter-store-setting ssm-update-and-reset-parameter-tier-setting ssm-managed-instance-activation-tier-toggle ssm-public-sharing-permission-toggle
Issue-Shape-Id: com.amazonaws.ssm#AmazonSSM
Issue-Key: ssm|ResetServiceSetting|2xx->2xx|$.ServiceSetting.LastModifiedUser
Issue-Key: ssm|GetServiceSetting|2xx->2xx|$.ServiceSetting.LastModifiedUser
Issue-Key: ssm|ResetServiceSetting|2xx->2xx|$.ServiceSetting.SettingValue
Issue-Key: ssm|ResetServiceSetting|2xx->2xx|$.ServiceSetting.Status
Issue-Key: ssm|GetServiceSetting|2xx->2xx|$.ServiceSetting.SettingValue
Issue-Key: ssm|GetServiceSetting|2xx->2xx|$.ServiceSetting.Status
Issue-Commit: 2f5222510f5cea04a0a0a06bf57973da91c39155

# SSM service-setting reset and read semantics diverge: LastModifiedUser, activation-tier downgrade, public-sharing default

> _Note: this issue was identified against the code base at commit `2f5222510f5cea04a0a0a06bf57973da91c39155`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ResetServiceSetting

## Description
Three net-new root causes in the ssm account-level service-setting surface (GetServiceSetting / UpdateServiceSetting / ResetServiceSetting). All seven net-new wire divergences of this replay are clustered here; every other ssm divergence in the run is already owned by an open issue. 3 CAUSES — each must be closed before submitting.

Cause 1 of 3 — ResetServiceSetting records the CALLING principal as LastModifiedUser, where AWS reports the provisioning user.
AWS: once a setting has been restored to the value its service team provisioned, the setting is no longer overwritten, so ServiceSetting$LastModifiedUser reports the provisioning user token `System` rather than an IAM principal ARN (Smithy: ServiceSetting$LastModifiedUser -> String, documented "The ARN of the last modified user. This field is populated only if the setting value was overwritten."; ResetServiceSetting "Reset the service setting for the account to the default value as provisioned by the Amazon Web Services service team"). Recorded behavior confirms it: the ResetServiceSetting response, and every later GetServiceSetting of the same setting, carry LastModifiedUser=`System` while Status is `Default`. The value is a fixed token, not a per-run value — only LastModifiedDate varies per call.
Emulator: the reset response and the read after it both carry LastModifiedUser as the emulator's ambient caller ARN (the IAM user ARN of the emulator's published credential) with Status `Default`. The reset writes a modification record whose modifier is the caller instead of reporting the provisioned state, i.e. it treats restoring the default as a customer overwrite.
Why: a client that uses LastModifiedUser to decide whether a setting is still at its provisioned value (the documented meaning of the member — populated only when overwritten) concludes that a reset setting is customer-owned, and audit/drift tooling attributes an AWS-provisioned value to a principal that never set it.
Fix: the ResetServiceSetting handler and the service-setting state it writes — a restore must report the provisioning user for the restored setting rather than the request's principal.
Traces: ssm-parameter-store-default-tier-round-trip#task#5 (ResetServiceSetting, 7-step cluster) and #task#6 (GetServiceSetting, 5-step cluster); the same field also diverges in ssm-public-sharing-permission-toggle#task#7 / #task#8. Emulator source: services/service-com-amazonaws-ssm-amazonssm/src/ops/reset_service_setting.rs lines 188-205 (last_modified_user: crate::state::service_settings::modifying_user()), against services/service-com-amazonaws-ssm-amazonssm/src/state/service_settings.rs lines 49-51 (PROVISIONING_USER = "System", defined but never used on the reset path) and lines 172-180 (modifying_user()).

Cause 2 of 3 — resetting /ssm/managed-instance/activation-tier is applied immediately instead of being held as a pending change.
AWS: a reset of the managed-instance activation tier away from `advanced` is not effective at once — it is a change request awaiting approval, so the ResetServiceSetting response reports the setting still at its customized value `advanced` with Status `PendingUpdate` (Smithy: ServiceSetting$Status -> String, documented values Default | Customized | PendingUpdate, where "PendingUpdate: The current setting uses a default or custom value, but a setting change request is pending approval"; the accepted values of this SettingId, `standard` and `advanced`, come from UpdateServiceSettingRequest$SettingValue). Recorded behavior only for which setting ids take that path: the subsequent GetServiceSetting reports the identical pair (SettingValue `advanced`, Status `PendingUpdate`).
Emulator: ResetServiceSetting answers 200 with SettingValue `standard` and Status `Default`, and the following GetServiceSetting reports the same — the downgrade is committed synchronously and PendingUpdate is never produced by any code path.
Why: a client that resets the activation tier and then reads the setting is told the account is back on the standard tier while AWS still has it on advanced pending approval; the PendingUpdate state the client must poll for, and must not treat as settled, never appears.
Fix: the ResetServiceSetting handler's restore path and the service-setting state's status model — restoring this setting must record the pending change (value unchanged, Status PendingUpdate) instead of overwriting the stored value with the catalogue default.
Traces: ssm-managed-instance-activation-tier-toggle#task#5 (ResetServiceSetting) and #task#6 (GetServiceSetting). Emulator source: services/service-com-amazonaws-ssm-amazonssm/src/ops/reset_service_setting.rs lines 188-209; services/service-com-amazonaws-ssm-amazonssm/src/state/service_settings.rs lines 40-47 (only Default and Customized exist as status constants) and lines 119-123 (the activation-tier catalogue entry).

Cause 3 of 3 — the provisioned value of /ssm/documents/console/public-sharing-permission is wrong, so an unmodified and a freshly reset setting both read back inverted.
AWS: the document console public-sharing-permission setting is provisioned with the value `Enable` (its documented value list is Enable | Disable, from UpdateServiceSettingRequest$SettingValue; Smithy states no default, so the provisioned value is recorded behavior only). A GetServiceSetting of the setting before the account changes anything reports SettingValue `Enable`, and a ResetServiceSetting — plus every read after it — restores and reports `Enable` with Status `Default`.
Emulator: every one of those reads answers 200 with SettingValue `Disable`: the catalogue provisions the opposite value, so the value is inverted both before any modification and after a reset (and the reset response additionally carries the Cause 1 LastModifiedUser defect).
Why: the setting controls whether SSM documents may be shared publicly, so a client that reads it to decide whether public sharing is permitted — or that resets it to return the account to the AWS-provisioned posture — gets the opposite answer from AWS and will either skip a change it must make or report a posture the account does not have.
Fix: the service-setting catalogue in the ssm state (the provisioned value for this SettingId), which both the Get and the Reset handlers resolve through.
Traces: ssm-public-sharing-permission-toggle#task#2 (GetServiceSetting, expected Enable / got Disable), #task#7 (ResetServiceSetting) and #task#8 (GetServiceSetting). Emulator source: services/service-com-amazonaws-ssm-amazonssm/src/state/service_settings.rs lines 114-118 (default_value: "Disable").

Related open issues that do NOT cover these causes: 20261104T121102Z-ssm-10-net-new-root-causes-across-getdocument.md (its cause 7 is the ServiceSettingNotFound message for an unprovisioned SettingId — a refusal path, not these 200-response values), and the stubs 20261103T021956Z-ssm-getservicesetting-stub.md / 20261103T215115Z-ssm-updateservicesetting-stub.md (both operations now answer 200 in this checkout; the divergences above are in the values those implemented handlers serve).

## Evidence
Replay of trace set simparity-wire-aws-v1 against this checkout (emulator up; /__admin/health 200). All seven divergences are status 200 -> 200 value mismatches, so the operations answer but with wrong field values.

Cause 1 — ssm-parameter-store-default-tier-round-trip#task#5 (ResetServiceSetting): $.ServiceSetting.LastModifiedUser expected 'System', got 'arn:aws:iam::000000000000:user/emulator' (cluster of 7: ssm-parameter-store-high-throughput-toggle#task#5, ssm-service-setting-update-and-reset#task#6, ssm-service-setting-update-and-reset-lifecycle#task#6, ssm-service-setting-update-and-reset-lifecycle#teardown#11, ssm-toggle-high-throughput-parameter-store-setting#teardown#7, ssm-update-and-reset-parameter-tier-setting#task#5). ssm-parameter-store-default-tier-round-trip#task#6 (GetServiceSetting): same field, same expectation (cluster of 5: ssm-parameter-store-high-throughput-toggle#task#6, ssm-service-setting-update-and-reset#task#7, ssm-service-setting-update-and-reset-lifecycle#task#7, ssm-update-and-reset-parameter-tier-setting#task#6). Note the account id and user name in the actual ARN are this environment's, not a requirement; the requirement is the fixed token 'System'.

Cause 2 — ssm-managed-instance-activation-tier-toggle#task#5 (ResetServiceSetting): $.ServiceSetting.SettingValue expected 'advanced', got 'standard'; $.ServiceSetting.Status expected 'PendingUpdate', got 'Default'. #task#6 (GetServiceSetting): identical pair of mismatches, so the wrong state is persisted, not just mis-serialized in the reset response.

Cause 3 — ssm-public-sharing-permission-toggle#task#2 (GetServiceSetting): $.ServiceSetting.SettingValue expected 'Enable', got 'Disable'. #task#7 (ResetServiceSetting) and #task#8 (GetServiceSetting): $.ServiceSetting.SettingValue expected 'Enable', got 'Disable' AND $.ServiceSetting.LastModifiedUser expected 'System', got 'arn:aws:iam::000000000000:user/emulator'.

Smithy model consulted: services/service-com-amazonaws-ssm-amazonssm/smithy-model/com-amazonaws-ssm-amazonssm.json — GetServiceSetting/ResetServiceSetting (input SettingId required, @length 1-1000; output ServiceSetting; errors InternalServerError, ServiceSettingNotFound, TooManyUpdates), ServiceSetting members SettingId/SettingValue/LastModifiedDate/LastModifiedUser/ARN/Status, ServiceSetting$Status doc (Default | Customized | PendingUpdate), ServiceSetting$LastModifiedUser doc ("populated only if the setting value was overwritten"), ResetServiceSetting doc ("Reset the service setting for the account to the default value as provisioned by the Amazon Web Services service team").

## Steps to Reproduce
1. Build and boot the emulator from this checkout. 2. Replay simparity-wire-aws-v1 and re-run fixture ssm-parameter-store-default-tier-round-trip step task#5 plus every fixture#step in trace_ids. 3. Per cause: (1) UpdateServiceSetting(SettingId='/ssm/parameter-store/default-parameter-tier', SettingValue='Advanced'), then ResetServiceSetting on the same id and GetServiceSetting after it — read ServiceSetting.LastModifiedUser and Status; AWS reports 'System' / 'Default'. (2) UpdateServiceSetting(SettingId='/ssm/managed-instance/activation-tier', SettingValue='advanced'), then ResetServiceSetting on that id and GetServiceSetting after it — read ServiceSetting.SettingValue and Status; AWS reports 'advanced' / 'PendingUpdate'. (3) On a fresh store, GetServiceSetting(SettingId='/ssm/documents/console/public-sharing-permission'), then ResetServiceSetting on it and GetServiceSetting again — AWS reports SettingValue 'Enable' with Status 'Default' every time.
