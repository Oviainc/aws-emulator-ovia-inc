Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: textract.analyze-expense-extracts-summary-fields textract.detect-document-text-from-s3-object-source textract.start-document-text-detection-job-lifecycle textract.start-document-analysis-async-tables-feature textract.start-document-text-detection-invalid-s3-object-rejected textract.create-adapter-idempotent-token-mismatch-rejected textract.textract-access-denied-without-permission-rejected
Issue-Shape-Id: com.amazonaws.textract#Textract
Issue-Key: textract|AnalyzeExpense|UnsupportedDocumentException
Issue-Key: textract|DetectDocumentText|InvalidS3ObjectException
Issue-Key: textract.start-document-text-detection-job-lifecycle|run#9|AssertionError|assert final["JobStatus"] == "SUCCEEDED", f"unexpected terminal status: {final['JobStatus']!r}"
Issue-Key: textract.start-document-analysis-async-tables-feature|run#9|AssertionError|assert final["JobStatus"] == "SUCCEEDED", f"unexpected terminal status: {final['JobStatus']!r}"
Issue-Key: textract.start-document-text-detection-invalid-s3-object-rejected|_client_error#7|AssertionError|raise AssertionError(f"real AWS rejects this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: textract.create-adapter-idempotent-token-mismatch-rejected|run#9|AssertionError|assert err["Code"] == "ValidationException", (
Issue-Key: textract.textract-access-denied-without-permission-rejected|run#14|AssertionError|assert err["Code"] == "AccessDeniedException", (
Issue-Commit: 17f16131651e3a76635c083c093a8581359ec045

# Textract: AnalyzeExpense and S3-sourced documents rejected, async jobs always FAIL, StartDocumentTextDetection accepts a missing S3 key, CreateAdapter token mismatch and IAM denial use wrong codes

> _Note: this issue was identified against the code base at commit `17f16131651e3a76635c083c093a8581359ec045`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DetectDocumentText

## Description
The emulator's textract service is bootstrapped and answers requests, but several operation families diverge from AWS. This issue bundles 7 distinct root causes, all in the textract crate. A fixer must close ALL SEVEN before submitting.

Cause 1 of 7 - AnalyzeExpense rejects a supported raster document instead of returning ExpenseDocuments.
AWS: Smithy (services/service-com-amazonaws-textract-textract/smithy-model/com-amazonaws-textract-textract.json): AnalyzeExpense input {Document required}, output {DocumentMetadata, ExpenseDocuments: ExpenseDocumentList}; UnsupportedDocumentException is a modeled client error (HTTP 400) reserved for document formats Textract does not support (PNG, JPEG, PDF, TIFF are supported). For a supported single-page PNG receipt AWS answers HTTP 200 with DocumentMetadata and at least one ExpenseDocument carrying ExpenseIndex, Blocks and non-empty SummaryFields whose text reflects the submitted document.
Emulator: HTTP 400, code UnsupportedDocumentException, for a well-formed PNG (message example: "the document's bytes are not text, and this emulator runs no OCR engine, so no text can be detected in it"). The rejection is unconditional on document content, so it is not a format decision.
Why: any invoice/receipt flow built on the emulator cannot run at all - the client gets a client error that blames its own input format where AWS returns a 200 result body.
Fix: the AnalyzeExpense handler and the shared document-decode / expense-document synthesis layer it calls (services/service-com-amazonaws-textract-textract/src/ops/analyze_expense.rs), reserving UnsupportedDocumentException for bytes that genuinely are not a supported format.
Traces: textract.analyze-expense-extracts-summary-fields.

