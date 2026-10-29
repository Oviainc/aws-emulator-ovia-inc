Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: c088b2b7fac1291fdb05737f13bee382d9ee2825

# Tag-key @pattern refusal bypasses the crate's own hand-applied-pattern renderer, so it states neither the modelled expression nor the member path

_Filed: 2026-10-29T03:07:21Z_
_Operation: TagResource | Class: error-response_

> _Note: this issue was identified against the code base at commit `c088b2b7fac1291fdb05737f13bee382d9ee2825`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate has to apply two modelled `@pattern` constraints by hand, because the framework's centralized constraint walk cannot: `EmailContactName`'s `[\w-.~]+` (whose character class the `regex` crate refuses) and `TagKey`'s `^(?!aws:).{1,128}$` (a map-key constraint the walk does not visit, and a look-ahead `regex` cannot compile).

The crate states ONE rule for how such a by-hand refusal must be worded, on the helper it wrote for the purpose (`crate::state::pattern_validation_failure`): the sentence is handed to the framework's own renderer so "a `@pattern` this service has to apply itself is refused in exactly the words the centralized walk uses for a `@pattern` it could apply" — the envelope, the member path, the submitted value and the expression the model declares — and `fieldList` carries the requirement half of that clause.

The `name` path obeys that rule (`check_email_contact_name` -> `pattern_validation_failure`), and a test pins it. The TagKey path does not: `check_tag_keys` hand-writes prose through `field_validation_failure` ("tags names a tag key that must not begin with the reserved prefix aws:"), so for the very same class of violation — a modelled `@pattern` this service applies itself — the response carries no `N validation error detected: … at '<member>' failed to satisfy constraint: …` envelope and never names the modelled expression the value broke. That path serves TagResource, UntagResource (`tagKeys`) and CreateEmailContact (`tags`), i.e. every tag write in the service.

The effect is two independent wordings for one documented rule inside one crate: the two can (and already do) drift, and a client that reads this service's validation messages gets a different message family depending on which of the two hand-applied patterns it broke.

## Evidence

The rule, on the helper the crate wrote for hand-applied patterns — services/service-com-amazonaws-notificationscontacts-notificationscontacts/src/state.rs:531-537:
```
/// The modelled 400 for a `value` outside the `@pattern` its member declares.
///
/// The sentence is NOT written here: the violation is handed to the framework's own
/// renderer, so a `@pattern` this service has to apply itself is refused in exactly the
/// words the centralized walk uses for a `@pattern` it could apply — the envelope, the
/// member path, the value the request submitted and the expression the model declares —
/// and `fieldList` blames the member with the requirement half of that same clause.
```

TagKey IS such a pattern — state.rs:99-103:
```
/// The modelled `TagKey` pattern, verbatim
/// (`smithy_query(query="shape",shape="TagKey")`).
///
/// Stated here because it has to be applied by hand: see [`tag_key_rejection`].
pub const TAG_KEY_PATTERN: &str = r"^(?!aws:).{1,128}$";
```
(`smithy_query(query="shape",shape="TagKey")` -> `"pattern": "^(?!aws:).{1,128}$"`.)

Side A — the path that follows the rule, state.rs:244-253:
```
pub fn check_email_contact_name(member: &str, name: &str) -> Result<(), ValidationException> {
    if satisfies_email_contact_name_pattern(name) {
        return Ok(());
    }
    Err(pattern_validation_failure(
        member,
        EMAIL_CONTACT_NAME_PATTERN,
        name,
    ))
}
```

Side B — the path that skips it, state.rs:185-191:
```
    match refused {
        Some((_, why)) => Err(field_validation_failure(
            member,
            &format!("names a tag key that {why}"),
        )),
        None => Ok(()),
    }
```
with the prose written at state.rs:146-150 ("must not begin with the reserved prefix {RESERVED_TAG_KEY_PREFIX}") and `field_validation_failure` building only `format!("{member} {why}")` (state.rs:516-525) — no envelope, no `at '<member>'`, no expression.

Callers of side B: src/ops/tag_resource.rs:158 `crate::state::check_tag_keys("tags", input.tags.keys()…)`, src/ops/untag_resource.rs:157 `check_tag_keys("tagKeys", …)`, src/ops/create_email_contact.rs:170 `check_tag_keys("tags", …)`.

Test coverage, which shows the two sides are pinned asymmetrically:
- tests/all/email_contact_name_pattern.rs:347-390 — `the_refusal_states_the_modelled_expression_at_the_member_it_blames` asserts `message.contains(EMAIL_CONTACT_NAME_PATTERN)` and `message.contains("at 'name'")`, and that `fieldList[].message` states the same expression.
- tests/all/tag_key_constraints.rs:201-217 — `assert_refused` checks only the 400 status and `error_code(body) == "ValidationException"`; nothing pins the tag-key message, so no existing test is tied to the current prose.

The renderer the rule points at exists and is reachable: vendor/smithy-schema-server/src/constraints.rs:148-150 `ViolationKind::Pattern(pattern) => format!("Member must satisfy regular expression pattern: {pattern}")`.

## Steps to Reproduce

1. CreateEmailContact(name="contact-a", emailAddress="a@example.com") and keep the returned arn.
2. TagResource(arn=<arn>, tags={"aws:owner": "v"}) -> 400 ValidationException with message "tags names a tag key that must not begin with the reserved prefix aws:": no `N validation error detected: …` envelope, no `at 'tags'` member path, and the modelled TagKey expression `^(?!aws:).{1,128}$` is never stated.
3. Same for UntagResource(arn=<arn>, tagKeys=["aws:owner"]) and CreateEmailContact(..., tags={"": "v"}) — all three go through `check_tag_keys`.
4. CreateEmailContact(name="team alpha", emailAddress="b@example.com") -> 400 ValidationException whose message carries the envelope, `at 'name'` and the modelled expression `[\w-.~]+` (pinned by tests/all/email_contact_name_pattern.rs:347-390).
5. Compare the two bodies: both are by-hand refusals of a modelled `@pattern`, but only one is worded the way state.rs:531-537 says such a refusal is worded.
