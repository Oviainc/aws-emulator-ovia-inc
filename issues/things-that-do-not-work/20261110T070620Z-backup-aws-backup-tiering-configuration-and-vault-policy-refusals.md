Issue-Source: trace-sweep-divergence
Issue-Trace: backup-backup-vault-access-policy
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: backup-backup-vault-access-policy backup-vault-access-policy-roundtrip backup-tiering-configuration-invalid-resource-type backup-tiering-configuration-boundary-values backup-tiering-configuration-duplicate-create-conflict backup-tiering-configuration-lifecycle
Issue-Shape-Id: com.amazonaws.backup#CryoControllerUserManager
Issue-Key: backup|GetBackupVaultAccessPolicy|4xx->2xx|$.Message
Issue-Key: backup|GetBackupVaultAccessPolicy|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: backup|GetBackupVaultAccessPolicy|4xx->2xx|status
Issue-Key: backup|CreateTieringConfiguration|4xx->2xx|$.Code
Issue-Key: backup|CreateTieringConfiguration|4xx->2xx|$.Message
Issue-Key: backup|CreateTieringConfiguration|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: backup|CreateTieringConfiguration|4xx->2xx|status
Issue-Key: backup|GetTieringConfiguration|4xx->2xx|$.Code
Issue-Key: backup|GetTieringConfiguration|4xx->2xx|$.Message
Issue-Key: backup|GetTieringConfiguration|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: backup|GetTieringConfiguration|4xx->2xx|status
Issue-Key: backup|GetTieringConfiguration|2xx->2xx|$.TieringConfiguration.LastUpdatedTime
Issue-Key: backup|ListTieringConfigurations|2xx->2xx|$.TieringConfigurations[*]
Issue-Commit: f6167f4eb06772a3465b684848e681d85c2982a9

# AWS Backup tiering-configuration and vault-policy refusals are not raised, and tiering reads omit LastUpdatedTime

