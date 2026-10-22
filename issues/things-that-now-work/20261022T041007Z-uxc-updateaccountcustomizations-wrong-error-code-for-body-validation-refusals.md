Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: uxc.visible-service-invalid-pattern-rejected uxc.visible-services-max-length-service-id-boundary-accepted uxc.visible-services-exceed-max-size-rejected uxc.invalid-account-color-enum-value-rejected uxc.arbitrary-pattern-valid-service-id-accepted-without-existence-check
Issue-Shape-Id: com.amazonaws.uxc#AWSAccountUXSetting
Issue-Key: uxc.visible-service-invalid-pattern-rejected|run#7|AssertionError|assert code == "BadRequestException", f"expected BadRequestException, got code={code!r}"
Issue-Key: uxc.visible-services-max-length-service-id-boundary-accepted|run#10|AssertionError|assert code == "BadRequestException", f"unexpected code={code!r}"
Issue-Key: uxc.visible-services-exceed-max-size-rejected|run#9|AssertionError|assert code == "BadRequestException", f"unexpected code={code!r}"
Issue-Key: uxc.invalid-account-color-enum-value-rejected|run#8|AssertionError|assert code == "BadRequestException", f"unexpected code={code!r}"
Issue-Key: uxc.arbitrary-pattern-valid-service-id-accepted-without-existence-check|_error#9|AssertionError|raise AssertionError(f"real AWS rejects this call, but it SUCCEEDED and returned {result!r}")
Issue-Commit: 6d3a954d57304c873e4e6a3ccc977c1ac1664bbd

# UpdateAccountCustomizations: wrong error code for body-validation refusals, and visibleServices accepted without catalog-membership check

