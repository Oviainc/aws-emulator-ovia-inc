Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: de2d8767cb82ce68bb0f28dff69738907970fd37

# UntagResource accepts a tagKeys entry carrying no characters and answers 200, where the sibling tag-write door refuses the same key with 400

_Filed: 2026-10-29T15:42:44Z_
_Operation: UntagResource | Class: validation_

> _Note: this issue was identified against the code base at commit `de2d8767cb82ce68bb0f28dff69738907970fd37`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UntagResource` is the second of the two write doors onto a record's one tag map (the other being `TagResource`; both go through `ComAmazonawsMediaconvertMediaconvertState::tags_for_write`). `TagResource` holds the tag set it is handed to the crate's one tag-set rule, `crate::state::check_tags`, which refuses a key carrying no characters with the modelled `BadRequestException` (400) and the sentence `empty_tag_key_message()`. `UntagResource` applies no test at all to the entries of `tagKeys`: it loops straight over them and calls `tags.remove(&key)`.

So `PUT /2017-08-29/tags/{Arn}` with body `{"tagKeys": [""]}` is answered with the operation's empty 200 success, having removed nothing — while the request that could have created that key, `TagResource` with `{"tags": {"": "v"}}`, is refused 400. The two doors onto the same map disagree about whether a key with no characters is a key.

That is exactly the disagreement `check_tags`' own doc forbids: it justifies the empty-key refusal by saying such a key "names nothing a later `UntagResource` or `ListTagsForResource` could address" — it names `UntagResource` as the reader that cannot address it, and yet `UntagResource` accepts it as a key to remove and reports success. It is also the crate-wide rule `tests/all/blank_resource_name.rs` states for every member that mints or resolves an identity: such a value is answered with `BadRequestException` (400) naming the member.

The same handler already answers a blank `Arn` with that 400, so the rule is applied to one member of this very request and skipped on the other.

## Evidence

The rule, in the state module — `services/service-com-amazonaws-mediaconvert-mediaconvert/src/state.rs:1684`:
```
/// own rule: `Tags` is documented as pairs where "you can tag resources with a key-value
/// pair or with only a key", so an empty VALUE is a tag but a key with no characters names
/// nothing a later `UntagResource` or `ListTagsForResource` could address, and a record's
/// tag count is bounded by [`MAX_TAGS_PER_RESOURCE`].
```
and its implementation, `state.rs:1690`:
```rust
pub fn check_tags(tags: &BTreeMap<String, String>) -> Result<(), TagRefusal> {
    if tags.keys().any(|key| key.is_empty()) {
        return Err(TagRefusal::EmptyKey);
    }
```

The sibling door applies it — `src/ops/tag_resource.rs:205`:
```rust
let requested: ::std::collections::BTreeMap<String, String> =
    input.tags.into_iter().collect();
if let Err(refusal) = crate::state::check_tags(&requested) {
    return Err(TagResourceErrorModeled::of_tag_refusal(refusal).into());
}
```

This door does not — `src/ops/untag_resource.rs:214`:
```rust
for key in input.tag_keys.unwrap_or_default() {
    // A key the resource never carried is simply not there afterwards either.
    tags.remove(&key);
}
```
…although the very same handler applies the crate's blank test to its other caller-sent member, `src/ops/untag_resource.rs:178`:
```rust
if crate::state::carries_no_characters(&input.arn) {
    return Err(UntagResourceErrorModeled::BadRequestException(BadRequestException {
        message: Some(crate::state::blank_member_message("Arn")),
    })
    .into());
}
```

The crate-wide rule — `tests/all/blank_resource_name.rs:9`:
```
//! carries a resource identity — a key with no characters addresses nothing a later read
//! can reach, and a reference with none resolves to nothing — so every door that mints or
//! resolves a named resource answers such a value with `BadRequestException` (400) naming
//! the member, and leaves the store exactly as it was.
```

Test coverage: `tests/all/resource_tagging.rs:1093-1107` pins the `TagResource` half (`a_value_that_is_not_a_mediaconvert_arn_is_a_bad_request`, final block: "A tag with no key names nothing, so it is refused the same way" — HTTP 400, `BadRequestException`). No case in `resource_tagging.rs` or `blank_resource_name.rs` drives a blank value through `tagKeys`: the only `tagKeys` values any test sends are `["env"]` (`blank_resource_name.rs:541`) and `[FIXTURE_TAG_KEY]` (`clock_sweep_scope.rs:159`), so the gap is unpinned either way.

## Steps to Reproduce

1. Create a queue: `POST /2017-08-29/queues` with `{"name": "q", "tags": {"env": "prod"}}` → 201.
2. Send the key that cannot be created: `POST /2017-08-29/tags` with `{"arn": "arn:aws:mediaconvert:<region>:<account>:queues/q", "tags": {"": "v"}}` → 400 `BadRequestException`, "Tags must not contain an empty tag key." (the rule in `check_tags`).
3. Send the same key to the other door onto that map: `PUT /2017-08-29/tags/arn%3Aaws%3Amediaconvert%3A<region>%3A<account>%3Aqueues%2Fq` with `{"tagKeys": [""]}`.
4. Observe HTTP 200 with an empty body instead of the 400 `BadRequestException` the crate's one blank-member rule (and the sibling write door on the same tag map) answers.

## Resolution

A tag key is the identity of an entry in a record's tag map, so which values ARE keys is one
question for both doors onto that map, whichever member the key arrived in (`Tags` pairs or a
`TagKeys` list, both over the model's bare `__string`): a key of zero length addresses no entry
— none a write can create, none a read or a removal can reach — so it is refused as a problem in
the request with the modelled `BadRequestException` (400) stating the crate's one
`empty_tag_key_message()` sentence, before the store is touched, while a key carrying ANY
character (whitespace included) is a key and is stored and addressed verbatim.

That predicate is now named once as `crate::state::names_no_tag`; `check_tags` (the pairs door)
is written in terms of it, and the new `crate::state::check_tag_keys` applies it to a key LIST —
no cap test there, because removing keys cannot carry a record past `MAX_TAGS_PER_RESOURCE`.
`src/ops/untag_resource.rs` calls it after the `Arn` form test and BEFORE `tags_for_write`,
exactly where `tag_resource.rs` calls `check_tags`, and answers through the shared
classification `TagRefusalError::of_tag_refusal`, so the status, the error shape and the
sentence are the state module's single wording rather than a second copy. A refused removal
therefore removes nothing, including the keys named beside the bad one.

Pinned by the new suite `tests/all/tag_key_identity.rs`: a table of key values spanning the
class (no characters at all, one space, three spaces, a tab, a word, a padded word,
punctuation, characters beyond ASCII, one character, 128 characters) driven through BOTH doors
with a read-back through `ListTagsForResource`; a table over where the bad value sits in the key
list and what travels beside it, with the exact remaining set asserted; and a case showing the
refusal is answered before the ARN is resolved, with the lookup refusal still served for a real
key.