> _Note: this issue was identified against the code base at commit `f6167f4eb06772a3465b684848e681d85c2982a9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTieringConfiguration

## Description
5 net-new causes. Three are refusals AWS raises that the emulator answers 200 to (so a client sees a success where AWS fails the call, and the emulator's state diverges from AWS's), and two are modeled timestamp members missing from tiering-configuration read bodies.

Cause 1 of 5 — GetBackupVaultAccessPolicy on a vault that has no access policy.
AWS: Smithy: GetBackupVaultAccessPolicy (GET /backup-vaults/{BackupVaultName}/access-policy) declares ResourceNotFoundException (httpError 400) among its errors, and its output's Policy member is only present when a policy exists. Recorded behavior: status 400 with Message 'Vault <backup-vault-ARN> has no associated vault policy' — the fixed text is 'Vault ' ... ' has no associated vault policy' and the variable part is the requested vault's ARN as the emulator would render it from the request/state (the recorded run's arn:aws:backup:us-west-2:000000000000:backup-vault:<name> is just one filling of that template, not a requirement).
Emulator: returns status 200 with no Message and no x-amzn-errortype header, i.e. it reports a policy read as successful for a vault that has none.
Why: a client that uses GetBackupVaultAccessPolicy to test whether a vault is governed by a resource policy concludes one exists; code that then edits-and-puts the returned policy operates on nothing, and error handling for the documented not-found case is never exercised.
Fix: the GetBackupVaultAccessPolicy handler — when the vault carries no access policy it must raise ResourceNotFoundException (400) instead of returning a success body.
Traces: backup-backup-vault-access-policy#task#6 (also backup-vault-access-policy-roundtrip#task#7).

Cause 2 of 5 — CreateTieringConfiguration with a resource type other than S3.
AWS: Smithy: CreateTieringConfiguration declares InvalidParameterValueException (httpError 400, members Code/Message/Type/Context); TieringConfiguration.ResourceSelection is required. Recorded behavior: status 400, x-amzn-errortype 'InvalidParameterValueException', Code 'ERROR_3C05', Message 'Tiering configuration is only supported for S3' — that message is entirely fixed text for this error class.
Emulator: status 200 with a created-configuration body; no Code, no Message, no errortype header. The unsupported resource type is accepted and a tiering configuration is created.
Why: a client is told tiering was configured for a resource type AWS refuses, so the misconfiguration is discovered only in production; worse, the emulator's state now holds a configuration AWS would never have.
Fix: CreateTieringConfiguration input validation — reject a ResourceSelection whose resource type is not S3 with InvalidParameterValueException (400, Code 'ERROR_3C05').
Traces: backup-tiering-configuration-invalid-resource-type#task#1.

Cause 3 of 5 — GetTieringConfiguration for a configuration whose create was refused.
AWS: Smithy: GetTieringConfiguration declares ResourceNotFoundException (httpError 400). Recorded behavior: status 400, x-amzn-errortype 'ResourceNotFoundException', Code 'ERROR_2C01', Message 'Cannot find tiering configuration' (all fixed text).
Emulator: status 200 returning the configuration. This is the downstream consequence of cause 2 — the refused create left state behind — but it is also a read-path defect: the read must 400 for a name that does not exist.
Why: the client sees a tiering configuration that AWS does not have, so any subsequent reconciliation (update, delete, drift check) diverges.
Fix: CreateTieringConfiguration must not persist a rejected configuration, and the GetTieringConfiguration handler must raise ResourceNotFoundException for an unknown name.
Traces: backup-tiering-configuration-invalid-resource-type#task#2.

Cause 4 of 5 — GetTieringConfiguration omits TieringConfiguration.LastUpdatedTime.
AWS: Smithy: shape TieringConfiguration has a LastUpdatedTime timestamp member ('The date and time a tiering configuration was created... updated, in Unix format and UTC', accurate to milliseconds). Recorded behavior: the member is present on a freshly created configuration (equal to its creation time); its value is a clock reading, so only presence and type are the requirement.
Emulator: the member is absent from GetTieringConfiguration's TieringConfiguration body (expected a timestamp, got None).
Why: a client that orders or caches configurations by LastUpdatedTime, or renders it, gets a null and either crashes or shows the configuration as never updated.
Fix: the tiering-configuration state/serializer — persist and emit LastUpdatedTime (set at create, refreshed on update).
Traces: backup-tiering-configuration-boundary-values#task#3 (also #task#4, backup-tiering-configuration-duplicate-create-conflict#task#3, backup-tiering-configuration-lifecycle#task#2).

Cause 5 of 5 — ListTieringConfigurations list members omit LastUpdatedTime.
AWS: Smithy: shape TieringConfigurationsListMember declares TieringConfigurationArn, TieringConfigurationName, BackupVaultName, CreationTime and LastUpdatedTime. Recorded behavior: every listed member carries LastUpdatedTime.
Emulator: each entry of $.TieringConfigurations carries only Arn/Name/BackupVaultName/CreationTime; LastUpdatedTime is missing.
Why: the list view cannot be used to find recently changed configurations, and a client mapping the list shape to a typed model sees a required-by-contract field as null.
Fix: the ListTieringConfigurations serializer / its projection of tiering-configuration state onto TieringConfigurationsListMember.
Traces: backup-tiering-configuration-lifecycle#task#3.

Note: the open issue 20261103T061309Z-backup-aws-backup-plan-selection-and-vault-refusals-diverge.md covers plan/selection and vault refusals but does not carry the replay key for the GetBackupVaultAccessPolicy cause, and no open issue carries any of the tiering-configuration keys; the replay report reported all five clusters as unowned.

## Evidence
Replay of corpus simparity-wire-aws-v1 against the emulator built from this checkout.

1) GetBackupVaultAccessPolicy, backup-backup-vault-access-policy#task#6 (also backup-vault-access-policy-roundtrip#task#7): expected status 400 with $.Message 'Vault <vault-arn> has no associated vault policy' and an x-amzn-errortype header; actual status 200, $.Message missing, no errortype header.
2) CreateTieringConfiguration, backup-tiering-configuration-invalid-resource-type#task#1: expected 400, $.Code 'ERROR_3C05', $.Message 'Tiering configuration is only supported for S3', x-amzn-errortype 'InvalidParameterValueException'; actual 200 with none of those.
3) GetTieringConfiguration, backup-tiering-configuration-invalid-resource-type#task#2: expected 400, $.Code 'ERROR_2C01', $.Message 'Cannot find tiering configuration', x-amzn-errortype 'ResourceNotFoundException'; actual 200.
4) GetTieringConfiguration, backup-tiering-configuration-boundary-values#task#3 (4 steps in cluster): $.TieringConfiguration.LastUpdatedTime missing (expected a timestamp, got None).
5) ListTieringConfigurations, backup-tiering-configuration-lifecycle#task#3: $.TieringConfigurations[0] lacks LastUpdatedTime (expected member present alongside TieringConfigurationArn/Name/BackupVaultName/CreationTime, got an entry without it).

Smithy (services/service-com-amazonaws-backup-cryocontrollerusermanager): GetBackupVaultAccessPolicy and GetTieringConfiguration both list ResourceNotFoundException (httpError 400); CreateTieringConfiguration lists InvalidParameterValueException (httpError 400); shapes TieringConfiguration and TieringConfigurationsListMember both declare LastUpdatedTime.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. Create a backup vault and do NOT put an access policy on it, then GET /backup-vaults/<vault>/access-policy. The emulator answers 200; AWS answers 400 ResourceNotFoundException with 'Vault <vault-arn> has no associated vault policy'.
3. PUT /tiering-configurations with a TieringConfiguration whose ResourceSelection names a non-S3 resource type. The emulator answers 200 and creates it; AWS answers 400 InvalidParameterValueException, Code 'ERROR_3C05', Message 'Tiering configuration is only supported for S3'.
4. GET /tiering-configurations/<that same name>. The emulator answers 200 with the configuration; AWS answers 400 ResourceNotFoundException, Code 'ERROR_2C01', Message 'Cannot find tiering configuration'.
5. Create a valid S3 tiering configuration, then GET /tiering-configurations/<name>: the TieringConfiguration body has no LastUpdatedTime.
6. GET /tiering-configurations: each entry of TieringConfigurations has no LastUpdatedTime.
