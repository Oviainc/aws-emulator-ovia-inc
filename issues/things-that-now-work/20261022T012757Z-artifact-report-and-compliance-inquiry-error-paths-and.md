Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: artifact.get-report-metadata-invalid-id-pattern-rejected artifact.get-report-metadata-not-found artifact.create-compliance-inquiry-invalid-client-token-pattern-rejected artifact.get-report-term-token-cross-resource-rejected artifact.get-report-download-roundtrip artifact.list-reports-max-results-out-of-range-rejected
Issue-Shape-Id: com.amazonaws.artifact#Artifact
Issue-Key: artifact.get-report-metadata-invalid-id-pattern-rejected|run#4|AssertionError|assert code == "UnauthorizedException", f"expected UnauthorizedException, got {code}"
Issue-Key: artifact.get-report-metadata-not-found|run#4|AssertionError|assert code == "UnauthorizedException", f"expected UnauthorizedException, got {code}"
Issue-Key: artifact.create-compliance-inquiry-invalid-client-token-pattern-rejected|run#9|AssertionError|assert code == "BadRequestException", f"expected BadRequestException, got {code}"
Issue-Key: artifact.get-report-term-token-cross-resource-rejected|run#14|AssertionError|assert err["Code"] == "ValidationException", f"expected ValidationException for mismatched termToken, got {err['Code']}"
Issue-Key: artifact.get-report-download-roundtrip|run#11|HTTPError|with urllib.request.urlopen(urllib.request.Request(target_url, headers=route_headers), timeout=30) as resp:
Issue-Key: artifact.list-reports-max-results-out-of-range-rejected|run#8|AssertionError|assert message == expected, f"expected AWS's exact refusal message, got {message!r}"
Issue-Commit: 5b247f22dea7573e9fb6c4376c776b3a345a666f

# Artifact report and compliance-inquiry error paths and document download diverge from AWS

