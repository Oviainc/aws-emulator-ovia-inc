Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: d5b1aef48aa2b695d86e475a0dc12b2743f5b459

# UntagResource skips the tag-key length bound its sibling TagResource applies to the same keys

_Filed: 2026-10-28T15:03:56Z_
_Operation: UntagResource | Class: validation_

> _Note: this issue was identified against the code base at commit `d5b1aef48aa2b695d86e475a0dc12b2743f5b459`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Both tagging writes judge the tag keys a request carries on the INPUT, ahead of the store. `TagResource` runs the crate's full input judgement for an unconstrained tagging member (`crate::state::validate_tags_member(.., TagConstraints::Unconstrained)`), which is the namespace reservation PLUS the length bounds a tag key/value may hold (`is_tag_key` = 1..`TAG_KEY_MAX_LEN` characters). `UntagResource` runs only the namespace half (`validate_tag_keys_unreserved`) and nothing else, so the very same string is judged by two different rules depending on which operation carries it: a 129-character tag key is HTTP 400 `ValidationException` (fieldName `tags`) through `TagResource` and HTTP 204 success through `UntagResource`.

The skip is not a deliberate narrowing of the rule to creation: the crate already decided the opposite for the other half of the same judgement — the handler comment states the reservation "covers removal as much as creation", and `validate_tag_keys_unreserved`'s own doc names `UntagResource$tagKeys` among the members it covers — while this crate's own suite states the unconstrained-member rule as including the key length bound and lists `UntagResource$tagKeys` as one of the unconstrained members it covers. The helper (`crate::state::is_tag_key`) already exists in the same module and is reached by the sibling path; only this path does not call it.

## Evidence

The sibling applies the full input judgement — `services/.../src/ops/tag_resource.rs:215-221`:
```
            || {
                crate::state::validate_tags_member(
                    &incoming,
                    crate::state::TagConstraints::Unconstrained,
                )
                .map_err(TagResourceErrorModeled::ValidationException)
            },
```
…whose key half is `services/.../src/state.rs:829-836`:
```
        if !is_tag_key(key) {
            return Err(ValidationException {
                message: format!(
                    "tag key '{key}' is outside the 1..{TAG_KEY_MAX_LEN} characters a tag key may hold"
                ),
```
This path applies only the namespace half — `services/.../src/ops/untag_resource.rs:186-192`:
```
            || {
                crate::state::validate_tag_keys_unreserved(
                    input.tag_keys.iter().map(String::as_str),
                    "tagKeys",
                )
                .map_err(UntagResourceErrorModeled::ValidationException)
            },
```
The crate states the rule it skips, and names this member as covered by it — `services/.../tests/all/unconstrained_tag_characters.rs:5-18`:
```
//! The rule under test: a tag set arriving through a member whose target map the
//! model leaves unconstrained (`map<String,String>` ...) is held only to the tagging limits AWS applies to
//! every taggable resource — a key of 1..128 characters, a value of at most 256,
//! at most 50 tags on the resource, and no key in the namespace AWS reserves ...
//! `TagResource$tags` and `ListTagsForResource$tags` target the unconstrained
//! `Tags`, and `UntagResource$tagKeys` an unconstrained `StringList`;
```
Test coverage: the 400 for an over-long key through `TagResource` is pinned at `tests/all/unconstrained_tag_characters.rs:182-188` ("an ASCII key one character past the maximum" / "a multi-byte key one character past the maximum" -> 400 `ValidationException`, fieldName `tags`). No test exercises `UntagResource` with a key outside the bound — `tests/all/unconstrained_tag_characters.rs:152-159` only round-trips keys inside it, and `tests/all/reserved_tag_keys.rs:211,290` only exercises the namespace rule — so nothing pins the 204.

## Steps to Reproduce

1. Create a deployment parameter: `POST /catalogs/AWSMarketplace/products/prod-default-1/deployment-parameters` with `{"agreementId":"agmt-default-1","deploymentParameter":{"name":"P","secretString":"s"},"tags":{"env":"dev"}}` and keep the returned `resourceArn`.
2. Call `POST /tags/{resourceArn}` (TagResource) with `{"tags":{"<129 'k' characters>":"v"}}` — observe HTTP 400 `ValidationException` with `fieldName: "tags"` and the message "tag key '…' is outside the 1..128 characters a tag key may hold".
3. Call `DELETE /tags/{resourceArn}?tagKeys=<the same 129-character key>` (UntagResource) — observe HTTP 204 success for a string the previous step refused as not being a tag key at all.
4. Call `GET /tags/{resourceArn}` — the stored map is unchanged, confirming the 204 reported success for a request that named no tag key the service can hold.
