Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.op-contract-create-volume ec2.snapshot-in-use-by-registered-image-delete-rejected ec2.copy-snapshot-malformed-presigned-url-rejected ec2.op-contract-disable-fast-snapshot-restores ec2.op-contract-lock-snapshot-blocks-deletion ec2.op-contract-restore-snapshot-tier-not-archived-rejected
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|CreateVolume|InvalidSnapshot.NotFound
Issue-Key: ec2.copy-snapshot-malformed-presigned-url-rejected|_error_detail#10|AssertionError|raise AssertionError(f"expected the call to be rejected but it succeeded and returned {result!r}")
Issue-Key: ec2.op-contract-disable-fast-snapshot-restores|run#23|AssertionError|assert message == "AvailabilityZone or AvailabilityZoneId must be specified in the request, but not both.", (
Issue-Key: ec2.op-contract-lock-snapshot-blocks-deletion|run#12|AssertionError|assert code == "InvalidRequest", f"unexpected error code: {code!r}"
Issue-Key: ec2.op-contract-restore-snapshot-tier-not-archived-rejected|run#13|AssertionError|assert code in _NOT_ARCHIVED_CODES, f"unexpected error code: {code}"
Issue-Commit: a5885c3be3f986c1a94ab18540ac47db2748e09d

# ec2: 5 net-new EBS snapshot/volume root causes — CreateVolume cannot resolve an existing SnapshotId, CopySnapshot accepts a malformed PresignedUrl, and LockSnapshot/RestoreSnapshotTier/DisableFastSnapshotRestores refusals diverge

