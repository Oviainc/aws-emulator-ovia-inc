Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: ab8204cef26f0e4f4f70841aaff540c660da88f2

# A reused ClientToken replays the first volume id even when the volume parameters differ, instead of answering ConflictException

_Filed: 2026-10-24T01:58:46Z_
_Operation: CreateVolume | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `ab8204cef26f0e4f4f70841aaff540c660da88f2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CreateVolume's idempotency lookup is on the ClientToken alone (`state.volume_client_tokens`): any later call carrying a known token returns the stored VolumeId without comparing AvailabilityZone, SizeInGB, VolumeType, Iops, Throughput, Encrypted, KmsKeyId, SnapshotId or TagSpecifications.

Expected: a retry that reuses a client token with different parameters is refused — on this operation with the modelled ConflictException (HTTP 409) — so a caller cannot believe a differently-sized or differently-typed volume was created. Actual: HTTP 200 carrying the first volume's id, and the stored record keeps the first call's configuration. ConflictException is unreachable in this handler.

## Evidence

Spec: CreateVolume is @idempotent with ClientToken @idempotencyToken and models ConflictException (HTTP 409, required Message/ResourceId/ResourceType); no other code path in the handler can raise it. Mismatched client-token retries are refused by the backing EBS CreateVolume (IdempotentParameterMismatch) and by modern AWS APIs generally. Handler: src/ops/create_volume.rs lines ~169-178 returns the stored volume id on a token hit before any other member is read. Tests cover only identical replays (tests/all/volume_lifecycle.rs `an_idempotency_token_replays_one_volume_and_the_token_is_retired_by_delete`).

## Steps to Reproduce

1. Call CreateVolume with {"ClientToken":"vtok-1","AvailabilityZone":"us-east-1a","SizeInGB":10,"VolumeType":"gp3"} — note the vol- id
2. Call CreateVolume with {"ClientToken":"vtok-1","AvailabilityZone":"eu-west-1b","SizeInGB":500,"VolumeType":"io2"}
3. Emulator answers 200 with the first volume's id and keeps the first configuration; no ConflictException (409) is produced for the mismatched retry