> _Note: this issue was identified against the code base at commit `6d3a954d57304c873e4e6a3ccc977c1ac1664bbd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateAccountCustomizations

## Description
THREE causes, all on uxc UpdateAccountCustomizations (PATCH /v1/account-customizations). All three must be closed before submitting; each names its own failing behavioral checks so each can be re-replayed.

Related open issue (wire sweep): '20261022T030944Z-uxc-updateaccountcustomizations-diverges-on-list-order-and-invalid-member.md'. Its Cause 1 describes the same error-code family for a visibleServices @pattern violation seen through a recorded fixture; these behavioral checks were not marked OWNED by it, and Causes 2 and 3 below (the accountColor enum guard and the missing catalog-membership check) are not covered there at all. Fixing that issue's Cause 1 alone will not make these checks pass.

Cause 1 of 3: shape/constraint violations in the request body are refused with the wrong error code.
AWS: recorded behavior only for the error identity. Smithy (com-amazonaws-uxc-awsaccountuxsetting): UpdateAccountCustomizations declares errors AccessDeniedException (403), InternalServerException (500), ThrottlingException (429) and ValidationException (400, httpError defaulted); its visibleServices member targets ServiceList (@length max 500) whose member Service is @length(1,64) @pattern ^[a-z0-9]+(-[a-z0-9]+)*$. The behavioral checks record that a body violating any of those shape bounds is refused with HTTP 400 and error code BadRequestException (an error code this service's model does not declare) with the generic body message 'Invalid request body' (that text is an EXAMPLE; the requirement is the status and the error code). Three distinct shape violations all land in this class: a member violating the pattern ('Invalid_Service!'), a member longer than the 64-char maximum ('a'*65, sent with botocore parameter validation disabled so the service performs the check), and a list of 501 entries exceeding @length max 500.
Emulator: HTTP 400 with error code ValidationException for all three, carrying a per-member constraint message instead.
Why: the error code is what an SDK uses to select the modeled exception it raises. A client that catches this service's BadRequestException for a malformed body never matches against the emulator and instead sees an unmatched/modeled ValidationException, so error-path branching, retry decisions and user-facing messaging diverge for every malformed PATCH body.
Fix: the request-body boundary refusal path for this service — the validation layer that renders a constraint violation in the PATCH body into a response — which must answer this class of malformed-body rejection with error code BadRequestException and status 400 rather than lifting it into the modeled ValidationException. (Source context from the related wire issue: services/service-com-amazonaws-uxc-awsaccountuxsetting/src/ops/update_account_customizations.rs, services/service-com-amazonaws-uxc-awsaccountuxsetting/src/state.rs.)
Traces: uxc.visible-service-invalid-pattern-rejected, uxc.visible-services-max-length-service-id-boundary-accepted, uxc.visible-services-exceed-max-size-rejected.

Cause 2 of 3: an accountColor value outside the enum is refused with the wrong error code.
AWS: Smithy: UpdateAccountCustomizations.accountColor targets enum AccountColor with values none, pink, purple, darkBlue, lightBlue, teal, green, yellow, orange, red. The check records that a value outside that set ('chartreuse') is refused with HTTP 400 and error code BadRequestException, message 'Invalid request body' (example text), the previously persisted color is left untouched, and a subsequent valid update still succeeds. This is the same malformed-body class as Cause 1, but the emulator refuses it in a different place, so it is listed separately: the handler has its own pre-state accountColor enum guard.
Emulator: HTTP 400 with error code ValidationException. The effect-absence and recovery assertions are not reached.
Why: same as Cause 1 — a client catching BadRequestException for a bad enum value gets a different exception class from the emulator.
Fix: the handler's accountColor enum guard in the UpdateAccountCustomizations handler (services/service-com-amazonaws-uxc-awsaccountuxsetting/src/ops/update_account_customizations.rs), which must emit the BadRequestException error code with status 400; ideally it routes through the same malformed-body refusal used for Cause 1.
Traces: uxc.invalid-account-color-enum-value-rejected.

Cause 3 of 3: visibleServices is not validated against the service catalog — a made-up identifier is ACCEPTED and PERSISTED.
AWS: the model documents visibleServices as 'The list of Amazon Web Services service identifiers to make visible in the Amazon Web Services Management Console... For valid service identifiers, call ListServices', i.e. membership in the ListServices catalog is part of the value's validity; beyond that this is recorded behavior only. The check records that an identifier which satisfies the @pattern and @length bounds but is not a real service ('totally-made-up-service-xyz') is refused with HTTP 400 and error code ValidationException (message example: 'Invalid services provided'), the previously persisted list is left unchanged, and a real identifier ('ec2') still succeeds afterwards.
Emulator: HTTP 200 with body {"accountColor":"none","visibleServices":["totally-made-up-service-xyz"]} — the bogus identifier is accepted and stored, so GetAccountCustomizations reports it afterwards.
Why: this is an over-permissive accept, the strongest class of divergence. A client (or console) that relies on the service to reject unknown identifiers persists garbage against the emulator and only discovers the rejection in production; test suites that assert the rejection pass trivially, and the stored customization record then names a service that cannot be rendered.
Fix: the UpdateAccountCustomizations handler's validation of visibleServices — it must check each member for membership in the same service catalog ListServices pages over (the catalog in services/service-com-amazonaws-uxc-awsaccountuxsetting/src/state.rs) BEFORE writing state, and refuse non-members with status 400 / error code ValidationException without mutating the stored record. Note the related open ListServices issue: the emulator's catalog contents themselves diverge from AWS, so this membership check must be wired to the catalog, and the catalog fix is tracked separately.
Traces: uxc.arbitrary-pattern-valid-service-id-accepted-without-existence-check.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 against this checkout: uxc 12/19 passed, 7 failed.

Cause 1:
- [mismatch] uxc.visible-service-invalid-pattern-rejected — PATCH with visibleServices=['Invalid_Service!']: AssertionError: expected BadRequestException, got code='ValidationException'.
- [mismatch] uxc.visible-services-max-length-service-id-boundary-accepted — PATCH (botocore parameter_validation=False) with visibleServices=['a'*65]: AssertionError: unexpected code='ValidationException' (expected BadRequestException, message example 'Invalid request body').
- [mismatch] uxc.visible-services-exceed-max-size-rejected — PATCH with 501 visibleServices entries: AssertionError: unexpected code='ValidationException' (expected BadRequestException).

Cause 2:
- [mismatch] uxc.invalid-account-color-enum-value-rejected — PATCH with accountColor='chartreuse' after priming accountColor='red': AssertionError: unexpected code='ValidationException' (expected BadRequestException, message example 'Invalid request body').

Cause 3:
- [mismatch] uxc.arbitrary-pattern-valid-service-id-accepted-without-existence-check — after priming visibleServices=['s3'], PATCH visibleServices=['totally-made-up-service-xyz'] SUCCEEDED: AssertionError: real AWS rejects this call, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'accountColor': 'none', 'visibleServices': ['totally-made-up-service-xyz']}. Expected code ValidationException (message example 'Invalid services provided') with the prior list ['s3'] left intact.

## Steps to Reproduce
1. Boot the emulator from this checkout and run the behavioral suite simparity-behavioral-aws-v1 (service uxc).
2. Cause 1: replay uxc.visible-service-invalid-pattern-rejected, uxc.visible-services-max-length-service-id-boundary-accepted and uxc.visible-services-exceed-max-size-rejected — each PATCHes /v1/account-customizations with a visibleServices value violating one shape bound (pattern, 64-char member length, 500-entry list length) and reads the error code: expect BadRequestException / 400, emulator answers ValidationException / 400.
3. Cause 2: replay uxc.invalid-account-color-enum-value-rejected — PATCH /v1/account-customizations with accountColor='chartreuse'; expect BadRequestException / 400, emulator answers ValidationException / 400. Then confirm the persisted color is unchanged and a valid update still succeeds.
4. Cause 3: replay uxc.arbitrary-pattern-valid-service-id-accepted-without-existence-check — set visibleServices=['s3'], then PATCH visibleServices=['totally-made-up-service-xyz']; expect 400 / ValidationException and GetAccountCustomizations still reporting ['s3'], emulator answers 200 and persists the bogus identifier.
