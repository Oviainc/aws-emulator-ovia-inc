Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: cbb9267c7abd2d15d6f906cf892ef291750ce9f0

# GetPolicyStore returns an empty `tags` object for an untagged store, where the model says the member is omitted

_Filed: 2026-10-28T19:13:26Z_
_Operation: GetPolicyStore | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `cbb9267c7abd2d15d6f906cf892ef291750ce9f0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GetPolicyStoreInput$tags` documents two cases: when the parameter is included the tags are returned, and when it is included but the store carries no tags the `tags` response parameter is OMITTED from the response. The handler implements only the first half: it maps the boolean straight onto the output member, so `tags: true` against a store with no tags yields `Some(<empty map>)` and the response carries `"tags": {}` instead of omitting the member.

A client that distinguishes "absent" from "present but empty" (the distinction the model's note exists for, and the one the handler already honours for `tags: false`) is told the store answered with a tag set when the model says it answers with nothing. Only the empty-set case is affected; a store with tags is reported correctly.

## Evidence

Smithy (smithy_query doc, shape="GetPolicyStoreInput", member="tags"): "Specifies whether to return the tags that are attached to the policy store. If this parameter is included in the API call, the tags are returned, otherwise they are not returned." <note> "If this parameter is included in the API call but there are no tags attached to the policy store, the <code>tags</code> response parameter is omitted from the response."

The code that contradicts it — services/.../src/ops/get_policy_store.rs:139-142:
```
            // The tags are reported only when the request asked for them.
            tags: input
                .tags
                .then(|| store.tags.clone().into_iter().collect()),
```
`bool::then` yields `Some(..)` whenever `input.tags` is true, including when `store.tags` is empty, so the member is serialized as `{}` rather than left absent. (Nothing downstream strips an empty map: `PolicyStore::tags` is a plain `BTreeMap` — src/state.rs:371.)

The current behaviour is observable in the crate's own test, which asserts the empty object for a store created without tags — tests/all/policy_store_authorization.rs:138-145:
```
        let expected_tags = request
            .get("tags")
            .cloned()
            .unwrap_or_else(|| json!({}));
        assert_eq!(
            fetched["tags"], expected_tags,
            "case {case:?}: tags were not persisted, got {fetched}"
        );
```
(the request on that row carries no `tags`, and the GetPolicyStore call on line 105 passes `"tags": true`). The neighbouring assertion at line 147-152 shows the member IS correctly absent when `tags` is not asked for.

## Steps to Reproduce

1. CreatePolicyStore with no `tags` member -> note policyStoreId.
2. GetPolicyStore { policyStoreId, tags: true }.
3. Observed body contains `"tags": {}`. Documented: "the tags response parameter is omitted from the response" when there are no tags attached.
4. Compare GetPolicyStore { policyStoreId } (tags not asked for): the member is absent, as the same handler already does.