Cause 2 of 7 - DetectDocumentText cannot read a Document.S3Object source at all.
AWS: Smithy: DetectDocumentText input Document (required); the Document structure accepts either Bytes or S3Object {Bucket, Name, Version}. With S3Object pointing at an object that exists and is readable, AWS answers HTTP 200 with DocumentMetadata, Blocks (PAGE -> LINE -> WORD with Text/Confidence/Geometry) and DetectDocumentTextModelVersion. InvalidS3ObjectException (modeled, client, HTTP 400) is for an object Textract cannot fetch (bad key, wrong region, missing access).
Emulator: HTTP 400, code InvalidS3ObjectException, for an object the harness just uploaded successfully via S3 PutObject (message example: "Amazon Textract is unable to access s3://<bucket>/<key>: this emulator's Amazon S3 cannot serve object bytes yet, so the document cannot be read"). The S3Object document source is therefore never usable, independently of the inline-Bytes path.
Why: half of Textract's document-input surface is dead. Clients that stage documents in S3 (the normal pattern for anything larger than the inline limit) get a modeled 'object unreachable' error for an object that demonstrably exists, which sends them debugging their bucket/key/permissions instead of the service.
Fix: the document-resolution path shared by the synchronous handlers - resolve Document.S3Object against the emulator's S3 object state and read the stored object bytes, then run the same decode/block-synthesis used for inline Bytes (services/service-com-amazonaws-textract-textract/src/ops/detect_document_text.rs plus the shared document-source module).
Traces: textract.detect-document-text-from-s3-object-source.

Cause 3 of 7 - the asynchronous text-detection job always terminates FAILED.
AWS: Smithy: StartDocumentTextDetection input {DocumentLocation required, ...} output {JobId}; GetDocumentTextDetection output {DocumentMetadata, JobStatus (JobStatus enum), NextToken, Blocks, Warnings, StatusMessage, DetectDocumentTextModelVersion}. For a readable S3 document the job walks IN_PROGRESS -> SUCCEEDED and the terminal GetDocumentTextDetection returns JobStatus SUCCEEDED with Blocks for the document (paginated by NextToken).
Emulator: the job reaches the terminal state FAILED for a PNG the harness uploaded to S3 and that S3 accepted. Hypothesis: the job executor fails because it cannot read the S3 object (Cause 2) and/or cannot decode the document, and records FAILED instead.
Why: the whole async document-text pipeline is unusable - a client polling to SUCCEEDED never gets there, and a FAILED terminal status is a permanent, non-retryable outcome in client logic.
Fix: the async job executor for text detection and the GetDocumentTextDetection handler's state machine (services/service-com-amazonaws-textract-textract/src/ops/start_document_text_detection.rs / get_document_text_detection.rs and the job-state module): on a readable document the job must reach SUCCEEDED and return Blocks.
Traces: textract.start-document-text-detection-job-lifecycle.

Cause 4 of 7 - the asynchronous document-analysis job always terminates FAILED.
AWS: Smithy: StartDocumentAnalysis input {DocumentLocation required, FeatureTypes required, ...} output {JobId}; GetDocumentAnalysis output {DocumentMetadata, JobStatus, NextToken, Blocks, Warnings, StatusMessage, AnalyzeDocumentModelVersion}. With FeatureTypes=[TABLES] on a readable S3 document AWS reaches JobStatus SUCCEEDED and returns TABLE blocks (with CELL children) plus a non-empty AnalyzeDocumentModelVersion.
Emulator: terminal JobStatus FAILED, so no Blocks and no AnalyzeDocumentModelVersion are ever produced for the analysis job path.
Why: same as Cause 3 for the analysis pipeline - the TABLES/FORMS feature set is unreachable asynchronously as well as synchronously.
Fix: the async job executor for document analysis and the GetDocumentAnalysis handler's state machine (services/service-com-amazonaws-textract-textract/src/ops/start_document_analysis.rs / get_document_analysis.rs), sharing the job-state fix of Cause 3.
Traces: textract.start-document-analysis-async-tables-feature.

