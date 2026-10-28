Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 6e8c3e9c5f69577c8704496d0849780bb1468b0d

# AnywhereSettings clusterId / channelPlacementGroupId are stored verbatim, skipping the reference resolution every other MediaLive Anywhere writer in this crate applies

_Filed: 2026-10-28T21:02:49Z_
_Operation: CreateChannel | Class: validation_

> _Note: this issue was identified against the code base at commit `6e8c3e9c5f69577c8704496d0849780bb1468b0d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateChannel` (and, identically, `UpdateChannel`) writes the request's `AnywhereSettings` onto the channel record with no resolution of either id it carries: `ClusterId` is never resolved against `clusters` and `ChannelPlacementGroupId` is never resolved against that cluster's `channel_placement_groups`. A channel claiming a cluster the account never created, or a placement group that cluster does not hold, is created with 201 and reported by `DescribeChannel` / `ListChannels` for the rest of its life.

This is the one cross-resource reference the crate does NOT resolve, and the crate states the rule it is skipping in two places:

* the channel-configuration module says the rule it owns is that "every id the configuration names has to resolve to a resource this account holds", because otherwise the channel is left "claiming an attachment to an input no read of the account can show (and the input-side derivation ... matching nothing)" — and it then resolves only `InputAttachments$InputId`;
* the channel-placement-group slice resolves the mirror-image reference for exactly that reason: a `ClusterId` the store holds no cluster for "is refused rather than recorded", and each node id is "RESOLVED against that cluster's nodes ... rather than recorded verbatim", because "refusing it is what keeps a group's `Nodes` and a node's own `ChannelPlacementGroups` two views of ONE association rather than two lists that can disagree".

The channel side has exactly that mirror derivation: `channels_in_cluster` derives `Cluster$ChannelIds` from `channel.anywhere_settings.cluster_id`, and `channels_in_channel_placement_group` derives `ChannelPlacementGroup$Channels` by matching BOTH halves of the channel's `anywhere_settings`. So an unresolved id leaves the two views permanently unable to agree: the channel reports a cluster/group id, while no `DescribeCluster` or `DescribeChannelPlacementGroup` in the account can ever show that channel — the condition the crate's own doc says the resolution exists to prevent.

This is not a new feature: both lookups already exist in this crate (`live_cluster`, `channel_placement_group(cluster_id, id)`) and are already used by `CreateNode`, `CreateChannelPlacementGroup` and `UpdateChannelPlacementGroup`; the channel writers simply do not call them.

Distinct from the two issues already on file for these handlers: the open `UpdateChannel` validation issue is about the `ChannelSecurityGroups` member (input security groups), and the fixed `CreateChannel` validation issue was about an empty/unusable `EncoderSettings` configuration. Neither touches `AnywhereSettings`.

## Evidence

The unresolved write — services/service-com-amazonaws-medialive-medialive/src/ops/create_channel.rs:315:
```
        channel.anywhere_settings = input.anywhere_settings;
```
and the identical write in src/ops/update_channel.rs:341-343:
```
        if let Some(anywhere_settings) = input.anywhere_settings {
            channel.anywhere_settings = Some(anywhere_settings);
        }
```
Nothing in between resolves either id: the only configuration rule the create runs is src/ops/create_channel.rs:264-270, which builds `crate::state::ChannelConfiguration { encoder_settings, input_attachments }` — `AnywhereSettings` is not a member of that struct (src/state/channel_configurations.rs:42-47).

The rule the crate states for exactly this situation — src/state/channel_configurations.rs:17-21:
```
//! * every id the configuration names has to resolve to a resource this account
//!   holds — an `InputAttachment`'s `InputId` is "The ID of the input", and an id
//!   no input of this account answers to would leave the channel claiming an
//!   attachment to an input no read of the account can show (and the input-side
//!   derivation of the channels attached to an input matching nothing).
```

