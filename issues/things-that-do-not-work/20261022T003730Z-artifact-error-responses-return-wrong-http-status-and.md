Issue-Source: trace-sweep-divergence
Issue-Trace: artifact-report-not-found-errors
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: artifact-report-not-found-errors artifact-compliance-inquiry-error-paths
Issue-Shape-Id: com.amazonaws.artifact#Artifact
Issue-Key: artifact|GetReportMetadata|4xx->4xx|$.message
Issue-Key: artifact|GetReportMetadata|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: artifact|GetReportMetadata|4xx->4xx|status
Issue-Key: artifact|ListReportVersions|4xx->4xx|$.message
Issue-Key: artifact|ListReportVersions|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: artifact|ListReportVersions|4xx->4xx|status
Issue-Key: artifact|GetTermForReport|4xx->4xx|$.message
Issue-Key: artifact|GetTermForReport|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: artifact|GetTermForReport|4xx->4xx|status
Issue-Key: artifact|GetComplianceInquiryMetadata|4xx->4xx|$.message
Issue-Key: artifact|GetComplianceInquiryMetadata|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: artifact|GetComplianceInquiryMetadata|4xx->4xx|status
Issue-Key: artifact|CreateComplianceInquiry|4xx->4xx|$.message
Issue-Key: artifact|CreateComplianceInquiry|4xx->4xx|$headers.x-amzn-errortype
Issue-Commit: 7d2cd57c0bb94f32b894957a728316a26e29b449

# Artifact error responses return wrong HTTP status and error type for unknown and malformed identifiers

> _Note: this issue was identified against the code base at commit `7d2cd57c0bb94f32b894957a728316a26e29b449`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetReportMetadata

## Description
Five causes, one fault: the artifact emulator answers a request the AWS front door rejects before dispatch with its own modeled exception, so both the HTTP status and the x-amzn-errortype are wrong. AWS Artifact is fronted by a gateway that authorizes the targeted resource and validates the request envelope first and answers 401 UnauthorizedException / 400 BadRequestException - neither of which is in the on-disk model (Smithy: service Artifact common errors are AccessDeniedException(403), InternalServerException, ThrottlingException, ValidationException(400); no UnauthorizedException or BadRequestException shape exists), so for all five causes AWS's behavior is recorded behavior only. The status and error code below are taken from the recorded responses; the recorded messages are quoted as examples only.

Cause 1 of 5 - GetReportMetadata with a report id that does not exist.
AWS: recorded behavior only - GET /v1/report/getMetadata?reportId=report-0000000000000000 answers HTTP 401 with x-amzn-errortype: UnauthorizedException (recorded message example: 'Unauthorized'). Hypothesis: resource-level authorization of the report ARN happens at the front door, so a caller that is not entitled to the named resource - including one that does not exist - never reaches the service's not-found path.
Emulator: HTTP 404 with x-amzn-errortype: ResourceNotFoundException (recorded message example: 'Report report-0000000000000000 not found').
Why: a client that distinguishes an authorization failure from a missing resource takes the wrong branch - it will retry after re-acquiring credentials where AWS tells it it is unauthorized, or treat an unauthorized report as simply absent and silently skip it. The status class also differs (4xx 401 vs 404), which changes SDK retry/error-shape handling.
Fix: the report read path's error mapping - the authorization check must precede the state lookup in the GetReportMetadata handler.
Traces: artifact-report-not-found-errors#task#1.

Cause 2 of 5 - ListReportVersions with a report id that does not exist.
AWS: recorded behavior only - GET /v1/report/listVersions for report-0000000000000000 answers 401, x-amzn-errortype: UnauthorizedException.
Emulator: 404, x-amzn-errortype: ResourceNotFoundException.
Why: same as cause 1, on the version-listing path.
Fix: same error mapping, in the ListReportVersions handler.
Traces: artifact-report-not-found-errors#task#2.

Cause 3 of 5 - GetTermForReport with a report id that does not exist.
AWS: recorded behavior only - GET /v1/report/getTermForReport for report-0000000000000000 answers 401, x-amzn-errortype: UnauthorizedException.
Emulator: 404, x-amzn-errortype: ResourceNotFoundException.
Why: same as cause 1, on the term-acceptance path that gates downloads.
Fix: same error mapping, in the GetTermForReport handler.
Traces: artifact-report-not-found-errors#task#3.

