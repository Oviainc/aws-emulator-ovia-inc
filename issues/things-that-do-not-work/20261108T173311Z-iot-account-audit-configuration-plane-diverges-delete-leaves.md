Issue-Source: trace-sweep-divergence
Issue-Trace: iot-account-audit-config-negative-paths
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iot-account-audit-config-negative-paths iot-account-audit-configuration-delete iot-account-audit-configuration-lifecycle iot-delete-account-audit-configuration iot-delete-account-audit-configuration-when-unset iot-event-configuration-and-endpoint-settings
Issue-Shape-Id: com.amazonaws.iot#AWSIotService
Issue-Key: iot|UpdateAccountAuditConfiguration|4xx->4xx|$.message
Issue-Key: iot|DescribeAccountAuditConfiguration|2xx->2xx|$.auditCheckConfigurations.DEVICE_CERTIFICATE_EXPIRING_CHECK.configuration
Issue-Key: iot|DescribeAccountAuditConfiguration|2xx->2xx|$.auditNotificationTargetConfigurations
Issue-Key: iot|DescribeAccountAuditConfiguration|2xx->2xx|$.auditCheckConfigurations.DEVICE_CERTIFICATE_EXPIRING_CHECK.enabled
Issue-Key: iot|DescribeAccountAuditConfiguration|2xx->2xx|$.roleArn
Issue-Key: iot|DescribeAccountAuditConfiguration|2xx->2xx|$.auditCheckConfigurations.LOGGING_DISABLED_CHECK.enabled
Issue-Commit: 67c908b9b54a08081e5adb412dff79abde54b844

# iot account-audit-configuration plane diverges: delete leaves state, phantom SNS notification target, dropped check configuration, emulator-authored refusal

> _Note: this issue was identified against the code base at commit `67c908b9b54a08081e5adb412dff79abde54b844`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAccountAuditConfiguration

## Description
The emulator's IoT Device Defender account-audit-configuration plane (PATCH/GET/DELETE /audit/configuration) diverges from AWS in five distinct ways: its write-path refusal is emulator-authored, its read projection both drops a modelled member and invents one, and DeleteAccountAuditConfiguration does not actually reset account audit state. This issue bundles 5 CAUSES; all five must be closed.

No existing open issue covers this plane: the other iot replay clusters are owned by the registry/authorizer, certificate, jobs-and-commands, policy-document, read-projection, not-found and refusal-envelope issues, none of which touches /audit/configuration.

=== Cause 1 of 5: UpdateAccountAuditConfiguration refuses a first call without roleArn using an emulator-authored sentence ===
AWS: Smithy: UpdateAccountAuditConfiguration, http PATCH /audit/configuration, input member roleArn -> RoleArn @length min 20 max 2048; modelled error InvalidRequestException (httpError 400). The recorded response is HTTP 400 with body member `message` = "Missing required configuration argument: RoleArn." That whole sentence is FIXED text for this error class, including the capitalised member spelling `RoleArn` and the trailing period; it has no slots filled from the request or from state (the only variable thing about it in general is WHICH configuration argument is named, and here that is the absent input member).
Emulator: HTTP 400 (status matches) but `message` = "On the first call to UpdateAccountAuditConfiguration, roleArn is required" — a sentence AWS never emits, with a different member spelling and no terminal period.
Why: a client (or a test/runbook) that branches on, logs, or surfaces the InvalidRequestException text for a missing audit-configuration argument never matches AWS's wording, so first-call provisioning failures are misreported and any message-based retry/repair logic does not fire.
Fix: the UpdateAccountAuditConfiguration handler's request validation — the message it attaches to the InvalidRequestException it raises for a missing required configuration argument.
Traces: iot-account-audit-config-negative-paths#task#3.

=== Cause 2 of 5: DescribeAccountAuditConfiguration drops AuditCheckConfiguration.configuration and invents auditNotificationTargetConfigurations ===
AWS: Smithy: DescribeAccountAuditConfiguration, http GET /audit/configuration, output members roleArn, auditCheckConfigurations -> map<AuditCheckName, AuditCheckConfiguration>, auditNotificationTargetConfigurations -> map<AuditNotificationType, AuditNotificationTarget>. AuditCheckConfiguration has members `enabled` (boolean, default false) and `configuration` -> CheckCustomConfiguration, documented as "a structure containing the configName and corresponding configValue for configuring audit checks". AWS echoes back the custom configuration that was set for a check, so a check configured with a custom threshold reads back with its configName/configValue pair present (in the recording, DEVICE_CERTIFICATE_EXPIRING_CHECK carries configuration with the CERT_EXPIRATION_THRESHOLD_IN_DAYS key — the key name is fixed by the check, the value is whatever the preceding UpdateAccountAuditConfiguration set). Recorded behavior only for the notification map: AWS OMITS auditNotificationTargetConfigurations entirely when no notification target has ever been configured for the account.
Emulator: returns the audit check with NO `configuration` member at all (null), and always returns auditNotificationTargetConfigurations = {"SNS": {"enabled": false}} even though no notification target was ever configured — and that synthesised AuditNotificationTarget also carries neither targetArn nor roleArn.
Why: a client that writes a per-check custom configuration and reads it back to confirm it (the normal configure-then-verify loop) sees the setting vanish and concludes the write was lost; conversely a client that tests presence of auditNotificationTargetConfigurations to decide whether audit notifications are wired up is told an SNS target configuration exists when none does, and then reads a target with no ARN.
Fix: the DescribeAccountAuditConfiguration handler's output projection/serializer — it must propagate the stored CheckCustomConfiguration onto each AuditCheckConfiguration, and must leave auditNotificationTargetConfigurations absent rather than defaulting a SNS entry into it.
Traces: iot-account-audit-configuration-delete#task#5 (also iot-account-audit-configuration-lifecycle#task#6, iot-delete-account-audit-configuration#task#5).

