Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: f9b86d9948f04d96bbe5506e2764e861468ce8ee

# The ARN-to-record resolution walk is implemented twice in state.rs (read and write copies can drift)

_Filed: 2026-10-27T04:44:51Z_
_Operation: TagResource | Class: maintainability_

> _Note: this issue was identified against the code base at commit `f9b86d9948f04d96bbe5506e2764e861468ce8ee`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ComAmazonawsManagedblockchainTaigawebserviceState::tagged_record` and `::tags_for_resource_mut` implement the same resolution rule twice: the same five record kinds, in the same precedence (accessors first, then per network: the network itself, its members, its nodes, its proposals), each matched by the same stored-ARN equality. The read copy is the one the tagging operations key their `ResourceNotFoundException`/`ResourceNotReadyException` on; the write copy is the one `TagResource`/`UntagResource` mutate through. Nothing ties them together, so adding a taggable record kind, or changing the precedence, has to be done twice — and if only one copy is changed the status a refusal keys on and the tag set a write lands in come from different records, which is precisely what the read copy's own doc says must not happen.

A fix can single-home the walk (e.g. a `TaggedRecordMut` counterpart of the existing `TaggedRecord` enum, or one macro-generated walk) with no change to any response or any existing test.

## Evidence

Copy 1, read — src/state.rs:1877-1904:

```
/// The record the resource `arn` names, resolved by the ARN each record was minted
/// with and STORED — the one walk of the store [`Self::tags_for_resource`] and
/// [`Self::status_not_ready`] both read through, so the status a refusal keys on and
/// the tag set a reply reports always come from the same record.
pub fn tagged_record(&self, arn: &str) -> Option<TaggedRecord<'_>> {
    if let Some(accessor) = self.accessors.values().find(|accessor| accessor.arn == arn) {
        return Some(TaggedRecord::Accessor(accessor));
    }
    for network in self.networks.values() {
        if network.arn == arn { return Some(TaggedRecord::Network(network)); }
        if let Some(member) = network.members.values().find(|member| member.arn == arn) { ... }
        if let Some(node) = network.nodes.values().find(|node| node.arn == arn) { ... }
        if let Some(proposal) = network.proposals.values().find(|proposal| proposal.arn == arn) { ... }
    }
    None
}
```

Copy 2, write — src/state.rs:1906-1940, the same five branches in the same order:

```
/// The tag set of the resource `arn` names, to write: the same resolution
/// [`Self::tags_for_resource`] reads through, so a tag written here is the one a read
/// of that resource reports.
pub fn tags_for_resource_mut(&mut self, arn: &str) -> Option<&mut BTreeMap<String, String>> {
    if let Some(accessor) = self.accessors.values_mut().find(|accessor| accessor.arn == arn) {
        return Some(&mut accessor.tags);
    }
    for network in self.networks.values_mut() {
        if network.arn == arn { return Some(&mut network.tags); }
        if let Some(member) = network.members.values_mut().find(|member| member.arn == arn) { ... }
        if let Some(node) = network.nodes.values_mut().find(|node| node.arn == arn) { ... }
        if let Some(proposal) = network.proposals.values_mut().find(|proposal| proposal.arn == arn) { ... }
    }
    None
}
```

Both copies are on the live path of the same request: src/ops/tag_resource.rs:206 (`guard.not_ready_refusal(...)`, which goes through `tagged_record`) and src/ops/tag_resource.rs:215-216 (`guard.tags_for_resource_mut(...)`).

Existing tests that cover both copies and must stay unchanged: tests/all/resource_tags.rs:271 `tags_round_trip_through_every_taggable_resource_type`, :396 `tag_counts_span_none_to_the_documented_maximum_and_one_past_it`, :493 `an_arn_naming_nothing_stored_is_not_found`, :567 `an_arn_that_names_no_taggable_resource_is_an_invalid_request`, plus tests/all/tagging_a_resource_not_ready.rs and tests/all/reserved_tag_keys.rs.

## Steps to Reproduce

1. Open services/service-com-amazonaws-managedblockchain-taigawebservice/src/state.rs and read `tagged_record` (lines 1877-1904) next to `tags_for_resource_mut` (lines 1906-1940): the same five record kinds, same precedence, same stored-ARN equality, written out twice.
2. Note that one request uses both: src/ops/tag_resource.rs:206 resolves the status through `tagged_record` and src/ops/tag_resource.rs:215 resolves the tag set through `tags_for_resource_mut`.
3. Single-home the walk (e.g. add a `TaggedRecordMut` counterpart to the existing `TaggedRecord` enum at state.rs:1579-1586 and have both accessors delegate to it) and confirm tests/all/resource_tags.rs, tests/all/tagging_a_resource_not_ready.rs and tests/all/reserved_tag_keys.rs pass unchanged.
