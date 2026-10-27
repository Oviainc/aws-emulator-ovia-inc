Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 58a1ce99e9431000682e2d92b8b10e21bce3c765

# TagResource judges the reserved `aws:` tag-key prefix only after resolving the ARN, so the same bad key answers 404 where UntagResource answers 400

_Filed: 2026-10-26T23:12:56Z_
_Operation: TagResource | Class: error-response_

> _Note: this issue was identified against the code base at commit `58a1ce99e9431000682e2d92b8b10e21bce3c765`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The reserved-key rule is a property of the request alone — a key the caller may not name — and `UntagResource` applies it as the FIRST thing it does, before it reads the request's Region, before `settle`, and before the ARN is resolved. `TagResource` applies the identical rule only inside `crate::state::merge_tags`, which runs after `store.resource_tags_mut(&region, &input.resource_arn)` has already turned an ARN that names no resource into the 404 `ResourceNotFoundException`.

So one and the same request shape is graded differently by the two siblings: `{"resourceArn": <ARN of no stored resource>, "tags":[{"key":"aws:createdBy","value":"v"}]}` answers 400 `ValidationException` through `UntagResource` and 404 `ResourceNotFoundException` through `TagResource`. A client that branches on "fix the tag key" vs "the resource is gone" gets a different answer depending on which of the two operations it called with the same bad key.

Note that `TagResource` already has a pre-ARN refusal stage (the service-managed keyspace check at tag_resource.rs:177-184), so the reserved-key rule is the one check it defers rather than a stage the handler does not have.

## Evidence

The sibling that checks first, `services/service-com-amazonaws-keyspaces-keyspacesservice/src/ops/untag_resource.rs:162-181` — above `request_region`, `settle` and the ARN lookup:
```
        // Refused before anything is removed, so a refused call leaves the tag set
        // exactly as it was: a reserved key could never have been assigned by a caller
        // ("Amazon Web Services-assigned tag names ... the user cannot assign", `Tag`), so
        // refusing it says the key is one the caller does not own ...
        if let Some(reserved) = input.tags.iter()
            .find(|tag| tag.key.starts_with(crate::state::RESERVED_TAG_KEY_PREFIX))
        {
            return Err(UntagResourceErrorModeled::ValidationException(ValidationException { ... }
```
Then the ARN, `untag_resource.rs:199-204`: `let Some(tags) = store.resource_tags_mut(&region, &input.resource_arn) else { return Err(... ResourceNotFoundException ...) };`

The handler that checks last, `src/ops/tag_resource.rs:189-211` — the 404 comes first and the reserved-key rule only afterwards, inside `merge_tags`:
```
        let Some(tags) = store.resource_tags_mut(&region, &input.resource_arn) else {
            return Err(TagResourceErrorModeled::ResourceNotFoundException(
                crate::state::arn_not_found(&input.resource_arn),
            ).into());
        };
        // `merge_tags` upserts by key and refuses BEFORE storing anything ... The two refusals are the
        // reserved `aws:` key prefix (400 `ValidationException`) and the per-resource tag quota ...
        crate::state::merge_tags(tags, &input.tags).map_err(...)?;
```
and the rule itself, `src/state.rs:623-628`: `if tag.key.starts_with(RESERVED_TAG_KEY_PREFIX) { return Err(TagRejection::ReservedKeyPrefix { ... }) }`.

Test coverage: `tests/all/resource_tagging.rs:594-626` (`reserved_tag_keys_are_refused_wherever_tags_are_supplied`) walks both operations over `arns` — ARNs of resources that EXIST (`let arns = with_resources(&emulator).await;`), so the divergence for a reserved key on an unknown ARN is pinned by nothing.

## Steps to Reproduce

1. CreateKeyspace {"keyspaceName":"tag_ks"} so the Region partition exists
2. UntagResource {"resourceArn":"arn:aws:cassandra:us-east-1:<account>:/keyspace/no_such_ks/","tags":[{"key":"aws:createdBy","value":"v"}]} — 400 ValidationException naming the reserved prefix
3. TagResource with the SAME resourceArn and the SAME reserved key — 404 ResourceNotFoundException instead, so the identical reserved key is graded differently by the two siblings
