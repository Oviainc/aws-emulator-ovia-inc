Issue-Source: trace-sweep-divergence
Issue-Trace: medialive-cmc-traj-attached-input-conflicts-and-delete-order
Issue-Step: teardown#16
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: medialive-cmc-traj-attached-input-conflicts-and-delete-order medialive-cmc-traj-role-mistakes-and-idempotent-retry
Issue-Shape-Id: com.amazonaws.medialive#MediaLive
Issue-Key: medialive|DeleteInputSecurityGroup|2xx->2xx|$.inputs[*]
Issue-Key: medialive|DeleteInputSecurityGroup|2xx->2xx|$.state
Issue-Commit: 4794fe8886d36994e3b764249716f059e206492d

# medialive input security group stays IN_USE with a stale input id after the attached input is deleted

> _Note: this issue was identified against the code base at commit `4794fe8886d36994e3b764249716f059e206492d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteInputSecurityGroup

## Description
MediaLive's input-security-group attachment bookkeeping is never recomputed when an input that referenced the group goes away, so the group is reported as still carrying that input and as still being IN_USE forever. This bundle has 1 cause; it must be closed.

Cause 1 of 1 - the input security group's `inputs` list and `state` are not recomputed when the input using it is deleted (fixture medialive-cmc-traj-attached-input-conflicts-and-delete-order, step teardown#16).

AWS: an input security group's membership and state are derived, not stored independently. Smithy (com.amazonaws.medialive#MediaLive): shape InputSecurityGroup member Inputs (jsonName "inputs", list of __string) is documented as "The list of inputs currently using this Input Security Group." and member State (jsonName "state", enum InputSecurityGroupState with values IDLE | IN_USE | UPDATING | DELETED) as "The current state of the Input Security Group." DescribeInputSecurityGroup (GET /prod/inputSecurityGroups/{InputSecurityGroupId}, code 200) returns exactly those members. Because the list is the inputs CURRENTLY using the group, an input that has been deleted is no longer a member, and a group that no input and no channel uses is IDLE, not IN_USE. At this point in the recorded teardown the input has already been deleted, so AWS reports the group with no `inputs` entries and state "IDLE". (Smithy note: DeleteInputSecurityGroupResponse is an empty structure - DELETE /prod/inputSecurityGroups/{InputSecurityGroupId}, code 200 - so the group projection compared at this step is the group's recorded state at delete time, and its two diverging members are `inputs` and `state`.)

Emulator: status matches (200 expected, 200 actual) but the group projection still carries the already-deleted input in its `inputs` list and still reports state "IN_USE". Replay detail: "$.inputs[0]: extra (expected None, got '1000000')" and "$.state: value_mismatch (expected 'IDLE', got 'IN_USE')". The group's membership is evidently a list appended at input-create time and never pruned, and `state` is a stored field never re-derived from it.

Why: a client tearing down in the documented order (delete the channels, delete the inputs, then delete the security group) gates on the group reporting IDLE / an empty `inputs` list before it releases the group. Against the emulator that condition is never reached: the group is permanently IN_USE and permanently names an input id that no longer resolves, so the client either loops until its own timeout or concludes resources are still live that are not. A client reading `inputs` to find out what still depends on the group is handed a dangling reference.

Fix: the medialive state module's input-security-group bookkeeping, driven from the input delete path - deleting (or detaching) an input must remove that input's id from every security group's `inputs` list, and the group's `state` must be re-derived from the remaining `inputs` and `channels` (IDLE when both are empty, IN_USE while either is non-empty) rather than held as a field written once at attach time. The same derivation must back the `inputs`/`state` members the DescribeInputSecurityGroup / ListInputSecurityGroups serializers emit.

Traces: medialive-cmc-traj-attached-input-conflicts-and-delete-order#teardown#16; the cluster also covers medialive-cmc-traj-role-mistakes-and-idempotent-retry#teardown#17.

Related open issues (none of them covers this cluster, which the replay reports as unowned): 20261102T091340Z-medialive-input-lifecycle-diverges-deletes-never-detach-conflicts.md owns the input/channel lifecycle itself - its DeleteInputSecurityGroup keys are the 4xx->2xx refusal at task#10 and its remaining keys are DeleteInput/DescribeInput - and its prose only notes in passing that the group "also stays IN_USE"; it does not carry this 200->200 `$.inputs`/`$.state` body key. 20261103T014723Z-medialive-refusals-missing-or-misworded-cross-field-reference.md owns DeleteInputSecurityGroup's refusal wording, not the group projection's membership and state. 20261103T014723Z-medialive-channel-tombstones-security-group-tags-and-pull.md owns security-group TAGS, not `inputs`/`state`.

## Evidence
From the replay of simparity-wire-aws-v1 against the emulator built from this checkout (verdict=mismatch, service=medialive, operation=DeleteInputSecurityGroup, expected_status=200, actual_status=200).

Divergence key: service=medialive|DeleteInputSecurityGroup|status=200->200|keys=$.inputs[0],$.state

Representative: medialive-cmc-traj-attached-input-conflicts-and-delete-order#teardown#16
Also affected (same cluster, affected_count=2): medialive-cmc-traj-role-mistakes-and-idempotent-retry#teardown#17

Replay detail, verbatim:
  $.inputs[0]: extra (expected None, got '1000000')
  $.state: value_mismatch (expected 'IDLE', got 'IN_USE')

Expected (AWS): `inputs` has no entries - the input that used the group has been deleted and is therefore no longer "currently using" it - and `state` is the IDLE member of InputSecurityGroupState.
Actual (emulator): `inputs` still holds the id of the deleted input and `state` is IN_USE.

The requirement is the derivation, not the recorded values: the id '1000000' is the per-run input id the fixture happened to mint and is only the value filling the `inputs` slot; what is required is that a deleted/detached input is absent from the list and that `state` is IDLE exactly when `inputs` and `channels` are both empty, IN_USE while either is non-empty. No error code or message is at issue here - both sides answer 200 - so the divergence is purely the two body members.

Model references (services/service-com-amazonaws-medialive-medialive/smithy-model/com-amazonaws-medialive-medialive.json): InputSecurityGroup.Inputs jsonName "inputs", doc "The list of inputs currently using this Input Security Group."; InputSecurityGroup.State jsonName "state", doc "The current state of the Input Security Group."; InputSecurityGroupState enum values [IDLE, IN_USE, UPDATING, DELETED]; DescribeInputSecurityGroup GET /prod/inputSecurityGroups/{InputSecurityGroupId} code 200 returns arn/id/inputs/state/tags/whitelistRules/channels; DeleteInputSecurityGroup DELETE /prod/inputSecurityGroups/{InputSecurityGroupId} code 200, DeleteInputSecurityGroupResponse is an empty structure.

## Steps to Reproduce
1. Clone this checkout and boot the emulator from it, then replay the simparity-wire-aws-v1 corpus (replay_wire_corpus).
2. Fixture medialive-cmc-traj-attached-input-conflicts-and-delete-order: create an input security group (POST /prod/inputSecurityGroups) and create an input in that group (POST /prod/inputs with securityGroups naming the group id).
3. Observe the group now reports state "IN_USE" with the new input id in `inputs` - correct at this point.
4. Delete the channel(s) holding the input, then delete the input (DELETE /prod/inputs/{InputId}).
5. Reach the group's teardown step (step teardown#16) and inspect the input-security-group projection: expect `inputs` to contain no entries and `state` to be "IDLE", observe the deleted input id still present at $.inputs[0] and `state` still "IN_USE".
6. The same cluster reproduces at medialive-cmc-traj-role-mistakes-and-idempotent-retry#teardown#17.

Note: the input lifecycle defects filed under 20261102T091340Z-medialive-input-lifecycle-diverges-deletes-never-detach-conflicts.md can keep the input from reaching DELETED in the first place; the group's `inputs`/`state` derivation must be fixed independently, since even a cleanly deleted input must leave the group's membership list and return it to IDLE.
