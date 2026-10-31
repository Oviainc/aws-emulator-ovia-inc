Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 4c075dcdeacefbec5f942454c2320d1c6eabf26d

# CreateEmailContact persists an `emailAddress` carrying a CR / U+2028 / U+2029, which the modelled `^(.+)@(.+)$` excludes

_Filed: 2026-10-29T00:49:49Z_
_Operation: CreateEmailContact | Class: validation_

> _Note: this issue was identified against the code base at commit `4c075dcdeacefbec5f942454c2320d1c6eabf26d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateEmailContactRequest$emailAddress` targets `EmailContactAddress`, whose `@pattern` is `^(.+)@(.+)$`. The handler relies entirely on the centralized constraint walk for that member and its comment says so explicitly. But a Smithy `@pattern` is an ECMA-262 expression, and THIS crate is the one that states what that means for `.`: in ECMA-262 `.` matches every character EXCEPT a line terminator — line feed, carriage return, line separator (U+2028) and paragraph separator (U+2029). The walk compiles the expression with the `regex` crate instead, whose `.` excludes only `\n`, so `\r`, U+2028 and U+2029 are matched by `(.+)` there and the value satisfies the compiled regex.

Consequence: `POST /2022-09-19/emailcontacts` with `emailAddress` = "a\r@b.com" (or an address carrying U+2028 / U+2029) is accepted with HTTP 201, persisted into `EmailContactRecord.address`, and reported back by `GetEmailContact`/`ListEmailContacts` through `EmailContact$address`, whose own member declares the same `^(.+)@(.+)$` pattern — so the service reports a value its own output member excludes.

This is the SAME ECMA-262-vs-`regex` divergence the crate already compensates for by hand twice: `crate::state::TAG_KEY_EXCLUDED_CHARS` exists for exactly these four line terminators on tag keys, and `satisfies_email_contact_name_pattern` admits no line terminator at all for `name`. `emailAddress` is the one member of this input left on the assumption that the walk's engine reproduces the modelled expression's semantics. It does not for those three codepoints, and the crate's own text is what proves it.

## Evidence

Model, verbatim — `smithy_query(query="operation",operation="CreateEmailContact")`: `"emailAddress": {"target": "EmailContactAddress", "required": true, "length": {"min": 6, "max": 254}, "pattern": "^(.+)@(.+)$"}`; OPERATIONS.md:53 renders it as "`emailAddress` (string, len 6..254, pattern ^(.+)@(.+)$, required)".

The crate's own statement of what `.` means in a Smithy `@pattern` — services/service-com-amazonaws-notificationscontacts-notificationscontacts/src/state.rs:117-123:
```
/// The characters [`TAG_KEY_PATTERN`]'s `.` does NOT match.
///
/// A Smithy `@pattern` is an ECMA-262 regular expression, and there `.` matches every
/// character except a LINE TERMINATOR: line feed, carriage return, line separator and
/// paragraph separator. A key carrying any of them therefore cannot be `.{1,128}`,
/// however many characters it has.
pub const TAG_KEY_EXCLUDED_CHARS: [char; 4] = ['\n', '\r', '\u{2028}', '\u{2029}'];
```
and the hand-applied check that follows from it — src/state.rs:157-162:
```
    if let Some(excluded) = key.chars().find(|c| TAG_KEY_EXCLUDED_CHARS.contains(c)) {
        return Some(format!(
            "must not carry the line terminator U+{:04X}",
            excluded as u32
        ));
    }
```

The handler comment that places `emailAddress` on the other side of that split — src/ops/create_email_contact.rs:154-158:
```
        // `name`'s 1..64 length, `emailAddress` (6..254, `^(.+)@(.+)$`) and the `tags`
        // map's 0..200 length and value length are modelled in a form the centralized
        // walk applies, so a value outside them is already a 400 `ValidationException`
        // before this body runs. Two modelled patterns in this input are NOT: the tag
        // KEY pattern, ...
```
and the body that then stores it unscreened — src/ops/create_email_contact.rs:208-210:
```
            // The create's `emailAddress` is what the wire shape reports as
            // `address`: one stored value, two modelled member names.
            address: input.email_address,
```

The walk's engine, which is where the semantics diverge — vendor/smithy-schema-server/src/constraints.rs:421-424 and :460-469:
```
/// True if `value` satisfies the Smithy `@pattern` `regex` (a find, not an
/// anchored match). A pattern that fails to compile is treated as matching
/// (we never reject a value because of an unparseable model pattern).
fn pattern_matches(regex: &'static str, value: &str) -> bool {
...
    let Ok(re) = regex::Regex::new(regex) else {
        return true;
    };
...
    re.is_match(value)
```
The `regex` crate's `.` excludes only `\n` (one-line check: `regex::Regex::new(r"^(.+)@(.+)$").unwrap().is_match("a\r@b.com")` is `true`), so the three non-`\n` line terminators the crate's own state.rs:117-123 says the modelled expression excludes are admitted by the walk.

Test coverage: no test in tests/all submits an `emailAddress` carrying a line terminator — tests/all/email_contacts.rs only uses well-formed addresses (`create_ok(em, name, address)` rows such as "oncall@example.com"), and tests/all/email_contact_name_pattern.rs:199-208 holds `emailAddress` fixed (`format!("contact-{index}@example.com")`) so that only `name` varies. Nothing pins the current acceptance, and the sibling line-terminator screening is pinned by tests/all/tag_key_constraints.rs (`tag_key_table`), which stays unchanged.

## Steps to Reproduce

1. Read src/state.rs:117-123: in a Smithy `@pattern` (ECMA-262) `.` matches every character except line feed, carriage return, U+2028 and U+2029 — the rule this crate applies by hand for tag keys.
2. Read src/ops/create_email_contact.rs:154-158: the handler assumes the centralized walk applies `emailAddress`'s `^(.+)@(.+)$` and screens the value itself nowhere.
3. Verify the engine's divergence in one line: `regex::Regex::new(r"^(.+)@(.+)$").unwrap().is_match("a\r@b.com")` returns `true`, because the `regex` crate's `.` excludes only `\n`; vendor/smithy-schema-server/src/constraints.rs:460-469 is the call site that decides the 400.
4. Start the emulator and POST /2022-09-19/emailcontacts with body {"name": "oncall", "emailAddress": "a\r@b.com"} (a literal CR inside the local part, length still inside 6..254).
5. Observe HTTP 201 and an `arn`, where the modelled `^(.+)@(.+)$` pattern excludes the value and the modelled answer is a 400 ValidationException.
6. GET /emailcontacts/{arn}: `emailContact.address` reads back the CR-carrying value, through a member (`EmailContact$address`) that declares the same `^(.+)@(.+)$` pattern. Repeat with U+2028 or U+2029 in place of the CR for the same result.
