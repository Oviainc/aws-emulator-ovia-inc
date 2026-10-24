Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: 4ffec8ed9c134267b14eefb2b325da9a0803e3ce

# Tag keys using the reserved `aws:` prefix are accepted and stored

_Filed: 2026-10-24T00:49:34Z_
_Operation: TagResource | Class: validation_

> _Note: this issue was identified against the code base at commit `4ffec8ed9c134267b14eefb2b325da9a0803e3ce`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

AWS reserves the `aws:` tag-key prefix for AWS-generated tags: a caller-supplied tag whose key begins with `aws:` is rejected with a client (validation) error, and such a tag is never stored on the resource. Expected: `TagResource` with `tags: [{"key": "aws:created-by", "value": "x"}]` is refused with a 4xx client error and the resource's tag set is unchanged.

Actual: the handler performs only the per-resource key-count check and then writes whatever keys the request carried (`stored.insert(tag.key.clone(), tag.value.clone())`), so the reserved key is accepted with HTTP 200 and is afterwards reported by `ListTagsForResource` and by `DescribeTunnel`'s `tunnel.tags`. The modeled `TagKey` pattern `^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$` admits `:`, so the boundary validator cannot catch it — the rule has to be enforced in the handler. The same omission exists on the `tags` member of `OpenTunnel`, which writes into the same tag store.

## Evidence

Spec: `com.amazonaws.iotsecuretunneling#Tag$key` -> `TagKey` (length 1..128, pattern `^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`), i.e. the model permits a leading `aws:` and expresses no reserved-prefix rule; the prefix reservation is AWS-wide tagging behavior enforced by the service at request time. `TagResource` models `ResourceNotFoundException` only, so the refusal is a non-modeled client error (the crate already has that channel: `crate::scaffolded::Scaffolded::validation`, used for other unmodeled rules in `rotate_tunnel_access_token.rs` and `list_tunnels.rs`).

Handler: `src/ops/tag_resource.rs` lines 133-164 — only the not-found check and the `MAX_TAGS_PER_TUNNEL` merge check precede the unconditional `insert`; `src/ops/open_tunnel.rs` lines 233-240 writes request tags the same way.

Tests: `tests/all/tunnel_lifecycle.rs::tags_are_keyed_by_tunnel_arn_across_every_tag_operation` and `a_tunnel_carries_at_most_its_limit_of_tags` cover ordinary keys and the count limit only; nothing asserts a reserved-prefix refusal.

## Steps to Reproduce

1. `OpenTunnel` and keep the returned `tunnelArn`.
2. Call `TagResource` with `resourceArn` = that ARN and `tags: [{"key": "aws:cloudformation:stack-name", "value": "x"}]`.
3. Observe HTTP 200 instead of a client validation error.
4. Call `ListTagsForResource` for the same ARN (or `DescribeTunnel`) and observe the reserved `aws:`-prefixed tag stored on the tunnel.
5. Same with `OpenTunnel`'s `tags` member carrying an `aws:`-prefixed key.