Cause 5 of 7 - StartDocumentTextDetection accepts a DocumentLocation pointing at a nonexistent S3 key.
AWS: Smithy: InvalidS3ObjectException is a modeled client error of StartDocumentTextDetection at HTTP 400, for an S3 object whose metadata Textract cannot obtain. Recorded behavior: starting a job on a key that does not exist in the bucket is rejected at submit time with HTTP 400 / InvalidS3ObjectException and no job is created.
Emulator: HTTP 200 with a JobId (observed response {'JobId': '<64-hex>'}) for key 'sim2real-does-not-exist.png' in a bucket that exists - the input is never validated against S3 state.
Why: the failure is deferred from submit time to never. A client relying on the start call to validate its S3 input receives a job id for a document that cannot be read, and its InvalidS3ObjectException recovery branch is dead code.
Fix: StartDocumentTextDetection input validation - resolve DocumentLocation.S3Object against the emulator's object state before creating a job and raise InvalidS3ObjectException (HTTP 400) when the object cannot be read (services/service-com-amazonaws-textract-textract/src/ops/start_document_text_detection.rs validation path). Related open issue 20261022T105401Z-textract-async-start-operations-accept-an-unreadable-s3.md describes the same acceptance from the WIRE corpus and carries only wire fixture keys, so it does not gate this behavioral check; this cause records the behavioral binding.
Traces: textract.start-document-text-detection-invalid-s3-object-rejected.

Cause 6 of 7 - CreateAdapter idempotency-token reuse with different parameters is rejected with the wrong error code.
AWS: Smithy: CreateAdapter input {AdapterName required @length 1..128 @pattern ^[a-zA-Z0-9-_]+$, ClientRequestToken @idempotencyToken, FeatureTypes required, ...}; both ValidationException and IdempotentParameterMismatchException are modeled client errors at HTTP 400. Recorded behavior: reusing a ClientRequestToken with DIFFERENT CreateAdapter parameters is rejected with code ValidationException, HTTP 400 (recorded message example: "ClientRequestToken already sent with different parameters."); the status and code are the requirement, the sentence is an example.
Emulator: code IdempotentParameterMismatchException for the same call.
Why: a client catching ValidationException on CreateAdapter (the code AWS actually uses for this conflict) does not catch IdempotentParameterMismatchException, so the retry/repair branch is skipped and the error escapes as an unexpected ClientError.
Fix: the CreateAdapter idempotency-token comparison path - raise ValidationException (HTTP 400) when a stored token's recorded parameters differ from the new request (services/service-com-amazonaws-textract-textract/src/ops/create_adapter.rs). Note the separate already-filed issue for unimplemented adapter listing operations does not cover this error-code choice.
Traces: textract.create-adapter-idempotent-token-mismatch-rejected.

Cause 7 of 7 - textract performs no authorization check, so a principal with no textract permissions is not denied.
AWS: Smithy: AccessDeniedException is a modeled client error (HTTP 400) on DetectDocumentText and on every other textract operation. Recorded behavior: credentials for an IAM role with no textract permissions at all (implicit deny) calling DetectDocumentText are rejected with code AccessDeniedException; the same call with permitted credentials succeeds.
Emulator: the request from the unprivileged assumed-role session is not denied at all - it falls through to the document path and returns code UnsupportedDocumentException, i.e. the authorization decision never happens (the emulator did accept the STS session credentials and route the call).
Why: an emulator that never denies cannot be used to test IAM-failure handling: least-privilege regressions pass locally and only surface against real AWS, and the error a client does receive points at the document rather than at its credentials.
Fix: the request-authorization layer in front of the textract operation handlers - evaluate the caller's identity/policy before document handling and return AccessDeniedException for an implicit deny (services/service-com-amazonaws-textract-textract/src/ops/detect_document_text.rs entry path / the service's shared auth check).
Traces: textract.textract-access-denied-without-permission-rejected.

## Evidence
Suite simparity-behavioral-aws-v1 replayed against the emulator booted from this checkout: textract 2/24 passed, 22 failed. Checks already covered by open/auto-filed issues were skipped; the seven causes above are the remaining clusters.

Cause 1 - textract.analyze-expense-extracts-summary-fields. Expected HTTP 200 with ExpenseDocuments[0].SummaryFields non-empty and the rendered amount/label in block text. Actual: botocore.errorfactory.UnsupportedDocumentException: An error occurred (UnsupportedDocumentException) when calling the AnalyzeExpense operation: the document's bytes are not text, and this emulator runs no OCR engine, so no text can be detected in it. (Related open WIRE issue 20261022T105401Z-textract-synchronous-data-plane-rejects-supported-documents-with.md describes the same AnalyzeExpense 400 from the wire corpus and carries only wire keys; it does not gate this behavioral check.)