Cause 4 of 5 - GetComplianceInquiryMetadata with a malformed compliance inquiry id.
AWS: recorded behavior only - GET /v1/compliance-inquiry/getMetadata?complianceInquiryId=not-a-valid-id answers HTTP 401 with x-amzn-errortype: UnauthorizedException (message example 'Unauthorized'), i.e. the front door refuses the call before the modeled member constraint is ever evaluated. For reference the modeled constraint is Smithy: GetComplianceInquiryMetadataRequest.complianceInquiryId @required @httpQuery complianceInquiryId @pattern ^compliance-inquiry-[a-zA-Z0-9]{16}$.
Emulator: HTTP 400 with x-amzn-errortype: ValidationException and a constraint-violation message.
Why: status and error code both differ, so a client cannot tell an authorization refusal from a client-side input bug; retry and credential-refresh logic diverges.
Fix: the GetComplianceInquiryMetadata handler's ordering of authorization versus input validation.
Traces: artifact-compliance-inquiry-error-paths#task#1.

Cause 5 of 5 - CreateComplianceInquiry with an out-of-set supportMode.
AWS: recorded behavior only - POST /v1/compliance-inquiry/create with supportMode 'BOGUS_MODE' answers HTTP 400 with x-amzn-errortype: BadRequestException (message example 'Invalid request body'), i.e. gateway request-body validation, not the service's modeled ValidationException. Modeled for reference: Smithy: CreateComplianceInquiryRequest.supportMode : InquirySupportMode (enum AI_ONLY | FULL_SUPPORT).
Emulator: HTTP 400 with x-amzn-errortype: ValidationException and an enum-constraint message.
Why: the status matches but the error code does not, so SDK error-type dispatch (catch BadRequestException) never matches and the failure surfaces as an unmodeled/unknown error class.
Fix: the CreateComplianceInquiry handler's envelope validation and its error-type mapping.
Traces: artifact-compliance-inquiry-error-paths#task#3.

## Evidence
Cause 1: fixture artifact-report-not-found-errors step task#1, GetReportMetadata - status expected 401, got 404; $headers.x-amzn-errortype expected 'UnauthorizedException', got 'ResourceNotFoundException'; $.message expected 'Unauthorized' (example), got 'Report report-0000000000000000 not found'.
Cause 2: fixture artifact-report-not-found-errors step task#2, ListReportVersions - status expected 401, got 404; errortype 'UnauthorizedException' vs 'ResourceNotFoundException'.
Cause 3: fixture artifact-report-not-found-errors step task#3, GetTermForReport - status expected 401, got 404; errortype 'UnauthorizedException' vs 'ResourceNotFoundException'.
Cause 4: fixture artifact-compliance-inquiry-error-paths step task#1, GetComplianceInquiryMetadata - status expected 401, got 400; errortype 'UnauthorizedException' vs 'ValidationException'; actual message example "1 validation error detected: Value 'not-a-valid-id' at 'complianceInquiryId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^compliance-inquiry-[a-zA-Z0-9]{16}$".
Cause 5: fixture artifact-compliance-inquiry-error-paths step task#3, CreateComplianceInquiry - status 400 both; errortype 'BadRequestException' vs 'ValidationException'; actual message example "1 validation error detected: Value 'BOGUS_MODE' at 'supportMode' failed to satisfy constraint: Member must satisfy enum value set: [AI_ONLY, FULL_SUPPORT]".
Model citations: service Artifact (restJson1, sigv4) declares common errors AccessDeniedException(403), InternalServerException, ThrottlingException, ValidationException(400); ResourceNotFoundException is httpError 404; no UnauthorizedException or BadRequestException shape is modeled, so the recorded 401/400 gateway behavior is recorded-only.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Causes 1-3: replay fixture artifact-report-not-found-errors steps task#1, task#2 and task#3 (GET /v1/report/getMetadata, /v1/report/listVersions, /v1/report/getTermForReport with reportId=report-0000000000000000) and compare HTTP status and the x-amzn-errortype header against the recorded 401 / UnauthorizedException.
3. Cause 4: replay artifact-compliance-inquiry-error-paths step task#1 (GET /v1/compliance-inquiry/getMetadata?complianceInquiryId=not-a-valid-id); expect 401 UnauthorizedException, observe 400 ValidationException.
4. Cause 5: replay artifact-compliance-inquiry-error-paths step task#3 (POST /v1/compliance-inquiry/create with supportMode='BOGUS_MODE'); expect 400 BadRequestException, observe 400 ValidationException.
