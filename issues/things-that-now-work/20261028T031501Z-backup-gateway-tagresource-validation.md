Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 8e0ac146ffee686dd3a46765dec581c593b08a30

# Tag values containing spaces are accepted although the model documents that spaces are not allowed

_Filed: 2026-10-28T03:15:01Z_
_Operation: TagResource | Class: validation_

> _Note: this issue was identified against the code base at commit `8e0ac146ffee686dd3a46765dec581c593b08a30`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The modelled `Tag` shape documents a rule its traits cannot express: "Spaces are not allowed in tag values." The `TagValue` pattern is `^[^\x00]*$`, which admits spaces, so the boundary validator cannot refuse them — the handler path has to.

`crate::state::merged_tags` is the crate's documented home for exactly this family of doc-only tag rules: its own comment says there are "Two rules, both from the model's docs rather than its traits — so the boundary validator cannot have refused them already", and enumerates the `aws:` key prefix and the per-resource tag count. The third doc-only rule from the SAME shape doc — no spaces in a tag value — is not implemented anywhere in the crate (no occurrence of a space/whitespace check in `src/`), so `TagResource` (and `CreateGateway` / `ImportHypervisorConfiguration`, which share the helper through `tag_map`) stores a tag value with spaces and `ListTagsForResource` reads it back.

## Evidence

Model (`smithy_query(query="shape",shape="Tag")`) — the sentence states the rule itself:
```
"doc": "<p>A key-value pair you can use to manage, filter, and search for your resources. Allowed characters include UTF-8 letters, numbers, and the following characters: + - = . _ : /. Spaces are not allowed in tag values.</p>"
```
and the trait that cannot enforce it: `"TagValue": {"type": "string", "length": {"min": 0, "max": 256}, "pattern": "^[^\\x00]*$"}`.

Repo side — services/service-com-amazonaws-backupgateway-backuponpremises-v20210101/src/state.rs:1925-1930 claims to hold the doc-only tag rules and lists only two:
```
/// Two rules, both from the model's docs rather than its traits — so the boundary
/// validator cannot have refused them already:
///
/// * `Tag$Key` "can't start with `aws:`" (that prefix is reserved for AWS-generated
///   tags), which the member's `@pattern` allows;
/// * a resource carries at most [`MAX_TAGS_PER_RESOURCE`] tags.
```
and the code under it, state.rs:1939-1948, checks the key prefix and the count only — the value is inserted unexamined:
```
for tag in requested {
    if tag.key.to_ascii_lowercase().starts_with("aws:") {
        return Err(format!(...));
    }
    merged.insert(tag.key.clone(), tag.value.clone());
}
```
No space/whitespace check exists in the crate (grep for `space|whitespace|is_whitespace` over `src/` matches only unrelated prose).

Test coverage: no test states a reason for accepting spaces in a VALUE — tests/all/resource_untagging.rs:231-236 carries `("full", "a value with spaces")` as incidental data for the case `"a key is removed whatever its value, including an empty one"`, and tests/all/resource_untagging.rs:165 states the space allowance for `TagKey` only ("(spaces) and digits are all legal key characters"), whose pattern `^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$` does admit `\p{Z}`.

## Steps to Reproduce

1. Issue an activation key and `CreateGateway` to get a gateway ARN.
2. Call `TagResource` with `{"ResourceARN": <gateway arn>, "Tags": [{"Key": "env", "Value": "a value with spaces"}]}`.
3. Observe 200 and the tag persisted; `ListTagsForResource` reads back `"a value with spaces"`.
4. Expected per the modelled `Tag` doc ("Spaces are not allowed in tag values."): the request is refused with the modelled `ValidationException`, the way `merged_tags` already refuses the other two doc-only tag rules, and nothing is written.
5. The same holds for `CreateGateway`/`ImportHypervisorConfiguration` `Tags`, which share `crate::state::tag_map` -> `merged_tags`.
