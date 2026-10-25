Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: ad9d9cbded3020216cba71d97cb223a025446b5b

# PutRawMessageContent accepts MIME content with no required header fields and never checks immutable headers, so two documented MessageRejected cases answer 200

_Filed: 2026-10-24T17:03:49Z_
_Operation: PutRawMessageContent | Class: validation_

> _Note: this issue was identified against the code base at commit `ad9d9cbded3020216cba71d97cb223a025446b5b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The model's `RawMessageContent` documentation enumerates the criteria all MIME content supplied to PutRawMessageContent must meet, and `MessageRejected` (HTTP 400) is the modeled answer when it does not ("The requested email could not be updated due to an error in the MIME content. Check the error message for more information about what caused the error."). Two of those criteria are not evaluated at all by the emulator:

1. "The message must contain all the required header fields." The handler's `validate_mime` only checks that the content's FIRST line looks like a header field (`starts_with_header_field`), not that the message carries the fields RFC 5322 §3.6 makes mandatory — `Date:` and `From:` (the same RFC section the handler already cites as its authority for the singleton-header list in `SINGLETON_HEADERS`). So content such as `a:\r\n` or `X-Pad: x\r\n\r\nbody` — a "message" with neither From nor Date — is accepted.

2. "The value of immutable headers must remain unchanged." The handler never compares the incoming header block against the header values of the message currently in transit, so an update that rewrites a header WorkMail treats as immutable (e.g. the message's own `Message-ID`/`Received` trace of the in-transit message) is installed silently.

Expected (AWS): PutRawMessageContent with S3 content missing a required header field, or changing the value of an immutable header, answers HTTP 400 with `x-amzn-errortype: MessageRejected` and a `message` explaining which criterion failed, and the in-transit message's content is left unchanged.

Actual (emulator): the call answers HTTP 200 with an empty body, `state.replace_message_content` installs the content, and the next GetRawMessageContent reports the header-less (or immutable-header-rewritten) bytes. A documented rejection path is therefore unreachable, and a client that uploads malformed MIME is told its update succeeded.

(The other two documented criteria — proper formatting of each multipart part, and the SES-supported attachment content types — state no rule the model carries, and are not what this issue asks for.)

## Evidence

Spec: `smithy_query(query="doc", shape="RawMessageContent")` lists "The message must contain all the required header fields. Check the returned error message for more information." and "The value of immutable headers must remain unchanged. Check the returned error message for more information." PutRawMessageContent models `MessageRejected` (client error, HTTP 400, member `message`) precisely for "an error in the MIME content". RFC 5322 §3.6 — the section the handler itself cites for `SINGLETON_HEADERS` — marks `orig-date` (Date) and `from` (From) as required exactly once.

Handler: services/service-com-amazonaws-workmailmessageflow-giraffemessageintransitservice/src/ops/put_raw_message_content.rs — `validate_mime` (lines 440-481) checks only: non-empty, under the 128 MiB process guard, first line parses as a header field (`starts_with_header_field`, lines 542-554), no line over 1000 octets, no duplicated singleton header. Its own doc comment (lines 426-436) states that "WHICH header fields are required" and "which headers are immutable" are "deliberately NOT enforced". Nothing in the handler reads the existing message's headers before `state.replace_message_content(&input.message_id, content, reference.clone())` (line 253).

Test coverage: tests/all/raw_message_content.rs `put_with_unusable_mime_content_is_rejected` asserts the CURRENT lenient behavior — its `accepted` table includes ("the shortest possible header-only message", `b"a:\r\n"`) expecting HTTP 200 and the bytes back (lines 953-995), and no case anywhere drives a missing required header or an altered immutable header. So the gap is not only unasserted, the lenient outcome is pinned by a test.

Not covered by the open issues: 20261024T015715Z (resolve the S3Reference against the shared S3 store) and 20261024T045353Z (bucket policy -> InvalidContentLocation) concern where/whether the object can be read; 20261024T101352Z concerns message existence; the IAM issue concerns authorization. None touches the MIME criteria checked after the bytes are read.

## Steps to Reproduce

1. Mount the WorkMailMessageFlow service on the shared engine and arrange for an in-transit message under some messageId with well-formed MIME content (today only the in-process seed helper can do this, so the step under test is the MIME validation, not message existence).
2. Make an S3 object resolvable at the reference you will pass, whose body is a header-only blob with no required header fields, e.g. exactly `a:\r\n` (or `X-Pad: x\r\n\r\nbody`).
3. Call PutRawMessageContent with that messageId and content={'s3Reference': {'bucket': <bucket>, 'key': <key>}}.
4. Expected (AWS): HTTP 400, x-amzn-errortype MessageRejected, body message naming the missing required header field(s); a following GetRawMessageContent still returns the original content.
5. Actual: HTTP 200 with an empty body, and the following GetRawMessageContent returns `a:\r\n` — the malformed content was installed.
6. Repeat with content identical to the stored message except that a header WorkMail treats as immutable has a different value: expected HTTP 400 MessageRejected; actual HTTP 200 and the rewritten header is installed.
