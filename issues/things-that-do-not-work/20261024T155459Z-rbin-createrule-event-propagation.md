Issue-Severity: MEDIUM
Issue-Gap-Class: event-propagation
Issue-Commit: 7b95de6c4fc8fc0be292d7fb35e4aa81be419d0a

# A retention rule has no effect on resource deletion: rbin rules are never consulted when EBS resources are deleted

_Filed: 2026-10-24T15:54:59Z_
_Operation: CreateRule | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `7b95de6c4fc8fc0be292d7fb35e4aa81be419d0a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The whole purpose of a Recycle Bin retention rule is cross-service: CreateRule documents that "Resources (of the specified type) that have at least one of these tag key and value pairs are automatically retained in the Recycle Bin upon deletion", and a Region-level rule "retains all deleted resources of the specified resource type in the Region in which the rule is created, even if the resources are not tagged". After a deletion that a rule matches, the resource must be visible in the recycle bin (EC2 `ListVolumesInRecycleBin` / `ListSnapshotsInRecycleBin` / `ListImagesInRecycleBin`, with `RecycleBinEnterTime` and a `RecycleBinExitTime` derived from the rule's retention period) and restorable (`RestoreVolumeFromRecycleBin`, etc.) until the retention period expires.

Expected: with an rbin rule for EBS_VOLUME in place, deleting a matching volume through the ec2 endpoint retains it — the volume appears in ListVolumesInRecycleBin with an exit time equal to enter time + the rule's retention period, and it leaves the bin (is permanently deleted) when that period expires.

Actual: the rbin emulator's rules are inert. The rule store lives entirely inside the rbin crate's own state type and is read by nothing outside it, so no other service can discover that a deletion must be retained; the ec2 emulator's recycle-bin state is never populated by any handler, and it has no snapshot/image recycle-bin state at all. A rule created through rbin therefore changes no observable behaviour anywhere in the emulator — it is a record that only its own control plane can read.

## Evidence

Spec/semantics: smithy_query(query="doc", shape="CreateRule") and `CreateRuleRequest$ResourceTags` — "Resources (of the specified type) that have at least one of these tag key and value pairs are automatically retained in the Recycle Bin upon deletion" / "It retains all deleted resources of the specified resource type in the Region in which the rule is created". The counterpart surface is modelled in ec2: `ListVolumesInRecycleBin`/`ListSnapshotsInRecycleBin`/`ListImagesInRecycleBin` with `VolumeRecycleBinInfo.RecycleBinEnterTime`/`RecycleBinExitTime`, and `RestoreVolumeFromRecycleBin`/`RestoreSnapshotFromRecycleBin`/`RestoreImageFromRecycleBin`.

rbin side: services/service-com-amazonaws-rbin-amazonrecyclebin/src/state.rs — `ComAmazonawsRbinAmazonrecyclebinState { rules, arn_index, next_identifier_seed }` reached only through `service_state(&*self.engine)` by this crate's own handlers; there is no exported lookup ("which rules match resource type T and tags X") and no hook invoked on deletion. create_rule.rs only inserts the record (`store.insert_rule(rule)`).

ec2 side: services/service-com-amazonaws-ec2-amazonec2/src/state.rs:18595 `move_volume_to_recycle_bin(...)` is called from NO handler (grep over src matches only its definition plus a doc comment in src/ops/list_volumes_in_recycle_bin.rs; the only callers are tests/all/fpga_image_tasks_and_volume_io.rs:1057 and tests/all/store_ceilings.rs:308). `grep -i 'rbin|recycle'` over the ec2 crate finds no reference to retention rules at all, and there is no `snapshots_in_recycle_bin`/`images_in_recycle_bin` state. ec2's `DeleteVolume` (src/ops/delete_volume.rs:4) and `DeleteSnapshot` (src/ops/delete_snapshot.rs:4) still carry the SCAFFOLDED marker, so even the deletion side of the workflow is unwritten — nothing will consult rbin when they are implemented.

Test coverage: none. The rbin suites (tests/all/*.rs) exercise only the rbin control plane; no test deletes a resource in another service and asserts retention. The ec2 recycle-bin tests seed the bin directly through state, bypassing any rule.

## Steps to Reproduce

1. POST /rules on the rbin endpoint with {"ResourceType":"EBS_VOLUME","RetentionPeriod":{"RetentionPeriodValue":7,"RetentionPeriodUnit":"DAYS"}} — 201, a Region-level rule that must retain every deleted EBS volume in the Region.
2. Create a volume through the ec2 endpoint (CreateVolume) and then delete it (DeleteVolume) — expect the deletion to be intercepted by the rule; actual the ec2 DeleteVolume handler answers HTTP 501 today and, once implemented, consults no rbin rule.
3. Call ec2 ListVolumesInRecycleBin — expect one entry for the deleted volume with RecycleBinEnterTime set and RecycleBinExitTime = enter time + 7 days; actual an empty `volumeSet`, because nothing ever calls `move_volume_to_recycle_bin`.
4. Repeat with {"ResourceType":"EBS_SNAPSHOT"} and a deleted snapshot, then call ec2 ListSnapshotsInRecycleBin — expect the retained snapshot; actual empty, and the ec2 state holds no snapshot recycle-bin collection at all.
5. DELETE /rules/{Identifier} on rbin and repeat the deletion — expected and actual are identical (no retention either way), showing the rule never influenced anything.
