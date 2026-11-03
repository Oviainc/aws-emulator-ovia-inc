Issue-Source: trace-sweep-divergence
Issue-Trace: payment-cryptography-key-lifecycle-symmetric-v2
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: payment-cryptography-key-lifecycle-symmetric-v2 payment-cryptography-key-replication-regions-v2
Issue-Shape-Id: com.amazonaws.paymentcryptography#PaymentCryptographyControlPlane
Issue-Key: payment-cryptography|GetKey|2xx->2xx|$.Key.Enabled
Issue-Key: payment-cryptography|GetKey|2xx->2xx|$.Key.UsageStopTimestamp
Issue-Key: payment-cryptography|GetKey|2xx->2xx|$
Issue-Key: payment-cryptography|GetKey|2xx->2xx|$.Key.MultiRegionKeyType
Issue-Key: payment-cryptography|GetKey|2xx->2xx|$.Key.ReplicationStatus
Issue-Key: payment-cryptography|GetKey|2xx->2xx|$.Key.ReplicationStatus.us-east-1.Status
Issue-Key: payment-cryptography|GetKey|2xx->2xx|$.Key.UsingDefaultReplicationRegions
Issue-Key: payment-cryptography|DeleteKey|2xx->2xx|$.Key.MultiRegionKeyType
Issue-Key: payment-cryptography|DeleteKey|2xx->2xx|$.Key.ReplicationStatus
Issue-Key: payment-cryptography|DeleteKey|2xx->2xx|$.Key.UsingDefaultReplicationRegions
Issue-Related: 20261031T105700Z-payment-cryptography-key-responses-omit-the-type-discriminator.md
Issue-Commit: 2b1389a4a6c4df71c4118b776bf7e768de987e7c

# payment-cryptography key mutations never reach GetKey: usage stop and multi-region replication unrecorded

