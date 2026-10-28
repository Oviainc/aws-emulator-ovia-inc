Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: c18a0019e5dd212e394ad1114dc50f0b0a8a5ba1

# CreateEmailContact persists a `name` outside the modelled `[\w-.~]+` pattern, which the constraint walk cannot apply

_Filed: 2026-10-28T07:17:09Z_
_Operation: CreateEmailContact | Class: validation_

> _Note: this issue was identified against the code base at commit `c18a0019e5dd212e394ad1114dc50f0b0a8a5ba1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateEmailContactRequest$name` targets `EmailContactName`, whose `@pattern` is `[\w-.~]+`. The handler relies entirely on the centralized constraint walk for that member and its comment says so, but that pattern is in the SAME class as the `TagKey` pattern this crate already compensates for by hand: it is not compilable by the `regex` crate the walk uses, and `vendor/smithy-schema-server/src/constraints.rs::pattern_matches` treats a pattern it cannot compile as matching EVERY value.

`[\w-.~]` is parsed as a character class whose second item is the range `\w`-`.`; a range endpoint must be a single literal codepoint, so `regex::Regex::new(r"[\w-.~]+")` returns `Err` (regex-syntax `ClassRangeLiteral`, "invalid range endpoint") exactly the way `^(?!aws:).{1,128}$` does for the tag key. Consequence: nothing in the service enforces `EmailContactName`'s pattern. A create whose `name` carries a space, a `!`, a `/`, a line feed or any other character the class excludes is accepted with HTTP 201, persisted into `EmailContactRecord.name`, and reported back by `GetEmailContact`/`ListEmailContacts` through `EmailContact$name`, whose own member declares the same pattern.

The crate already states the rule and already owns the mechanism to apply it (`crate::state::tag_key_rejection`, written for precisely this framework limitation and invoked by this very handler two statements earlier for `tags`); the `name` member is the one member of this operation left outside it. Note this is NOT the already-fixed tag-key gap (issue 20261024T052311Z-notificationscontacts-createemailcontact-validation, which covered `tags` keys only): it is the second uncompilable pattern in the same input, still unhandled.

## Evidence

Model, verbatim: `smithy_query(query="shape",shape="EmailContactName")` -> `{"shape_id": "com.amazonaws.notificationscontacts#EmailContactName", "type": "string", "length": {"min": 1, "max": 64}, "pattern": "[\\w-.~]+", "sensitive": true}`, and OPERATIONS.md:53 renders the input member as "`name` (string, len 1..64, pattern [\w-.~]+, required)".

Framework no-op (the quoted reason the pattern is never applied) — vendor/smithy-schema-server/src/constraints.rs:421-423: "True if `value` satisfies the Smithy `@pattern` `regex` (a find, not an anchored match). A pattern that fails to compile is treated as matching (we never reject a value because of an unparseable model pattern)." and constraints.rs:454-462: "// TODO: a pattern that fails to compile is treated as matching, so we never reject a value because of an unparseable model pattern. ... let Ok(re) = regex::Regex::new(regex) else { return true; };". `[\w-.~]` makes `\w`-`.` a class range, whose start is not a literal, so `Regex::new` fails here (verify with one line: `regex::Regex::new(r"[\w-.~]+").is_err()`).

The crate's own statement of the rule it does not apply to `name` — services/service-com-amazonaws-notificationscontacts-notificationscontacts/src/state.rs:130-137: "the pattern opens with the negative lookahead `(?!aws:)`, which the `regex` crate cannot compile — and a `@pattern` that fails to compile is treated there as matching EVERY value. So the pattern is applied here instead, by every operation that accepts a tag key".

The handler comment that places `name` on the wrong side of that split — src/ops/create_email_contact.rs:154-160: "// `name` (1..64), `emailAddress` (6..254, `^(.+)@(.+)$`) and the `tags` map's / 0..200 length and value length are modelled in a form the centralized walk / applies, so a value outside them is already a 400 `ValidationException` / before this body runs. The tag KEY pattern is not: ..." — `name`'s `@pattern` is cited nowhere, and the handler body (src/ops/create_email_contact.rs:198-212) stores `name: input.name` with no screening.

Test coverage: tests/all/email_contacts.rs:273-302 only walks names the pattern ADMITS ("name carrying the pattern's punctuation", "oncall-primary.v2~1", "underscores, digits and a multi-label domain"); no test in tests/all asserts that a name outside `[\w-.~]+` is refused, so nothing pins the current acceptance.

## Steps to Reproduce

1. POST /2022-09-19/emailcontacts with body {"name": "team alpha!", "emailAddress": "team@example.com"} — the space and `!` are outside the modelled `[\w-.~]+` class
2. Observe HTTP 201 and an `arn`: the walk never applied the pattern, because `regex::Regex::new(r"[\w-.~]+")` returns Err and vendor/smithy-schema-server/src/constraints.rs:460-462 then answers `true` for every value
3. GET /emailcontacts/{arn} — `emailContact.name` reads back "team alpha!", a value `EmailContact$name`'s own `@pattern` excludes
4. Repeat with {"name": "a/b"} or a name containing "\n": also accepted and persisted, while the sibling path in the same handler (crate::state::check_tag_keys on `tags`) refuses the equivalent violation of the other uncompilable pattern
