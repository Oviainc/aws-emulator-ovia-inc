Issue-Source: trace-sweep-divergence
Issue-Trace: textract-async-start-and-get-error-paths
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: textract-async-start-and-get-error-paths textract-sync-analyze-error-paths textract-adapter-and-resource-error-paths
Issue-Shape-Id: com.amazonaws.textract#Textract
Issue-Key: textract|StartDocumentTextDetection|4xx->2xx|$.Message
Issue-Key: textract|StartDocumentTextDetection|4xx->2xx|$.__type
Issue-Key: textract|StartDocumentTextDetection|4xx->2xx|status
Issue-Key: textract|StartDocumentAnalysis|4xx->2xx|$.Message
Issue-Key: textract|StartDocumentAnalysis|4xx->2xx|$.__type
Issue-Key: textract|StartDocumentAnalysis|4xx->2xx|status
Issue-Key: textract|DetectDocumentText|4xx->2xx|$.Message
Issue-Key: textract|DetectDocumentText|4xx->2xx|$.__type
Issue-Key: textract|DetectDocumentText|4xx->2xx|status
Issue-Key: textract|ListAdapters|4xx->2xx|$.Message
Issue-Key: textract|ListAdapters|4xx->2xx|$.__type
Issue-Key: textract|ListAdapters|4xx->2xx|status
Issue-Commit: 89d5e152ff27d848f663b8ae7517a0827a99e56e

# Textract accepts invalid requests with HTTP 200 instead of the modeled 400 client error

