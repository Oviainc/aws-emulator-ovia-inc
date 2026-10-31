Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 772fb5ab5b7496e2df886721dbd75d9882bc460e

# TagResource accepts a tags list that repeats a key, which its own modelled member documentation forbids

_Filed: 2026-10-28T21:34:46Z_
_Operation: TagResource | Class: validation_

> _Note: this issue was identified against the code base at commit `772fb5ab5b7496e2df886721dbd75d9882bc460e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`TagResourceRequest$tags` is documented in this service's Smithy model as "A list of tags (key-value pairs) to add to the resource. All tag keys in the request must be unique." That uniqueness rule cannot be enforced by the boundary validator: `Tags` is a plain `list` of `Tag` structures with only `@length(min: 0, max: 50)` and no `@uniqueItems`, and uniqueness is over the nested `key` member, not over whole list entries — so nothing ahead of the handler checks it.

The handler does not check it either. It folds the request's list into the stored tag set with a last-write-wins merge, so a request carrying the SAME key twice is answered 200 and the second occurrence silently overwrites the first instead of the request being refused with the modelled `ValidationException` ("Thrown when the HTTP request contains invalid input or is missing required input"). The caller gets no signal that half of what it sent was discarded: `ListTagsForResource` afterwards reports only the last value, and which of the two values survives is decided by list order rather than by the caller.

The same merge loop is also what the `MAX_TAGS_PER_RESOURCE` ceiling is computed over, so a request that repeats a key counts the key once toward the cap while a request that was supposed to be refused outright is instead measured and accepted.

## Evidence

The model's own rule, verbatim — `smithy_query(query="doc", shape="TagResourceRequest", member="tags")`: "<p>A list of tags (key-value pairs) to add to the resource. All tag keys in the request must be unique.</p>"

That the boundary cannot enforce it — `smithy_query(query="shape", shape="Tags,Tag")`: `"Tags": {"type": "list", "length": {"min": 0, "max": 50}, "member": "Tag"}` and `"Tag": {"type": "structure", "members": {"key": {"target": "TagKey", "required": true, "length": {"min": 1, "max": 128}}, "value": {...}}}` — no `uniqueItems`, and no constraint over the nested `key`.

The handler, which merges instead of refusing — services/service-com-amazonaws-opensearchserverless-opensearchserverless/src/ops/tag_resource.rs:179-191:
```
        // The resulting tag set, computed before anything is stored: a key the resource
        // already carries takes the new value (a tag set is a map keyed by `key`), and a
        // key it does not is added.
        let mut tags = store
            .collection_by_arn(&input.resource_arn)
            .map(|record| record.tags.clone())
            .unwrap_or_default();
        for tag in input.tags {
            match tags.iter_mut().find(|stored| stored.key == tag.key) {
                Some(stored) => stored.value = tag.value,
                None => tags.push(tag),
            }
        }
```
The second occurrence of a repeated key takes the `Some(stored) => stored.value = tag.value` arm (tag_resource.rs:188) — it is applied, not refused. The handler's only refusals are the not-found at tag_resource.rs:168-177 and the tag-count quota at tag_resource.rs:192-210; neither looks at duplicate keys.

The modelled refusal that is not raised — `TagResource` declares `ValidationException` (HTTP 400): see the handler's own contract header, services/service-com-amazonaws-opensearchserverless-opensearchserverless/src/ops/tag_resource.rs error enum `TagResourceErrorModeled::ValidationException`, which this handler never constructs.

Test coverage: tests/all/encryption_indexes_and_tagging.rs:882-925 enumerates "One row per shape of tag write: adding keys, replacing a key's value, a no-op list, removing a subset, removing a key the resource never carried, and removing them all" — every case uses distinct keys within one request (e.g. line 886 `[{ "key": "env", ... }, { "key": "owner", ... }]`). No test sends the same key twice, so nothing pins today's silent-merge behaviour.

## Steps to Reproduce

1. CreateSecurityPolicy(type="encryption", name="enc", policy='{"Rules":[{"ResourceType":"collection","Resource":["collection/*"]}],"AWSOwnedKey":true}') -> 200.
2. CreateCollection(name="logs-prod") -> 200; note `createCollectionDetail.arn`.
3. Wait for the collection to settle to ACTIVE.
4. TagResource(resourceArn=<arn>, tags=[{key:"env", value:"prod"}, {key:"env", value:"staging"}]) — the same key twice.
5. Observe HTTP 200 instead of the modelled 400 ValidationException, although `TagResourceRequest$tags` documents "All tag keys in the request must be unique".
6. ListTagsForResource(resourceArn=<arn>) -> tags contain exactly one `env` entry with value "staging": the first value the request carried was silently discarded.
