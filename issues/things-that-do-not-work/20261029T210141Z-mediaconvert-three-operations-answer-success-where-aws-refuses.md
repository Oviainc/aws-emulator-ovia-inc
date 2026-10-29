Issue-Source: trace-sweep-divergence
Issue-Trace: mediaconvert-account-metadata
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mediaconvert-account-metadata mediaconvert-certificate-association-errors mediaconvert-job-submission-workflow
Issue-Shape-Id: com.amazonaws.mediaconvert#MediaConvert
Issue-Key: mediaconvert|ListVersions|4xx->2xx|$.accountId
Issue-Key: mediaconvert|ListVersions|4xx->2xx|$.errorType
Issue-Key: mediaconvert|ListVersions|4xx->2xx|$.httpStatus
Issue-Key: mediaconvert|ListVersions|4xx->2xx|$.message
Issue-Key: mediaconvert|ListVersions|4xx->2xx|$.operation
Issue-Key: mediaconvert|ListVersions|4xx->2xx|$.requestId
Issue-Key: mediaconvert|ListVersions|4xx->2xx|$.settingsValidationErrorsJsonBlob
Issue-Key: mediaconvert|ListVersions|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: mediaconvert|ListVersions|4xx->2xx|status
Issue-Key: mediaconvert|AssociateCertificate|4xx->2xx|$
Issue-Key: mediaconvert|AssociateCertificate|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: mediaconvert|AssociateCertificate|4xx->2xx|status
Issue-Key: mediaconvert|CreateResourceShare|4xx->2xx|$
Issue-Key: mediaconvert|CreateResourceShare|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: mediaconvert|CreateResourceShare|4xx->2xx|status
Issue-Commit: fa14c0f225749a1f587b9baa3463a5a1c30960f9

# MediaConvert: three operations answer success where AWS refuses the request

