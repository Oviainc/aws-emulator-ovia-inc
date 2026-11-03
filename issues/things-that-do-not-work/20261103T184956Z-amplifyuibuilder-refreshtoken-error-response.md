Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: ee0868494649d03b19e6cb313685ea30e4c93f71

# RefreshToken answers an unmodelled ServiceQuotaExceededException (402) for an emulator store bound, where StartCodegenJob states the opposite rule for the same condition

_Filed: 2026-11-03T18:49:56Z_
_Operation: RefreshToken | Class: error-response_

> _Note: this issue was identified against the code base at commit `ee0868494649d03b19e6cb313685ea30e4c93f71`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate meets one situation three times — "an emulator-owned bound on a store is reached on an operation whose model declares NO quota error" — and resolves it two contradictory ways. `RefreshToken` is the third site, and it takes the side its sibling `StartCodegenJob` explicitly argues against.

`RefreshToken`'s model declares exactly ONE error: `InvalidParameterException` (400). The handler widens `ServiceQuotaExceededException` (402) in through `extra_errors` and answers it when the live-session store is at `PROVIDER_TOKEN_SESSION_QUOTA`, justifying it with "the store is bounded like the component and metadata stores". `StartCodegenJob` is in exactly the same position — an emulator bound (`CODEGEN_JOB_QUOTA`) on an operation whose model declares no quota error — and that handler writes down the OPPOSITE rule in a comment: "This operation models no quota error, so the refusal is the modelled `ThrottlingException` (HTTP 429)". Both reasons cannot hold at once: either widening an unmodelled quota shape in is acceptable (and StartCodegenJob's stated reasoning is wrong) or it is not (and RefreshToken's 402 is wrong). As written, the crate's own rule for this one condition is self-contradictory, and because this service strips the error-type discriminator from every refusal (`src/service/untyped_errors.rs`), the status is ALL a client has to classify the answer by — so an SDK client of `RefreshToken`, whose only modelled refusal is a 400, gets a 402 no modelled exception class matches.

The same contradiction is already filed for the second site (`PutMetadataFlag`, issue 20261030T063927Z-amplifyuibuilder-putmetadataflag-error-response); this is the RefreshToken side of the one rule, and it differs in one respect the fixer must know: a test pins the current 402 here (see evidence), restating RefreshToken's own justification, so resolving the crate's rule changes that test with it. The `InternalServerException` (500) this handler also widens in is NOT part of this defect: a store this handler cannot reach is a server-side condition every handler in the crate reports that way (`GetMetadata` widens the same shape for the same reason).

## Evidence

The model gives this operation one error and no quota error (smithy_query(query="operation", operation="RefreshToken")):

    "errors": [{"shape": "InvalidParameterException", "errorKind": "client", "httpError": 400}]

Side 1 — the handler widens the unmodelled 402 in and answers it:

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/refresh_token.rs:98-106
    // * `ServiceQuotaExceededException` (HTTP 402) — the session a refresh stores is keyed by
    //   request-supplied values, so the store is bounded like the component and metadata
    //   stores (`crate::state::PROVIDER_TOKEN_SESSION_QUOTA`), and past that bound a refresh
    //   that would start ANOTHER session is refused rather than growing the state without
    //   end.
    #[smithy_macros::operation_error(
        "com.amazonaws.amplifyuibuilder#RefreshToken",
        extra_errors = ["InternalServerException", "ServiceQuotaExceededException"]
    )]

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/refresh_token.rs:213-223
        if !store.has_room_for_provider_token_session(&key) {
            return Err(RefreshTokenErrorModeled::ServiceQuotaExceededException(
                ServiceQuotaExceededException {
                    message: Some(format!(
                        "this emulator already holds the maximum of {} live token sessions",
                        crate::state::PROVIDER_TOKEN_SESSION_QUOTA
                    )),
                },
            )
            .into());
        }

Side 2 — the sibling handler in the SAME crate, facing the same condition, states the rule the other way round:

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/start_codegen_job.rs:187-204
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

Test coverage today — the current 402 IS pinned, with RefreshToken's own justification restated, so this test moves with the rule:

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/tests/all/provider_token_refresh.rs:424-426
    /// Rule 6: the session store is bounded. Past the bound a refresh naming a session the
    /// store does not hold is refused with HTTP 402 — the quota shape `CreateComponent` uses
    /// for the same kind of bound — while a session already held still refreshes.
  services/.../tests/all/provider_token_refresh.rs:449-451
            refused.status, 402,
            "case {case:?}: a refresh past the bound must be refused with HTTP 402, got {} with \

(`CreateComponent` and `PutMetadataFlag` differ exactly here: `CreateComponent` MODELS `ServiceQuotaExceededException` (402) — smithy_query(query="operation", operation="CreateTheme") shows the same for the other creates — while `RefreshToken` and `PutMetadataFlag` model neither a 402 nor a 429.)

## Steps to Reproduce

1. Read services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/refresh_token.rs:70-73 — the operation's modelled error list is `InvalidParameterException` (HTTP 400) alone.
2. Read the same file at lines 98-106 and 213-223 — `ServiceQuotaExceededException` (402) is widened in through `extra_errors` and answered for the emulator's own `PROVIDER_TOKEN_SESSION_QUOTA` bound.
3. Read src/ops/start_codegen_job.rs:187-204 — the sibling handler states that an operation modelling no quota error must refuse with a MODELLED error instead, and answers the modelled 429.
4. POST /tokens/figma/refresh with 1000 distinct `refreshTokenBody.token` values (state.rs:1390, PROVIDER_TOKEN_SESSION_QUOTA = 1_000), each answering 200.
5. POST /tokens/figma/refresh once more with a fresh token: observed HTTP 402 with a `ServiceQuotaExceededException` payload and no error-type discriminator — a status this operation's model never declares, where the sibling bound on StartCodegenJob answers the modelled 429 by the rule quoted in step 3.
