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
(`state::SnapshotRecord`, in `state/snapshots.rs`) and answers the set read back out of the
store. The scaffold header line is gone. The volumes come from the account's volume store
through `VolumeRecord::attachments_at`, so this call and `DescribeVolumes` cannot disagree
about which volumes an instance has; the instance's ROOT volume is the attachment at the
device its launch put the root volume at, which is what `ExcludeBootVolume` drops and what
`ExcludeDataVolumeIds` may not name. Each snapshot is handed back `pending` with no progress
and reaches `completed` on the clock, never on a call. `DryRun` reports whether the request
would have worked without writing, and the store's record and byte ceilings are asked for the
whole set before anything is minted.

EVERY launch now gives its instance exactly one EBS root volume
(`state::ComAmazonawsEc2Amazonec2State::mint_launched_root_volume`, called from the one place
every launch path stores an instance). Its device, size, type and encryption state come from
the AMI's root block device mapping when the account can see that AMI, and otherwise from the
crate's default EBS-backed Linux image — the ONE resolution of that device,
`state::ComAmazonawsEc2Amazonec2State::launched_root_device`, which `CreateSnapshots` reads
for the boot volume too. Without it an instance launched from an AMI the published catalog
does not hold had no volume at all, so the rule above produced an empty set for it and no
describe of volumes could account for the instance.

Covered end to end by `tests/all/create_snapshots.rs`, whose last table spans the AMI a launch
names: the published Linux image, the published Windows one (another root device, a bigger
root volume) and an id no image of the account has.

Still diverging, on a cause this issue does not own: the recorded refusal of an unresolvable
instance id matches AWS on status (400) and error code (`InvalidInstanceID.Malformed`) but not
on the message text, which the corpus withholds — nine wordings drawn from EC2's recorded
message templates for that code were tried against the replay and none is the recorded one.
The replayed units also still stop on the `DescribeSnapshots`, `DeleteSnapshot`, `CreateImage`
and `CreateImageUsageReport` stubs, each its own issue.
