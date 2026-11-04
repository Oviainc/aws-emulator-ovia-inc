Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 024100b485faef6e8a7d69224b42c4d112173d44

# ExchangeCodeForToken answers an unmodelled ServiceQuotaExceededException (402) for an emulator store bound, where StartCodegenJob writes down the opposite rule for the same condition

_Filed: 2026-11-04T04:58:02Z_
_Operation: ExchangeCodeForToken | Class: error-response_

> _Note: this issue was identified against the code base at commit `024100b485faef6e8a7d69224b42c4d112173d44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate meets one condition at four sites — "an emulator-owned bound on a store is reached on an operation whose model declares NO quota error" — and resolves it two contradictory ways. `ExchangeCodeForToken` is the fourth site and it takes the side its sibling `StartCodegenJob` explicitly argues against.

`ExchangeCodeForToken`'s model declares exactly ONE error: `InvalidParameterException` (400). The handler widens `ServiceQuotaExceededException` (402) in through `extra_errors` and answers it when the live-session store is at `PROVIDER_TOKEN_SESSION_QUOTA`, justifying it with "that store is bounded like the component and metadata stores". `StartCodegenJob` is in exactly the same position — an emulator bound (`CODEGEN_JOB_QUOTA`) on an operation whose model declares no quota error — and that handler writes the OPPOSITE rule into a comment: "This operation models no quota error, so the refusal is the modelled `ThrottlingException` (HTTP 429)". Both stated reasons cannot hold at once: either widening an unmodelled quota shape in is acceptable (and StartCodegenJob's reasoning is wrong) or it is not (and this handler's 402 is wrong). As written, the crate's own rule for this one condition is self-contradictory.

The consequence for a client is concrete because this service strips the error-type discriminator from every refusal (`src/service/untyped_errors.rs`), so the STATUS is all a client has to classify an answer by: an SDK client of `ExchangeCodeForToken`, whose only modelled refusal is a 400, receives a 402 that matches no modelled exception class of this operation.

This is the ExchangeCodeForToken side of a rule already filed for two of the other three sites (`PutMetadataFlag`, issue 20261030T063927Z-amplifyuibuilder-putmetadataflag-error-response; `RefreshToken`, issue 20261103T184956Z-amplifyuibuilder-refreshtoken-error-response) and is not covered by either: this operation has its own `extra_errors` declaration, its own refusal site and its own pinning test. As at the RefreshToken site, a test restates this handler's justification and pins the current 402, so resolving the crate's rule moves that test with it. The `InternalServerException` (500) this handler also widens in is NOT part of this defect: an unreachable store is a server-side condition that every handler of this crate reports that way.

## Evidence

The model gives this operation one error, and no quota error (smithy_query(query="operation", operation="ExchangeCodeForToken")):

    "errors": [{"shape": "InvalidParameterException", "errorKind": "client", "httpError": 400}]

Side 1 — the handler widens the unmodelled 402 in and answers it:

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/exchange_code_for_token.rs:66-69
    //! * `ServiceQuotaExceededException` (HTTP 402) — widened in through `extra_errors` (see
    //!   below): the shape is this service's own, and it is how this emulator refuses an
    //!   exchange past its own bound on the token-session store, as `PutMetadataFlag` refuses
    //!   a write past its bound on the metadata store.

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/exchange_code_for_token.rs:96-104
    // * `ServiceQuotaExceededException` (HTTP 402) — every exchange starts a token
    //   session, so that store is bounded like the component and metadata stores
    //   (`crate::state::PROVIDER_TOKEN_SESSION_QUOTA`, the same bound `RefreshToken`
    //   reads), and past that bound an exchange is refused rather than growing the
    //   state without end.
    #[smithy_macros::operation_error(
        "com.amazonaws.amplifyuibuilder#ExchangeCodeForToken",
        extra_errors = ["InternalServerException", "ServiceQuotaExceededException"]
    )]

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/exchange_code_for_token.rs:223-235
        if !store.has_room_for_another_provider_token_session() {
            return Err(
                ExchangeCodeForTokenErrorModeled::ServiceQuotaExceededException(
                    ServiceQuotaExceededException {
                        message: Some(format!(
                            "this emulator already holds the maximum of {} live token sessions",
                            crate::state::PROVIDER_TOKEN_SESSION_QUOTA
                        )),
                    },
                )
                .into(),
            );
        }

Side 2 — the sibling handler in the SAME crate, facing the same condition, states the rule the other way round:

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/start_codegen_job.rs:187-195
        // The store is bounded: past its quota a start is refused rather than adding
        // another record. This operation models no quota error, so the refusal is the
        // modelled `ThrottlingException` (HTTP 429) — the one client refusal whose
        // semantics, the service declining to take more work, fit a store that cannot
        // take another job; a `ValidationException` would blame the request, which is
        // well formed.
        if store.total_codegen_jobs() >= crate::state::CODEGEN_JOB_QUOTA {
            return Err(StartCodegenJobErrorModeled::ThrottlingException(

Why the status is the whole answer to a client here:

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/service/untyped_errors.rs:16-18
    //! A client therefore classifies the failure by STATUS: botocore, finding no code in
    //! either place, reports `Error.Code` as the bare status string (`"404"`, `"400"`,
    //! `"409"`) rather than a shape name

The bound itself is the emulator's own, not the model's:

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/state.rs:1622-1630
    /// A session is keyed by request-supplied values, so without a bound every
    /// request naming a fresh refresh token would add an entry. Amplify UI Builder
    /// publishes no quota this emulator can quote, so — exactly like
    /// [`COMPONENT_QUOTA`] — this is the emulator's OWN bound on the store
    pub const PROVIDER_TOKEN_SESSION_QUOTA: usize = 1_000;

Test coverage today — the current 402 IS pinned, restating this handler's own justification, so this test moves with the rule:

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/tests/all/provider_tokens.rs:575-577
    /// Rule 6: the session store is bounded. Past the emulator's own bound an exchange is
    /// refused with HTTP 402 — the quota shape `CreateComponent` uses for the same kind of
    /// bound — and stores nothing.
  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/tests/all/provider_tokens.rs:606-607
            refused.status, 402,
            "case {label:?}: an exchange past the bound must be refused with HTTP 402, got {} \

(`CreateComponent` differs exactly here: it MODELS `ServiceQuotaExceededException` (402), so using it for an emulator bound names a shape the operation declares. `ExchangeCodeForToken` models neither a 402 nor a 429.)

## Steps to Reproduce

1. Read services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/exchange_code_for_token.rs:61-73 — the operation's modelled error list is `InvalidParameterException` (HTTP 400) alone, confirmed by smithy_query(query="operation", operation="ExchangeCodeForToken").
2. Read the same file at lines 96-104 and 223-235 — `ServiceQuotaExceededException` (402) is widened in through `extra_errors` and answered for the emulator's own `PROVIDER_TOKEN_SESSION_QUOTA` bound (state.rs:1630, 1_000).
3. Read src/ops/start_codegen_job.rs:187-195 — the sibling handler states that an operation modelling no quota error must refuse with a MODELLED error instead, and answers the modelled 429 for its own emulator bound.
4. POST /tokens/figma with request = {"code": "code-<n>", "redirectUri": "https://app.example/cb"} for 1000 distinct codes, each answering 200 and starting one live session.
5. POST /tokens/figma once more with a fresh code: observed HTTP 402 carrying a `ServiceQuotaExceededException` payload with no error-type discriminator — a status this operation's model never declares, where the sibling bound on StartCodegenJob answers the modelled 429 by the rule quoted in step 3.