=== Cause 3 of 5: DeleteAccountAuditConfiguration does not reset account audit state (roleArn and enabled checks survive) ===
AWS: Smithy: DeleteAccountAuditConfiguration, http DELETE /audit/configuration, input deleteScheduledAudits (httpQuery, default false), errors InvalidRequestException/ResourceNotFoundException/ThrottlingException. Recorded behavior: after a successful delete the account is back to its unconfigured state — the following DescribeAccountAuditConfiguration returns NO roleArn, every audit check with enabled=false, and no auditNotificationTargetConfigurations.
Emulator: the delete returns success but the subsequent read still reports the previously configured role ARN and still reports DEVICE_CERTIFICATE_EXPIRING_CHECK with enabled=true, plus the phantom {"SNS": {"enabled": false}} notification map from Cause 2. (The role ARN value itself is per-run and is not the requirement; the requirement is that the member is ABSENT after delete.)
Why: delete is not idempotent-clean — a teardown-then-reprovision sequence, or any client that deletes the audit configuration to stop audits, is told the delete succeeded while Device Defender audit state remains in force and enabled checks still appear active. State leaks across every scenario that shares the account.
Fix: the DeleteAccountAuditConfiguration handler's state mutation — it must clear the stored role ARN, clear/disable all audit check configurations, and clear the notification-target configuration, not merely answer 200.
Traces: iot-account-audit-configuration-delete#task#7 (also iot-delete-account-audit-configuration#task#7).

=== Cause 4 of 5: an account that never configured a notification target still reads back an SNS notification target ===
AWS: Smithy: DescribeAccountAuditConfiguration output auditNotificationTargetConfigurations -> map<AuditNotificationType, AuditNotificationTarget>, whose `enabled` member defaults to false. Recorded behavior only: on an account with no audit notification target configured, AWS omits the auditNotificationTargetConfigurations member from the 200 response rather than emitting a default-valued entry.
Emulator: HTTP 200 (status matches) but the body always contains auditNotificationTargetConfigurations = {"SNS": {"enabled": false}}, on a freshly set-up account and on an account where the configuration was never touched.
Why: the default-valued map entry is indistinguishable, to a client, from a real SNS notification target that was configured and then disabled, so audit-notification configuration drift detection and "is anything configured?" checks both read wrong — and it makes the response shape differ from AWS for every single Describe call in this plane.
Fix: the DescribeAccountAuditConfiguration output projection/serializer — do not synthesise map entries for notification types that have no stored AuditNotificationTarget.
Traces: iot-account-audit-configuration-lifecycle#task#4 (also iot-delete-account-audit-configuration-when-unset#task#4, iot-event-configuration-and-endpoint-settings#task#4).

=== Cause 5 of 5: after DeleteAccountAuditConfiguration every previously enabled check stays enabled, not just one ===
AWS: recorded behavior: after DeleteAccountAuditConfiguration, DescribeAccountAuditConfiguration reports EVERY audit check with enabled=false and no roleArn — the reset covers all check names that had been enabled, not a subset.
Emulator: HTTP 200 (status matches) but both DEVICE_CERTIFICATE_EXPIRING_CHECK.enabled and LOGGING_DISABLED_CHECK.enabled are still true, roleArn is still populated, and the phantom notification map is still present. This is the same reset gap as Cause 3 observed over multiple enabled checks, so a fix that clears only the check the delete-fixture happens to touch does not close it; the whole auditCheckConfigurations map must be reset.
Why: a client that enables several audit checks, deletes the account audit configuration, then re-reads, sees all of its checks still enabled and must assume audits are still running — the opposite of what the delete it just made succeed means.
Fix: the DeleteAccountAuditConfiguration handler's state mutation — reset the entire audit-check configuration map and the role ARN, and re-verify with a multi-check lifecycle, not a single-check one.
Traces: iot-account-audit-configuration-lifecycle#task#8.

## Evidence
All five causes are mismatch clusters from the simparity-wire-aws-v1 replay (service=iot), none of them carrying a stub_path or an owned_by issue.

Cause 1 — fixture iot-account-audit-config-negative-paths step task#3, UpdateAccountAuditConfiguration, expected status 400 / actual 400:
  $.message: value_mismatch (expected 'Missing required configuration argument: RoleArn.', got 'On the first call to UpdateAccountAuditConfiguration, roleArn is required')

