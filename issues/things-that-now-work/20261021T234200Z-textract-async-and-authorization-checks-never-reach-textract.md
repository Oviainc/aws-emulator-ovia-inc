Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: textract.start-document-text-detection-job-lifecycle textract.start-document-analysis-async-tables-feature textract.detect-document-text-from-s3-object-source textract.start-document-text-detection-invalid-s3-object-rejected textract.textract-access-denied-without-permission-rejected
Issue-Shape-Id: com.amazonaws.textract#Textract
Issue-Key: textract|CreateBucket|500
Issue-Key: textract.textract-access-denied-without-permission-rejected|run#6|ResponseParserError|iam.create_role(
Issue-Commit: 5a6a25397292b69464374d16e097ea3d0febc1b1

# Textract async and authorization checks never reach textract: S3 CreateBucket answers HTTP 500 and the router reports two services claiming 'iam'

> _Note: this issue was identified against the code base at commit `5a6a25397292b69464374d16e097ea3d0febc1b1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartDocumentTextDetection

## Description
Five textract behavioral checks fail before any textract call is issued, because two emulator-side dependencies they must set up first fail. The failures are the emulator's own responses (a 500 body and an AmbiguousService body), not harness faults. This issue bundles 2 distinct root causes. A fixer must close BOTH before submitting. Note: the fixes live outside the textract crate (the S3 data plane and the request router); the issue is filed under textract because that is the service token of every grounding check.

Cause 1 of 2 - s3.CreateBucket answers HTTP 500 Internal Server Error, so no document can be staged in S3 for textract's async operations.
AWS: recorded behavior only for the staging step: CreateBucket on a fresh, unused bucket name succeeds (HTTP 200) and the object can then be referenced as Document.S3Object / DocumentLocation.S3Object. Smithy (textract): StartDocumentTextDetection/StartDocumentAnalysis take DocumentLocation with an S3Object and return JobId; GetDocumentTextDetection returns JobStatus (IN_PROGRESS then SUCCEEDED) with Blocks, and models InvalidS3ObjectException (client, HTTP 400) for a key that does not exist. None of that is reachable today.
Emulator: CreateBucket returns HTTP 500 "Internal Server Error" on every attempt (the SDK exhausts 3 retries). A 500 is a server-fault class, so SDKs retry and then surface an unrecoverable error.
Why: the whole asynchronous Textract surface (Start*/Get* job lifecycle, S3-sourced documents, and the InvalidS3ObjectException rejection path) cannot be exercised or used by any client, and the 500 teaches clients to retry a request that will never succeed. Related open issue: 'Bootstrap the Amazon S3 emulator' (20261021T221645Z-bootstrap-service-com-amazonaws-s3-amazons3.md) - it does not carry these replay keys, so this cause is tracked here; if S3 is stood up, re-replay the checks listed under Traces to confirm.
Fix: the S3 CreateBucket handler/data plane that returns the 500 (the s3 service crate's create_bucket operation and the bucket state it writes); textract's async handlers then need the S3-sourced document path and job lifecycle verified.
Traces: textract.start-document-text-detection-job-lifecycle, textract.start-document-analysis-async-tables-feature, textract.detect-document-text-from-s3-object-source, textract.start-document-text-detection-invalid-s3-object-rejected.

Cause 2 of 2 - the request router reports two emulated services claiming 'iam' and rejects the request with a JSON AmbiguousService body.
AWS: recorded behavior only for the setup: iam.CreateRole and sts.AssumeRole succeed, and the assumed role with no textract permissions is then denied on DetectDocumentText with code AccessDeniedException (HTTP 400; AccessDeniedException is modeled on DetectDocumentText), with the implicit-deny message built from the request principal and action (User: arn:aws:sts::<account>:assumed-role/<role>/<session> is not authorized to perform: textract:DetectDocumentText because no identity-based policy allows the textract:DetectDocumentText action), while permitted credentials succeed on the identical bytes.
Emulator: the iam.CreateRole call is answered with body {"__type":"AmbiguousService","message":"2 emulated services all claim requests for 'iam': iam, iam; ..."}. Two things are wrong: (a) a duplicate service registration for the 'iam' endpoint prefix makes the router refuse to route at all, and (b) the refusal is serialized as JSON to a client that speaks iam's query/XML protocol, so botocore raises ResponseParserError ('not well-formed (invalid token): line 1, column 0') instead of receiving any modeled error.
Why: every IAM-dependent flow is unreachable, and because the error body does not match the wire protocol the client cannot even report a code - it reports a parse failure. Any emulator-internal routing refusal must still be rendered in the target protocol with a modeled error code and status.
Fix: the service router/registry that resolves an endpoint prefix to a service (deduplicate the two registrations claiming 'iam'), and the error serializer used for router-level refusals so the body is encoded in the resolved service's protocol.
Traces: textract.textract-access-denied-without-permission-rejected.

## Evidence
Suite simparity-behavioral-aws-v1, textract 0/24 passed.

Cause 1 - expected: CreateBucket succeeds, then StartDocumentTextDetection returns a JobId and GetDocumentTextDetection reaches JobStatus=SUCCEEDED with Blocks (and InvalidS3ObjectException/400 for a missing key). Actual, identically in all four checks: botocore.exceptions.ClientError: An error occurred (500) when calling the CreateBucket operation (reached max retries: 3): Internal Server Error. No textract request was ever sent.

Cause 2 - expected: iam.CreateRole and sts.AssumeRole succeed, then DetectDocumentText from the unprivileged principal returns Error.Code == 'AccessDeniedException' with the implicit-deny message naming the assumed-role ARN and textract:DetectDocumentText, while the harness credentials succeed on the same bytes. Actual: botocore.parsers.ResponseParserError: Unable to parse response (not well-formed (invalid token): line 1, column 0), invalid XML received. Further retries may succeed: b'{"__type":"AmbiguousService","message":"2 emulated services all claim requests for \'iam\': iam, iam; the request is rejected rather than served by an arbitrary one"}'

## Steps to Reproduce
1. Build and boot the emulator from the checkout and replay the textract behavioral checks.
2. Cause 1: call s3.CreateBucket with a fresh unique bucket name against the emulator - observe HTTP 500 Internal Server Error after the SDK's retries. Re-replay textract.start-document-text-detection-job-lifecycle, textract.start-document-analysis-async-tables-feature, textract.detect-document-text-from-s3-object-source and textract.start-document-text-detection-invalid-s3-object-rejected, each of which stages the document in S3 before calling textract.
3. Cause 2: call iam.CreateRole against the emulator - observe the JSON AmbiguousService body returned to a query/XML-protocol client, which botocore cannot parse. Re-replay textract.textract-access-denied-without-permission-rejected, which needs CreateRole + AssumeRole before asserting AccessDeniedException on DetectDocumentText.
