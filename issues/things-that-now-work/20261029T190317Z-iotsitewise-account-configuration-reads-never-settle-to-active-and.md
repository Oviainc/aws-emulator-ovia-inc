Issue-Source: trace-sweep-divergence
Issue-Trace: iotsitewise-account-configuration-workflow
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iotsitewise-account-configuration-workflow
Issue-Shape-Id: com.amazonaws.iotsitewise#AWSIoTSiteWise
Issue-Key: iotsitewise|PutDefaultEncryptionConfiguration|2xx->2xx|$.configurationStatus.state
Issue-Key: iotsitewise|DescribeDefaultEncryptionConfiguration|2xx->2xx|$.configurationStatus.state
Issue-Key: iotsitewise|DescribeStorageConfiguration|2xx->2xx|$.disassociatedDataStorage
Issue-Key: iotsitewise|DescribeStorageConfiguration|2xx->2xx|$.warmTier
Issue-Commit: f5ba8024c64455531a4bd3d9779b46ec0fe104e4

# Account configuration reads never settle to ACTIVE and omit storage members

> _Note: this issue was identified against the code base at commit `f5ba8024c64455531a4bd3d9779b46ec0fe104e4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDefaultEncryptionConfiguration

## Description
The account-level configuration surface (encryption + storage) diverges in 3 ways. All three are in the account-configuration handlers/serializers for /configuration/account/encryption and /configuration/account/storage.

Cause 1 of 3: PutDefaultEncryptionConfiguration reports a configuration state that never becomes ACTIVE.
AWS: Smithy: PutDefaultEncryptionConfiguration output member configurationStatus (ConfigurationStatus, @required) -> ConfigurationStatus.state (ConfigurationState, @required) with values ACTIVE | UPDATE_IN_PROGRESS | UPDATE_FAILED. For a default SITEWISE_DEFAULT_ENCRYPTION put, the recorded response is HTTP 200 with configurationStatus.state = ACTIVE: the configuration is applied synchronously and reported settled on the same response. UPDATE_IN_PROGRESS is the state AWS reports only while an asynchronous key change is still being applied.
Emulator: answers HTTP 200 but with $.configurationStatus.state = 'UPDATE_IN_PROGRESS' — it marks the configuration as pending and never transitions it.
Why: every SDK/CLI waiter and every client that polls until state == ACTIVE before continuing loops forever, and a client that branches on UPDATE_IN_PROGRESS takes the error/retry path for a configuration that is in fact in place.
Fix: the PutDefaultEncryptionConfiguration handler's status construction (and the account-configuration state record it writes) must settle the stored ConfigurationStatus to ACTIVE when the put is applied, rather than storing a permanently pending state.
Traces: iotsitewise-account-configuration-workflow#task#2.

Cause 2 of 3: DescribeDefaultEncryptionConfiguration reads back the same unsettled state.
AWS: Smithy: DescribeDefaultEncryptionConfiguration (GET /configuration/account/encryption) output configurationStatus @required. A read of an applied account encryption configuration answers HTTP 200 with configurationStatus.state = ACTIVE.
Emulator: answers HTTP 200 with $.configurationStatus.state = 'UPDATE_IN_PROGRESS' — the read faithfully reports the state the put stored, so the defect is visible on the read path too and must be re-verified there.
Why: a client that reads the account's encryption configuration to decide whether it is usable concludes the account is mid-update and refuses to proceed.
Fix: same account-configuration state record; the describe handler must report the settled ACTIVE state once the put has been applied.
Traces: iotsitewise-account-configuration-workflow#task#3.

Cause 3 of 3: DescribeStorageConfiguration omits the storage-tier state members.
AWS: Smithy: DescribeStorageConfiguration output members disassociatedDataStorage (DisassociatedDataStorageState enum) and warmTier (WarmTierState enum). AWS always reports the account's effective tier state for these on a storage-configuration read; the recorded response is HTTP 200 with disassociatedDataStorage = 'ENABLED' and warmTier = 'DISABLED' (the effective values after the workflow's PutStorageConfiguration), i.e. the members are present and carry the configured/defaulted state, not absent.
Emulator: answers HTTP 200 but omits both members entirely ($.disassociatedDataStorage missing, $.warmTier missing), so a deserialized response reports them as unset.
Why: a client that reads warmTier/disassociatedDataStorage to decide whether disassociated (alias-only) ingest is allowed or whether warm-tier retention applies sees 'unknown' and either refuses valid ingest or skips a tier precondition AWS enforces — and the same omission is why a CreateBulkImportJob tier precondition can be evaluated against no tier state at all.
Fix: the DescribeStorageConfiguration handler/serializer must project the stored disassociatedDataStorage and warmTier state (including their service defaults when the account never set them) onto the response.
Traces: iotsitewise-account-configuration-workflow#task#5.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from mainline (score 72.5%, 11/40 steps mismatched):

1) iotsitewise-account-configuration-workflow#task#2 PutDefaultEncryptionConfiguration — expected 200, got 200; $.configurationStatus.state: value_mismatch (expected 'ACTIVE', got 'UPDATE_IN_PROGRESS').
2) iotsitewise-account-configuration-workflow#task#3 DescribeDefaultEncryptionConfiguration — expected 200, got 200; $.configurationStatus.state: value_mismatch (expected 'ACTIVE', got 'UPDATE_IN_PROGRESS').
3) iotsitewise-account-configuration-workflow#task#5 DescribeStorageConfiguration — expected 200, got 200; $.disassociatedDataStorage: missing (expected 'ENABLED', got None); $.warmTier: missing (expected 'DISABLED', got None).

Model grounding: ConfigurationState enum = [ACTIVE, UPDATE_IN_PROGRESS, UPDATE_FAILED]; ConfigurationStatus.state is @required; DescribeStorageConfiguration declares disassociatedDataStorage (DisassociatedDataStorageState) and warmTier (WarmTierState).

## Steps to Reproduce
1. Replay fixture iotsitewise-account-configuration-workflow against the emulator.
2. Step task#2: POST /configuration/account/encryption (PutDefaultEncryptionConfiguration) with the default encryption type — observe configurationStatus.state = UPDATE_IN_PROGRESS instead of ACTIVE.
3. Step task#3: GET /configuration/account/encryption (DescribeDefaultEncryptionConfiguration) — observe the same UPDATE_IN_PROGRESS state instead of ACTIVE.
4. Step task#5: GET /configuration/account/storage (DescribeStorageConfiguration) after the workflow's storage put — observe the response body has no disassociatedDataStorage and no warmTier member.
