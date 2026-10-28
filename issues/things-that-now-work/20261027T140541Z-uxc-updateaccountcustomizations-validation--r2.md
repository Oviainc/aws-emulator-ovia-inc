Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: f39310ffb44e85959dc0d40e62557237e3e92192

# A request body of the literal JSON document `null` escapes the front door and is served as an empty update

_Filed: 2026-10-27T14:05:41Z_
_Operation: UpdateAccountCustomizations | Class: validation_

> _Note: this issue was identified against the code base at commit `f39310ffb44e85959dc0d40e62557237e3e92192`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate's front door states ONE rule for a request document it will not act on: a document that is not well-formed JSON, *or* is well-formed and is not a JSON object, is refused with HTTP 400 `BadRequestException` and nothing is written. The rule is applied to every non-object document the suites exercise (`17`, `"red"`, `["red"]`, truncated objects, non-JSON text), but NOT to the one well-formed non-object document the JSON codec silently accepts: the literal `null`.

`FrontDoor::run` -> `request_rejection` -> `read_request` only reports `RequestReading::UnreadableBody` when `read_input` FAILS (src/service.rs:450-463). For a body of `null`, `read_input` SUCCEEDS: restJson1 builds the JSON body deserializer for any non-empty body (vendor/smithy-schema-server/src/rest_json.rs:209-212) and `JsonShapeDeserializer::read_struct` deliberately "Allow[s] `null` as an absent struct" (vendor/smithy-schema-server/src/json.rs:778-782), consuming the token and returning an input with every member `None`. Every member of `UpdateAccountCustomizationsInput` is optional, so `constraints::validate` finds no violation, `request_rejection` returns `None`, and the handler runs: the request is answered HTTP 200 with the stored record, exactly as `{}` is — the "nothing is written" half holds, but the refusal the crate's own rule requires does not happen, so a client sending a body the service claims it will not read is told the call succeeded.

The crate already has the distinction it needs and uses it on a neighbouring path: `members_sent_as_null` reads the body with `serde_json` and matches only `serde_json::Value::Object` (src/service.rs:366), its doc stating it is "Empty for a request with no body, a body that is not a JSON object". The refusal path never asks that question.

A fix changes no existing response: `{}` and an absent body must keep their 200 (tests/all/unreadable_body_refusal.rs:233-250), and every row already refused must keep its `BadRequestException`.

## Evidence

The rule, stated by this crate's own suite — services/service-com-amazonaws-uxc-awsaccountuxsetting/tests/all/unreadable_body_refusal.rs:180-187:

    /// A request document that is not well-formed JSON — or is well-formed and is not
    /// a JSON object — is refused the same way: it is a body the service will not act
    /// on, so the front door owns it.
    ///
    /// The rows differ along HOW the document fails to be a JSON object the input
    /// shape can be read from: truncated at several depths, a stray delimiter, a
    /// trailing comma, an unquoted key, a bare scalar, an array, and text that is not
    /// JSON at all.

and its rows (unreadable_body_refusal.rs:199-202) cover every non-object document EXCEPT `null`:

        ("a bare string", br#""red""#),
        ("a bare number", b"17"),
        ("a JSON array", br#"["red"]"#),
        ("text that is not JSON", b"not json at all"),

The same rule in the front door's own doc — src/service.rs:66-72: "The front door refuses a body it cannot READ on the same terms as one it read and rejected ... both earn the same `BadRequestException` answer."

Why `null` does not reach that answer — src/service.rs:450-463 (`read_request`), which refuses only when the codec FAILS:

        let failure = match read_input::<O>(protocol, req) {
            Ok(input) => return RequestReading::Read(constraints::validate(&input, O::INPUT_SCHEMA)),
            Err(failure) => failure,
        };

and the codec does not fail — vendor/smithy-schema-server/src/json.rs:777-782:

        let _depth = self.enter_depth()?;
        // Allow `null` as an absent struct.
        if self.peek_kind()? == TokenKind::ValueNull {
            self.next_token()?;
            return Ok(());
        }

reached for any non-empty body — vendor/smithy-schema-server/src/rest_json.rs:209-212:

        let codec = JsonCodec::rest_json();
        let body = codec.create_deserializer(body_bytes);
        let d = HttpBindingDeserializer::new(uri_pattern.as_deref(), &path, &query, headers, body);

The crate can already tell a non-object body, on the other front-door path — src/service.rs:358-368:

    /// Empty for a request with no body, a body that is not a JSON object, and a body
    /// this operation's input declares no `null` member of ...
    fn members_sent_as_null<O: Operation>(req: &RtRequest) -> Vec<String> {
        let Some(bytes) = req.body().bytes() else {
            return Vec::new();
        };
        let Ok(serde_json::Value::Object(body)) = serde_json::from_slice::<serde_json::Value>(bytes)

Test coverage: tests/all/unreadable_body_refusal.rs:189-224 pins the refused rows, and :233-250 (`an_empty_body_is_not_an_unreadable_one`) pins `{}` and an absent body at 200 — neither covers the `null` document, which is why the gap is unobserved.

## Steps to Reproduce

1. Build the crate's wire harness: `let router = build_router();` (tests/all/wire.rs:23).
2. Prime a record: `update_customizations(&router, json!({"accountColor": "red"})).await` -> 200.
3. Send the literal document `null` as the request body: `update_customizations_bytes(&router, b"null").await` (tests/all/wire.rs:118).
4. Observe HTTP 200 with the stored record — the same answer `{}` gets — instead of the HTTP 400 `BadRequestException` the crate's stated rule gives every other well-formed non-object document.
5. Contrast with the neighbouring rows `b"17"`, `br#"\"red\""#` and `br#"[\"red\"]"#` (tests/all/unreadable_body_refusal.rs:199-201), which are refused 400 `BadRequestException` in both `x-amzn-errortype` and `__type`.
