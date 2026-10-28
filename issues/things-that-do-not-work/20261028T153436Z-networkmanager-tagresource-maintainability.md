Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 48bb9604e330d1bf8edda7143fefbdaecc9daf20

# The ARN -> stored-tag-list dispatch is written twice in state.rs, once read-only and once mutable

_Filed: 2026-10-28T15:34:36Z_
_Operation: TagResource | Class: maintainability_

> _Note: this issue was identified against the code base at commit `48bb9604e330d1bf8edda7143fefbdaecc9daf20`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state` resolves "which stored record does this networkmanager ARN name, and where are its tags" in TWO separate nine-arm `match parts.kind` blocks: `tags_of` (read) and `tags_of_mut` (write). The two blocks are character-for-character the same rule — same nine `*_KIND` arms in the same order, same `.get(parts.id)`, same `.filter(|record| record.arn == arn)` identity re-check, same `_ => None` fallthrough — differing only in `get` vs `get_mut` and `&record.tags` vs `&mut record.tags`. Nothing ties them together, so the copies can drift: adding a taggable kind (or changing the `record.arn == arn` identity re-check, which is what makes a scoped ARN naming the wrong parent resolve to nothing) to one block and not the other makes `TagResource`/`UntagResource` and `ListTagsForResource` disagree about which resources exist — a tag written through one ARN and then 404 on read back, or the reverse. The same `tags_of` block is additionally used as the "does this ARN still name a stored resource" probe in `put_network_resource_metadata`, so a drift in only the read copy also silently changes which metadata entries `UpdateNetworkResourceMetadata` prunes. The crate already has the pattern for collapsing this kind of per-record repetition into one statement: the `client_token_records!` macro. Collapsing the two blocks into one statement of the mapping changes no response and no existing test: both functions must keep answering exactly the same records for exactly the same ARNs.

## Evidence

Copy 1 — read, services/service-com-amazonaws-networkmanager-networkmanager/src/state.rs:5078-5128:
```
pub fn tags_of(&self, arn: &str) -> Option<&Vec<Tag>> {
    let parts = parse_resource_arn(arn)?;
    match parts.kind {
        GLOBAL_NETWORK_KIND => self
            .global_networks
            .get(parts.id)
            .filter(|record| record.arn == arn)
            .map(|record| &record.tags),
        SITE_KIND => self
            .sites
            .get(parts.id)
            .filter(|record| record.arn == arn)
            .map(|record| &record.tags),
        ... LINK_KIND, DEVICE_KIND, CONNECTION_KIND, CORE_NETWORK_KIND,
        ... ATTACHMENT_KIND, PEERING_KIND, CONNECT_PEER_KIND ...
        _ => None,
    }
}
```

Copy 2 — write, services/service-com-amazonaws-networkmanager-networkmanager/src/state.rs:5130-5181, whose own doc comment admits it is the same rule:
```
/// The stored tag list of the resource `arn` names, for writing. See [`Self::tags_of`].
pub fn tags_of_mut(&mut self, arn: &str) -> Option<&mut Vec<Tag>> {
    let parts = parse_resource_arn(arn)?;
    match parts.kind {
        GLOBAL_NETWORK_KIND => self
            .global_networks
            .get_mut(parts.id)
            .filter(|record| record.arn == arn)
            .map(|record| &mut record.tags),
        ... the same nine arms, in the same order ...
        _ => None,
    }
}
```

The two copies are reached by sibling handlers of this crate that must agree:
- services/service-com-amazonaws-networkmanager-networkmanager/src/ops/tag_resource.rs:176 — `let Some(held) = store.tags_of_mut(&input.resource_arn) else {` ... `ResourceNotFoundException`
- services/service-com-amazonaws-networkmanager-networkmanager/src/ops/untag_resource.rs:172 — `let Some(held) = store.tags_of_mut(&input.resource_arn) else {`
- services/service-com-amazonaws-networkmanager-networkmanager/src/ops/list_tags_for_resource.rs:165 — `let tags = store.tags_of(&input.resource_arn).ok_or_else(|| ... ResourceNotFoundException ...)`
- services/service-com-amazonaws-networkmanager-networkmanager/src/state.rs:4660 — `.filter(|arn| self.tags_of(arn.as_str()).is_none())` (the deleted-resource prune inside `put_network_resource_metadata`)

The crate already states this class of per-record repetition once, with a macro, at services/service-com-amazonaws-networkmanager-networkmanager/src/state.rs:8158-8177:
```
macro_rules! client_token_records {
    ($($record:ty),+ $(,)?) => { $( impl HasClientToken for $record { ... } )+ };
}
client_token_records!(AttachmentRecord, ConnectPeerRecord, CoreNetworkPolicyRecord, CoreNetworkRecord, PeeringRecord, PrefixListAssociationRecord,);
```

Test coverage that must stay green and unchanged:
- tests/all/resource_tagging.rs — "End-to-end coverage of the tagging slice — `TagResource`, `UntagResource` and `ListTagsForResource` — over every taggable resource this service holds." (resource_tagging.rs:3-4)
- tests/all/arn_scoping.rs — "the key `TagResource` / `UntagResource` / `ListTagsForResource` resolve back to the stored resource." (arn_scoping.rs:11-12)
- tests/all/list_tags_for_resource.rs — "End-to-end coverage of `ListTagsForResource` as a READER of stored tag state." (list_tags_for_resource.rs:3)
- tests/all/resource_not_found_message.rs:235,259,275 — `ListTagsForResource` on ARNs naming nothing
- tests/all/network_resource_metadata.rs — the `UpdateNetworkResourceMetadata` round trip that goes through the `tags_of` probe

## Steps to Reproduce

1. Open services/service-com-amazonaws-networkmanager-networkmanager/src/state.rs and read `tags_of` at lines 5078-5128.
2. Read `tags_of_mut` at lines 5131-5181 and compare arm by arm: the nine `*_KIND` arms, their order, the `.filter(|record| record.arn == arn)` identity re-check and the `_ => None` fallthrough are identical; only `get`/`get_mut` and `&`/`&mut` differ.
3. Confirm the two copies back operations that must agree: `store.tags_of_mut(...)` in src/ops/tag_resource.rs:176 and src/ops/untag_resource.rs:172 versus `store.tags_of(...)` in src/ops/list_tags_for_resource.rs:165 and in `put_network_resource_metadata` at src/state.rs:4660.
4. Simulate the drift: add a tenth taggable kind arm to `tags_of_mut` only. `TagResource` then stores tags on that kind while `ListTagsForResource` answers 404 for the same ARN, and `UpdateNetworkResourceMetadata` prunes that resource's metadata as if the resource were deleted — with no compile error and no test naming the missing arm.
5. Note the crate already collapses an identical per-record repetition into one statement with the `client_token_records!` macro at src/state.rs:8158-8177, so the single-statement form is available without changing any response.