> _Note: this issue was identified against the code base at commit `89d5e152ff27d848f663b8ae7517a0827a99e56e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartDocumentTextDetection

## Description
Replaying the wire corpus against the emulator booted from this checkout shows four textract operations that answer HTTP 200 where AWS answers a modeled HTTP 400 client error. In every case the emulator never performs the input/state validation at all, so the client's error branch is dead code. This issue bundles 4 distinct root causes, all in the textract crate; a fixer must close ALL FOUR before submitting.

Cause 1 of 4 - StartDocumentTextDetection accepts a DocumentLocation whose S3 object cannot be read.
AWS: Smithy (services/service-com-amazonaws-textract-textract/smithy-model/com-amazonaws-textract-textract.json): StartDocumentTextDetection input {DocumentLocation required, ClientRequestToken, JobTag, NotificationChannel, OutputConfig, KMSKeyId}, output {JobId}; InvalidS3ObjectException is a modeled client error at HTTP 400 and is the error AWS returns when it cannot obtain the object's metadata. Recorded response: HTTP 400, __type 'InvalidS3ObjectException' (recorded message example: 'Unable to get object metadata from S3. Check object key, region and/or access permissions.'); no job is created. The status and the error code are the requirement, the sentence is an example.
Emulator: HTTP 200 with a JobId body and no __type - the DocumentLocation.S3Object is never resolved against S3 state before a job is created.
Why: a client that stages documents in S3 relies on the start call to reject an unreadable key at submit time. Instead it receives a job id for a document that can never be read, so its InvalidS3ObjectException recovery path never runs and the failure is deferred to a poll loop (or to nothing at all).
Fix: StartDocumentTextDetection's input-validation path in the operation handler - resolve DocumentLocation.S3Object against the emulator's S3 object state before creating job state and raise InvalidS3ObjectException (HTTP 400) when the object's metadata cannot be obtained.
Traces: textract-async-start-and-get-error-paths#task#1.

Cause 2 of 4 - StartDocumentAnalysis accepts the same unreadable DocumentLocation.
AWS: Smithy: StartDocumentAnalysis input {DocumentLocation required, FeatureTypes required, ClientRequestToken, JobTag, NotificationChannel, OutputConfig, KMSKeyId, QueriesConfig, AdaptersConfig}, output {JobId}; InvalidS3ObjectException is likewise a modeled client error at HTTP 400. Recorded response: HTTP 400 with __type 'InvalidS3ObjectException' (same recorded message example as Cause 1) and no job.
Emulator: HTTP 200 with a JobId and no __type, i.e. the analysis start path shares the missing S3 resolution with Cause 1 rather than having its own validation.
Why: identical to Cause 1 for the FORMS/TABLES/QUERIES analysis pipeline - the asynchronous analysis entry point cannot be used to test the unreadable-document failure mode.
Fix: StartDocumentAnalysis's input-validation path, sharing the DocumentLocation resolution helper fixed for Cause 1, so both start operations reject an unresolvable S3 object with InvalidS3ObjectException (HTTP 400).
Traces: textract-async-start-and-get-error-paths#task#3.

Cause 3 of 4 - DetectDocumentText accepts a document whose format Textract does not support.
AWS: Smithy: DetectDocumentText input {Document required}, output {DocumentMetadata, Blocks, DetectDocumentTextModelVersion}; UnsupportedDocumentException is a modeled client error at HTTP 400 for bytes that are not a format Textract can process. Recorded response: HTTP 400 with __type 'UnsupportedDocumentException' (recorded message example: 'Request has unsupported document format').
Emulator: HTTP 200 with a response body and no __type - the submitted bytes are never format-checked, so an unsupported payload is answered as if it had been read successfully.
Why: a client cannot distinguish a document the service can read from one it cannot. Code that catches UnsupportedDocumentException to fall back to a conversion step never triggers, and the caller instead consumes a success response built from a document that was never decoded.
Fix: DetectDocumentText's document-decode/validation layer in the operation handler - classify the submitted Document.Bytes and return UnsupportedDocumentException (HTTP 400) for a format the service does not support, before any Blocks are synthesized.
Traces: textract-sync-analyze-error-paths#task#1.

Cause 4 of 4 - ListAdapters accepts a creation-time window that is not a valid range.
AWS: Smithy: ListAdapters input {AfterCreationTime timestamp, BeforeCreationTime timestamp, MaxResults @range min 1, NextToken @length 1..1024 @pattern \S}, output {Adapters, NextToken}; ValidationException and InvalidParameterException are both modeled client errors at HTTP 400. Recorded response for an invalid creation-time window: HTTP 400 with __type 'ValidationException' (recorded message example: 'Request has invalid time range.').
Emulator: HTTP 200 with a normal list body and no __type - the AfterCreationTime/BeforeCreationTime pair is not validated as a range, so an impossible window silently returns results.
Why: a client paging adapters with a computed time window gets a success for a request AWS rejects, which masks the bad window and yields a result set that does not correspond to any real filter.
Fix: ListAdapters' input validation in the operation handler - validate the AfterCreationTime/BeforeCreationTime pair before filtering adapter state and raise ValidationException (HTTP 400) for an invalid range.
Traces: textract-adapter-and-resource-error-paths#task#5.

Related: the open behavioral issue 20261022T192423Z-textract-analyzeexpense-and-s3-sourced-documents-rejected-async.md describes the StartDocumentTextDetection acceptance (its Cause 5) from the behavioral suite and carries only behavioral check ids, so it does not gate these wire steps; the earlier wire issues on the textract data plane are in the resolved folder and cover none of these steps.

## Evidence
Corpus simparity-wire-aws-v1 replayed against the emulator booted from this checkout: 38 steps, 17 passed, 12 divergence clusters. All four causes below are 'expected 400, got 200' clusters with the response's __type and Message absent entirely.

Cause 1 - fixture textract-async-start-and-get-error-paths, step task#1, operation StartDocumentTextDetection. Expected HTTP 400, $.__type 'InvalidS3ObjectException', $.Message 'Unable to get object metadata from S3. Check object key, region and/or access permissions.' (example). Actual HTTP 200, $.__type missing, $.Message missing.

Cause 2 - fixture textract-async-start-and-get-error-paths, step task#3, operation StartDocumentAnalysis. Expected HTTP 400, $.__type 'InvalidS3ObjectException', same recorded message example. Actual HTTP 200, $.__type missing, $.Message missing.

Cause 3 - fixture textract-sync-analyze-error-paths, step task#1, operation DetectDocumentText. Expected HTTP 400, $.__type 'UnsupportedDocumentException', $.Message 'Request has unsupported document format' (example). Actual HTTP 200, $.__type missing, $.Message missing.

Cause 4 - fixture textract-adapter-and-resource-error-paths, step task#5, operation ListAdapters. Expected HTTP 400, $.__type 'ValidationException', $.Message 'Request has invalid time range.' (example). Actual HTTP 200, $.__type missing, $.Message missing.

state_reset_failures was 0 and bootstrap_signal false, so each fixture ran against a clean state and the emulator was serving the corpus.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the simparity-wire-aws-v1 wire corpus.
2. Cause 1: replay fixture textract-async-start-and-get-error-paths step task#1 - call textract.StartDocumentTextDetection with a DocumentLocation.S3Object whose metadata cannot be obtained; observe HTTP 200 with a JobId instead of HTTP 400 / InvalidS3ObjectException.
3. Cause 2: replay fixture textract-async-start-and-get-error-paths step task#3 - call textract.StartDocumentAnalysis with the same DocumentLocation and a FeatureTypes list; observe HTTP 200 with a JobId instead of HTTP 400 / InvalidS3ObjectException.
4. Cause 3: replay fixture textract-sync-analyze-error-paths step task#1 - call textract.DetectDocumentText with Document.Bytes in a format Textract does not support; observe HTTP 200 instead of HTTP 400 / UnsupportedDocumentException.
5. Cause 4: replay fixture textract-adapter-and-resource-error-paths step task#5 - call textract.ListAdapters with an AfterCreationTime/BeforeCreationTime pair that is not a valid range; observe HTTP 200 with an Adapters list instead of HTTP 400 / ValidationException.
6. Re-replay all four fixture steps after the fix; each must return the modeled HTTP 400 with the error code named above.
