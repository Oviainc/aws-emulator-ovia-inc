Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 8b4869ff85a1bb54e2d6ff99978d62233f666034

# A payload the codec cannot read is answered with the typed, unmodelled ValidationException, which this crate's response path says no response names

_Filed: 2026-11-03T20:11:42Z_
_Operation: CreateForm | Class: error-response_

> _Note: this issue was identified against the code base at commit `8b4869ff85a1bb54e2d6ff99978d62233f666034`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This service's response path exists to make every refusal UNTYPED: `untyped_errors.rs` strips the `x-amzn-errortype` header and the `__type`/`code`/`Code` body member from every exception this service's model declares, and its module doc states as a fact that no response of this service names the framework's own `ValidationException`.

That claim holds only for the refusals the framework can describe structurally. `EmulatedService::override_refusal` only rewords `Refusal::Invalid` (a missing required member and a value outside a declared `@length`/`@range`/`@pattern`/enum bound), and the framework builds a `Refusal` for nothing else: for any OTHER codec failure — a body that is not well-formed JSON, or a member carrying the wrong JSON type (e.g. `"name": 5` where a string is modelled) — `refusal_of` answers `None`, so the seam is never consulted and the restJson1 pre-dispatch renderer answers its own `400 ValidationException`, typed, with the error-type header AND a `__type` member. `untyped_errors::without_discriminator` then deliberately leaves it alone, because `ValidationException` is not in `MODELLED_EXCEPTIONS`.

So a caller that POSTs a malformed `formToCreate` payload receives a shape `com.amazonaws.amplifyuibuilder` does not declare, named on the wire in both places a client reads it, while every other 400 of this service (an out-of-bound value since the sibling fix, a reserved property name, an unknown nextToken, a wrong-scope id) is this service's `InvalidParameterException` with no error type at all. The defect is in the service-wide seam (`src/service.rs` + `src/service/untyped_errors.rs`), so it is reachable on every operation that takes an `@httpPayload` body; `CreateForm` is named because `formToCreate` is `@httpPayload`-required and is the simplest reproduction. NOTE: this is NOT the already-fixed modelled-bound class filed under CreateComponent/error-response — that fix added the `Refusal::Invalid` arm, which this class never reaches.

## Evidence

The crate's own claim, which the code below breaks:

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/service/untyped_errors.rs:33-38
    //! A request the framework refused before any handler ran reaches here already naming one
    //! of this service's own exceptions, because this service's refusal seam
    //! ([`crate::service::EmulatedService`]'s `override_refusal`) answers that refusal with the
    //! shape the model declares for an invalid input value; so it loses its discriminator here
    //! like every other refusal of this service, and no response names the framework's own
    //! `ValidationException`, which this service's model does not declare.

  services/.../src/service/untyped_errors.rs:3-7
    //! How this service names the exceptions it answers on the wire: it does not.
    //! AmplifyUIBuilder answers a refused call with the status and a `message`, and with NO
    //! error-type discriminator — neither the `x-amzn-errortype` header restJson1 resolves an
    //! error shape from, nor a `__type` / `code` / `Code` member in the body

The seam, which covers only one of the two ways a request can fail to be read:

  services/.../src/service.rs:158-164
        match refusal {
            Refusal::Invalid(violations) => Some(Refused {
                code: INVALID_PARAMETER_EXCEPTION.to_string(),
                message: aws_smithy_schema_server::envelope(violations)?,
            }),
            _ => None,
        }

Why a codec failure never reaches that seam, and what it is rendered as instead (cited, not read as design input):

  vendor/smithy-server-runtime/src/pre_dispatch.rs:198-212 (`refusal_of`)
        ProtocolError::Serde(::aws_smithy_schema::serde::SerdeError::MissingMember { member_name }) => Some(Refusal::Invalid(...)),
        ProtocolError::Invalid(violations) => Some(Refusal::Invalid(violations.clone())),
        ProtocolError::Unauthenticated { .. } => Some(Refusal::Unauthenticated),
        _ => None,
  vendor/smithy-server-runtime/src/pre_dispatch.rs:217-223 (`render_refusal`)
        let renderer = renderer?;
        renderer(&refusal_of(error)?)
  vendor/smithy-server-runtime/src/pre_dispatch.rs:447 (the restJson1 renderer)
        ProtocolError::Serde(e) => (400, "ValidationException", serde_message(&e)),

And the strip that then leaves it typed, plus the unit test that pins exactly this body as untouched — with the stated reason that is the claim above:

  services/.../src/service/untyped_errors.rs:55-63
    const MODELLED_EXCEPTIONS: [&str; 7] = [ "InternalServerException", "InvalidParameterException", "ResourceConflictException", "ResourceNotFoundException", "ServiceQuotaExceededException", "ThrottlingException", "UnauthorizedException" ];
  services/.../src/service/untyped_errors.rs:304-315
    /// Nothing to strip: a code this service does not model — the scaffold's 501 and the
    /// framework's own `ValidationException`, which no response of this service names any
    /// more ...
            ("a shape this service's model does not declare", r#"{"__type":"ValidationException","Message":"no"}"#),

Test coverage today: `tests/all/invalid_parameter_refusals.rs:218-323` enumerates the refusal rows — too long, too short, absent member, below/above range, outside pattern — i.e. only `Refusal::Invalid` classes; no row sends a malformed body or a wrong-typed member, so the one remaining typed refusal of this service is untested, and its own doc (`invalid_parameter_refusals.rs:3-15`: "a request refused because it does not satisfy this service's own input rules is answered with ... `InvalidParameterException` ... naming no error type") describes the behaviour the codec path does not deliver.

## Steps to Reproduce

1. POST /app/d1234abcd/environment/staging/forms with an authorization header scoped to amplifyuibuilder and the body `{"name":` (not well-formed JSON), content-type application/json.
2. Observe the response: HTTP 400 with `x-amzn-errortype: ValidationException` and a body whose `__type` is `ValidationException` — a shape com.amazonaws.amplifyuibuilder declares nowhere, named in both places a client reads a code.
3. Repeat with a well-formed body carrying a wrong JSON type for a modelled string, e.g. {"name": 5, "formActionType": "create", "dataType": {"dataSourceType": "DataStore", "dataTypeName": "Post"}, "fields": {}, "style": {}, "sectionalElements": {}, "schemaVersion": "1.0"} — same typed answer.
4. Contrast with POST /app/d1234abcd/environment/staging/forms carrying `{"name": "<256 chars>", ...}` (a modelled @length violation): HTTP 400 with NO error-type header and no `__type`, i.e. this service's InvalidParameterException — the two refusals of the same class answer in two different wire forms.
