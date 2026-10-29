Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: 772fb5ab5b7496e2df886721dbd75d9882bc460e

# UntagResource accepts a tagKeys list that repeats a key, which its own modelled member documentation forbids

_Filed: 2026-10-28T21:34:46Z_
_Operation: UntagResource | Class: validation_

> _Note: this issue was identified against the code base at commit `772fb5ab5b7496e2df886721dbd75d9882bc460e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UntagResourceRequest$tagKeys` is documented in this service's Smithy model as "The tag or set of tags to remove from the resource. All tag keys in the request must be unique." The boundary validator cannot enforce that: `TagKeys` is a plain `list` with `@length(min: 0, max: 50)` and a per-member `@length`, and carries no `@uniqueItems`.

The handler does not enforce it either. It removes keys with a single `retain` pass over the stored tag set, so a `tagKeys` list that names the same key twice is answered 200 and the duplicate is simply ignored, instead of the request being refused with the modelled `ValidationException` ("Thrown when the HTTP request contains invalid input or is missing required input"). This is the same rule its sibling write `TagResource` is given verbatim by the model and also skips, so the crate enforces the uniqueness rule on neither side of the tagging plane.

## Evidence

The model's own rule, verbatim — `smithy_query(query="doc", shape="UntagResourceRequest", member="tagKeys")`: "<p>The tag or set of tags to remove from the resource. All tag keys in the request must be unique.</p>"

That the boundary cannot enforce it — `smithy_query(query="shape", shape="TagKeys")`: `{"type": "list", "length": {"min": 0, "max": 50}, "member": {"target": "TagKey", "length": {"min": 1, "max": 128}}}` — no `uniqueItems`.

The handler, which never inspects the list for repeats — services/service-com-amazonaws-opensearchserverless-opensearchserverless/src/ops/untag_resource.rs:184-188:
```
        // Removing a key the resource does not carry removes nothing: the request asks for
        // a state (these keys absent) rather than for an event, so it is not a failure.
        record
            .tags
            .retain(|tag| !input.tag_keys.contains(&tag.key));
```
The only other exits in the body are the store-unreachable 500 (untag_resource.rs:145-152) and the two not-found refusals (untag_resource.rs:163-182); `UntagResourceErrorModeled::ValidationException` is declared for this operation and never constructed.

Test coverage: tests/all/encryption_indexes_and_tagging.rs:907-924 covers "removing one key" (`tagKeys: ["owner"]`), "removing a key the resource does not carry" (`tagKeys: ["never-set"]`) and "removing the rest" (`tagKeys: ["env", "team"]`) — all with distinct keys, so no test pins the duplicate case.

## Steps to Reproduce

1. CreateSecurityPolicy(type="encryption", name="enc", policy='{"Rules":[{"ResourceType":"collection","Resource":["collection/*"]}],"AWSOwnedKey":true}') -> 200.
2. CreateCollection(name="logs-prod", tags=[{key:"env", value:"prod"}]) -> 200; note `createCollectionDetail.arn`.
3. Wait for the collection to settle to ACTIVE.
4. UntagResource(resourceArn=<arn>, tagKeys=["env", "env"]) — the same key twice.
5. Observe HTTP 200 instead of the modelled 400 ValidationException, although `UntagResourceRequest$tagKeys` documents "All tag keys in the request must be unique".
