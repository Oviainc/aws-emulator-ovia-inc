Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 1211f7611505016a47ab1bf8932fdc778b7fd399

# UntagResource skips the tag-key length bound its sibling TagResource and both creates apply to the same keys

_Filed: 2026-10-29T01:44:46Z_
_Operation: UntagResource | Class: validation_

> _Note: this issue was identified against the code base at commit `1211f7611505016a47ab1bf8932fdc778b7fd399`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`bound_tag_entries` is documented as the single place the crate decides AWS's tagging rules, and it judges each key by three things: its LENGTH (`MAX_TAG_KEY_CHARS`), the character set, and the reserved `aws:` prefix. `TagResource`, `CreateChannel` and `CreateOriginEndpoint` all go through it.

`UntagResource` does not: it calls `validate_tag_key` directly, which implements only the last two of those three rules. So an over-long `tagKeys` entry is accepted (204 No Content, removing nothing) where `TagResource` refuses the identical key with 422 `UnprocessableEntityException` "key ... may be at most 128 characters, got N".

That contradicts the handler's own stated rationale for validating at all. `untag_resource.rs` refuses a key because "A key outside the documented character set could never have been stored" — a key longer than `MAX_TAG_KEY_CHARS` could never have been stored either, by the very bound `bound_tag_entries` enforces on every write path, so by the handler's own rule it should be refused rather than silently answered 204. The practical effect is that the two halves of one rule disagree about what a well-formed tag key is, and a caller gets a success for a key the service would never have let it create.

## Evidence

Both sides are in this crate.

The one place the rules are supposed to live — `services/service-com-amazonaws-mediapackage-mediapackage/src/state.rs:616-641`:
```
/// Refuse a tag set that breaks AWS's tagging rules: by entry count, by the length of each
/// key and value, and by what each key and value is ALLOWED TO SAY — the character set,
/// and the reserved `aws:` key prefix only AWS itself may use.
///
/// This is the one place those rules are decided, so a create's request-shaped tag map
/// and a `TagResource`'s merged result cannot be judged differently.
pub fn bound_tag_entries<'a, I>(tags: I) -> Result<(), ServiceError>
...
        bound_part_chars("tags", &format!("key {key:?}"), key, MAX_TAG_KEY_CHARS)?;
        ...
        validate_tag_key("tags", key)?;
```

What `UntagResource` actually applies — `services/service-com-amazonaws-mediapackage-mediapackage/src/ops/untag_resource.rs:183-193`:
```
        // Judged BEFORE anything is removed, so a refused call leaves the resource's tag
        // set exactly as it found it. A key outside the documented character set could
        // never have been stored, and a key carrying the reserved `aws:` prefix names a
        // tag AWS owns — which a caller may no more DELETE than create ...
        for key in &input.tag_keys {
            crate::state::validate_tag_key("tagKeys", key)?;
        }
```
There is no `bound_part_chars(..., MAX_TAG_KEY_CHARS)` anywhere in `untag_resource.rs`, and `validate_tag_key` (`state.rs:578-598`) checks only `offending_tag_character` and `RESERVED_TAG_KEY_PREFIX` — "Two rules", as its own doc at `state.rs:570` says.

Tag writers that DO apply the bound: `src/ops/tag_resource.rs:215` (`bound_tag_entries(merged...)`), `src/ops/create_channel.rs:172` and `src/ops/create_origin_endpoint.rs:220` (both `bound_tags`, which forwards to `bound_tag_entries` at `state.rs:646-651`).

Test coverage: `tests/all/resource_tags.rs:207-213` pins a key at exactly `MAX_TAG_KEY_CHARS` as ACCEPTED on the create/TagResource path, and `tests/all/reserved_and_malformed_tags.rs:318-335` drives `UntagResource` only with character-set and reserved-prefix rows (`.filter(|row| row.key_at_fault)`), so no test pins UntagResource's behaviour for an over-long key either way.

## Steps to Reproduce

1. Boot the emulator with the MediaPackage service mounted.
2. POST /channels with {"id": "len-channel"} to create a channel.
3. Let K be a 5000-character key, e.g. "k" repeated 5000 times.
4. POST /tags/arn%3Aaws%3Amediapackage%3Aus-east-1%3A<account>%3Achannels%2Flen-channel with {"tags": {K: "v"}} — observe 422 UnprocessableEntityException, errors.tags == "key \"kkk...\" may be at most 128 characters, got 5000".
5. DELETE /tags/arn%3Aaws%3Amediapackage%3Aus-east-1%3A<account>%3Achannels%2Flen-channel?tagKeys=<K percent-encoded> — observe 204 No Content.
6. The same key is refused by one tagging door and accepted by its sibling.