> _Note: this issue was identified against the code base at commit `fa14c0f225749a1f587b9baa3463a5a1c30960f9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListVersions

## Description
Three MediaConvert doors share one defect: each one has NO refusal path for a condition AWS refuses, so the emulator answers its modeled success status (200/201/202) and an empty-or-populated success body where AWS answers a modeled error with the crate's standard error envelope (errorType, httpStatus, accountId, requestId, operation, message, settingsValidationErrorsJsonBlob plus the x-amzn-errortype header). The envelope itself is correct everywhere it is produced in this corpus — what is missing is the decision to produce it at all. THIS BUNDLE CARRIES 3 CAUSES; all three must be closed before submitting.

Cause 1 of 3: ListVersions does not refuse an account that is not enrolled in job-engine version selection. (fixture mediaconvert-account-metadata, step task#2)
AWS: Smithy: ListVersions, GET /2017-08-29/versions, success 200, modeled errors include ForbiddenException (httpError 403, documented "You don't have permissions for this action with the credentials you sent."). Recorded behavior for the account-metadata probe: HTTP 403, errorType/x-amzn-errortype "ForbiddenException", and the message "Your account does not have access to MediaConvert version selection. For more information, or to request access, contact AWS Support." That sentence is entirely FIXED text for this error class - it names no request value and no state value; the only per-environment parts of the response are accountId and requestId, which the envelope supplies.
Emulator: HTTP 200 with a ListVersionsResponse page built from the seeded job_engine_versions catalogue; no 403 is reachable from the handler at all (it can only answer BadRequestException for an unissued nextToken, or InternalServerErrorException).
Why: a client probes version selection by calling ListVersions and treats 200 as "this account may pin a job-engine version". Against the emulator every account looks enrolled, so the client goes on to send CreateJob with JobEngineVersion and builds workflows on a capability real AWS refuses at the first call - the feature-detection branch that exists precisely to avoid that is never taken.
Fix: the ListVersions handler plus the account-capability state it should read - the enrollment flag belongs in the service state and the refusal in the handler, ahead of the catalogue read.
Traces: mediaconvert-account-metadata#task#2.

Cause 2 of 3: AssociateCertificate associates an ACM certificate ARN that does not resolve to an existing resource. (fixture mediaconvert-certificate-association-errors, step task#1)
AWS: Smithy: AssociateCertificate, POST /2017-08-29/certificates, success 201 with an empty body, input member Arn (@required, jsonName "arn"), modeled errors include NotFoundException (httpError 404, documented "The resource you requested doesn't exist."). Recorded behavior when the ARN is well formed but names no certificate: HTTP 404, errorType/x-amzn-errortype "NotFoundException", message "The resource you specified in arn was not found." Fixed text: "The resource you specified in " ... " was not found."; the single variable part is the request member the lookup read, rendered in the model's wire spelling (`arn`).
Emulator: HTTP 201 with an empty body. The handler only checks that Arn carries characters and that it looks like an ACM certificate ARN, then inserts it into the account's certificate set; AssociateCertificateOutcome has no NotFound arm, so a syntactically valid ARN for a certificate that does not exist is always accepted.
Why: a client that associates a mistyped or deleted certificate ARN gets a success, caches the association and only discovers the certificate is absent when playback/delivery fails later; error-handling paths for a bad certificate reference are never exercised, and the emulator's certificate set accumulates entries that could not exist.
Fix: the AssociateCertificate handler and its state operation - resolve the ARN against the resolvable certificate set and answer the modeled NotFoundException with the crate's one not-found sentence instead of inserting unconditionally.
Traces: mediaconvert-certificate-association-errors#task#1.

Cause 3 of 3: CreateResourceShare accepts a supportCaseId that does not match the documented resource-id format. (fixture mediaconvert-job-submission-workflow, step task#8)
AWS: Smithy: CreateResourceShare, POST /2017-08-29/resourceShares, success 202 with an empty body, required members JobId (jsonName "jobId") and SupportCaseId (jsonName "supportCaseId", documented "AWS Support case identifier"), modeled errors include BadRequestException (httpError 400, documented "The service can't process your request because of a problem in the request."). Recorded behavior for a support-case identifier of the wrong form: HTTP 400, errorType "BadRequestException", message "You specified an invalid value for supportCaseId. The resource ID must match the format '^\\d{15}$'." Fixed text: "You specified an invalid value for " ... ". The resource ID must match the format '" ... "'."; the variable parts are the offending member's wire name (supportCaseId) and the pattern the service enforces for it - not the value from any one run.
Emulator: HTTP 202 with an empty body. The handler validates only that jobId and supportCaseId carry characters (crate::state::carries_no_characters) and then records the share attempt; no format test exists, so any non-blank string is accepted as a support case.
Why: a client that passes a support case in the wrong form (a case display id, a truncated number, a typo) is told the share succeeded and waits on a resource share AWS never created; the 400 branch that would surface the operator's mistake is unreachable.
Fix: CreateResourceShare input validation in the operation handler, after the existing blank-member refusal and before the store write, answering the modeled BadRequestException.
Traces: mediaconvert-job-submission-workflow#task#8.

No open mediaconvert issue covers any of these three: the existing open set concerns queue/job control plane idempotency, the Probe stub, job lifecycle state, UpdateQueue commitments, tagging/not-found sentences, ARN namespace parsing, job-template and jobs-query validation, pagination cursors and blank-member handling - none of them names ListVersions, AssociateCertificate or CreateResourceShare.

## Evidence
Replay of trace set simparity-wire-aws-v1 (9 fixtures, 62 steps, 58 passed, score 93.5%) reported exactly these three mismatch clusters, all service=mediaconvert, all with empty stub_path and empty owned_by:

1) mediaconvert-account-metadata#task#2, ListVersions - expected 403, got 200. Every error member is absent from the actual response because the emulator served a success body: $.errorType missing (expected 'ForbiddenException'), $.httpStatus missing (expected 403), $.operation missing (expected 'ListVersions'), $.message missing (expected 'Your account does not have access to MediaConvert version selection. For more information, or to request access, contact AWS Support.'), $.settingsValidationErrorsJsonBlob missing (expected ''), $headers.x-amzn-errortype missing (expected 'ForbiddenException'). (accountId/requestId also missing; those are per-environment envelope values, not requirements.)

2) mediaconvert-certificate-association-errors#task#1, AssociateCertificate - expected 404, got 201. $: empty_body - expected {errorType: 'NotFoundException', httpStatus: 404, operation: 'AssociateCertificate', message: 'The resource you specified in arn was not found.', settingsValidationErrorsJsonBlob: ''}, got {}. $headers.x-amzn-errortype missing (expected 'NotFoundException').

3) mediaconvert-job-submission-workflow#task#8, CreateResourceShare - expected 400, got 202. $: empty_body - expected {errorType: 'BadRequestException', httpStatus: 400, operation: 'CreateResourceShare', message: "You specified an invalid value for supportCaseId. The resource ID must match the format '^\\d{15}$'.", settingsValidationErrorsJsonBlob: ''}, got {}. $headers.x-amzn-errortype expected but absent.

Code confirming the missing refusals: services/service-com-amazonaws-mediaconvert-mediaconvert/src/ops/list_versions.rs:155-199 (only nextToken and state-unavailable refusals); .../src/ops/associate_certificate.rs:153-210 (blank + ARN-shape refusals, then unconditional insert; outcome enum has no NotFound arm); .../src/ops/create_resource_share.rs:138-218 (blank-member refusals only, no supportCaseId format test).

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it; replay trace set simparity-wire-aws-v1.
2. Cause 1: replay fixture mediaconvert-account-metadata, step task#2 - GET /2017-08-29/versions against an account that is not enrolled in job-engine version selection. Emulator answers 200 with a versions page; AWS answers 403 ForbiddenException with the version-selection message and x-amzn-errortype: ForbiddenException.
3. Cause 2: replay fixture mediaconvert-certificate-association-errors, step task#1 - POST /2017-08-29/certificates with a well-formed ACM certificate ARN that names no existing certificate. Emulator answers 201 with an empty body; AWS answers 404 NotFoundException, message 'The resource you specified in arn was not found.'
4. Cause 3: replay fixture mediaconvert-job-submission-workflow, step task#8 - POST /2017-08-29/resourceShares with a valid jobId and a supportCaseId that is non-blank but does not match '^\\d{15}$'. Emulator answers 202 with an empty body; AWS answers 400 BadRequestException naming supportCaseId and the required format.
5. After the fix, re-replay all three fixture steps named in Issue-Trace/Issue-Step and trace_ids; all three must answer the modeled error status, errorType, x-amzn-errortype header and message.