> _Note: this issue was identified against the code base at commit `a5885c3be3f986c1a94ab18540ac47db2748e09d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateVolume

## Description
Five net-new root causes in the EC2 emulator's EBS snapshot/volume surface, each grounded in failing behavioral checks of this run. EC2 uses the ec2Query protocol and its errors are NOT modelled as Smithy shapes (smithy_query on CopySnapshot, CreateVolume, LockSnapshot, RestoreSnapshotTier and DisableFastSnapshotRestores all report `errors: []`), so every error code and message below is "recorded behavior only" — taken from what the recording asserts AWS answers. Resource ids, names and timestamps quoted under Evidence come from one run and fill the message template; they are not part of it.

Cause 1 of 5: CreateVolume refuses a SnapshotId that exists and has completed.
AWS: recorded behavior only. CreateVolume(AvailabilityZone=<az>, SnapshotId=<snap>, VolumeType=gp3) on a snapshot that CreateSnapshot minted and DescribeSnapshots reports in state `completed` succeeds: it returns a volume whose State is `creating` (or `available`) and whose SnapshotId echoes the request, and the volume then converges to `available`. Only a snapshot id that is genuinely absent is refused, with HTTP 400, error code InvalidSnapshot.NotFound and the fixed message "Snapshot does not exist" (no variable part — AWS does not interpolate the id on this path).
Emulator: the positive control fails. CreateVolume naming the still-existing, completed snapshot is refused HTTP 400 InvalidSnapshot.NotFound with the message "The snapshot '<snapshot-id>' does not exist." — so CreateVolume's snapshot lookup does not see snapshots that CreateSnapshot created and DescribeSnapshots serves, and the refusal message is an emulator-authored, id-interpolated sentence instead of AWS's fixed "Snapshot does not exist".
Why: restore-from-snapshot is the whole point of EBS snapshots. A client cannot create a volume from any snapshot at all, so AMI/backup/restore flows abort; and because the one refusal AWS has for this path is indistinguishable from the emulator's refusal for an existing snapshot, a caller cannot tell "your snapshot is gone" from "this service cannot resolve snapshots". The wrong message text also breaks exact-match assertions on AWS's wording.
Fix: the SnapshotId resolution in the CreateVolume handler and the snapshot state it reads (services/service-com-amazonaws-ec2-amazonec2/src/ops/create_volume.rs and the shared EBS snapshot store), plus that handler's InvalidSnapshot.NotFound message construction.
Traces: ec2.op-contract-create-volume, ec2.snapshot-in-use-by-registered-image-delete-rejected.

Cause 2 of 5: CopySnapshot accepts a structurally invalid PresignedUrl and creates a copy.
AWS: recorded behavior only. The Smithy model types PresignedUrl as CopySnapshotRequestPSU (sensitive) with no pattern, so the rule is behavioral: a PresignedUrl whose value is not an http/https URL is refused before any copy is made, with HTTP 400, error code InvalidParameterValue and the fixed message "The protocol in PresignedUrl should be http or https" (no request-derived part). No snapshot is minted and the request's TagSpecifications are not applied anywhere. The same CopySnapshot succeeds once the unneeded PresignedUrl is omitted.
Emulator: CopySnapshot(SourceRegion=<region>, SourceSnapshotId=<snap>, PresignedUrl="not-a-valid-presigned-url", TagSpecifications=[...]) answers HTTP 200 and returns a new SnapshotId plus the request's tags — the malformed parameter is never validated and a copy is created.
Why: a caller that mis-signs or mis-builds a presigned URL gets a silent success and an unexpected snapshot (and the charges/quota it consumes) instead of a deterministic client error, and validation-boundary tests that expect the refusal see a 200.
Fix: PresignedUrl validation in the CopySnapshot handler, ahead of minting the copy (services/service-com-amazonaws-ec2-amazonec2/src/ops/copy_snapshot.rs).
Traces: ec2.copy-snapshot-malformed-presigned-url-rejected.

Cause 3 of 5: DisableFastSnapshotRestores' missing-zone refusal message is emulator-authored.
AWS: recorded behavior only. The model makes AvailabilityZones and AvailabilityZoneIds both optional (SourceSnapshotIds is the only required member), so the one-of rule is enforced at runtime: a DisableFastSnapshotRestores naming neither zone form is refused HTTP 400 with error code MissingParameter and the fixed message "AvailabilityZone or AvailabilityZoneId must be specified in the request, but not both." — fixed text end to end, nothing taken from the request.
Emulator: the code is right (MissingParameter) but the message is "Either availabilityZone or availabilityZoneId must be specified" — different wording, lower-cased member names, and the "but not both" half of the rule dropped.
Why: the message is part of the contract for this error class; a client or test that matches AWS's sentence does not recognise the emulator's, and the dropped clause hides that naming BOTH forms is equally refused.
Fix: the one-of validation and its error construction in the DisableFastSnapshotRestores handler (services/service-com-amazonaws-ec2-amazonec2/src/ops/disable_fast_snapshot_restores.rs).
Traces: ec2.op-contract-disable-fast-snapshot-restores.

Cause 4 of 5: LockSnapshot answers NotImplemented, so neither its mutually-exclusive-parameter refusal nor snapshot locking exists.
AWS: recorded behavior only. LockSnapshot(SnapshotId, LockMode=governance) with BOTH LockDuration and ExpirationDate is refused HTTP 400 with error code InvalidRequest and the fixed message "You must specify either ExpirationDate or LockDuration in the request, but not both." With only LockDuration it succeeds, returning LockState="governance" and LockDuration echoing the request; DeleteSnapshot on the locked snapshot is then refused with error code SnapshotLocked and the message "The snapshot '<snapshot-id>' is locked." (the id is the request-derived part), and the snapshot survives; after UnlockSnapshot the delete succeeds and the id reads back InvalidSnapshot.NotFound with "The snapshot '<snapshot-id>' does not exist.".
Emulator: the first call already comes back with error code NotImplemented — LockSnapshot is an unimplemented scaffold stub, so the parameter-exclusivity refusal, the governance lock state and the whole lock/unlock-gates-deletion behaviour are all unreachable.
Why: snapshot lock is a data-protection control; against this emulator a snapshot that a client believes is locked is freely deletable, and a caller that mis-specifies the lock parameters gets an error class it must treat as "server broken" rather than "fix your request".
Fix: implement the LockSnapshot handler and its lock state, and the SnapshotLocked gate in DeleteSnapshot (services/service-com-amazonaws-ec2-amazonec2/src/ops/lock_snapshot.rs, .../unlock_snapshot.rs, .../delete_snapshot.rs). Note: the suite auto-filed a scaffold-stub issue naming `LockSnapshot` (services/service-com-amazonaws-ec2-amazonec2/src/ops/lock_snapshot.rs) for the sibling check ec2.op-contract-unlock-snapshot; that issue does not carry THIS check's replay key, so the cause is filed here and named there so both can be closed together.
Traces: ec2.op-contract-lock-snapshot-blocks-deletion.

Cause 5 of 5: RestoreSnapshotTier answers NotImplemented instead of the never-archived state conflict.
AWS: recorded behavior only. RestoreSnapshotTier(SnapshotId=<snap>, PermanentRestore=true) on a snapshot that is still on the standard tier (DescribeSnapshots reports StorageTier absent or "standard", i.e. never archived) is refused HTTP 400 with a state-conflict error — InvalidState with the fixed message "Cannot restore snapshot that has not been archived" (the recording also accepts the sibling classifications InvalidSnapshot.NotArchived, IncorrectState or InvalidParameterValue) — and the rejected call leaves the snapshot untouched: StorageTier stays "standard" and no RestoreExpiryTime appears.
Emulator: the call is refused with error code NotImplemented, so the state-conflict class and its message never happen and the effect-absent guarantee is unverifiable.
Why: a client distinguishing "this snapshot was never archived, nothing to restore" (a permanent caller error) from a server fault takes the wrong branch and may retry forever; archive-tier tooling cannot probe tier state at all.
Fix: implement the RestoreSnapshotTier handler, including the never-archived state check ahead of any tier mutation (services/service-com-amazonaws-ec2-amazonec2/src/ops/restore_snapshot_tier.rs) and the StorageTier/RestoreExpiryTime projection in DescribeSnapshots.
Traces: ec2.op-contract-restore-snapshot-tier-not-archived-rejected.

## Evidence
Cause 1 — ec2.op-contract-create-volume: the check creates a 1 GiB gp3 volume, snapshots it, polls the snapshot to `completed`, then runs a POSITIVE CONTROL CreateVolume from that same snapshot id. Observed: botocore.exceptions.ClientError: An error occurred (InvalidSnapshot.NotFound) when calling the CreateVolume operation: The snapshot 'snap-00000000000000027' does not exist. The same uncaught failure ends ec2.snapshot-in-use-by-registered-image-delete-rejected (identical operation, code and message). Expected for the genuinely-absent case, per the check: code InvalidSnapshot.NotFound with message exactly "Snapshot does not exist".

Cause 2 — ec2.copy-snapshot-malformed-presigned-url-rejected: AssertionError: expected the call to be rejected but it succeeded and returned {'Tags': [{'Key': 'sim2real-copysnap-marker', 'Value': 'bad-presigned'}], 'SnapshotId': 'snap-00000000000000029', 'ResponseMetadata': {... 'HTTPStatusCode': 200 ...}}. Expected: 400 InvalidParameterValue, message "The protocol in PresignedUrl should be http or https", and no new snapshot carrying the request's tag.

Cause 3 — ec2.op-contract-disable-fast-snapshot-restores: AssertionError: unexpected missing-AZ validation message: 'Either availabilityZone or availabilityZoneId must be specified'. Expected message: "AvailabilityZone or AvailabilityZoneId must be specified in the request, but not both." (error code MissingParameter already matches).

Cause 4 — ec2.op-contract-lock-snapshot-blocks-deletion: AssertionError: unexpected error code: 'NotImplemented' at the assertion `code == "InvalidRequest"` for LockSnapshot(SnapshotId=..., LockMode="governance", LockDuration=1, ExpirationDate=<now+1d>). Expected: 400 InvalidRequest, message "You must specify either ExpirationDate or LockDuration in the request, but not both."; then LockState="governance"/LockDuration=1 on the valid lock, DeleteSnapshot refused SnapshotLocked with "The snapshot '<snapshot-id>' is locked.", and the snapshot still readable.

Cause 5 — ec2.op-contract-restore-snapshot-tier-not-archived-rejected: AssertionError: unexpected error code: NotImplemented at the assertion that the code is one of InvalidSnapshot.NotArchived / IncorrectState / InvalidParameterValue / InvalidState; for InvalidState the message must be exactly "Cannot restore snapshot that has not been archived". DescribeSnapshots before the call correctly reported StorageTier "standard".

In every message above, the fixed text is the requirement; the only variable parts are the snapshot id in the SnapshotLocked / InvalidSnapshot.NotFound sentences (taken from the request) — the ids, volume ids and timestamps quoted here are one run's values, not requirements.

## Steps to Reproduce
1. git_clone(), then run_behavioral_suite().
2. Cause 1: CreateVolume(Size=1, VolumeType=gp3, AvailabilityZone=<az>) -> wait available -> CreateSnapshot -> wait `completed` -> CreateVolume(AvailabilityZone=<az>, SnapshotId=<that snapshot>, VolumeType=gp3). Expect a volume in state creating/available; observe 400 InvalidSnapshot.NotFound "The snapshot '<id>' does not exist.". Then delete the snapshot and repeat: the refusal message must be exactly "Snapshot does not exist".
3. Cause 2: with a completed snapshot, call CopySnapshot(SourceRegion=<region>, SourceSnapshotId=<snap>, PresignedUrl="not-a-valid-presigned-url", TagSpecifications=[{snapshot, marker tag}]). Expect 400 InvalidParameterValue "The protocol in PresignedUrl should be http or https" and no new tagged snapshot; observe HTTP 200 and a new SnapshotId. Re-run without PresignedUrl: it must succeed.
4. Cause 3: EnableFastSnapshotRestores(AvailabilityZones=[az], SourceSnapshotIds=[snap]), then DisableFastSnapshotRestores(SourceSnapshotIds=[snap]) with neither AvailabilityZones nor AvailabilityZoneIds. Compare the MissingParameter message with "AvailabilityZone or AvailabilityZoneId must be specified in the request, but not both."
5. Cause 4: LockSnapshot(SnapshotId=<snap>, LockMode="governance", LockDuration=1, ExpirationDate=<now+1d>) -> expect 400 InvalidRequest with the either/or message; observe NotImplemented. Then LockSnapshot with only LockDuration=1, DeleteSnapshot (must be refused SnapshotLocked and leave the snapshot readable), UnlockSnapshot, DeleteSnapshot (must now succeed), and finally DescribeSnapshots/UnlockSnapshot on the dead id (must both be InvalidSnapshot.NotFound).
6. Cause 5: with a freshly created, never-archived snapshot (DescribeSnapshots StorageTier "standard"), call RestoreSnapshotTier(SnapshotId=<snap>, PermanentRestore=True). Expect a state-conflict error (InvalidState with "Cannot restore snapshot that has not been archived"); observe NotImplemented. Re-read the snapshot: StorageTier must still be "standard" and RestoreExpiryTime must be absent.
