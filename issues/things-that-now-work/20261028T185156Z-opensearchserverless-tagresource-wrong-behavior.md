Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 293c15726352cf981422bfe1d7cee5f6a5389f12

# The tagging plane resolves only collection ARNs, so a collection group's tags — which this crate stores and reports — can never be written, changed or listed

_Filed: 2026-10-28T18:51:56Z_
_Operation: TagResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `293c15726352cf981422bfe1d7cee5f6a5389f12`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate holds a real tag set on every collection group: `CreateCollectionGroup` stores the `tags` the request carried, and BOTH collection-group detail shapes report it back (`createCollectionGroupDetail.tags` and `collectionGroupDetail.tags`). It also mints and hands the caller the group's own ARN (`arn:aws:aoss:<region>:<account>:collection-group/<id>`), which it returns as `arn` on those same shapes.

But all three tagging handlers resolve `resourceArn` through ONE lookup — `store.collection_by_arn(...)` — and refuse anything else with `ResourceNotFoundException`. There is no collection-group branch and no `collection_group_by_arn` sibling of `collection_by_arn`, even though the groups are stored in `store.collection_groups` with an `arn` field on every record, exactly as collections are.

Consequences, all symptoms of the one root cause:

* `TagResource(resourceArn = <the group ARN the service just returned>)` -> 404 `ResourceNotFoundException`. A collection group's tags are therefore WRITE-ONCE: only `CreateCollectionGroup` can ever set them, and nothing can add, change or correct one afterwards.
* `UntagResource` on the same ARN -> 404, so a tag put on a group at create time can never be removed for the life of the group.
* `ListTagsForResource` on the same ARN -> 404, so the service denies holding tags that `BatchGetCollectionGroup` reports for that very group in the same instant. That is a direct self-contradiction: one read says the group carries `{team: search}`, the tag read says the resource does not exist.

The model treats a collection group as a tagged resource of this service (it models `tags` on the group's create request and on both of its detail shapes, and `TagResource` is documented against "an OpenSearch Serverless resource", not against a collection specifically), so the group ARN this service itself issues is an address the tagging operations are expected to accept.

## Evidence

Model — the collection group is a tagged resource of this API:
* `TagResource`: "Associates tags with an OpenSearch Serverless resource." (smithy_query doc, shape="TagResource")
* `CreateCollectionGroupRequest$tags`: "An arbitrary set of tags (key–value pairs) to associate with the OpenSearch Serverless collection group."
* `CollectionGroupDetail$tags`: "A map of key-value pairs associated with the collection group."
* `TagResourceRequest$resourceArn`: "The Amazon Resource Name (ARN) of the resource. The resource must be active (not in the `DELETING` state), and must be owned by the account ID included in the request." — it names no resource KIND.

Repo side A — the crate stores and reports collection-group tags:
* `src/state.rs:827-830`: "/// The tags the create request carried. `CreateCollectionGroupDetail` and\n    /// `CollectionGroupDetail` both model `tags`, so unlike a collection a group reports\n    /// its own tags on a read.\n    pub tags: Vec<Tag>,"
* `src/state.rs:847`: "tags: Some(self.tags.clone())," (in `CollectionGroupRecord::create_detail`)
* `src/state.rs:865`: "tags: Some(self.tags.clone())," (in `CollectionGroupRecord::detail`)
* `src/ops/create_collection_group.rs:292`: "tags: input.tags.unwrap_or_default(),"
* `src/state.rs:435-446`: "/// `arn:aws:aoss:<region>:<account>:collection-group/<id>`.\n…\npub fn collection_group_arn(region: &str, collection_group_id: &str) -> String" — the ARN handed to the caller as `createCollectionGroupDetail.arn`, pinned by tests/all/collection_group_lifecycle.rs:196: "format!(\"arn:aws:aoss:{region}:{ACCOUNT}:collection-group/{id}\"),"

Repo side B — the tagging plane can only address a collection:
* `src/ops/tag_resource.rs:165-177`: "let taggable = store\n            .collection_by_arn(&input.resource_arn)\n            .is_some_and(|record| !record.is_deleting());\n        if !taggable {\n            return Err(TagResourceErrorModeled::ResourceNotFoundException("
* `src/ops/untag_resource.rs:160-172`: "let taggable = store\n            .collection_by_arn(&input.resource_arn)\n            .is_some_and(|record| !record.is_deleting());\n        if !taggable {\n            return Err(UntagResourceErrorModeled::ResourceNotFoundException("
* `src/ops/list_tags_for_resource.rs:160-169`: "let Some(record) = store.collection_by_arn(&input.resource_arn) else {\n            return Err(ListTagsForResourceErrorModeled::ResourceNotFoundException("
* `src/state.rs:2264-2266`: "pub fn collection_by_arn(&self, arn: &str) -> Option<&CollectionRecord> {\n        self.collections.values().find(|record| record.arn == arn)\n    }" — the pattern a group lookup would follow over `self.collection_groups`, which this crate never provides.
* `src/state.rs:1845-1848`: "/// The `resourceType` a quota refusal names for a TAGGED resource: the collections this\n/// slice tags are `collection` resources (the segment their ARN carries,\n/// `…:collection/<id>`).\npub const TAGGED_RESOURCE_TYPE: &str = \"collection\";"

Test coverage: the tagging tests exercise ONLY collection ARNs and the two ways a COLLECTION ARN can miss — tests/all/encryption_indexes_and_tagging.rs:949-975: "// A resource ARN that names no collection this account holds is the modelled\n    // not-found, for every tagging operation and for each way the ARN can miss.\n    let other_region_arn = …:collection/{id}\");\n    let absent_arn = …\n        (\"an id no collection carries\", absent_arn.clone()),\n        (\"a collection of another Region\", other_region_arn.clone()),". No test anywhere in the crate passes a `collection-group/...` ARN to TagResource, UntagResource or ListTagsForResource, so nothing pins or justifies the 404.

## Steps to Reproduce

1. CreateSecurityPolicy(type="encryption", name="enc", policy='{"Rules":[{"ResourceType":"collection","Resource":["collection/*"]}],"AWSOwnedKey":true}') -> 200 (so collections can be made, not needed for the group itself).
2. CreateCollectionGroup(name="tagged-group", standbyReplicas="ENABLED", tags=[{"key":"team","value":"search"}]) -> 200. Note the returned `createCollectionGroupDetail.arn` (e.g. `arn:aws:aoss:us-east-1:<account>:collection-group/<id>`) and that `createCollectionGroupDetail.tags` echoes the tag (src/state.rs:847).
3. BatchGetCollectionGroup(ids=[<id>]) -> 200 with `collectionGroupDetails[0].tags == [{"key":"team","value":"search"}]` (src/state.rs:865) — the service demonstrably holds the tag set.
4. ListTagsForResource(resourceArn=<that same group ARN>) -> ResourceNotFoundException, although step 3 just reported the tags of that resource (src/ops/list_tags_for_resource.rs:160).
5. TagResource(resourceArn=<that group ARN>, tags=[{"key":"team","value":"relevance"}]) -> ResourceNotFoundException, so the tag set created in step 2 can never be changed (src/ops/tag_resource.rs:165-177).
6. UntagResource(resourceArn=<that group ARN>, tagKeys=["team"]) -> ResourceNotFoundException, so it can never be removed either (src/ops/untag_resource.rs:160-172).
7. Repeat steps 4-6 with a COLLECTION ARN instead: all three answer 200, confirming the refusal comes from the single collection-only lookup rather than from the ARN being malformed.
