Issue-Source: trace-sweep-divergence
Issue-Trace: textract-async-start-and-get-error-paths
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: textract-async-start-and-get-error-paths
Issue-Shape-Id: com.amazonaws.textract#Textract
Issue-Key: textract|StartDocumentTextDetection|4xx->2xx|$.Message
Issue-Key: textract|StartDocumentTextDetection|4xx->2xx|$.__type
Issue-Key: textract|StartDocumentTextDetection|4xx->2xx|status
Issue-Key: textract|StartDocumentAnalysis|4xx->2xx|$.Message
Issue-Key: textract|StartDocumentAnalysis|4xx->2xx|$.__type
Issue-Key: textract|StartDocumentAnalysis|4xx->2xx|status
Issue-Commit: dcb14db06d6eb8eb19e965c6ac371e7edd3d0be4

# Textract async Start operations accept an unreadable S3 document instead of InvalidS3ObjectException

> _Note: this issue was identified against the code base at commit `dcb14db06d6eb8eb19e965c6ac371e7edd3d0be4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartDocumentTextDetection

## Description
Both asynchronous job-start operations skip validation of the DocumentLocation they are handed: they return a job id with HTTP 200 for an S3 object whose metadata AWS cannot read, where AWS rejects the request up front. This issue bundles 2 distinct root causes. A fixer must close BOTH before submitting.

Cause 1 of 2 - StartDocumentTextDetection accepts an unreadable S3 object.
AWS: Smithy (services/service-com-amazonaws-textract-textract/smithy-model/com-amazonaws-textract-textract.json): StartDocumentTextDetection input {DocumentLocation required, ClientRequestToken, JobTag, NotificationChannel, OutputConfig, KMSKeyId}, output {JobId}; InvalidS3ObjectException is a modeled client error at HTTP 400 for an S3 object Textract cannot fetch (bad key, wrong region, or missing access). The recorded response for this step is HTTP 400 with __type 'InvalidS3ObjectException' (recorded message example: 'Unable to get object metadata from S3. Check object key, region and/or access permissions.' - the status and the error code are the requirement, the sentence is an example).
Emulator: HTTP 200, with no __type and no Message - the request is accepted and a job id is handed back for a document that cannot be read.
Why: the failure is deferred from submit time to never. A client that relies on the start call to validate its S3 input gets a job id it will poll forever, instead of a modeled InvalidS3ObjectException it can catch and correct; error-handling branches in async document pipelines are never exercised.
Fix: the StartDocumentTextDetection handler's input validation - resolve DocumentLocation.S3Object against the emulator's object state before creating a job, and raise InvalidS3ObjectException (HTTP 400) when the object's metadata cannot be obtained.
Traces: textract-async-start-and-get-error-paths#task#1.

Cause 2 of 2 - StartDocumentAnalysis accepts the same unreadable S3 object.
AWS: Smithy: StartDocumentAnalysis input {DocumentLocation required, FeatureTypes required, ClientRequestToken, JobTag, NotificationChannel, OutputConfig, KMSKeyId, QueriesConfig, AdaptersConfig}, output {JobId}; InvalidS3ObjectException modeled, client, HTTP 400. The recorded response is HTTP 400 with __type 'InvalidS3ObjectException' (same recorded message example).
Emulator: HTTP 200 with neither __type nor Message, i.e. a job id for an unreadable document.
Why: same as Cause 1, for the analysis job path - a request AWS refuses is accepted, so the client's recovery path is dead code and the job it created can never complete.
Fix: the StartDocumentAnalysis handler's input validation, sharing the same DocumentLocation/S3 resolution check as Cause 1, applied before the job is created.
Traces: textract-async-start-and-get-error-paths#task#3.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator booted from this checkout.

Cause 1 - fixture textract-async-start-and-get-error-paths step task#1, StartDocumentTextDetection: expected status 400, got 200. Detail: $.__type missing (expected 'InvalidS3ObjectException'), $.Message missing (recorded example 'Unable to get object metadata from S3. Check object key, region and/or access permissions.').

Cause 2 - fixture textract-async-start-and-get-error-paths step task#3, StartDocumentAnalysis: expected status 400, got 200. Detail: $.__type missing (expected 'InvalidS3ObjectException'), $.Message missing (same recorded example).

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. Replay fixture textract-async-start-and-get-error-paths.
3. Step task#1 calls StartDocumentTextDetection with a DocumentLocation whose S3 object metadata cannot be read: the emulator returns 200 with a JobId instead of 400 / InvalidS3ObjectException.
4. Step task#3 calls StartDocumentAnalysis with the same location: again 200 instead of 400 / InvalidS3ObjectException.
5. Both must return HTTP 400 with __type InvalidS3ObjectException for the fixture steps to pass.
