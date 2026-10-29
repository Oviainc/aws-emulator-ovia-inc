Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 50acf5c50341569d447ddf04c6c32ab03f586e64

# A RequestId spent on a channel placement group the cluster has since deleted still replays that DELETED group instead of creating a new one

_Filed: 2026-10-29T03:51:33Z_
_Operation: CreateChannelPlacementGroup | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `50acf5c50341569d447ddf04c6c32ab03f586e64`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DeleteChannelPlacementGroup` is a tombstone delete: the group moves to `DELETING` and the clock settles it into the terminal `DELETED`, and the record stays in `self.channel_placement_groups` so `DescribeChannelPlacementGroup`/`ListChannelPlacementGroups` keep reporting it there.

`settle_channel_placement_groups` materialises that transition but — unlike `settle_clusters` in the sibling slice, and unlike the channel and network arms of `ComAmazonawsMedialiveMedialiveState::settle`, and unlike `delete_signal_map` — it never forgets the group's idempotency tokens. `channel_placement_group_for_token` resolves a token through `Self::channel_placement_group`, which carries no state filter at all (it only re-checks the cluster), so it still answers for the tombstone, and `channel_placement_group_description` happily projects that tombstone with `state: "DELETED"`.

So a later `CreateChannelPlacementGroup` on the same cluster carrying the `RequestId` of a group that has since been deleted takes the handler's replay branch and answers 201 with the DELETED group — a group `UpdateChannelPlacementGroup` refuses with 404 "already deleted", and whose name the slice has already freed for a new group (`channel_placement_group_named` skips a delete-accepted group) — while creating and storing nothing. The caller is handed a success naming a resource it can neither use nor change, and the group its request described never exists.

The handler's own comment beside `record_channel_placement_group_token` asserts the invariant that is missing ("neither accumulate past the cap nor name a group of another cluster" — it names the cluster half of the rule and the lifetime half is not enforced anywhere), and the slice already owns the predicate the token lookup would need (`ChannelPlacementGroupRecord::is_delete_accepted`), which the update and the create's own name lookup both resolve against. Only the token path does not.

## Evidence

The rule the shared token helper states — services/service-com-amazonaws-medialive-medialive/src/state.rs:1790-1796:
```
    /// Drop every token that created `resource_id`, so a token never outlives
    /// the resource it minted.
    fn forget(&mut self, resource_id: &str) {
        self.tokens.retain(|_, created| created != resource_id);
```

The sibling slice implements it — src/state/clusters.rs:463-480:
```
    pub(super) fn settle_clusters(&mut self, now: SystemTime) {
        ...
        // A token never outlives the cluster it minted: once that cluster has
        // reached the terminal `DELETED` state its idempotency window is over,
        // so a retried create mints a new cluster rather than answering with a
        // deleted one — the rule the channel and network tokens follow too.
        for id in just_deleted {
            self.cluster_tokens.forget(&id);
        }
    }
```
(and src/state.rs:2459-2465 for `channel_tokens`, src/state.rs:2472-2485 for `network_tokens`, src/state.rs:3856-3858 for `signal_map_tokens`: "A token never outlives the resource it minted, so a retried create cannot resurrect a deleted signal map.")

The group slice does not — src/state/channel_placement_groups.rs:389-398:
```
impl ComAmazonawsMedialiveMedialiveState {
    /// Materialise every channel placement group transition the clock had
    /// already made due as of `now`. ...
    pub(super) fn settle_channel_placement_groups(&mut self, now: SystemTime) {
        for group in self.channel_placement_groups.values_mut() {
            group.settle(now);
        }
    }
```
(`grep -n '\.forget(' src` lists `alarm_template_tokens`, `alarm_template_group_tokens`, `event_bridge_rule_template_tokens`, `event_bridge_rule_template_group_tokens`, `cluster_tokens`, `channel_tokens`, `network_tokens`, `signal_map_tokens`; `channel_placement_group_tokens` is absent.)

The token lookup that therefore still answers for a deleted group — src/state/channel_placement_groups.rs:797-811:
```
    /// The group a `RequestId` presented for `cluster_id` created, if that group
    /// still exists in that cluster.
    ///
    /// The cluster is part of the key AND re-checked on the record, so a token
    /// can only ever surface a group of the cluster the request named.
    pub fn channel_placement_group_for_token(
        &self,
        cluster_id: &str,
        request_id: &str,
    ) -> Option<&ChannelPlacementGroupRecord> {
        let id = self
            .channel_placement_group_tokens
            .created(&channel_placement_group_token_key(cluster_id, request_id))?;
        self.channel_placement_group(cluster_id, id)
    }
```
and that lookup has no state filter — src/state/channel_placement_groups.rs:545-553:
```
    pub fn channel_placement_group(
        &self,
        cluster_id: &str,
        id: &str,
    ) -> Option<&ChannelPlacementGroupRecord> {
        self.channel_placement_groups
            .get(id)
            .filter(|group| group.cluster_id == cluster_id)
    }
```
while the predicate every MUTATION of a group resolves against sits in the same file — src/state/channel_placement_groups.rs:245-253:
```
    /// Whether a delete has already been accepted for this group — it is in the
    /// transient `DELETING` state the clock owes the terminal one, or in that
    /// terminal `DELETED` state itself. The notion a MUTATION of the group
    /// resolves against, exactly as [`super::ChannelRecord::is_delete_accepted`]
    /// does for a channel.
    pub fn is_delete_accepted(&self) -> bool {
```

The replay branch that answers with the tombstone — src/ops/create_channel_placement_group.rs:202-220:
```
        // A retried create (same `@idempotencyToken` `RequestId` for the same
        // cluster) must answer with the group that one created, not a second
        // group.
        if let Some(existing) = input
            .request_id
            .as_deref()
            .and_then(|token| {
                state.channel_placement_group_for_token(&input.cluster_id, token)
            })
            .map(|group| group.id.clone())
        {
            let group = state
                .channel_placement_group_description(&input.cluster_id, &existing)
                .ok_or_else(unavailable)?;
            return Ok(crate::state::channel_placement_group_output!(
                CreateChannelPlacementGroupOutput,
                group
            ));
        }
```
and the projection reports the tombstone rather than refusing — src/state/channel_placement_groups.rs:710-726 (`channel_placement_group_description` -> `state: Some(group.state().to_string())`).

The replayed id is unusable — src/state/channel_placement_groups.rs:617-623:
```
        match self.channel_placement_group(cluster_id, id) {
            None => return Err(ChannelPlacementGroupUpdateRefusal::NoSuchGroup),
            Some(group) if group.is_delete_accepted() => {
                return Err(ChannelPlacementGroupUpdateRefusal::AlreadyDeleted)
            }
```
answered 404 "already deleted" — src/ops/update_channel_placement_group.rs:282-288.

And the name the tombstone carried is already free for a real create — src/state/channel_placement_groups.rs:574-577 + 585-595:
```
    /// A group whose delete has been accepted reserves its name no longer: it
    /// is on its way out of the account, so the name it carried is free for a
    /// new group ...
    fn channel_placement_group_named(...) -> Option<&ChannelPlacementGroupRecord> {
        self.channel_placement_groups.values().find(|group| {
            group.cluster_id == cluster_id
                && !group.is_delete_accepted()
```

Test coverage: the group idempotency table covers only live-group rows and never re-presents a token after a delete — tests/all/channel_placement_groups.rs:818-836 (`a_retried_create_returns_the_group_the_first_one_created`, rows "the same token" / "two tokens" / "no token at all" / "only the second names one" / "the same token, another cluster"), while the same rule past the delete IS pinned for a resource that implements it — tests/all/signal_map_deletion.rs (`a_deleted_signal_map_is_not_resurrected_by_its_idempotency_token`).

## Steps to Reproduce

1. POST /prod/clusters with a name and valid networkSettings; poll GET /prod/clusters/{C} until state is ACTIVE.
2. POST /prod/clusters/{C}/nodes to register a node N of that cluster.
3. POST /prod/clusters/{C}/channelplacementgroups with {"name":"group-a","nodes":["N"],"requestId":"token-a"} -> 201; note the group id G.
4. DELETE /prod/clusters/{C}/channelplacementgroups/{G} -> the group reports state DELETING; wait for the transient span and confirm GET /prod/clusters/{C}/channelplacementgroups/{G} reports state DELETED (the record is kept, src/state/channel_placement_groups.rs:255-262).
5. POST /prod/clusters/{C}/channelplacementgroups again with {"name":"group-b","requestId":"token-a"} -> the emulator answers 201 carrying group G with "state":"DELETED" and creates nothing; GET /prod/clusters/{C}/channelplacementgroups shows no new group. Expected, per src/state.rs:1790-1792 and src/state/clusters.rs:473-479, a freshly minted group — which is what POST /prod/clusters gives for a token whose cluster was deleted.
6. PUT /prod/clusters/{C}/channelplacementgroups/{G} with {"name":"group-b"} -> 404 "already deleted" (src/ops/update_channel_placement_group.rs:282-288), so the 201 in the previous step named a group the caller can neither use nor change.
7. Repeat step 5 with a fresh token and the name "group-a": it succeeds, proving the tombstone no longer reserves anything and only the stale token blocked the create.
