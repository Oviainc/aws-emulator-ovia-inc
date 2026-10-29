Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 1211f7611505016a47ab1bf8932fdc778b7fd399

# A group's Inputs and Channels read "currently using" at two different cut points, so State flips to IDLE for an input in DELETING but stays IN_USE for a channel in DELETING

_Filed: 2026-10-29T01:48:53Z_
_Operation: DescribeInputSecurityGroup | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `1211f7611505016a47ab1bf8932fdc778b7fd399`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`InputSecurityGroup$Inputs`, `$Channels` and the derived `$State` are all computed from the same pair of helpers on every read. The two helpers disagree about when a resource stops "currently using" the group: the channel side drops a channel only once it has reached the terminal `DELETED` (`!channel.is_deleted()`), while the input side drops an input as soon as its delete has been ACCEPTED (`!input.is_delete_accepted()`, i.e. already in `DELETING`).

The store's own module documentation fixes one cut point for both members, and it is the channel side's: a resource "in `DELETED` is no longer *currently using* anything, so it leaves the derived `Inputs`, `Channels`, `AttachedChannels` and `AssociatedClusterIds` lists". The doc comment on the input-side helper also claims it "drops a `DELETED` input for the same reason" as the channel side — but the code under it drops `DELETING` inputs too.

Observable consequences inside the transient delete window:
* `DescribeInputSecurityGroup` / `ListInputSecurityGroups` report the group as `IDLE` with an empty `inputs` list while the only input using it is still `DELETING`, but as `IN_USE` with the channel listed while the only channel using it is still `DELETING`.
* `BatchDelete`'s per-group delete refuses a group whose last user is a `DELETING` channel (`BadRequestException`, "is in use by …") and accepts the very same situation when the last user is a `DELETING` input.

The same split exists one slice over: `inputs_using_sdi_source` cuts at delete-accepted while `channels_attaching` cuts at deleted, so an SdiSource's `Inputs` and an input's `AttachedChannels` are measured by two different rules too.

## Evidence

The one rule the store states for both members — services/service-com-amazonaws-medialive-medialive/src/state.rs:71-80:
```
//! A delete never drops a record: a deleted channel, cluster, input, input
//! security group, network or SdiSource keeps its record in the terminal
//! `DELETED` state its own
//! state enum models, so a reader goes on reporting it there ...
//! The `DELETING` step a channel's, a cluster's, an input's and a network's
//! enum models is
//! what the clock owes between the call and that terminal state, and a resource in
//! `DELETED` is no longer *currently using* anything, so it leaves the derived
//! `Inputs`, `Channels`, `AttachedChannels` and `AssociatedClusterIds` lists.
```

The channel side, cutting at DELETED — src/state.rs:2527-2547:
```
    /// A channel that has reached the terminal `DELETED` state is no longer
    /// using anything, so it is not reported — the member says *currently*.
    pub fn channels_using_security_group(&self, group_id: &str) -> Vec<String> {
        self.channels
            .values()
            .filter(|channel| !channel.is_deleted())
```

The input side, cutting at DELETE-ACCEPTED while its own doc says DELETED — src/state.rs:2549-2570:
```
    /// The ids of the inputs currently using `group_id` as one of their input
    /// security groups — `InputSecurityGroup$Inputs` ("The list of inputs
    /// currently using this Input Security Group"). Derived from the input store
    /// for the same reason as [`Self::channels_using_security_group`], and it
    /// drops a `DELETED` input for the same reason too.
    pub fn inputs_using_security_group(&self, group_id: &str) -> Vec<String> {
        self.inputs
            .values()
            // An input whose deletion has been accepted is on its way out of the
            // account and no longer "currently using" anything — the same
            // reading of "currently using" that `inputs_using_sdi_source`
            // applies to the source side.
            .filter(|input| !input.is_delete_accepted())
```
with `is_delete_accepted` covering BOTH halves of the lifecycle — src/state.rs:5533-5535:
```
    pub fn is_delete_accepted(&self) -> bool {
        self.is_deleting() || self.is_deleted()
    }
```

Both lists, and the `State` derived from them, come out of the same read — src/state.rs:2605-2631:
```
        let group = self.input_security_groups.get(id)?;
        let inputs = self.inputs_using_security_group(id);
        let channels = self.channels_using_security_group(id);
        let state = match view { ... };
```
and the batch delete's in-use refusal is judged on that same pair — src/ops/batch_delete.rs:401-414:
```
        let inputs = state.inputs_using_security_group(id);
        let channels = state.channels_using_security_group(id);
        if !inputs.is_empty() || !channels.is_empty() {
            ... failure_code::BAD_REQUEST ...
                "Input security group {id} is in use by {} and cannot be deleted",
```

The same split on the SdiSource side — src/state.rs:2673-2686 (`inputs_using_sdi_source`, `.filter(|input| !input.is_delete_accepted())`) against src/state.rs:3452-3465 (`channels_attaching`, `.filter(|channel| !channel.is_deleted())`).

Test coverage: the group-usage suite only ever reads the group AFTER the clock has settled the delete, so neither cut point is pinned inside the DELETING window — tests/all/input_security_group.rs:538-563:
```
    // Deleting the channel releases the group: once the delete has come due the
    // group is used by nothing again.
    wait_for_the_clock();
    let (status, deleting) = call(&router, "DELETE", &format!("/prod/channels/{channel_id}"), None).await;
    ...
    wait_for_the_clock();
```
and the rule the suite states (tests/all/input_security_group.rs:434-437: "a group is `IN_USE` exactly while some resource in state references it") does not distinguish the two kinds.

## Steps to Reproduce

1. POST /prod/inputSecurityGroups with {"whitelistRules":[{"cidr":"10.0.0.0/16"}]} -> note the group id G.
2. POST /prod/inputs with {"name":"pusher","type":"RTP_PUSH","inputSecurityGroups":["G"]} -> note the input id I. GET /prod/inputSecurityGroups/G reports state IN_USE with inputs == [I].
3. DELETE /prod/inputs/I -> 200 with state DELETING. WITHOUT waiting for the clock, GET /prod/inputSecurityGroups/G -> state is already IDLE and inputs is [], although the input is still only DELETING.
4. Now the channel half of the same member pair: POST /prod/channels with {"name":"c","channelSecurityGroups":["G"],"encoderSettings":...} -> note C; wait for CREATING to settle; DELETE /prod/channels/C -> 200 with state DELETING. WITHOUT waiting for the clock, GET /prod/inputSecurityGroups/G -> state is IN_USE and channels == [C].
5. POST /prod/batch/delete with {"inputSecurityGroupIds":["G"]} inside each window: the DELETING-channel case is refused with code BadRequestException "is in use by C" (src/ops/batch_delete.rs:403-414) while the DELETING-input case is accepted — one situation, two answers, against the single cut point src/state.rs:71-80 states for both derived lists.