> _Note: this issue was identified against the code base at commit `5b247f22dea7573e9fb6c4376c776b3a345a666f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetReportMetadata

## Description
SIX causes in the artifact emulator (sdkId Artifact, restJson1, sigv4), all grounded in behavioral checks of suite simparity-behavioral-aws-v1. They are bundled because they share one service; each must be closed on its own. Causes 1, 2 and 3 are a single family (the emulator answers with its own modeled exception where AWS Artifact's front door refuses the call first), cause 4 is a wrong modeled error code, cause 5 is a dead data-plane URL, cause 6 is a request-derived message rendering.

Cause 1 of 6 - GetReportMetadata with a malformed reportId is refused with the wrong status and error code.
AWS: recorded behavior only - the model does declare the constraint (Smithy: GetReportMetadataRequest.reportId @required @httpQuery reportId @pattern ^report-[a-zA-Z0-9]{16}$), but AWS Artifact does NOT evaluate it ahead of resource authorization: a malformed reportId is refused with HTTP 401 and error code UnauthorizedException (recorded message example: 'Unauthorized'). No UnauthorizedException shape exists in the on-disk model (service common errors are AccessDeniedException 403, InternalServerException 500, ThrottlingException 429, ValidationException 400), so the 401/UnauthorizedException pair is recorded-only. Hypothesis: resource-level authorization of the report ARN happens at the gateway, so a caller never reaches the member-constraint check for a report it is not entitled to - including an unparseable one.
Emulator: HTTP 400 with error code ValidationException and a constraint-violation message (example: "1 validation error detected: Value 'not a valid report id!' at 'reportId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^report-[a-zA-Z0-9]{16}$").
Why: a client cannot distinguish an authorization refusal from a client-side input bug; it will treat a 401 (re-acquire credentials / escalate permissions) path as a 400 non-retryable input error, and SDK error-type dispatch on UnauthorizedException never matches.
Fix: the GetReportMetadata handler's ordering of authorization versus input validation, and its error-type mapping.
Traces: artifact.get-report-metadata-invalid-id-pattern-rejected.

Cause 2 of 6 - GetReportMetadata with a well-formed but unknown reportId answers not-found instead of unauthorized.
AWS: recorded behavior only - a well-formed reportId that does not exist is refused with HTTP 401, error code UnauthorizedException (recorded message example: 'Unauthorized'); AWS deliberately does not confirm the absence of a report the caller has no access to. ResourceNotFoundException is modeled at httpError 404 but is not what this path returns.
Emulator: HTTP 404 with error code ResourceNotFoundException.
Why: existence information is leaked where AWS hides it, and a client that branches on not-found versus unauthorized takes the wrong branch (silently skipping a report instead of reporting an entitlement problem).
Fix: the GetReportMetadata handler's error mapping - authorization must precede the state lookup. NOTE: the open WIRE issue 20261022T003730Z-artifact-error-responses-return-wrong-http-status-and.md describes the same GetReportMetadata behavior from fixture artifact-report-not-found-errors#task#1; it carries no behavioral replay key for this check, so this cause is recorded here and must be re-replayed against artifact.get-report-metadata-not-found.
Traces: artifact.get-report-metadata-not-found.

Cause 3 of 6 - CreateComplianceInquiry with a clientToken that violates the modeled pattern is refused with ValidationException instead of BadRequestException.
AWS: recorded behavior only for the error identity - the constraint is modeled (Smithy: CreateComplianceInquiryRequest.clientToken : IdempotentClientToken @idempotencyToken @length 1..128 @pattern ^[\w\-]+$) but a malformed request body on this operation is refused by the gateway with HTTP 400 and error code BadRequestException (recorded message example: 'Invalid request body'); no BadRequestException shape is modeled. The rejected call creates nothing, and a retry with a valid clientToken and the same inquiry content succeeds (POST /v1/compliance-inquiry/create, 202).
Emulator: HTTP 400 with error code ValidationException and a pattern-constraint message.
Why: the status matches but the error code does not, so a client catching BadRequestException never matches and the failure surfaces as an unmodeled/unknown error class. This is the same class of fault as cause 5 of the open WIRE issue 20261022T003730Z-artifact-error-responses-return-wrong-http-status-and.md (which covers supportMode, not clientToken, and carries no behavioral replay key).
Fix: the CreateComplianceInquiry handler's envelope validation and error-type mapping.
Traces: artifact.create-compliance-inquiry-invalid-client-token-pattern-rejected.

Cause 4 of 6 - GetReport with a termToken minted for a report governed by a different term answers ConflictException instead of ValidationException.
AWS: Smithy: GetReportRequest.reportId @required @pattern ^report-[a-zA-Z0-9]{16}$; GetReportRequest.termToken @required @httpQuery termToken @length 1..256 @pattern ^[a-zA-Z0-9_\-\s]*$, documented as the 'Unique download token provided by GetTermForReport API'. Recorded behavior: the token is bound to the TERM, not to the report - a token minted for any report sharing the same term document is accepted (200), and a cross-term pairing is refused with HTTP 400 ValidationException (recorded message example: 'The provided token is invalid.'). ValidationException is modeled on GetReport at httpError 400; ConflictException is modeled at httpError 409.
Emulator: refuses the cross-term pairing with error code ConflictException (HTTP 409).
Why: both the status class and the error code differ, so a client that retries on ValidationException by re-minting a term token instead sees a 409 conflict, which SDK and application error handling treat as a concurrent-modification/state condition rather than a bad input. The open issue 20261022T003730Z-artifact-error-message-bodies-do-not-follow-the.md records this path as a MESSAGE-only divergence with a matching error type; that is no longer what the emulator does, so the wrong error code is filed here.
Fix: the GetReport handler's term-token validation error construction and its error-type selection.
Traces: artifact.get-report-term-token-cross-resource-rejected.

Cause 5 of 6 - the documentPresignedUrl returned by GetReport does not serve a document.
AWS: Smithy: GetReportResponse.documentPresignedUrl : String @length 1..10240, and GetTermForReportResponse.{termToken, documentPresignedUrl}; the download flow is ListReports -> GetTermForReport -> GetReport -> HTTP GET of the presigned URL. Recorded behavior: that URL is fetchable and answers HTTP 200 with Content-Type application/pdf and a body beginning with the %PDF- magic bytes - the document actually exists.
Emulator: GetTermForReport and GetReport return a documentPresignedUrl, but fetching it over HTTP answers HTTP 404 Not Found (urllib.error.HTTPError: HTTP Error 404: Not Found), so no document bytes are ever served. Note the host/port/signature of the URL are environment-specific and are not the requirement; the requirement is that the announced URL resolves to a 200 application/pdf document.
Why: the entire report-download capability is unusable against the emulator - any client or compliance pipeline that fetches an Artifact report gets a dead link after a 200 control-plane response, which is worse than an error because it looks like success.
Fix: the data-plane document endpoint behind the presigned URL (the handler/route that serves report and term documents) and the URL the GetReport/GetTermForReport serializers mint for it, plus the seeded document bytes in report state.
Traces: artifact.get-report-download-roundtrip.

Cause 6 of 6 - the ValidationException message for an out-of-range maxResults echoes the rejected value, which AWS does not.
AWS: Smithy: ListReportsRequest.maxResults : MaxResultsAttribute @httpQuery maxResults @range min 1 max 300; a violation is refused with HTTP 400 ValidationException whose message is BUILT FROM THE REQUEST - the rule is therefore known and part of the contract. The recorded rule for a numeric range violation names the member and the bound but does NOT echo the submitted value: "1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 300". maxResults=300 (the upper boundary) succeeds with 200.
Emulator: correct status (400) and correct error code (ValidationException), but the range-constraint branch of the message renderer injects the value: "1 validation error detected: Value '301' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 300".
Why: callers and compliance tooling that match AWS's request-derived validation text get a rendering AWS never emits for range constraints.
Fix: the shared ValidationException message renderer used by the artifact input-validation layer (range-constraint branch). NOTE: the open WIRE issue 20261022T003730Z-artifact-error-message-bodies-do-not-follow-the.md carries this same cause for fixture artifact-list-pagination#task#3 but no behavioral replay key; this cause must be re-replayed against artifact.list-reports-max-results-out-of-range-rejected.
Traces: artifact.list-reports-max-results-out-of-range-rejected.

## Evidence
Suite simparity-behavioral-aws-v1, service artifact: 4/22 checks passed (18%), 18 failed. Twelve of those failures are blocked by unimplemented scaffold stubs (GetAccountSettings, CreateComplianceInquiry, ListCustomerAgreements) and are already filed; the six below are the remaining ones.

Cause 1: [mismatch] artifact.get-report-metadata-invalid-id-pattern-rejected - GetReportMetadata(reportId='not a valid report id!'); AssertionError: expected UnauthorizedException, got ValidationException. Expected HTTP 401 / UnauthorizedException (message example 'Unauthorized'); observed HTTP 400 / ValidationException.

Cause 2: [mismatch] artifact.get-report-metadata-not-found - GetReportMetadata(reportId='report-<16 random alnum>'); AssertionError: expected UnauthorizedException, got ResourceNotFoundException. Expected HTTP 401 / UnauthorizedException; observed HTTP 404 / ResourceNotFoundException.

Cause 3: [mismatch] artifact.create-compliance-inquiry-invalid-client-token-pattern-rejected - CreateComplianceInquiry(name=..., inquiryContent={'query': ...}, supportMode='AI_ONLY', clientToken='not a valid token!'); AssertionError: expected BadRequestException, got ValidationException. Expected HTTP 400 / BadRequestException (message example 'Invalid request body'); observed HTTP 400 / ValidationException. The check reached this assertion, i.e. validation ran before the CreateComplianceInquiry stub.

Cause 4: [mismatch] artifact.get-report-term-token-cross-resource-rejected - GetReport(reportId=A, termToken=token minted for report B with a different term document); AssertionError: expected ValidationException for mismatched termToken, got ConflictException. Expected HTTP 400 / ValidationException (message example 'The provided token is invalid.'); observed ConflictException (modeled httpError 409). The correctly paired (A, tokenA) call did succeed and returned a documentPresignedUrl, so the rejection is about the mismatch.

Cause 5: [error] artifact.get-report-download-roundtrip - ListReports -> GetTermForReport -> GetReport all answered, GetReport returned a documentPresignedUrl, and the HTTP fetch of that URL raised urllib.error.HTTPError: HTTP Error 404: Not Found (expected HTTP 200, Content-Type 'application/pdf', body starting with b'%PDF-').

Cause 6: [mismatch] artifact.list-reports-max-results-out-of-range-rejected - ListReports(maxResults=301); code ValidationException matched, message did not: expected "1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 300", got "1 validation error detected: Value '301' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 300". ListReports(maxResults=300) succeeded in the same check.

Model citations used: service Artifact (restJson1, sigv4), common errors AccessDeniedException(403)/InternalServerException(500)/ThrottlingException(429)/ValidationException(400); GetReportMetadataRequest.reportId @required @httpQuery @pattern ^report-[a-zA-Z0-9]{16}$, ResourceNotFoundException httpError 404; CreateComplianceInquiryRequest.clientToken @idempotencyToken @length 1..128 @pattern ^[\w\-]+$, operation http code 202; GetReportRequest.termToken @required @length 1..256 @pattern ^[a-zA-Z0-9_\-\s]*$, GetReport errors include ValidationException(400) and ConflictException(409); GetReportResponse.documentPresignedUrl @length 1..10240; ListReportsRequest.maxResults @range 1..300. No UnauthorizedException or BadRequestException shape is modeled, so those identities are recorded-only.

## Steps to Reproduce
1. Clone this checkout and run the behavioral suite simparity-behavioral-aws-v1 against the booted emulator (service artifact).
2. Cause 1: replay artifact.get-report-metadata-invalid-id-pattern-rejected - call GetReportMetadata with reportId='not a valid report id!' and compare the HTTP status and error code against 401 / UnauthorizedException.
3. Cause 2: replay artifact.get-report-metadata-not-found - call GetReportMetadata with a well-formed but unknown reportId ('report-' + 16 random alphanumerics); expect 401 / UnauthorizedException, observe 404 / ResourceNotFoundException. The positive control GetReportMetadata on a real id from ListReports must still return reportDetails.id unchanged.
4. Cause 3: replay artifact.create-compliance-inquiry-invalid-client-token-pattern-rejected - POST /v1/compliance-inquiry/create with clientToken='not a valid token!'; expect 400 / BadRequestException, observe 400 / ValidationException. Then retry the same inquiry content with a valid clientToken and confirm it is created and listed.
5. Cause 4: replay artifact.get-report-term-token-cross-resource-rejected - pick two EXPLICIT/PUBLISHED reports with different term documents from ListReports, mint a termToken for each with GetTermForReport, confirm GetReport(A, tokenA) returns 200 with a documentPresignedUrl, then call GetReport(A, tokenB); expect 400 / ValidationException, observe ConflictException.
6. Cause 5: replay artifact.get-report-download-roundtrip - find a PASSTHROUGH/PUBLISHED report whose uploadState is absent or COMPLETE, call GetTermForReport then GetReport, and HTTP GET the returned documentPresignedUrl; expect 200 with Content-Type application/pdf and a %PDF- body, observe HTTP 404.
7. Cause 6: replay artifact.list-reports-max-results-out-of-range-rejected - call ListReports(maxResults=300) (must succeed) then ListReports(maxResults=301) and compare the 400 ValidationException message with the recorded rendering that omits the rejected value.

## Resolution
All six causes are closed in `services/service-com-amazonaws-artifact-artifact/`:

1 and 2. The front door (`src/service.rs`'s `front_door`, reached through `Service::override_operation`) authorizes the resource a request targets ahead of the modelled member constraints and ahead of the state lookup, and every report and inquiry handler answers the same 401 `UnauthorizedException` for an identifier naming no resource the account holds. The modelled `ResourceNotFoundException` is kept for a VERSION a reachable report never published.
3. The same front door refuses a request BODY that does not bind to the modelled shape with 400 `BadRequestException`, deciding body-versus-URI from the member's own HTTP binding traits; a URI-bound value stays the service's modelled 400 `ValidationException`.
4. `GetReport` refuses a term token that does not cover the requested report's TERM with the modelled 400 `ValidationException`, through the single `grant_covers` predicate on this service's state that the document plane shares.
5. Report and term documents are persisted beside the catalog record (`state::Document`, the report document a one-page PDF written with the `pdf-writer` crate) and served from `state::DOCUMENT_PATH_PREFIX` under the endpoint the caller reached, so the `documentPresignedUrl` both operations announce resolves to a 200 `application/pdf` document.
6. `state::constraint_violation_message` renders a constraint violation as the AWS validation envelope naming the member and the requirement WITHOUT echoing the rejected value, for every constraint class this service refuses.

Covered by `tests/all/front_door_errors.rs`, `tests/all/document_data_plane.rs`,
`tests/all/validation_message_rendering.rs`, `tests/all/front_door.rs` and
`tests/all/report_workflow.rs`.
