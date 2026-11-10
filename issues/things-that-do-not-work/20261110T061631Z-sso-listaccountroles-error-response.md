Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 1ecf6f83c17663b1cceb804b029a144947c92d6a

# A `max_result` the codec cannot read answers the framework's unmodelled `ValidationException`, although the service's own seam says it answers `InvalidRequestException`

_Filed: 2026-11-10T06:16:31Z_
_Operation: ListAccountRoles | Class: error-response_

> _Note: this issue was identified against the code base at commit `1ecf6f83c17663b1cceb804b029a144947c92d6a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`max_result` is the only non-string wire-bound member this service has (`@httpQuery("max_result")` over `MaxResultType`, an `integer`). When an AUTHENTICATED caller sends a value the codec cannot parse as an integer, the request is refused 400 with `__type: ValidationException` and `x-amzn-errortype: ValidationException` — an error shape that does not exist anywhere in this service's Smithy model (the service declares exactly `InvalidRequestException`, `TooManyRequestsException`, `UnauthorizedException` as common errors, plus `ResourceNotFoundException` on this operation). A restJson1 SDK resolves the error shape from that `x-amzn-errortype` header, so it cannot map the refusal onto any modelled SSO Portal error at all.

The crate states the opposite rule for itself in `EmulatedService::override_refusal`: "This service states a request it will not read as `InvalidRequestException` ... rather than the framework's generic `ValidationException`." That rewrite only ever fires for `Refusal::Invalid`, and a codec parse failure never becomes one: the query integer parse fails as `SerdeError::custom("invalid integer `…`")`, and `refusal_of` maps every non-`MissingMember` `Serde` error to `None`, so `override_refusal` is never consulted and the framework's default rendering stands. The crate's own test admits the resulting divergence in the `RefusedByTheCodec` variant and then asserts only the status (400), never the code — so nothing in the suite pins the code, and the comment in `service.rs` is simply false for this class of unreadable request.

The modelled error for exactly this condition exists and is a SERVICE-COMMON error of all four operations: `InvalidRequestException` — "Indicates that a problem occurred with the input to the request." A value that is not an integer is a problem with the input to the request.

The crate already has the seam pattern needed to answer it: `override_operation` (same file) returns a crate-owned `ErasedOperation` to answer a request before the framework reads it — that is how the portal's 401 gate works — so refusing an unreadable `max_result` as the modelled 400 needs no new mechanism and no change in `vendor/`.

The same seam serves `ListAccounts` identically (`max_result` is modelled on it too, and the crate's test row uses `/assignment/accounts`); it is filed once, on `ListAccountRoles`, because `override_refusal` is service-wide and single-homed in `src/service.rs`. It is a DIFFERENT root cause from the already-fixed ListAccounts/error-response issue (which was about authentication-first precedence for `Refusal::Invalid`): here the caller IS authenticated, the route IS matched, and the gap is that this refusal class never reaches the service's wording seam at all.

## Evidence

The rule the service states for itself — `services/service-com-amazonaws-sso-swbportalservice/src/service.rs:337-348`:
```rust
    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // This service states a request it will not read as `InvalidRequestException`,
        // the error its model declares for exactly that — "a required parameter might
        // be missing or out of range" — rather than the framework's generic
        // `ValidationException`. The status (400) is the protocol's and matches the
        // modelled `@httpError`. Both the code and the message are replaced together.
        ...
        if let Refusal::Invalid(violations) = refusal {
            return Some(Refused {
                code: "InvalidRequestException".to_string(),
```

The modelled error it names, and the fact that no `ValidationException` exists in this model — `smithy_query(query="service_info")`:
```
"common_errors": ["InvalidRequestException", "TooManyRequestsException", "UnauthorizedException"]
```
`smithy_query(query="doc", shape="com.amazonaws.sso#InvalidRequestException")`:
```
"<p>Indicates that a problem occurred with the input to the request. For example, a required\n      parameter might be missing or out of range.</p>"
```
`smithy_query(query="operation", operation="ListAccountRoles")` — the declared error set has four members and `ValidationException` is not one of them:
```
"errors": [{"shape": "InvalidRequestException", ... 400}, {"shape": "ResourceNotFoundException", ... 404}, {"shape": "TooManyRequestsException", ... 429}, {"shape": "UnauthorizedException", ... 401}]
"maxResults": {"target": "MaxResultType", "httpQuery": "max_result", "range": {"min": 1, "max": 100}, "type": "integer"}
```
(the same four are listed in `services/service-com-amazonaws-sso-swbportalservice/OPERATIONS.md:39`).

Why the seam is never consulted — the query value is parsed into the member's type and the failure is a plain custom `SerdeError` (`vendor/smithy-schema-server/src/http_binding.rs:523-525`):
```rust
        s.parse::<T>()
            .map_err(|e| SerdeError::custom(format!("invalid {kind} `{s}`: {e}")))
```
and `refusal_of` gives such an error no structured form (`vendor/smithy-server-runtime/src/pre_dispatch.rs:198-212`):
```rust
pub fn refusal_of(error: &ProtocolError) -> Option<Refusal> {
    match error {
        ProtocolError::Serde(::aws_smithy_schema::serde::SerdeError::MissingMember { member_name }) => Some(Refusal::Invalid(...)),
        ProtocolError::Invalid(violations) => Some(Refusal::Invalid(violations.clone())),
        ProtocolError::Unauthenticated { .. } => Some(Refusal::Unauthenticated),
        _ => None,
    }
}
```
pinned by the framework's own unit test (`vendor/smithy-server-runtime/src/pre_dispatch.rs:585-588`):
```rust
        assert_eq!(
            refusal_of(&ProtocolError::Serde(SerdeError::custom("other"))),
            None
        );
```
so the default rendering stands, and for restJson1 it is `ValidationException` in both the body and the shape-resolving header (`vendor/smithy-server-runtime/src/pre_dispatch.rs:446-448` and `:475-477`):
```rust
    let (status, type_name, message) = match error {
        ProtocolError::Serde(e) => (400, "ValidationException", serde_message(&e)),
...
    if errortype_header {
        resp.headers_mut().insert("x-amzn-errortype", type_name);
    }
```

The crate's own admission of the divergence, and the reason the suite does not catch it — `tests/all/portal_session.rs:793-796`:
```rust
    /// Refused with HTTP 400 and this error code.
    Refused(&'static str),
    /// Refused with HTTP 400 by the codec, before any rule of this service applies, so
    /// the code is the framework's `ValidationException` rather than a modeled one.
    RefusedByTheCodec,
```
with the row (`tests/all/portal_session.rs:817-819`):
```rust
        // Not an integer, so the request never becomes a value with a range to check:
        // the codec refuses it (400) before any rule of this service's applies.
        ("not-a-number", Expect::RefusedByTheCodec),
```
and the assertion for it, which checks the STATUS only and never the code (`tests/all/portal_session.rs:845-852`):
```rust
            Expect::RefusedByTheCodec => assert_eq!(
                response.status(),
                StatusCode::BAD_REQUEST,
```
Every sibling row of the same test asserts `(status, error_code) == (400, "InvalidRequestException")` (`tests/all/portal_session.rs:837-844`, `:880-889`, `:922-936`), so this one row is the only unreadable-input class of the service that answers an unmodelled code.

## Steps to Reproduce

1. Stand the service up as `tests/all/portal_session.rs:85-114` does: an `InMemoryEngine`, `EmulatedService::new(engine)` registered on a `MultiServiceRouter`, `put_account("111111111111", "Production", "production@example.com", ["AdministratorAccess", "ReadOnly"])`.
2. Seed a live sign-in session: `put_session("tok-live", "tok-live@example.com", ["111111111111"], 8h)` (i.e. `portal.sign_in("tok-live", &["111111111111"])`).
3. Dispatch `GET https://portal.sso.us-east-1.amazonaws.com/assignment/roles?account_id=111111111111&max_result=not-a-number` with `host: portal.sso.us-east-1.amazonaws.com` and `x-amz-sso_bearer_token: tok-live` (an authenticated caller, so the gate in `override_operation` at src/service.rs:307-335 lets it through).
4. Observed: HTTP 400 with body `{"__type":"ValidationException","message":"invalid integer `not-a-number`: …"}` and header `x-amzn-errortype: ValidationException` — the codec error is a `SerdeError::custom`, `refusal_of` returns `None`, `override_refusal` is never called, and the framework's json renderer names `ValidationException` (vendor/smithy-server-runtime/src/pre_dispatch.rs:447, :476).
5. Expected per the service's own seam comment (src/service.rs:338-342) and the modelled error for exactly this condition (`InvalidRequestException`, 400, "Indicates that a problem occurred with the input to the request", a common error of all four operations): 400 `InvalidRequestException`, as every other unreadable-input row of `tests/all/portal_session.rs` already gets.
6. Contrast, same operation and same caller: `GET /assignment/roles?account_id=111111111111&max_result=0` (out of range) and `GET /assignment/roles?account_id=111111111111&next_token=not-a-cursor` both answer 400 `InvalidRequestException` (pinned at tests/all/portal_session.rs:814-816 and :922-936) — so two unreadable spellings of the SAME parameter get different, and one unmodelled, error codes.
7. Repeat on the sibling listing (`GET /assignment/accounts?max_result=not-a-number` with the same token): identical answer, since `override_refusal` is service-wide.