> _Note: this issue was identified against the code base at commit `2b1389a4a6c4df71c4118b776bf7e768de987e7c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetKey

## Description
Mutating operations on a key answer 200 but do not change the stored Key record, so the subsequent read serves the create-time snapshot. Three causes across two independent mutations.

Cause 1 of 3: StopKeyUsage does not disable the key, and no UsageStopTimestamp is recorded.
AWS: Smithy — StopKeyUsage ("Disables an Amazon Web Services Payment Cryptography key, which makes it inactive within Amazon Web Services Payment Cryptography") takes KeyIdentifier @required and returns StopKeyUsageOutput.Key : Key. On that shape, `Enabled` (@required, Boolean) "specifies whether the key is enabled", and `UsageStopTimestamp` (Timestamp) is "the date and time after which Amazon Web Services Payment Cryptography will stop using the key material for cryptographic operations". Recorded behavior: after StopKeyUsage succeeds, a GetKey on the same key reports Enabled=false and carries UsageStopTimestamp (an epoch-seconds timestamp set at the moment usage stopped — its value is per-run, its PRESENCE is the requirement).
Emulator: GetKey after StopKeyUsage answers 200 but reports `$.Key.Enabled` = true (expected false) and omits `$.Key.UsageStopTimestamp` entirely, i.e. StopKeyUsage was accepted as a no-op.
Why: Enabled is the switch that decides whether a key may be used for cryptographic operations. A caller that disables a key for incident response, then verifies via GetKey, is told the key is still enabled and cannot confirm the control took effect; conversely any downstream gate keyed on Enabled keeps treating a stopped key as live.
Fix: the StopKeyUsage handler and the key record in the payment-cryptography control-plane state: it must clear Enabled and stamp UsageStopTimestamp on the stored key, so GetKey and the other Key-returning reads observe it.
Traces: payment-cryptography-key-lifecycle-symmetric-v2#task#5.

Cause 2 of 3: multi-region replication is not modeled, so a replicated key reads back as a single-Region key and never reaches SYNCHRONIZED.
AWS: Smithy — AddKeyReplicationRegions "adds replication Amazon Web Services Regions to an existing ... key". On the Key shape this is reported by three members: `MultiRegionKeyType` ("indicates whether this key is a Multi-Region key and its role in the Multi-Region key hierarchy" — PRIMARY for the key the regions were added to), `ReplicationStatus`, a map of Region -> com.amazonaws.paymentcryptography#ReplicationStatusType whose required `Status` member (KeyReplicationState) progresses through states such as IN_PROGRESS to SYNCHRONIZED, and `UsingDefaultReplicationRegions`, which per its own documentation is true only when the key follows the account default and "false" when "the key has a custom replication configuration that overrides the account defaults". Recorded behavior: after replication regions are added explicitly, GetKey reports MultiRegionKeyType=PRIMARY, ReplicationStatus with an entry per added Region whose Status converges to SYNCHRONIZED, and UsingDefaultReplicationRegions=false.
Emulator: GetKey answers 200 with the create-time record — `$.Key.MultiRegionKeyType` missing, `$.Key.ReplicationStatus` missing, and `$.Key.UsingDefaultReplicationRegions` = true although the key was given an explicit replication configuration. Because ReplicationStatus never appears, the fixture's poll for Status=SYNCHRONIZED cannot ever be satisfied and is capped at 60s.
Why: a caller that adds a replication Region and then waits for the replica to become usable waits forever and has no way to learn the replication state; and a key with a custom configuration is misreported as following the account default, so a client reading UsingDefaultReplicationRegions draws the wrong conclusion about which regions the key lives in.
Fix: the AddKeyReplicationRegions / RemoveKeyReplicationRegions handlers and the key record's replication fields in state: adding regions must set MultiRegionKeyType, populate the ReplicationStatus map and advance each entry's Status to SYNCHRONIZED, and clear UsingDefaultReplicationRegions when an explicit configuration is supplied.
Traces: payment-cryptography-key-replication-regions-v2#task#3.

Cause 3 of 3: the same replication fields are absent from DeleteKey's response, where AWS reports the replicas tearing down.
AWS: Smithy — DeleteKey -> DeleteKeyOutput.Key : Key, carrying the same MultiRegionKeyType / ReplicationStatus / UsingDefaultReplicationRegions members; ReplicationStatusType.Status documents DELETE_IN_PROGRESS among its states. Recorded behavior: deleting a multi-Region key returns the Key with MultiRegionKeyType=PRIMARY, UsingDefaultReplicationRegions=false, and a ReplicationStatus entry per replication Region whose Status is DELETE_IN_PROGRESS.
Emulator: answers 200 but omits `$.Key.MultiRegionKeyType` and `$.Key.ReplicationStatus` and reports `$.Key.UsingDefaultReplicationRegions` = true.
Why: a caller deleting a multi-Region key cannot observe that the replicas are being torn down, so cleanup verification has nothing to poll.
Fix: as cause 2 — once the replication fields are held in the key record, the DeleteKey path must project them and move each entry's Status to DELETE_IN_PROGRESS.
Traces: payment-cryptography-key-replication-regions-v2#teardown#6.

(The KeyModesOfUse.__type omission that also appears in these diffs is filed separately as the serializer issue; it is not part of this root cause.)

## Evidence
Replay of trace set simparity-wire-aws-v1. Three divergence clusters, verdict=mismatch, expected_status 200 / actual_status 200 — the mutations are accepted but the reads are stale:
- service=payment-cryptography|GetKey|status=200->200|keys=$.Key.Enabled,...,$.Key.UsageStopTimestamp, fixture payment-cryptography-key-lifecycle-symmetric-v2, step task#5: "$.Key.Enabled: value_mismatch (expected False, got True)" and "$.Key.UsageStopTimestamp: missing (expected <timestamp>, got None)". The step follows StopKeyUsage in the same fixture.
- service=payment-cryptography|GetKey|status=200->200, fixture payment-cryptography-key-replication-regions-v2, step task#3: "$.Key.MultiRegionKeyType: missing (expected 'PRIMARY', got None)", "$.Key.ReplicationStatus: missing (expected {'us-east-1': {'Status': 'SYNCHRONIZED'}}, got None)", "$.Key.UsingDefaultReplicationRegions: value_mismatch (expected False, got True)", "$.Key.ReplicationStatus.us-east-1.Status: poll: not ..." and "$: poll capped at 60s". The emulator's body is the create-time snapshot (KeyState CREATE_COMPLETE, no replication members).
- service=payment-cryptography|DeleteKey|status=200->200, same fixture, step teardown#6: "$.Key.MultiRegionKeyType: missing (expected 'PRIMARY', got None)", "$.Key.ReplicationStatus: missing (expected {'us-east-1': {'Status': 'DELETE_IN_PROGRESS'}}, got None)", "$.Key.UsingDefaultReplicationRegions: value_mismatch (expected False, got True)".
The region name us-east-1 and the timestamp value are the fixtures' own inputs/clock; the required behavior is an entry per requested Region reaching SYNCHRONIZED (then DELETE_IN_PROGRESS) and the presence of UsageStopTimestamp.
Smithy: Key.Enabled (required Boolean), Key.UsageStopTimestamp (Timestamp), Key.MultiRegionKeyType, Key.ReplicationStatus (map, cfnMutability read), Key.UsingDefaultReplicationRegions (Boolean); ReplicationStatusType.Status (required KeyReplicationState).

## Steps to Reproduce
1. Replay fixture payment-cryptography-key-lifecycle-symmetric-v2. After the StopKeyUsage step, step task#5 calls GetKey: the emulator reports Key.Enabled=true and no Key.UsageStopTimestamp, where the recording has Enabled=false plus a UsageStopTimestamp.
2. Replay fixture payment-cryptography-key-replication-regions-v2. After AddKeyReplicationRegions with an explicit region, step task#3 polls GetKey for Key.ReplicationStatus.<region>.Status == SYNCHRONIZED: the member never appears, MultiRegionKeyType is absent and UsingDefaultReplicationRegions is true, so the poll is capped at 60s.
3. Same fixture, step teardown#6 calls DeleteKey on that multi-Region key: the response omits MultiRegionKeyType and ReplicationStatus (recorded: PRIMARY and a per-region entry with Status DELETE_IN_PROGRESS) and reports UsingDefaultReplicationRegions=true.
