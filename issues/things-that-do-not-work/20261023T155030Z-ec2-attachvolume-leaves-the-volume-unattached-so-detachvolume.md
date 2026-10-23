Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-conflict-volume-attachment
Issue-Step: task#9
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-conflict-volume-attachment ec2-t12-ebs-volume-attach-detach
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DescribeVolumes|2xx->2xx|$
Issue-Key: ec2|DescribeVolumes|2xx->2xx|$.DescribeVolumesResponse.volumeSet.item.attachmentSet
Issue-Key: ec2|DescribeVolumes|2xx->2xx|$.DescribeVolumesResponse.volumeSet.item.attachmentSet.item.status
Issue-Key: ec2|DescribeVolumes|2xx->2xx|$.DescribeVolumesResponse.volumeSet.item.status
Issue-Key: ec2|DetachVolume|2xx->4xx|status
Issue-Key: ec2|DetachVolume|2xx->4xx|$.DetachVolumeResponse
Issue-Key: ec2|DetachVolume|2xx->4xx|$headers.content-type
Issue-Related: 20261023T102725Z-ec2-response-bodies-omit-modeled-members-and-report.md
Issue-Commit: 6d7833292dd9d4a45d8e52e2aa7ee7e5a822f7f5

# EC2 AttachVolume leaves the volume unattached so DetachVolume fails

> _Note: this issue was identified against the code base at commit `6d7833292dd9d4a45d8e52e2aa7ee7e5a822f7f5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeVolumes

## Description
Three causes with one root: a successful AttachVolume does not record an attachment in EC2 state, so every later read and the matching DetachVolume behave as though the volume were never attached.

Cause 1 of 3 — DescribeVolumes never reports the attachment.
AWS: after AttachVolume succeeds, DescribeVolumes reports the volume with status 'in-use' and an attachmentSet containing one attachment for the instance (volumeId, instanceId, device, status progressing 'attaching' -> 'attached', attachTime, deleteOnTermination, availabilityZone). Smithy: DetachVolume's output shape VolumeAttachment models exactly these members (VolumeId/InstanceId/Device/State(status)/AttachTime/DeleteOnTermination/EbsCardIndex), confirming the attachment is modeled state; the DescribeVolumes field set and the empty-string serialization of snapshotId are recorded behavior only.
Emulator: the volume stays status 'available' with attachmentSet serialized as the empty string for the full 60s poll window, and the item also omits snapshotId (expected the empty element) and the operator block. The fixture's poll for status 'in-use' therefore caps out.

Cause 2 of 3 — DetachVolume refuses the teardown detach with 400 where AWS answers 200.
AWS: 200 with a VolumeAttachment body (status 'detaching').
Emulator: 400 — it cannot find the attachment it never stored.
Trace: ec2-conflict-volume-attachment#teardown#15.

Cause 3 of 3 — same refusal on the dedicated attach/detach fixture, with the whole success body missing.
AWS: 200, DetachVolumeResponse with volumeId vol-..., instanceId, device '/dev/sdf', status 'detaching', attachTime.
Emulator: 400 and no DetachVolumeResponse at all.
Trace: ec2-t12-ebs-volume-attach-detach#task#14.

Why: the whole EBS attach/detach lifecycle is unusable. A client that attaches a volume and then waits for 'in-use' (the documented wait condition, and what the AWS waiters do) hangs until its timeout; a client that attaches and then detaches gets a hard 400 on a call that must succeed, which breaks teardown and leaves orchestration unable to release the volume. Because the detach failure is a 400 (a permanent client error) and not a 5xx, SDK retry does not help and the caller aborts.

Fix: the ec2 service crate's volume state — AttachVolume's handler must persist the attachment onto the volume (and flip its status to in-use/attaching), DescribeVolumes' serializer must render attachmentSet plus snapshotId and operator from it, and DetachVolume's handler must resolve the attachment from that state instead of rejecting.

Related open issues: 20261023T102725Z-ec2-reads-and-mutations-that-aws-answers-200.md and 20261023T102725Z-ec2-response-bodies-omit-modeled-members-and-report.md look similar but carry none of these three replay keys (owned_by: [] on all three clusters), so they are filed here. Note also a separate, NOT-filed message-only difference on DetachVolume at ec2-conflict-volume-attachment#task#13, where status and code match and only the wording of the 'available state' message differs.

Traces: ec2-conflict-volume-attachment#task#9 (representative), ec2-conflict-volume-attachment#teardown#15, ec2-t12-ebs-volume-attach-detach#task#14. Cause 1 also reproduces at ec2-t12-ebs-volume-attach-detach#task#8.

## Evidence
Replay of simparity-wire-aws-v1, 3 divergence clusters.

Cause 1 — divergence_key 'service=ec2|DescribeVolumes|status=200->200|keys=$,...attachmentSet,...attachmentSet.item.status,...operator,...snapshotId,...status,$headers.content-type', fixture ec2-conflict-volume-attachment step task#9 (also ec2-t12-ebs-volume-attach-detach#task#8):
  expected: volumeSet.item.status 'in-use', attachmentSet.item {volumeId vol-00000000000000021, instanceId {{instance_id}}, device '/dev/sdf', status 'attached', attachTime, deleteOnTermination 'false', ebsCardIndex '0'}, snapshotId '', operator {managed 'false', hiddenByDefault 'false'}
  actual:   status 'available', attachmentSet '', snapshotId absent, operator absent; detail '$: poll capped at 60s'.

Cause 2 — divergence_key 'service=ec2|DetachVolume|status=200->400|keys=status', fixture ec2-conflict-volume-attachment step teardown#15: expected_status 200, actual_status 400.

Cause 3 — divergence_key 'service=ec2|DetachVolume|status=200->400|keys=$.DetachVolumeResponse,$headers.content-type,status', fixture ec2-t12-ebs-volume-attach-detach step task#14: expected_status 200 with DetachVolumeResponse {volumeId, instanceId, device '/dev/sdf', status 'detaching', attachTime}; actual_status 400 with '$.DetachVolumeResponse: missing (... got None)'.

## Steps to Reproduce
1. git_clone() then replay_wire_corpus().
2. Replay fixture ec2-conflict-volume-attachment. At task#9 the fixture polls DescribeVolumes for the attached volume: the emulator keeps reporting status 'available' with an empty attachmentSet until the 60s poll cap.
3. Let the same fixture run to teardown#15 (DetachVolume on that volume): the emulator answers 400 where the recording has 200.
4. Replay fixture ec2-t12-ebs-volume-attach-detach to task#14 (DetachVolume): the emulator answers 400 with no DetachVolumeResponse body.
