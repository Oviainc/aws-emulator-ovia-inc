Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 564feb857476f3247a0693cb65aa2d44e3322d3a

# Per-tunnel tag-count limit is implemented twice: OpenTunnel hand-rolls the count instead of calling state::merged_tag_count

_Filed: 2026-10-24T21:58:30Z_
_Operation: OpenTunnel | Class: maintainability_

> _Note: this issue was identified against the code base at commit `564feb857476f3247a0693cb65aa2d44e3322d3a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The crate has exactly one rule for the per-tunnel tag-key limit ("the number of distinct tag keys the resource would end with must not exceed MAX_TAGS_PER_TUNNEL"), and it owns a helper for it on the state: `ComAmazonawsIotsecuretunnelingIotsecuredtunnelingState::merged_tag_count`, whose own doc says it is what "a caller checks ... against [`MAX_TAGS_PER_TUNNEL`] before writing".

`TagResource` calls that helper. `OpenTunnel` — the other entry point for the same rule, whose comment explicitly claims "the two tagging entry points agree" — does not: it hand-rolls the distinct-key count with its own inline `BTreeSet<&str>` collection. The two copies compute the same number today (OpenTunnel mints a fresh `tunnelArn`, so there are no already-stored tags to merge with), so no response changes; but the rule now lives in two places and the copies can drift — e.g. a future change to how keys are compared or deduplicated in `merged_tag_count` would silently not apply to tags supplied at open time, which is exactly the agreement the OpenTunnel comment asserts.

The fix is to have OpenTunnel reach the same helper for its count (the ARN is minted a few lines below the check, so the mint and the check need to be ordered accordingly), leaving the refusal status, the error code and every message string as they are.

## Evidence

Copy 1 — the helper, and `TagResource` calling it:

`src/state.rs:236-248`:
```
    /// The number of distinct tag keys `arn` would carry after merging
    /// `incoming` into what is already stored. Stores nothing: a caller checks
    /// this against [`MAX_TAGS_PER_TUNNEL`] before writing.
    pub fn merged_tag_count(&self, arn: &str, incoming: &[Tag]) -> usize {
        let mut keys: ::std::collections::BTreeSet<&str> = self
            .tags
            .get(arn)
            .map(|stored| stored.keys().map(String::as_str).collect())
            .unwrap_or_default();
        keys.extend(incoming.iter().map(|tag| tag.key.as_str()));
        keys.len()
    }
```

`src/ops/tag_resource.rs:167-173`:
```
        let merged = store.merged_tag_count(&input.resource_arn, &input.tags);
        if merged > crate::state::MAX_TAGS_PER_TUNNEL {
            return Err(crate::scaffolded::Scaffolded::validation(format!(
                "tags would take this resource to {merged} tags; a tunnel carries at most {}",
                crate::state::MAX_TAGS_PER_TUNNEL
            )));
        }
```

Copy 2 — the same rule hand-rolled in `src/ops/open_tunnel.rs:201-216`, under a comment asserting the two paths agree:
```
        // A tunnel carries at most `MAX_TAGS_PER_TUNNEL` tags, and the two tagging
        // entry points agree: refused here as well as in `TagResource`, before
        // anything is minted or stored. The modelled `TagList` length bounds one
        // request's list; this bounds what one tunnel accumulates.
        let tag_count = input.tags.as_deref().map_or(0, |tags| {
            tags.iter()
                .map(|tag| tag.key.as_str())
                .collect::<::std::collections::BTreeSet<&str>>()
                .len()
        });
        if tag_count > crate::state::MAX_TAGS_PER_TUNNEL {
```

Test coverage that must stay green and unchanged: `tests/all/tunnel_lifecycle.rs:1176-1342` `a_tunnel_carries_at_most_its_limit_of_tags` drives both paths around the bound (`open_cases` rows `1`, `MAX_TAGS_PER_TUNNEL`, `MAX_TAGS_PER_TUNNEL + 1` at lines 1192-1196, then the `TagResource` accumulation rows at 1240-1342) and asserts only the status and `__type` (`assert_eq!(body["__type"].as_str(), Some("ValidationException"), ...)` at 1228-1232), never the message text — so routing OpenTunnel through `merged_tag_count` leaves every assertion satisfied. `tests/all/reserved_tag_keys.rs:159-264` also exercises both entry points.

## Steps to Reproduce

1. Open `services/service-com-amazonaws-iotsecuretunneling-iotsecuredtunneling/src/state.rs` and read `merged_tag_count` (lines 236-248): the crate's one place for counting the distinct tag keys a resource would end up with.
2. Open `src/ops/tag_resource.rs` line 167 and observe it calls `store.merged_tag_count(&input.resource_arn, &input.tags)` before comparing against `MAX_TAGS_PER_TUNNEL`.
3. Open `src/ops/open_tunnel.rs` lines 201-216 and observe the same rule re-implemented inline with its own `BTreeSet<&str>` collection, under a comment claiming "the two tagging entry points agree".
4. Note there is no second constant or second bound — only one rule with two implementations, so a change to one copy does not reach the other.
5. Confirm with `cargo test -p service-com-amazonaws-iotsecuretunneling-iotsecuredtunneling` that `a_tunnel_carries_at_most_its_limit_of_tags` asserts only status and `__type`, so consolidating the two copies changes no response and no existing assertion.
