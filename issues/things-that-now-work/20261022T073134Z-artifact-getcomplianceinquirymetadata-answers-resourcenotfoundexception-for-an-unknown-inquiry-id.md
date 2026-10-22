Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: artifact.get-compliance-inquiry-metadata-not-found
Issue-Shape-Id: com.amazonaws.artifact#Artifact
Issue-Key: artifact.get-compliance-inquiry-metadata-not-found|run#9|AssertionError|assert code == "UnauthorizedException", f"expected UnauthorizedException, got {code}"
Issue-Commit: ceabbcc456fe2a783c33070041a4243561c3aa32

# GetComplianceInquiryMetadata answers ResourceNotFoundException for an unknown inquiry id where AWS Artifact refuses with UnauthorizedException

> _Note: this issue was identified against the code base at commit `ceabbcc456fe2a783c33070041a4243561c3aa32`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetComplianceInquiryMetadata

## Description
ONE cause in the artifact emulator (sdkId Artifact, restJson1, sigv4), grounded in a behavioral check of suite simparity-behavioral-aws-v1.

Cause 1 of 1 - GetComplianceInquiryMetadata with a well-formed but unknown/not-owned complianceInquiryId answers not-found instead of unauthorized.
AWS: Smithy (services/service-com-amazonaws-artifact-artifact/smithy-model/com-amazonaws-artifact-artifact.json): GetComplianceInquiryMetadata GET /v1/compliance-inquiry/getMetadata, input GetComplianceInquiryMetadataRequest.complianceInquiryId : InquiryId @required @httpQuery complianceInquiryId @pattern ^compliance-inquiry-[a-zA-Z0-9]{16}$; modeled errors AccessDeniedException(403), InternalServerException(500), ResourceNotFoundException(404), ThrottlingException(429), ValidationException(400). No UnauthorizedException shape is modeled, so the refusal identity is recorded behavior only: the recording shows that a complianceInquiryId which satisfies the pattern but is not a known/owned inquiry is refused with HTTP 401 and error code UnauthorizedException (recorded message example: 'Unauthorized'), i.e. AWS does not confirm the absence of an inquiry the caller has no access to. Hypothesis: resource-level authorization of the inquiry id happens ahead of the state lookup, so an id the account does not own is indistinguishable from one that does not exist. The same recorded family already applies to GetReportMetadata (see open issue 20261022T012757Z-artifact-report-and-compliance-inquiry-error-paths-and.md, cause 2), but that issue covers only GetReportMetadata and carries no replay key for this check or this operation.
Emulator: the inquiry-metadata path leaks existence - HTTP 404 with error code ResourceNotFoundException for an unknown id. The positive control in the same check passes (CreateComplianceInquiry then GetComplianceInquiryMetadata on the real id returns complianceInquiryDetail.id unchanged), so the divergence is specific to the unknown-id path and not a blanket denial.
Why: existence information about other accounts' compliance inquiries is disclosed where AWS deliberately hides it, and a client that branches on not-found versus unauthorized takes the wrong branch - it silently treats the inquiry as absent (and may re-create it) instead of surfacing an entitlement/credential problem. SDK error-type dispatch on UnauthorizedException never matches, and the retry class differs (401 re-auth vs 404 terminal).
Fix: the GetComplianceInquiryMetadata handler's ordering of resource authorization versus state lookup, and its error-type mapping (services/service-com-amazonaws-artifact-artifact/src/ops/get_compliance_inquiry_metadata.rs). The recorded message text 'Unauthorized' is an example; the requirement is the HTTP 401 status and the UnauthorizedException error code.
Traces: artifact.get-compliance-inquiry-metadata-not-found.

## Evidence
Suite simparity-behavioral-aws-v1, service artifact: 9/22 checks passed (41%), 13 failed. Twelve of those failures are either blocked by unimplemented scaffold stubs (GetAccountSettings, ListComplianceInquiries, ListComplianceInquiryQueries, ExportComplianceInquiry, TagResource, ListTagsForResource - already filed) or owned by 20261022T012757Z-artifact-report-and-compliance-inquiry-error-paths-and.md; the one below is the remaining net-new failure.

Cause 1: [mismatch] artifact.get-compliance-inquiry-metadata-not-found - after a successful positive control (CreateComplianceInquiry(name=..., inquiryContent={'query': ...}, supportMode='AI_ONLY') then GetComplianceInquiryMetadata on the returned id, which asserted detail['id'] == inquiry_id and passed), the check called GetComplianceInquiryMetadata with complianceInquiryId='compliance-inquiry-' + 16 random alphanumerics (pattern-valid, never issued).
  Expected: HTTP 401, error code UnauthorizedException (recorded message example 'Unauthorized').
  Observed: error code ResourceNotFoundException (modeled httpError 404).
  Assertion: check_artifact_get_compliance_inquiry_metadata_not_found.py line 41 - AssertionError: expected UnauthorizedException, got ResourceNotFoundException.
  The check's helper raises if the call unexpectedly SUCCEEDS, so the emulator did refuse - it refused with the wrong identity.

Model citations used: GetComplianceInquiryMetadata input complianceInquiryId @required @httpQuery @pattern ^compliance-inquiry-[a-zA-Z0-9]{16}$; errors AccessDeniedException(403), InternalServerException(500), ResourceNotFoundException(404), ThrottlingException(429), ValidationException(400); http GET /v1/compliance-inquiry/getMetadata code 200. No UnauthorizedException shape is modeled, so that identity is recorded-only.

## Steps to Reproduce
1. Clone this checkout and run the behavioral suite simparity-behavioral-aws-v1 against the booted emulator (service artifact).
2. Replay artifact.get-compliance-inquiry-metadata-not-found.
3. Positive control: call CreateComplianceInquiry(name=<unique>, inquiryContent={'query': 'What is included in the AWS shared responsibility model?'}, supportMode='AI_ONLY'), take complianceInquirySummary.id, and call GetComplianceInquiryMetadata(complianceInquiryId=<that id>); it must return complianceInquiryDetail.id equal to the created id (this already passes).
4. Call GetComplianceInquiryMetadata(complianceInquiryId='compliance-inquiry-' + 16 random alphanumerics), i.e. an id matching ^compliance-inquiry-[a-zA-Z0-9]{16}$ that was never issued.
5. Compare the HTTP status and error code against 401 / UnauthorizedException; observe 404 / ResourceNotFoundException. Treat the message as an example only.