The sibling writer that DOES resolve the same two ids — src/state/channel_placement_groups.rs:416-428:
```
    /// The cluster has to EXIST: a group is created inside one Cluster ("The
    /// Node and the ChannelPlacementGroup must be in the same Cluster"), so a
    /// `ClusterId` this store holds no cluster for is refused rather than
    /// recorded, which is what keeps every stored group reachable through a real
    /// cluster.
    ///
    /// Each node id the request carries is RESOLVED against that cluster's nodes
    /// ([`ComAmazonawsMedialiveMedialiveState::first_unassociable_node`]) rather than
    /// recorded verbatim: `Nodes` names "the Node that you want to associate
    /// with the ChannelPlacementGroup", so an id that names none is a request
    /// there is nothing to carry out for. Refusing it is what keeps a group's
    /// `Nodes` and a node's own `ChannelPlacementGroups` two views of ONE
    /// association rather than two lists that can disagree.
```
with the refusal at src/state/channel_placement_groups.rs:446-464 (`ChannelPlacementGroupRefusal::NoSuchCluster`, `UnassociableNode`).

The mirror derivations on the channel side, which can therefore never report such a channel — src/state/clusters.rs:733-755:
```
    /// "An array of the IDs of the Channels that are associated with this
    /// Cluster."
    ...
    pub fn channels_in_cluster(&self, cluster_id: &str) -> Vec<String> {
        ... channel.anywhere_settings ... .cluster_id.as_deref() == Some(cluster_id)
```
and src/state/channel_placement_groups.rs:685-699:
```
    pub fn channels_in_channel_placement_group(
        ...
                settings.channel_placement_group_id.as_deref() == Some(group_id)
                    && settings.cluster_id.as_deref() == Some(cluster_id)
```

The lookups the handlers would call already exist: src/state/clusters.rs:520 `pub fn live_cluster(&self, id: &str) -> Option<&ClusterRecord>` and src/state/channel_placement_groups.rs:545 `pub fn channel_placement_group(`. The response path only echoes what was stored — src/state.rs:5872-5877 `fn described_anywhere` copies both ids straight through.

Model text (no constraint of its own, which is why the service is what must refuse): `smithy_query(query="shape", shape="AnywhereSettings")` -> `ChannelPlacementGroupId`: "The ID of the channel placement group for the channel.", `ClusterId`: "The ID of the cluster for the channel."

Test coverage: the resolution rule is pinned for the group side by tests/all/channel_placement_group_node_resolution.rs. On the channel side two suites currently pass unresolved ids and would need real ids (the same adjustment the open `channelSecurityGroups` issue calls for): tests/all/channel_update.rs:219-229 round-trips `{"anywhereSettings": {"channelPlacementGroupId": "7000000", "clusterId": "5000000"}}` under the "every member an update supplies is read back" table, and tests/all/cluster_alerts.rs:551-556 creates a channel with `{"clusterId": id, "channelPlacementGroupId": "pg-1"}` where `pg-1` is not an id any `CreateChannelPlacementGroup` mints (group ids are numeric, src/state/channel_placement_groups.rs mints them from a counter). tests/all/channel_placement_groups.rs:150-163 already passes real (cluster, group) pairs, so it is unaffected.

## Steps to Reproduce

1. POST /prod/channels with {"name":"anywhere-demo","encoderSettings":<valid settings>,"anywhereSettings":{"clusterId":"5000000","channelPlacementGroupId":"7000000"}} — no CreateCluster and no CreateChannelPlacementGroup was ever called. Expected the refusal the crate's own rule gives an unresolved reference (as CreateChannelPlacementGroup gives for a ClusterId naming no cluster / a Nodes entry naming no node); the emulator answers 201.
2. GET /prod/channels/<id> — the channel reports anywhereSettings.clusterId "5000000" and channelPlacementGroupId "7000000" (src/state.rs:5872 echoes both back).
3. GET /prod/clusters — no cluster 5000000 exists, so `channels_in_cluster` (src/state/clusters.rs:742) can never list this channel under Cluster$ChannelIds, and `channels_in_channel_placement_group` (src/state/channel_placement_groups.rs:685) can never list it under ChannelPlacementGroup$Channels: the channel claims an association no read of the account can show.
4. For comparison: POST /prod/clusters/5000000/channelPlacementGroups is refused 404 (ClusterId resolves to no cluster) and POST /prod/clusters/<real>/channelPlacementGroups with {"nodes":["9999999"]} is refused with `unassociable_node_message` — the same class of reference, resolved there and not here.
5. Same for the update door: PUT /prod/channels/<id> with {"anywhereSettings":{"clusterId":"5000000","channelPlacementGroupId":"7000000"}} (src/ops/update_channel.rs:341) is accepted and stored.