Cause 2 - textract.detect-document-text-from-s3-object-source. The harness creates a bucket, PutObject succeeds, then DetectDocumentText(Document={'S3Object': {...}}) is expected to return 200 with Blocks and DetectDocumentTextModelVersion. Actual: botocore.errorfactory.InvalidS3ObjectException: ... Amazon Textract is unable to access s3://sim2real-textract-<uniq>/sim2real-doc.png: this emulator's Amazon S3 cannot serve object bytes yet, so the document cannot be read.

Cause 3 - textract.start-document-text-detection-job-lifecycle. StartDocumentTextDetection returns a JobId; polling GetDocumentTextDetection to a terminal state is expected to yield JobStatus 'SUCCEEDED' with LINE blocks. Actual: AssertionError: unexpected terminal status: 'FAILED'.

Cause 4 - textract.start-document-analysis-async-tables-feature. StartDocumentAnalysis(FeatureTypes=['TABLES']) + GetDocumentAnalysis expected JobStatus 'SUCCEEDED' with a TABLE block and a non-empty AnalyzeDocumentModelVersion. Actual: AssertionError: unexpected terminal status: 'FAILED'.

Cause 5 - textract.start-document-text-detection-invalid-s3-object-rejected. Expected HTTP 400 / Error.Code 'InvalidS3ObjectException' with no job created. Actual: the call SUCCEEDED and returned {'JobId': '26db78268eb3564ea381e057cc6bc31ac9d1939f96b9acac72177b7b256ccbd4', ResponseMetadata.HTTPStatusCode: 200}.

Cause 6 - textract.create-adapter-idempotent-token-mismatch-rejected. Expected Error.Code 'ValidationException' (HTTP 400). Actual: AssertionError: expected ValidationException, got 'IdempotentParameterMismatchException'.

Cause 7 - textract.textract-access-denied-without-permission-rejected. A freshly created IAM role with no policies is assumed via STS and calls DetectDocumentText; expected Error.Code 'AccessDeniedException'. Actual: AssertionError: ... got 'UnsupportedDocumentException'.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the textract behavioral checks.
2. Cause 1: render a receipt PNG and call textract.AnalyzeExpense(Document={'Bytes': png}) - observe UnsupportedDocumentException/400 instead of 200 with DocumentMetadata and ExpenseDocuments[].SummaryFields.
3. Cause 2: s3.CreateBucket + s3.PutObject a PNG, then textract.DetectDocumentText(Document={'S3Object': {'Bucket': b, 'Name': k}}) - observe InvalidS3ObjectException/400 for an object that exists, instead of 200 with Blocks and DetectDocumentTextModelVersion.
4. Cause 3: textract.StartDocumentTextDetection(DocumentLocation={'S3Object': {...}}) on that object, then poll textract.GetDocumentTextDetection(JobId=...) to a terminal state - observe JobStatus 'FAILED' instead of 'SUCCEEDED' with Blocks.
5. Cause 4: textract.StartDocumentAnalysis(DocumentLocation={'S3Object': {...}}, FeatureTypes=['TABLES']) and poll textract.GetDocumentAnalysis - observe JobStatus 'FAILED' instead of 'SUCCEEDED' with a TABLE block and AnalyzeDocumentModelVersion.
6. Cause 5: textract.StartDocumentTextDetection with DocumentLocation.S3Object.Name set to a key that was never uploaded - observe HTTP 200 with a JobId instead of HTTP 400 / InvalidS3ObjectException and no job.
7. Cause 6: textract.CreateAdapter(AdapterName=n, FeatureTypes=['QUERIES'], ClientRequestToken=t), then CreateAdapter again with the same token but AdapterName=n+'-different' - observe Error.Code 'IdempotentParameterMismatchException' instead of 'ValidationException' (HTTP 400).
8. Cause 7: iam.CreateRole with a trust policy for the account root and no permission policies, sts.AssumeRole into it, and call textract.DetectDocumentText with those credentials - observe a non-authorization error instead of AccessDeniedException.
