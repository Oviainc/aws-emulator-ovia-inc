Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 7959e6ca7cb81d5c32ad32f8e1e29786f97ad065

# ListTagsForResource accepts a resourceArn in DELETING, which its own modelled precondition forbids

_Filed: 2026-10-28T10:01:50Z_
_Operation: ListTagsForResource | Class: validation_

> _Note: this issue was identified against the code base at commit `7959e6ca7cb81d5c32ad32f8e1e29786f97ad065`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListTagsForResourceRequest$resourceArn` is documented as requiring the named resource to be active and NOT in the `DELETING` state. The handler deliberately drops that precondition: it looks the collection up by ARN and answers its stored tags whatever status it is in, so a collection whose deletion has been accepted (status `DELETING`, purge pending) is still served a 200 with its tag set. The two sibling handlers that read the same member — `TagResource` and `UntagResource` — do apply the check, with identical code (`.is_some_and(|record| !record.is_deleting())` → `ResourceNotFoundException`), so the one operation whose model text carries the same sentence is the only one that ignores it.

## Evidence

Model text for the member this handler reads (smithy_query doc, ListTagsForResourceRequest$resourceArn): "The Amazon Resource Name (ARN) of the resource. The resource must be active (not in the DELETING state), and must be owned by the account ID included in the request."

The handler states it skips that: "// A collection whose deletion has been accepted is still a collection every read\n        // reports (`BatchGetCollection` answers it in `DELETING` until the purge), so its\n        // stored tags are still readable: only the tag WRITES are refused while it is on\n        // its way out." followed by the bare lookup `let Some(record) = store.collection_by_arn(&input.resource_arn) else {` — src/ops/list_tags_for_resource.rs:155-160.

The siblings enforce it: "let taggable = store\n            .collection_by_arn(&input.resource_arn)\n            .is_some_and(|record| !record.is_deleting());\n        if !taggable { … ResourceNotFoundException …" — src/ops/tag_resource.rs:165-177 and src/ops/untag_resource.rs:160-172.

Test coverage: tests/all/encryption_indexes_and_tagging.rs:1050-1070 pins the current answer ("While DELETING: the writes are refused, the read still answers the stored tags.", `let expected = if *operation == "ListTagsForResource" { (200, "") }`), i.e. the divergence is asserted rather than justified by the model sentence quoted above.

## Steps to Reproduce

1. CreateCollection(name="c", encryptionConfig={aWSOwnedKey:true}); wait for ACTIVE; note its ARN.
2. TagResource(resourceArn=<arn>, tags=[{key:"team",value:"search"}]) → 200.
3. DeleteCollection(id=<id>) → 200, status DELETING.
4. While still DELETING: TagResource(resourceArn=<arn>, …) → ResourceNotFoundException, UntagResource(…) → ResourceNotFoundException, but ListTagsForResource(resourceArn=<arn>) → 200 with the tags, although the member's documentation says the resource must not be in the DELETING state.
