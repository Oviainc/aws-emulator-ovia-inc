Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 87b02e9e0446b34956525b9761a3171af51589ac

# Record eviction orders closed tunnels by lastUpdatedAt, so a tagged closed tunnel survives while a tunnel closed later is destroyed

_Filed: 2026-10-28T13:36:08Z_
_Operation: OpenTunnel | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `87b02e9e0446b34956525b9761a3171af51589ac`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

When the store is at its record bound, `OpenTunnel` calls `state::ComAmazonawsIotsecuretunnelingIotsecuredtunnelingState::make_room_for_tunnel` (open_tunnel.rs:271) to reclaim a record. That function documents its selection rule as "the one closed longest ago goes first, so what a caller loses is the oldest history", and the crate's own test restates it as "the longest-closed record must be the one evicted".

The code does not implement that rule: it picks the closed record with the smallest `last_updated_at` (state.rs:457). `last_updated_at` is not the instant a tunnel closed — it is `Tunnel$lastUpdatedAt` ("The last time the tunnel was updated"), and `state::stamp_tunnel_updated` (state.rs:372-376) rewrites it on ANY record regardless of status. `TagResource` (tag_resource.rs:207-209) and `UntagResource` (untag_resource.rs:176-178) call it with no status check, and tag_resource.rs:154-157 explicitly contemplates a record the clock has just closed, so the crate knowingly stamps `last_updated_at = now` onto CLOSED tunnels.

Consequence: tagging (or untagging) a long-closed tunnel moves it to the BACK of the eviction order. The next `OpenTunnel` at the bound then destroys a record that was closed LATER — including its tags, via `remove_tunnel` (state.rs:409-415) — while the older closed record survives. That is the opposite of the rule the crate states, and the state a caller loses is the newest history rather than the oldest. A caller that can still `DescribeTunnel`/`ListTagsForResource` a recently closed tunnel loses it because an unrelated, older tunnel was tagged.

## Evidence

Documented rule, src/state.rs:437-450 (doc of `make_room_for_tunnel`):
```
/// Only a CLOSED record is ever evicted — an open tunnel is live state a
/// caller is still using — and the one closed longest ago goes first, so
/// what a caller loses is the oldest history.
```
The code under that doc, src/state.rs:456-457:
```
                .filter(|(_, tunnel)| tunnel.status != TunnelStatus::Open)
                .min_by_key(|(_, tunnel)| (tunnel.last_updated_at, tunnel.creation_seq))
```
Why `last_updated_at` is not the close instant, src/state.rs:365-376:
```
    /// `Tunnel$lastUpdatedAt` is "The last time the tunnel was updated", so
    /// every call that changes something the tunnel reports goes through here
    /// with the instant of its change
    pub fn stamp_tunnel_updated(&mut self, region: &str, tunnel_id: &str, at: SystemTime) {
        if let Some(tunnel) = self.tunnel_mut(region, tunnel_id) {
            tunnel.last_updated_at = at;
```
The writers that reach it with no status check — src/ops/tag_resource.rs:207-209:
```
        if store.put_tags(&input.resource_arn, &input.tags) {
            store.stamp_tunnel_updated(&region, &tunnel_id, now);
        }
```
and src/ops/untag_resource.rs:176-178:
```
        if store.remove_tags(&input.resource_arn, input.tag_keys.iter().map(String::as_str)) {
            store.stamp_tunnel_updated(&region, &tunnel_id, now);
        }
```
tag_resource.rs:154-157 shows a closed record is an expected subject of that stamp: "A tunnel whose lifetime has already run out is closed by the clock before this call writes its own stamp, so the stamp this call leaves is never overtaken by a close that was owed earlier."

The call site that loses the record, src/ops/open_tunnel.rs:271:
```
        if !store.make_room_for_tunnel() {
```

Test coverage: tests/all/tunnel_concurrency_quota.rs:338-364 pins the rule only for records that were never tagged after closing — `"case {case:?}: the longest-closed record must be the one evicted"` (line 363) — so the tagged-after-close case is unasserted and the divergence is uncaught.

## Steps to Reproduce

1. Mount the service and fill the store to `state::MAX_RETAINED_TUNNELS` records with OPEN tunnels, the way tests/all/tunnel_concurrency_quota.rs::the_store_holds_a_bounded_number_of_records_across_every_region does (fill each Region's concurrency quota, then move to the next Region).
2. Call `CloseTunnel {"tunnelId": A}` on one record, then `CloseTunnel {"tunnelId": B}` on a second record, so A was closed longer ago than B. Both records stay readable (no `delete`).
3. Call `TagResource {"resourceArn": <A's tunnelArn>, "tags": [{"key":"env","value":"test"}]}` — this stamps A's `last_updated_at` with the current instant.
4. Call `OpenTunnel` once more, so `make_room_for_tunnel` has to reclaim a record.
5. Call `DescribeTunnel` on A and on B: B — the record closed MORE RECENTLY — answers 404 (it was evicted, together with its tags), while A — the one closed longest ago — is still readable. The documented rule ("the one closed longest ago goes first", state.rs:443) and the test's wording ("the longest-closed record must be the one evicted") both say the opposite.
