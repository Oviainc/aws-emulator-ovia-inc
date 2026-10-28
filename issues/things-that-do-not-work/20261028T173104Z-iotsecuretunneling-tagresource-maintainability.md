Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 4a5766ed2caf698065daa3a6a1a461ee27489a64

# Per-tunnel tag-limit rule is branched in two handlers instead of one state helper, unlike the sibling services rule

_Filed: 2026-10-28T17:31:04Z_
_Operation: TagResource | Class: maintainability_

> _Note: this issue was identified against the code base at commit `4a5766ed2caf698065daa3a6a1a461ee27489a64`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The crate enforces the per-tunnel tag-count rule ("is the merged key count past `MAX_TAGS_PER_TUNNEL`? then refuse with `LimitExceededException` carrying `tag_limit_message(count)`") by writing the SAME branch out twice, once in each tagging entry point: `TagResource` (src/ops/tag_resource.rs:191-199) and `OpenTunnel` (src/ops/open_tunnel.rs:236-244). Only the count (`state::merged_tag_count`) and the wording (`state::tag_limit_message`) are shared; the comparison operator, the threshold constant and the construction of the refusal are duplicated, so the two copies can drift (e.g. one becoming `>=`, or one forgetting to pass the merged count to the message) while every doc in the crate insists the two entry points refuse identically.

This crate already hoists the directly analogous rule exactly once: the `destinationConfig.services` bound lives in `state::stored_destination_config`, which returns the refusal MESSAGE as a `String` so each of its two callers can wrap it in its own operation-specific error enum (`OpenTunnel` and `RotateTunnelAccessToken`). The same shape applies here — a `state`-level helper that takes the store, the ARN and the incoming tags and returns `Option<String>` (the refusal message) — leaving both handlers to wrap it in their own `LimitExceededException` variant. Every response byte and every existing test stays unchanged; the only change is that the rule exists in one place, the way the crate's own sibling rule already does.

## Evidence

Copy 1 — src/ops/tag_resource.rs:191-199:
```
        let merged = store.merged_tag_count(&input.resource_arn, &input.tags);
        if merged > crate::state::MAX_TAGS_PER_TUNNEL {
            return Err(TagResourceErrorModeled::LimitExceededException(
                LimitExceededException {
                    message: Some(crate::state::tag_limit_message(merged)),
                },
            )
            .into());
        }
```

Copy 2 — src/ops/open_tunnel.rs:236-244:
```
        let tag_count = store.merged_tag_count(&tunnel_arn, input.tags.as_deref().unwrap_or(&[]));
        if tag_count > crate::state::MAX_TAGS_PER_TUNNEL {
            return Err(OpenTunnelErrorModeled::LimitExceededException(
                LimitExceededException {
                    message: Some(crate::state::tag_limit_message(tag_count)),
                },
            )
            .into());
        }
```

The crate's own precedent for hoisting a two-writer rule into `state` and returning the message so each caller wraps it in its own error enum — src/state.rs:138-154 (doc + body of `stored_destination_config`):
```
/// The single place that rule is applied, so every writer of the field —
/// `OpenTunnel` and `RotateTunnelAccessToken` — refuses the same lists in the
/// same words, and a refused request is refused before it mints or stores
/// anything.
pub fn stored_destination_config(
    member: &str,
    config: &DestinationConfig,
) -> Result<StoredDestinationConfig, String> {
    let count = config.services.len();
    if count > MAX_SERVICES_PER_TUNNEL {
        return Err(format!(
```
called from src/ops/open_tunnel.rs:180-185 and src/ops/rotate_tunnel_access_token.rs:202-207.

The duplicated-rule intent is stated in both copies' comments: src/ops/open_tunnel.rs:229-235 — "the two tagging entry points agree because both count through `state::merged_tag_count`: refused here as well as in `TagResource`" — and src/ops/tag_resource.rs:184-190 — "the refusal goes out as that modelled error, the same one `OpenTunnel` answers for this limit". The message helper itself is documented as existing for this reason, src/state.rs:113-118: "Kept beside the constant so every tagging entry point refuses in the same words."

Existing tests that pin the behaviour and must stay green unchanged: tests/all/limit_refusals.rs:127 `the_tag_limit_is_refused_with_the_modelled_limit_error_at_both_entry_points` (it drives the overshoot through `OpenTunnel` and then "Entry point 2: the same overshoot through `TagResource`", limit_refusals.rs:162-177, asserting both with the one `assert_limit_refusal` helper at limit_refusals.rs:82); tests/all/tag_key_count.rs:187 `both_tagging_entry_points_count_the_same_distinct_keys` and tag_key_count.rs:285 `the_count_is_of_the_keys_the_tunnel_would_end_up_with`; tests/all/tunnel_lifecycle.rs:1178 `a_tunnel_carries_at_most_its_limit_of_tags`.

## Steps to Reproduce

1. Open src/ops/tag_resource.rs and read lines 191-199: the merged-count comparison against `crate::state::MAX_TAGS_PER_TUNNEL` and the `LimitExceededException { message: Some(crate::state::tag_limit_message(..)) }` construction.
2. Open src/ops/open_tunnel.rs and read lines 236-244: the identical comparison and identical refusal construction, differing only in the local variable name and the operation's error enum.
3. Compare with src/state.rs:144-159, where the sibling `destinationConfig.services` bound is applied in ONE helper that returns the message as a `String`, and is called from src/ops/open_tunnel.rs:180-185 and src/ops/rotate_tunnel_access_token.rs:202-207 — the shape the tag-limit rule does not use.
4. Change `>` to `>=` in only one of the two copies and run `cargo test -p service-com-amazonaws-iotsecuretunneling-iotsecuredtunneling`: the two entry points now refuse at different counts, which is the drift a single helper makes impossible.
