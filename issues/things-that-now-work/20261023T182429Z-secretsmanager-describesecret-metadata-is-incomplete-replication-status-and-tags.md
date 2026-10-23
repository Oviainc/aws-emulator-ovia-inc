Issue-Source: trace-sweep-divergence
Issue-Trace: secretsmanager-cross-region-replication-lifecycle
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: secretsmanager-cross-region-replication-lifecycle secretsmanager-tagging-and-secret-discovery
Issue-Shape-Id: com.amazonaws.secretsmanager#secretsmanager
Issue-Key: secretsmanager|DescribeSecret|2xx->2xx|$.ReplicationStatus
Issue-Key: secretsmanager|DescribeSecret|2xx->2xx|$.PrimaryRegion
Issue-Key: secretsmanager|DescribeSecret|2xx->2xx|$.Tags[Key="team"]
Issue-Commit: 3dc196f76d19116b5d570e1dc438e2b93d0e0d47

# DescribeSecret metadata is incomplete: replication status and tags are dropped while PrimaryRegion is reported on an unreplicated secret

> _Note: this issue was identified against the code base at commit `3dc196f76d19116b5d570e1dc438e2b93d0e0d47`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeSecret

## Description
DescribeSecret answers 200 but its metadata projection does not follow the secret's own state: replication and tag attachments never appear, and the replication-only PrimaryRegion member is emitted unconditionally. Three causes.

Cause 1 of 3: ReplicationStatus is omitted after a secret is replicated.
AWS: Smithy DescribeSecret -> DescribeSecretResponse.ReplicationStatus : ReplicationStatusListType of ReplicationStatusType {Region, KmsKeyId, Status (InProgress|Failed|InSync), StatusMessage, LastAccessedDate}. After ReplicateSecretToRegions, DescribeSecret reports one entry per replica region; the recording shows one entry for the target region with Status InProgress and the default Secrets Manager KMS alias as KmsKeyId, status 200.
Emulator: 200 with ReplicationStatus absent entirely (got None).
Why: ReplicationStatus is the only way a client can observe whether a replica reached InSync or Failed, so a caller polling after ReplicateSecretToRegions polls forever on a null field and can never discover a failed replica.
Fix: the ReplicateSecretToRegions/RemoveRegionsFromReplication handlers must record per-region replica state, and the DescribeSecret serializer must project it.

Cause 2 of 3: PrimaryRegion is emitted for a secret with no replicas.
AWS: PrimaryRegion (RegionType, pattern ^([a-z]+-)+\\d+$) is part of the replication view and is omitted once the secret is not a replication primary - after the replica regions are removed the recorded DescribeSecret response (200) has no PrimaryRegion member.
Emulator: 200 but PrimaryRegion is present (unexpected_present; recorded actual value was the emulator's own region).
Why: presence of PrimaryRegion is how clients and the console decide a secret participates in cross-region replication; an always-present value makes every secret look replicated, so removal of replication is undetectable.
Fix: the DescribeSecret serializer - make PrimaryRegion conditional on replication state rather than a constant.

Cause 3 of 3: tags attached to a secret are not returned by DescribeSecret.
AWS: Smithy DescribeSecret -> DescribeSecretResponse.Tags : TagListType. After TagResource (or CreateSecret with Tags), DescribeSecret returns each attached tag as a {Key, Value} entry; the recording expects the tag key written by the fixture to be present, status 200.
Emulator: 200 but the tag entry is missing from Tags (got None for the expected key).
Why: tags drive discovery, cost allocation and tag-based authorization; a client that tags a secret and then reads it back sees no tags, so tag-filtered discovery returns nothing and any tag-conditioned logic takes the wrong branch.
Fix: the TagResource/UntagResource handlers and the secret's tag state, plus the DescribeSecret serializer.

Traces: secretsmanager-cross-region-replication-lifecycle#task#3 (cause 1), secretsmanager-cross-region-replication-lifecycle#task#7 (cause 2), secretsmanager-tagging-and-secret-discovery#task#4 (cause 3).

## Evidence
Replay of simparity-wire-aws-v1 (emulator_up=true, state_reset_failures=0), all three clusters DescribeSecret with expected_status 200 and actual_status 200.

Cause 1 - secretsmanager-cross-region-replication-lifecycle#task#3: $.ReplicationStatus: missing (expected [{'KmsKeyId': 'alias/aws/secretsmanager', 'Region': 'us-east-1', 'Status': 'InProgress'}], got None). The region and alias are the recording's example values; the requirement is one ReplicationStatusType entry per replica with a Status from the modeled enum.

Cause 2 - secretsmanager-cross-region-replication-lifecycle#task#7: $.PrimaryRegion: unexpected_present (expected None, got 'us-west-2').

Cause 3 - secretsmanager-tagging-and-secret-discovery#task#4: $.Tags[Key="team"]: missing (expected {'Key': 'team', 'Value': 'payments'}, got None). Key/value are the recording's example values; the requirement is that tags written by TagResource round-trip.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the corpus: replay_wire_corpus().
2. Cause 1: fixture secretsmanager-cross-region-replication-lifecycle - CreateSecret, ReplicateSecretToRegions, then step task#3 DescribeSecret and inspect ReplicationStatus.
3. Cause 2: same fixture, step task#7 - after RemoveRegionsFromReplication, DescribeSecret and check whether PrimaryRegion is present.
4. Cause 3: fixture secretsmanager-tagging-and-secret-discovery - TagResource, then step task#4 DescribeSecret and inspect Tags.
