Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 7805cfa6f36158072c58d31caff72f5961880244

# TagResource accepts reserved aws:-prefixed tag keys instead of refusing them

_Filed: 2026-10-24T07:15:59Z_
_Operation: TagResource | Class: validation_

> _Note: this issue was identified against the code base at commit `7805cfa6f36158072c58d31caff72f5961880244`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: `aws:` is a reserved tag-key prefix across AWS. A caller may not create, modify or delete a tag whose key begins with `aws:` — the service refuses the request (for MediaPackage, with its modelled client-input error `UnprocessableEntityException`, HTTP 422) and the resource's tag set is left exactly as it was. The same rule applies to the tag maps supplied on `CreateChannel` / `CreateOriginEndpoint`.

Actual: no part of the crate looks at the tag KEY's content. `crate::state::bound_tag_entries` (src/state.rs ~449-466) is the one place tag limits are decided, and it checks only the entry COUNT (<=50), the key length (<=128) and the value length (<=256). `TagResource` therefore merges `{"aws:cloudformation:stack-name": "mine"}` into the stored map and answers 204, after which `ListTagsForResource` and `Describe*` report the reserved key as a user tag that the caller can also overwrite and delete through `UntagResource`. Because `CreateChannel` (create_channel.rs line 169) and `CreateOriginEndpoint` (create_origin_endpoint.rs line 214) route their `Tags` through the same `bound_tags`/`bound_tag_entries`, tag-on-create accepts reserved keys too — one fix in that helper covers all of them.

Also unvalidated in the same helper: the tag character set. AWS accepts letters, digits, spaces and `_ . : / = + - @` in keys and values; a key carrying anything else is refused, whereas here any UTF-8 string within the length bound is stored.

## Evidence

Spec grounding: `smithy_query(query="operation",operation="TagResource")` types `Tags` as the required `map<string,string>` with no key constraint, so the rule cannot come from the model's constraint walk — it is AWS's universal tagging rule ("you cannot edit or delete tag keys with the `aws:` prefix ... they do not count against your tags-per-resource limit"), which every tag-accepting AWS API enforces.

Handler: `services/service-com-amazonaws-mediapackage-mediapackage/src/ops/tag_resource.rs` lines 191-206 — the merged map is judged only by `crate::state::bound_tag_entries(...)` and then assigned (`*tags = merged;`). `src/state.rs` 449-466 shows that function performing count and length checks only; nothing anywhere in `src/` matches on an `aws:` prefix.

In-repo corroboration: the open sibling issue `20261024T051159Z-mediapackagev2-tagresource-validation.md` ("TagResource accepts reserved aws:-prefixed tag keys instead of refusing them") and `20261024T010403Z-mediapackage-vod-createpackaginggroup-validation.md` ("Tag-on-create accepts reserved `aws:` tag keys ...") file this exact gap against the two MediaPackage siblings; the v1 crate has the same cause and no issue of its own.

Test coverage: no test asserts the refusal — on the contrary, `tests/all/resource_tags.rs` `tag_set_cases()` line 189 includes the key `"aws:not:reserved_here"` in its "keys and values across the AWS tag character set" row and asserts the create/tag succeeds and the key round-trips, so the divergent acceptance is pinned by the emulator's own fixture and will have to be updated with the fix.

## Steps to Reproduce

1. Boot the emulator and point boto3 at it; mp.create_channel(Id="c1").
2. mp.tag_resource(ResourceArn="arn:aws:mediapackage:us-west-2:<account>:channels/c1", Tags={"aws:cloudformation:stack-name": "mine"}) -> observe HTTP 204; AWS refuses a reserved key with UnprocessableEntityException / 422 and changes nothing.
3. mp.list_tags_for_resource(ResourceArn=...) -> the reserved key is reported as a user tag.
4. mp.untag_resource(ResourceArn=..., TagKeys=["aws:cloudformation:stack-name"]) -> observe 204; AWS does not let a caller delete an aws:-prefixed key.
5. mp.create_channel(Id="c2", Tags={"aws:anything": "v"}) -> observe HTTP 200 with the reserved key stored on the new channel.
