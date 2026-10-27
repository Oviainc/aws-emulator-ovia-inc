Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: f39310ffb44e85959dc0d40e62557237e3e92192

# PutRawMessageContent rejects content whose FIRST header line has a whitespace-padded field name, which the crate's own parser accepts everywhere else

_Filed: 2026-10-27T14:02:29Z_
_Operation: PutRawMessageContent | Class: validation_

> _Note: this issue was identified against the code base at commit `f39310ffb44e85959dc0d40e62557237e3e92192`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`validate_mime` gates the content on `starts_with_header_field(content)`, which only accepts a first line whose field name consists entirely of octets 33..=126 — i.e. it forbids the space (32) and the horizontal tab (9) before the colon. The crate's own header parser does the opposite: `header_fields` strips exactly those two octets off a name (`trim_wsp(&line[..colon])`) and treats `Date\t:` as the `date` field, and the crate states that rule explicitly ("a name padded with ASCII whitespace ... is still that field").

The result is that one and the same header line is read two different ways depending on where it sits in the header block:

* `Date\t: <date>\r\nFrom: a@example.com\r\n\r\nbody\r\n` is refused with HTTP 400 `MessageRejected` and the message "The message does not begin with a header field; a message must contain the required header fields." — a statement that is false about that content, since the handler's own `header_fields`/`header_field_counts` do find both required fields in it.
* The same message with any unpadded field first (e.g. a leading `Subject:` line) is accepted with HTTP 200 and installed.

So an update whose MIME content breaks no criterion the model states — and which the handler's own parser reads as carrying `Date` and `From` — is refused, and the refusal message misreports why. The in-transit message is left unchanged, so a caller is told its well-formed content is malformed MIME.

This is distinct from the two fixed PutRawMessageContent validation issues (20261024T170349Z, which ADDED the required/immutable-header checks, and 20261025T165823Z, the lossy immutable comparison): neither touches `starts_with_header_field`, which predates both, and this defect is a false rejection rather than a missing one. No existing test puts a WSP-padded name on the first line — the test helper deliberately avoids it — so no test pins the current outcome.

## Evidence

The two sides of the contradiction are both in this crate.

The strict first-line check — services/.../src/ops/put_raw_message_content.rs:739-753:
```
/// Whether `content` opens with an RFC 5322 header field: a field name of
/// printable US-ASCII without a colon, then a colon.
fn starts_with_header_field(content: &[u8]) -> bool {
    ...
    colon > 0
        && first_line[..colon]
            .iter()
            .all(|byte| (33..=126).contains(byte))
}
```
(32 = SP and 9 = HTAB are both outside 33..=126.) It gates the content at put_raw_message_content.rs:544-550:
```
    if !starts_with_header_field(content) {
        return Err(
            "The message does not begin with a header field; a message must contain the required \
             header fields."
                .to_string(),
        );
    }
```

The permissive parser used for every other header rule — put_raw_message_content.rs:686:
```
        let name = trim_wsp(&line[..colon]).to_ascii_lowercase();
```
with put_raw_message_content.rs:693-698 stating what is trimmed:
```
/// `octets` without the whitespace RFC 5322 pads a header field with.
///
/// That whitespace is the space and the horizontal tab (§3.2.2 `WSP`), and
/// nothing else: every other octet, printable or not, is part of the field name
/// or of its value, ...
```

The rule the crate states, and the workaround that shows the divergence is known — tests/all/mime_header_octets.rs:385-387:
```
    // A field whose name is the required octets is that field whatever its value
    // carries, and a name padded with ASCII whitespace — which the obsolete field
    // syntax of RFC 5322 §4.5.8 admits before the colon — is still that field.
```
and the row that asserts it, tests/all/mime_header_octets.rs:399-409, which has to place a padded name SECOND and spells the reason out in the first field's value:
```
        (
            "required field names padded with a tab and a space",
            message(
                &[
                    (b"Subject", b"a first line whose name needs no padding"),
                    (b"Date\t", DATE.as_bytes()),
                    (b"From ", b"a@example.com"),
                ],
                b"body\r\n",
            ),
        ),
```
That test passes only because the padded fields are not first; moving `Date\t` to the first line turns the same content into a 400.

Test coverage: tests/all/raw_message_content.rs `put_with_unusable_mime_content_is_rejected` pins only `b": no name\r\n\r\nbody"` (empty name) and `b"Bad Header Name\r\n\r\nbody"` (no colon at all) as first-line rejections — neither is a WSP-padded name, so no test pins the behaviour reported here.

## Steps to Reproduce

1. Mount the WorkMailMessageFlow service on the shared engine and put a message in transit under some messageId with well-formed MIME content (today the in-process seed helper, since no producer is wired — see issue 20261024T101352Z).
2. Make an S3 object resolvable at the reference you will pass, whose body is `Date\t: Tue, 24 Oct 2026 17:03:49 +0000\r\nFrom: a@example.com\r\n\r\nbody\r\n` — a message carrying both required header fields, with the first field name padded with a horizontal tab before the colon (RFC 5322 §4.5.8 obsolete field syntax, which this crate's `trim_wsp`/`header_fields` accept).
3. Call PutRawMessageContent with that messageId and content={'s3Reference': {'bucket': <bucket>, 'key': <key>}}.
4. Observed: HTTP 400, x-amzn-errortype MessageRejected, body message "The message does not begin with a header field; a message must contain the required header fields.", and the next GetRawMessageContent still reports the old content — even though `header_field_counts` on that same content finds both `date` and `from`.
5. Re-run with the identical content except that an unpadded field is placed first (`Subject: s\r\nDate\t: ...\r\nFrom: a@example.com\r\n\r\nbody\r\n`): HTTP 200 and the bytes are installed — the same padded line is accepted as a header field when it is not first.