Cause 2 — fixture iot-account-audit-configuration-delete step task#5, DescribeAccountAuditConfiguration, expected 200 / actual 200 (cluster of 3: iot-account-audit-configuration-delete#task#5, iot-account-audit-configuration-lifecycle#task#6, iot-delete-account-audit-configuration#task#5):
  $.auditCheckConfigurations.DEVICE_CERTIFICATE_EXPIRING_CHECK.configuration: missing (expected {'CERT_EXPIRATION_THRESHOLD_IN_DAYS': '30'}, got None)
  $.auditNotificationTargetConfigurations: type_mismatch (expected None, got {'SNS': {'enabled': False}})

Cause 3 — fixture iot-account-audit-configuration-delete step task#7, DescribeAccountAuditConfiguration, expected 200 / actual 200 (cluster of 2, with iot-delete-account-audit-configuration#task#7):
  $.auditCheckConfigurations.DEVICE_CERTIFICATE_EXPIRING_CHECK.enabled: value_mismatch (expected False, got True)
  $.auditNotificationTargetConfigurations: type_mismatch (expected None, got {'SNS': {'enabled': False}})
  $.roleArn: type_mismatch (expected None, got a retained role ARN) — the member must be ABSENT after delete; the particular ARN string is per-run.

Cause 4 — fixture iot-account-audit-configuration-lifecycle step task#4, DescribeAccountAuditConfiguration, expected 200 / actual 200 (cluster of 3: iot-account-audit-configuration-lifecycle#task#4, iot-delete-account-audit-configuration-when-unset#task#4, iot-event-configuration-and-endpoint-settings#task#4):
  $.auditNotificationTargetConfigurations: type_mismatch (expected None, got {'SNS': {'enabled': False}})

Cause 5 — fixture iot-account-audit-configuration-lifecycle step task#8, DescribeAccountAuditConfiguration, expected 200 / actual 200:
  $.auditCheckConfigurations.DEVICE_CERTIFICATE_EXPIRING_CHECK.enabled: value_mismatch (expected False, got True)
  $.auditCheckConfigurations.LOGGING_DISABLED_CHECK.enabled: value_mismatch (expected False, got True)
  $.auditNotificationTargetConfigurations: type_mismatch (expected None, got {'SNS': {'enabled': False}})
  $.roleArn: type_mismatch (expected None, got a retained role ARN)

Model citations used: Smithy iot — UpdateAccountAuditConfiguration (PATCH /audit/configuration; roleArn -> RoleArn @length 20..2048; errors InternalFailureException/InvalidRequestException/ThrottlingException), DescribeAccountAuditConfiguration (GET /audit/configuration; output roleArn, auditCheckConfigurations, auditNotificationTargetConfigurations), DeleteAccountAuditConfiguration (DELETE /audit/configuration; deleteScheduledAudits httpQuery default false), shape AuditCheckConfiguration {enabled: Enabled default false, configuration: CheckCustomConfiguration map}, shape AuditNotificationTargetConfigurations map<AuditNotificationType, AuditNotificationTarget>, shape AuditNotificationTarget {targetArn, roleArn, enabled default false}.

## Steps to Reproduce
Replay the simparity-wire-aws-v1 corpus against the emulator built from this checkout, then re-run just these fixtures/steps:

1. Cause 1: on an account with no audit configuration, call UpdateAccountAuditConfiguration (PATCH /audit/configuration) with auditCheckConfigurations but WITHOUT roleArn. Observe HTTP 400 and compare the `message` member against AWS's "Missing required configuration argument: RoleArn." — fixture iot-account-audit-config-negative-paths, step task#3.
2. Cause 2: UpdateAccountAuditConfiguration with a roleArn and with DEVICE_CERTIFICATE_EXPIRING_CHECK enabled plus a custom configuration (CERT_EXPIRATION_THRESHOLD_IN_DAYS), then GET /audit/configuration. Observe the check's `configuration` map is absent and that auditNotificationTargetConfigurations is present with a synthesised SNS entry — fixture iot-account-audit-configuration-delete, step task#5.
3. Cause 3: from that configured state call DeleteAccountAuditConfiguration (DELETE /audit/configuration), then GET /audit/configuration again. Observe roleArn is still returned and the previously enabled check is still enabled=true — fixture iot-account-audit-configuration-delete, step task#7.
4. Cause 4: on an account where no audit notification target was ever configured, GET /audit/configuration. Observe auditNotificationTargetConfigurations = {"SNS": {"enabled": false}} where AWS omits the member — fixture iot-account-audit-configuration-lifecycle, step task#4.
5. Cause 5: enable TWO checks (DEVICE_CERTIFICATE_EXPIRING_CHECK and LOGGING_DISABLED_CHECK) via UpdateAccountAuditConfiguration, call DeleteAccountAuditConfiguration, then GET /audit/configuration. Observe BOTH checks still enabled=true and roleArn still present — fixture iot-account-audit-configuration-lifecycle, step task#8.

All five steps must pass before this issue is closed.
