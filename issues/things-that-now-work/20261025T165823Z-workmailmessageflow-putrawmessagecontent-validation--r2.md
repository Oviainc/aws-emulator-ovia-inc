Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: f3b5c182376fbd512bc0d8c0af2ad2661dfcff47

# Immutable-header comparison decodes header values lossily, so an update that changes a non-UTF-8 immutable header value is accepted

_Filed: 2026-10-25T16:58:23Z_
_Operation: PutRawMessageContent | Class: validation_

> _Note: this issue was identified against the code base at commit `f3b5c182376fbd512bc0d8c0af2ad2661dfcff47`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`PutRawMessageContent` enforces the model's "The value of immutable headers must remain unchanged" criterion by comparing the header block of the message in transit with the header block of the update (`immutable_header_conflict`). Both sides are parsed by `header_fields`, which converts each field name and value with `String::from_utf8_lossy` before the comparison. Lossy decoding is NOT injective: every invalid byte collapses to U+FFFD, so two header values that differ in their bytes decode to the same `String` and compare equal.

Consequence: an update whose `Message-ID` (or `Received`) value differs from the in-transit message's only in invalid-UTF-8 bytes — e.g. `<\xFF@example.com>` in transit vs. `<\xFE@example.com>` in the update — is judged "unchanged", passes the conflict check, and is installed by `state.replace_message_content(..)`. The call answers HTTP 200 and the next `GetRawMessageContent` reports content whose immutable header value has changed, which is exactly the outcome the check exists to prevent (the documented answer is `MessageRejected`, HTTP 400). Nothing anywhere in the operation requires the content to be UTF-8: `validate_mime` checks emptiness, a size guard, line length, required field presence and singleton duplicates, and `starts_with_header_field` constrains only the bytes of the FIRST line's field NAME to 33..=126 — the values are never constrained, and the bytes already in transit are arbitrary (the suite itself seeds all 256 byte values).

The same root cause has a second symptom of the same class: `header_field_counts` counts the lossily-decoded names, so two distinct field names that differ only in invalid bytes are counted as one field.

This is not the already-fixed validation issue (which was "the immutable headers are never checked at all") re-filed: the check now exists; the defect is that its equality test is computed over lossily-decoded text instead of the bytes, so a documented refusal is still reachable only for UTF-8 content. It is also distinct from the open PutRawMessageContent wrong-behavior issue, which is about where the S3 object is resolved from, not about the MIME comparison.

## Evidence

Model (smithy_query(query="doc", shape="RawMessageContent")): "The value of immutable headers must remain unchanged. Check the returned error message for more information." — and `MessageRejected` is modelled (HTTP 400) for "The requested email could not be updated due to an error in the MIME content."

The handler states the rule it then breaks — services/.../src/ops/put_raw_message_content.rs:578-580: "/// Why `updated` may not replace `current`, when it changes the value of a header /// in [`IMMUTABLE_HEADERS`], and `None` when every one of those headers reads /// exactly as it does on the message in transit."

The comparison, put_raw_message_content.rs:591-595:
    let before: Vec<&str> = values_of(&before, name);
    let after: Vec<&str> = values_of(&after, name);
    if before == after {
        continue;
    }

what is compared, put_raw_message_content.rs:661-666:
    let name = String::from_utf8_lossy(&line[..colon])
        .trim()
        .to_ascii_lowercase();
    let value = String::from_utf8_lossy(&line[colon + 1..])
        .trim()
        .to_string();

the immutable set, put_raw_message_content.rs:474: `const IMMUTABLE_HEADERS: &[&str] = &["message-id", "received"];`

the install that follows a "no conflict" verdict, put_raw_message_content.rs:269-274: `state.replace_message_content(&request_region, &input.message_id, content, reference.clone(),);`

nothing requires UTF-8: `validate_mime` (put_raw_message_content.rs:522-576) checks empty / size / `starts_with_header_field` / `overlong_line` / required names / singleton counts only, and `starts_with_header_field` (put_raw_message_content.rs:701-713) constrains only `first_line[..colon]` to `(33..=126)`.

Non-UTF-8 content in transit is already exercised by this crate's own tests — tests/all/regional_message_namespace.rs:80 `content: (0u8..=255).collect(),` and tests/all/raw_message_content.rs:1143-1150 (`case_bytes`: "filled with a byte sequence that walks all 256 values").

Test coverage: tests/all/mime_header_criteria.rs drives the immutable-header table with ASCII-only values (e.g. lines 271-379, `Message-ID: <00000001@example.com>` / `<99999999@example.com>`); no row anywhere in the suite puts a non-UTF-8 byte in a header value, so the lossy path is unasserted.

## Steps to Reproduce

1. Mount the WorkMailMessageFlow service on the shared engine and put a message in transit under id `mime-lossy` whose content is `Date: Tue, 24 Oct 2026 17:03:49 +0000\r\nFrom: a@example.com\r\nMessage-ID: <\xFF@example.com>\r\n\r\nbody\r\n` (today only the in-process seed helper can produce an in-transit message, so the step under test is the MIME comparison, not message existence).
2. Make an S3 object resolvable at `s3://mailbucket/updated/object` whose bytes are the same message with the single byte `\xFE` in place of `\xFF` in the Message-ID value.
3. Call `PutRawMessageContent` with messageId `mime-lossy` and content={'s3Reference': {'bucket': 'mailbucket', 'key': 'updated/object'}}.
4. Expected: HTTP 400 with `x-amzn-errortype: MessageRejected` and a message saying the Message-ID header is immutable and must remain unchanged; the in-transit content is unchanged.
5. Actual: HTTP 200 with an empty body — `header_fields` decodes both `\xFF` and `\xFE` to U+FFFD, so `before == after` at put_raw_message_content.rs:593 holds and the update is installed.
6. Call `GetRawMessageContent` for `mime-lossy`: the payload now carries `\xFE` in the Message-ID, i.e. the value of an immutable header changed.
