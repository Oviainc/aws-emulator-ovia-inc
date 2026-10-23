Issue-Source: trace-sweep-divergence
Issue-Trace: appconfig-account-settings-roundtrip
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appconfig-account-settings-roundtrip
Issue-Shape-Id: com.amazonaws.appconfig#AmazonAppConfig
Issue-Key: appconfig|GetAccountSettings|2xx->5xx|$.DeletionProtection
Issue-Key: appconfig|GetAccountSettings|2xx->5xx|$.VendedMetrics
Issue-Key: appconfig|GetAccountSettings|2xx->5xx|status
Issue-Commit: e2dfe022d58b39493003b3d90859f84840a12f00

# AppConfig GetAccountSettings answers 501 instead of the AccountSettings document

> _Note: this issue was identified against the code base at commit `e2dfe022d58b39493003b3d90859f84840a12f00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAccountSettings

## Description
=== Cause 1 of 1: the account-settings surface is not served ===
AWS: GetAccountSettings is GET /settings with HTTP 200 and output AccountSettings{DeletionProtection, VendedMetrics}; the input is Unit, and the modeled errors are BadRequestException (400) and InternalServerException (500) — 501 is not a response this operation can produce (Smithy: GetAccountSettings http {method:GET, uri:/settings, code:200}, output_shape AccountSettings). The recorded response is 200 with DeletionProtection={'Enabled': False, 'ProtectionPeriodInMinutes': 60} and VendedMetrics={}.
Emulator: answers HTTP 501 with no AccountSettings body — $.DeletionProtection and $.VendedMetrics are both absent. The step is a plain mismatch in the replay report (no stub_path), so no auto-filed scaffold issue covers it.
Why: a client cannot read account-level deletion protection at all; it gets a 501 the SDK cannot classify as a modeled AppConfig error, and every flow that reads settings before deleting an environment or configuration profile is blocked, with the deletion-protection control surface unobservable.
Fix: route and implement the account-settings operations in the appconfig service (GET /settings, and the sibling PATCH /settings) over an account-level settings record in appconfig state, defaulting DeletionProtection to disabled with a 60-minute protection period and VendedMetrics to an empty document.
Traces: appconfig-account-settings-roundtrip#task#2 (cluster also covers appconfig-account-settings-roundtrip#task#4).

Related: the open behavioral issue 20261023T032202Z-appconfig-control-plane-fidelity-settings-route-missing-environmentstate.md reports the same missing route from the behavioral corpus (its cause 1); it carries no wire replay key, so this issue binds the failing fixture step.

## Evidence
Wire replay (simparity-wire-aws-v1): appconfig-account-settings-roundtrip#task#2 GetAccountSettings — status: expected 200, got 501; $.DeletionProtection missing (expected {'Enabled': False, 'ProtectionPeriodInMinutes': 60}, got None); $.VendedMetrics missing (expected {}, got None). Same cluster: appconfig-account-settings-roundtrip#task#4.

## Steps to Reproduce
Boot the emulator, point an appconfig client at it and call get_account_settings(). It must return HTTP 200 with DeletionProtection{Enabled, ProtectionPeriodInMinutes} and VendedMetrics; the emulator answers HTTP 501 with no settings document. Re-replay appconfig-account-settings-roundtrip#task#2.
