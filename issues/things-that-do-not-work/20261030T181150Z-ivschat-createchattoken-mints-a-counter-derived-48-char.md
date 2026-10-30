Issue-Source: trace-sweep-divergence
Issue-Trace: ivschat-chat-token-and-messaging
Issue-Step: task#2
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ivschat-chat-token-and-messaging
Issue-Shape-Id: com.amazonaws.ivschat#AmazonInteractiveVideoServiceChat
Issue-Key: ivschat|CreateChatToken|2xx->2xx|$.token
Issue-Commit: 8d2d24246537bab720d4405ea7ed505663212620

# ivschat CreateChatToken mints a counter-derived 48-char token instead of AWS's opaque encrypted token

> _Note: this issue was identified against the code base at commit `8d2d24246537bab720d4405ea7ed505663212620`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateChatToken

## Description
Cause 1 of 1: the token CreateChatToken returns does not have the form AWS's chat token has; it is a short, fixed-width, counter-derived string.

AWS: CreateChatToken answers 200 with `token`, targeting com.amazonaws.ivschat#ChatToken (Smithy: string, @sensitive; no @length and no @pattern), documented as "The issued client token, encrypted." and, on the operation, as "Creates an encrypted token that is used by a chat participant to establish an individual WebSocket chat connection to a room" — so the model states no concrete form and this is recorded behavior only: the recorded response carries a long, high-entropy opaque ciphertext blob rendered in a base64-style alphabet, which is exactly why the fixture asserts the member by SHAPE and not by literal value (expected is the marker '<shaped-not-asserted>'; the literal bytes are per-run and are not a requirement). Status, `tokenExpirationTime` and `sessionExpirationTime` are not in dispute — the replayed step is 200 -> 200 and the only diverging key is $.token.

Emulator: it mints the token as two u64s rendered in base62 and zero-padded to 24 characters each, concatenated — a value that is always exactly 48 characters, drawn only from [0-9A-Za-z], carries long runs of leading '0' padding in each half, and is a deterministic function of the service store's monotonic mint counter (so consecutive mints differ only in their low-order characters and are predictable from one observed token). The replayed step returned a value of exactly that form, and the replay reports `$.token: value_mismatch (expected '<shaped-not-asserted>')` for ivschat|CreateChatToken|status=200->200.

Why: a client cannot tell from the emulator that its handling of a real chat token is wrong. Anything that sizes, stores or sanity-checks the token (buffer or column widths, "looks like an encrypted blob" checks, alphabet/length assertions in a client's own test fixtures) is validated against a 48-character base62 string and then overflows or rejects AWS's much longer base64-style ciphertext. Worse, because the value is a pure function of a per-process counter, any client or test that depends on a token being unguessable — forgery, enumeration or token-reuse negative tests — passes here and fails on AWS, where the token is encrypted material.

Fix: this belongs in the operation's state layer, the chat-token mint that `CreateChatToken`'s handler calls (the handler itself is otherwise correct: it validates the room, the capability values and sessionDurationInMinutes, and reads the response back out of the stored record). The mint should produce a token of AWS's recorded form — a long, high-entropy opaque string over a base64-style alphabet, with no fixed width padding and no derivation from a visible sequence counter — rather than a fixed 48-character zero-padded base62 rendering of the mint counter. Uniqueness within the store, pruning and the expiry timestamps all stay as they are. No status and no error changes.

This is not covered by any open ivschat issue: 20261029T171751Z-ivschat-client-error-responses-carry-no-error-envelope.md is about missing error envelopes on refusals, 20261030T164443Z-ivschat-empty-bodied-204-tag-responses-omit-the.md about content-length on 204 tag responses, and 20261023T054149Z-ivschat-logging-configuration-fidelity-missing-s3-destination-validation.md about logging configurations; none of them touch the success-path token value.

Traces: ivschat-chat-token-and-messaging#task#2 (CreateChatToken, 200 -> 200, $.token).

## Evidence
Replay cluster service=ivschat|CreateChatToken|status=200->200|keys=$.token, fixture ivschat-chat-token-and-messaging step task#2 (affected_count 1).

Expected (fixture): HTTP 200 with $.token present and matching the recorded token's shape — a long opaque encrypted token; the recording's literal bytes are deliberately not asserted ('<shaped-not-asserted>'), because the value is per-run.

Actual (emulator): HTTP 200 with $.token = a 48-character string of the form <24 base62 chars><24 base62 chars> whose halves are zero-padded renderings of a counter-derived u64 (the replayed run produced one such value, '0000000000000Jf92XanNkbJ00000000000008GzzK6FD1PP' — quoted only to show the form: the 13-/14-character '0' padding runs and the fixed 48-character total are the defect, the particular characters are a per-run value).

Model basis: smithy_query(query="operation", operation="CreateChatToken") -> output member token targets ChatToken; smithy_query(query="shape", shape="ChatToken") -> {"type": "string", "sensitive": true} (no length, no pattern); smithy_query(query="doc", shape="CreateChatTokenResponse", member="token") -> "The issued client token, encrypted."

Code: services/service-com-amazonaws-ivschat-amazoninteractivevideoservicechat/src/state.rs:289-297 (`mint_chat_token` = base62_fixed(mix64(n ^ CHAT_TOKEN_DOMAIN), 24) ++ base62_fixed(mix64(n ^ CHAT_TOKEN_DOMAIN_HI), 24), with `minted` the monotonic store counter and base62_fixed zero-padding at state.rs:207-214); consumed at services/service-com-amazonaws-ivschat-amazoninteractivevideoservicechat/src/ops/create_chat_token.rs:235 and returned at :254-256.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus), or drive the step directly:
2. POST /CreateRoom to create a room and take its `arn` from the response.
3. POST /CreateChatToken with {"roomIdentifier": "<that room ARN>", "userId": "<any user id>"}.
4. Observe the 200 response's `token`: exactly 48 characters, only [0-9A-Za-z], with a long run of leading '0' characters in each 24-character half.
5. Repeat step 3: the next token differs only in its low-order characters, showing it is a rendering of a monotonic counter rather than opaque encrypted material of AWS's recorded form.
6. Replay fixture ivschat-chat-token-and-messaging, step task#2: the step reports $.token value_mismatch against the fixture's shape assertion while the status stays 200.
