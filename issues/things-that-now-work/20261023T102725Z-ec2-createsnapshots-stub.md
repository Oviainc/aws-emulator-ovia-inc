Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-create-snapshots-from-instance
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-create-snapshots-from-instance ec2-create-snapshots-invalid-instance-error ec2-instance-dependent-ops-error-paths
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|stub:CreateSnapshots
Issue-Commit: 91a1d12adf13e91eb189076c1b2883c7fc3c7ce7

# Implement ec2 CreateSnapshots: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `91a1d12adf13e91eb189076c1b2883c7fc3c7ce7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSnapshots

## Description
`CreateSnapshots` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ec2-amazonec2/src/ops/create_snapshots.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

3 recorded unit(s) fail on their `CreateSnapshots` call, so each one unblocks only when `CreateSnapshots` works: `ec2-create-snapshots-from-instance#task#5`, `ec2-create-snapshots-invalid-instance-error#task#1`, `ec2-instance-dependent-ops-error-paths#task#3`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ec2-amazonec2/src/ops/create_snapshots.rs`. Replaying the corpus, the emulator refused the `CreateSnapshots` call: status: expected 200, got 501
$.CreateSnapshotsResponse: missing (expected {'requestId': '<shaped-not-asserted>', 'snapshotSet': {'item': {'description': 'probe multi-volume snapshot set', 'encrypted': 'false', 'ownerId': '000000000000', 'progress': '', 'snapshotId': '<shaped-not-asserted>', 'startTime': '<shaped-not-asserted>', 'state': 'pending', 'tagSet': {'item': {'key': 'Purpose', 'value': 'probe'}}, 'volumeId': '<shaped-not-asserted>', 'volumeSize': '8'}}}, got None)
$headers.content-type: value_mismatch (expected 'text/xml;charset=UTF-8', got 'text/xml')

## Steps to Reproduce
Boot the ec2 emulator
Call CreateSnapshots and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

## Resolution

Implemented: `CreateSnapshots` makes ONE snapshot of every EBS volume the named instance
holds an attachment of, stores each as the modelled `Snapshot` shape
(`state::SnapshotRecord`, in the new `state/snapshots.rs`) and answers the set read back out
of the store. The scaffold header line is gone. The volumes come from the account's volume
store through `VolumeRecord::attachments_at`, so this call and `DescribeVolumes` cannot
disagree about which volumes an instance has; the instance's ROOT volume is the attachment at
the device its AMI publishes, which is what `ExcludeBootVolume` drops and what
`ExcludeDataVolumeIds` may not name. Each snapshot is handed back `pending` with no progress
and reaches `completed` on the clock, never on a call. `DryRun` reports whether the request
would have worked without writing, and the store's record and byte ceilings are asked for the
whole set before anything is minted.

Launching an EBS-backed instance now creates that root volume
(`state::ComAmazonawsEc2Amazonec2State::mint_launched_root_volume`, called from the one place
every launch path stores an instance), restored from the AMI's root block device mapping — the
size, type, encryption state and source snapshot are the image's, and `DescribeVolumes`
reports it as the instance's. Without it no instance of this emulator has a volume, so the
rule above would never produce a snapshot through the public API.

Covered end to end by `tests/all/create_snapshots.rs`.

Still diverging, on causes this issue does not own: the recorded
`ec2-create-snapshots-from-instance` launch names an AMI the published catalog does not hold,
so that instance has no root volume to snapshot (`RunInstances` not resolving its `ImageId`
is its own defect), and the two recorded refusals match on status and error code but not yet
on AWS's message text, which the corpus withholds.
