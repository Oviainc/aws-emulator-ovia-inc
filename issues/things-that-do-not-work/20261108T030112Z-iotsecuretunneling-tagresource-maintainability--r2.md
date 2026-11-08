Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 9b76c16e6570bc8ab57c9a9c50f7ada5b1690683

# The tag store's key is derived two ways — from the stored record in two handlers, from un-normalised request input in three

_Filed: 2026-11-08T03:01:12Z_
_Operation: TagResource | Class: maintainability_

> _Note: this issue was identified against the code base at commit `9b76c16e6570bc8ab57c9a9c50f7ada5b1690683`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`state.rs` documents the tag store as "keyed by `tunnelArn`" — the tunnel record's own, service-minted ARN — and two handlers derive the key that way (`OpenTunnel` from the ARN it just minted, `DescribeTunnel` from `stored.tunnel_arn`). The three ARN-addressed handlers derive the SAME key a second way: straight from the caller-supplied `input.resource_arn`, even though each of them has already resolved the record (`store.tunnel_id_for_arn(&region, &input.resource_arn)`) and so could name the record's own ARN.

So "the tag-store key of this tunnel" is one computation written twice in this crate, and the two copies agree only by the accident of a third rule: `tunnel_id_for_arn` is a byte-for-byte `arn_index` lookup, so today the only `resource_arn` that resolves is the minted string itself. The two derivations can drift the moment that resolution stops being exact-string — exactly what open issue `20261104T093152Z-iotsecuretunneling-listtagsforresource-wrong-behavior--r2` asks for — at which point `TagResource` would write tags under the caller's spelling of the ARN while `DescribeTunnel` reads them under the record's, silently splitting one tunnel's tag set across two keys. The fix is to key every tag-store call from the resolved record's `tunnel_arn`, as the store's own doc and the two sibling handlers already do; every response and every existing test stays byte-identical, because the two keys are equal for every input that resolves today.

## Evidence

The store's documented key — the record's own `tunnelArn`, shared by all five handlers — `services/service-com-amazonaws-iotsecuretunneling-iotsecuredtunneling/src/state.rs:354-358`:

    /// The authoritative tag store, keyed by `tunnelArn` then by tag key. Shared
    /// by `OpenTunnel`, `TagResource`, `UntagResource`, `ListTagsForResource` and
    /// `DescribeTunnel`'s `tunnel.tags`. A tunnel ARN names its own Region, so
    /// this key is already Region-specific.
    pub tags: BTreeMap<String, BTreeMap<String, String>>,

Derivation #1 — from the record. `src/ops/open_tunnel.rs:318`:

            store.put_tags(&tunnel_arn, input.tags.as_deref().unwrap_or(&[]));

and `src/ops/describe_tunnel.rs:169`:

            let tags = store.projected_tag_list(&stored.tunnel_arn);

Derivation #2 — from un-normalised request input, in handlers that have already resolved the record. `src/ops/tag_resource.rs:161` resolves it:

            let Some(tunnel_id) = store.tunnel_id_for_arn(&region, &input.resource_arn) else {

then keys the store from the request instead, `src/ops/tag_resource.rs:194` and `:209`:

            if let Some(message) = store.tag_limit_refusal(&input.resource_arn, &input.tags) {
    ...
            if store.put_tags(&input.resource_arn, &input.tags) {
                store.stamp_tunnel_updated(&region, &tunnel_id, now);

the same in `src/ops/untag_resource.rs:176`:

            if store.remove_tags(&input.resource_arn, input.tag_keys.iter().map(String::as_str)) {

and in `src/ops/list_tags_for_resource.rs:168`:

            let tags = store.tag_list(&input.resource_arn);

What makes the two copies agree today — and only today — `src/state.rs:516-518`:

        pub fn tunnel_id_for_arn(&self, region: &str, arn: &str) -> Option<String> {
            self.arn_index.get(&tunnel_key(region, arn)).cloned()
        }

Test coverage that must stay unchanged, and that already crosses the two derivations (tags written through `TagResource`/`UntagResource` and read back through `DescribeTunnel` and `ListTagsForResource`): `tests/all/tunnel_tag_projection.rs:5-10` ("the `tags` member of a reported tunnel always carries the tunnel's stored tag set ... the tag store round-trips"), `tests/all/tag_write_stamps.rs`, `tests/all/shared_tag_limit_rule.rs`, `tests/all/repeated_tag_key_in_one_write.rs`, and `tests/all/owed_close_settled_by_every_entry_point.rs:379-383` ("ListTagsForResource must report the stored tags").

## Steps to Reproduce

1. Read `src/state.rs:354-358`: the tag store is documented as "keyed by `tunnelArn`", the tunnel record's own ARN, and named as shared by all five tag-touching handlers.
2. Read `src/ops/open_tunnel.rs:318` and `src/ops/describe_tunnel.rs:169`: both derive that key from the record (`&tunnel_arn`, `&stored.tunnel_arn`).
3. Read `src/ops/tag_resource.rs:161`, then `:194` and `:209`: the handler resolves the record with `tunnel_id_for_arn(&region, &input.resource_arn)` — so it holds the record — yet keys `tag_limit_refusal` and `put_tags` from the raw `input.resource_arn`. Same in `src/ops/untag_resource.rs:176` and `src/ops/list_tags_for_resource.rs:168`.
4. Read `src/state.rs:516-518`: the resolver is an exact-string `arn_index` lookup, which is the only reason the two derivations produce the same key for every request served today.
5. Confirm the fix is behaviour-neutral: replace `&input.resource_arn` in those four call sites with the resolved record's `tunnel_arn` and run `tests/all/tunnel_tag_projection.rs`, `tests/all/tag_write_stamps.rs`, `tests/all/shared_tag_limit_rule.rs` and `tests/all/owed_close_settled_by_every_entry_point.rs` — every assertion still holds, since the two strings are equal whenever the ARN resolves.
