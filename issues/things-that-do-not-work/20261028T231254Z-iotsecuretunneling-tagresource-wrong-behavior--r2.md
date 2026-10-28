Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: d204f113841562be393bb0d029d4ee5d3cf48852

# put_tags reports a change for a request that leaves the stored tag set identical, so TagResource stamps lastUpdatedAt when nothing changed

_Filed: 2026-10-28T23:12:54Z_
_Operation: TagResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `d204f113841562be393bb0d029d4ee5d3cf48852`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`state::put_tags` is documented as reporting whether the resource's stored tag set CHANGED, and `TagResource` uses that answer as the sole condition for stamping the tunnel's `lastUpdatedAt`. The implementation does not compute that: for each element it compares the value it just overwrote against the value it is writing, i.e. against the value an EARLIER ELEMENT OF THE SAME REQUEST may have written, not against the value the resource carried before the call.

So a request that names the same key twice, where the LAST occurrence restores the stored value and an earlier occurrence differs, leaves the stored set byte-for-byte identical yet makes `put_tags` return `true`. `TagResource` then calls `state::stamp_tunnel_updated`, moving the record's `lastUpdatedAt` for a call that changed nothing the tunnel reports — the exact case the helper's doc, the handler's own comment, and the crate's stamp test all say must leave every stamp alone.

(The same helper is called by `OpenTunnel`, which discards the boolean, so the wrong answer is only observable through `TagResource`. This is the opposite direction of the already-fixed TagResource stamping issue, which was about a change that failed to stamp; this is a non-change that stamps.)

## Evidence

The documented contract, `src/state.rs:527-534`:

    /// Merge `tags` into the tag store of the resource `arn` names, and report
    /// whether the stored set CHANGED.
    ///
    /// A tag whose key is already stored with the same value leaves the set
    /// exactly as it is, so `false` means the resource is unchanged and its
    /// `lastUpdatedAt` has nothing to record (see
    /// [`Self::stamp_tunnel_updated`]).
    pub fn put_tags(&mut self, arn: &str, tags: &[Tag]) -> bool {

The code under it, `src/state.rs:540-543` — the comparison is against what THIS insert displaced, which for a repeated key is what an earlier element of the same request wrote:

        for tag in tags {
            let previous = stored.insert(tag.key.clone(), tag.value.clone());
            changed |= previous.as_deref() != Some(tag.value.as_str());
        }

The handler that consumes the answer, `src/ops/tag_resource.rs:201-209`:

        // `DescribeTunnel` reports the tag store inside the same `Tunnel` shape
        // as `lastUpdatedAt` ("The last time the tunnel was updated"), so
        // changing a tag the tunnel reports IS an update of the tunnel and is
        // stamped on the record. A request that only re-writes the key/value
        // pairs the tunnel already carries changes nothing, and so stamps
        // nothing.
        if store.put_tags(&input.resource_arn, &input.tags) {
            store.stamp_tunnel_updated(&region, &tunnel_id, now);
        }

The rule as the crate's own test derives it, `tests/all/tag_write_stamps.rs:421-423`:

        // The rule's condition: the stamp moves exactly when the set the tunnel
        // reports is not the set it reported before.
        let changes = expected != before.tags;

Test coverage: `tests/all/tag_write_stamps.rs::a_tag_write_stamps_the_tunnel_exactly_when_it_changes_what_it_reports` spans rows for "the one stored pair written again, identically" (line 280) and "three keys at once: two new and one rewritten unchanged" (line 273), but no row names the SAME key twice inside one `tags` list, so this case is uncovered and the divergence is not caught.

## Steps to Reproduce

1. Mount the service and call `OpenTunnel` with `{"destinationConfig":{"thingName":"thing-1","services":["SSH"]},"tags":[{"key":"env","value":"test"}]}`; note `tunnelArn` and `tunnelId`.
2. Call `DescribeTunnel` with `{"tunnelId":"<id>"}` and record `tunnel.lastUpdatedAt` (optionally rewind the stored record's stamps into the past, as `tests/all/tag_write_stamps.rs` does, so the next stamp is distinguishable).
3. Call `TagResource` with `{"resourceArn":"<arn>","tags":[{"key":"env","value":"prod"},{"key":"env","value":"test"}]}` — the last occurrence restores the stored value, so the resulting set is identical to step 1's.
4. Call `ListTagsForResource` with `{"resourceArn":"<arn>"}` and confirm the tag set is still exactly `{env: test}` — the call changed nothing the tunnel reports.
5. Call `DescribeTunnel` again: `tunnel.lastUpdatedAt` has moved to the instant of the `TagResource` call, although `put_tags` is documented to answer `false` ("the resource is unchanged and its `lastUpdatedAt` has nothing to record") for a request that leaves the set as it is.
