Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: textract.detect-document-text-corrupt-image-rejected textract.analyze-document-queries-without-feature-flag-rejected textract.create-adapter-validation-rejected textract.get-document-text-detection-invalid-job-id-rejected
Issue-Shape-Id: com.amazonaws.textract#Textract
Issue-Key: textract.detect-document-text-corrupt-image-rejected|run#5|AssertionError|assert err["Code"] == "UnsupportedDocumentException", f"unexpected code: {err['Code']!r}"
Issue-Key: textract.analyze-document-queries-without-feature-flag-rejected|run#7|AssertionError|assert err.get("Message") == expected_message, (
Issue-Key: textract.create-adapter-validation-rejected|run#6|AssertionError|assert err.get("Message") == expected_message, (
Issue-Key: textract.get-document-text-detection-invalid-job-id-rejected|run#6|AssertionError|assert err.get("Message") == expected_message, (
Issue-Commit: 3b929b447a52a39dfc36e4a4cbd93099ed5ef8cf

# Textract refusals use the wrong error code and non-AWS error messages on DetectDocumentText, AnalyzeDocument, CreateAdapter and GetDocumentTextDetection

> _Note: this issue was identified against the code base at commit `3b929b447a52a39dfc36e4a4cbd93099ed5ef8cf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DetectDocumentText

## Description
Four textract refusal paths do refuse the request, but with a different error shape than AWS: one returns the wrong modeled exception entirely, and three return the emulator's own explanatory prose where AWS returns a fixed, short message. An HTTP status, an error code and an error message are all requirements, so each of these is a divergence a client sees. This issue bundles 4 distinct root causes, all in the textract crate; a fixer must close ALL FOUR before submitting.

Cause 1 of 4 — a truncated/corrupt PNG is refused as BadDocumentException instead of UnsupportedDocumentException.
AWS: Smithy (services/service-com-amazonaws-textract-textract/smithy-model/com-amazonaws-textract-textract.json): DetectDocumentText models both UnsupportedDocumentException ("The format of the input document isn't supported. Documents for operations can be in PNG, JPEG, PDF, or TIFF format.") and BadDocumentException ("Amazon Textract isn't able to read the document."), both client errors at HTTP 400. For a blob that carries a PNG signature but whose stream is truncated (no IDAT/IEND), AWS answers HTTP 400 with code UnsupportedDocumentException and message "Request has unsupported document format" — that message is wholly fixed text, with no part taken from the request.
Emulator: HTTP 400 with code 'BadDocumentException'. crate::state classifies an undecodable raster as Unreadable::Corrupt, and each handler's unreadable_error (e.g. services/service-com-amazonaws-textract-textract/src/ops/analyze_document.rs unreadable_error, and the same mapping on the DetectDocumentText path) maps Unreadable::Corrupt to BadDocumentException and only Unreadable::UnsupportedFormat to UnsupportedDocumentException.
Why: the error code is the branch key. A client that catches UnsupportedDocumentException to convert or re-encode the document never catches this, and the exception escapes as an unhandled BadDocumentException; conversely code written against the emulator handles a code AWS does not send for this input.
Fix: the undecodable-raster classification in crate::state (read_content / the Unreadable enum) and the Unreadable→error mapping in the DetectDocumentText and AnalyzeDocument/AnalyzeExpense handlers — bytes in a Textract-accepted container that the decoder cannot read must be answered UnsupportedDocumentException (HTTP 400) with message "Request has unsupported document format".
Traces: textract.detect-document-text-corrupt-image-rejected.

Cause 2 of 4 — the QueriesConfig/FeatureTypes consistency refusal carries the emulator's own sentence instead of AWS's message.
AWS: Smithy: AnalyzeDocument input {Document required, FeatureTypes required, QueriesConfig, ...}; QueriesConfig{Queries required @length min 1}; InvalidParameterException is the modeled client error at HTTP 400 for an input parameter that violated a constraint. Supplying QueriesConfig while FeatureTypes does not contain QUERIES is refused HTTP 400 / InvalidParameterException with message "QueriesConfig must be used with QUERIES FeatureType." — entirely fixed text (including the trailing period), no values interpolated from the request.
Emulator: the code is correct (InvalidParameterException) but the message is 'QueriesConfig was supplied, but FeatureTypes does not contain QUERIES: QueriesConfig must be used with the QUERIES FeatureType' — a different sentence, with an extra prefix clause, an inserted "the" and no trailing period. The string is built in crate::state::queries_of (services/service-com-amazonaws-textract-textract/src/state.rs, the (false, Some(_)) arm of queries_of) and surfaced through AnalyzeDocument's `invalid` helper.
Why: the message is part of the refusal a client logs, asserts on and shows to an operator. A client (or test) matching AWS's text does not recognize the emulator's, so a correctly-detected bad request reads as an unknown failure.
Fix: the message produced by crate::state::queries_of for the "QueriesConfig without QUERIES" arm (shared by AnalyzeDocument and StartDocumentAnalysis) — emit exactly "QueriesConfig must be used with QUERIES FeatureType.".
Traces: textract.analyze-document-queries-without-feature-flag-rejected.

Cause 3 of 4 — CreateAdapter's AdapterName pattern refusal reports the constraint internals instead of AWS's message.
AWS: Smithy: CreateAdapter input {AdapterName required @length 1..128 @pattern ^[a-zA-Z0-9-_]+$, ClientRequestToken, Description, FeatureTypes required, AutoUpdate, Tags}; InvalidParameterException is a modeled client error at HTTP 400. An AdapterName containing characters outside that pattern is refused HTTP 400 / InvalidParameterException with message "Request has invalid parameters" — fixed text that names neither the member nor the offending value.
Emulator: the code is correct but the message is a constraint report of the form "Value '<the submitted name>' at 'adapterName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9-_]+$" (the quoted value is whatever name the request sent; the rest is fixed emulator text). That is AWS's generic framework-validation wording, not what Textract answers for this member.
Why: a client cannot match the documented refusal text, and the emulator additionally echoes the rejected input back into the message, which AWS does not do here — so anything asserting on or logging the message diverges and caller-supplied data leaks into the error surface.
Fix: CreateAdapter's AdapterName validation path in its operation handler (services/service-com-amazonaws-textract-textract/src/ops/create_adapter.rs) — refuse a name violating @length/@pattern with InvalidParameterException (HTTP 400) and message "Request has invalid parameters", rather than the generic constraint-violation renderer.
Traces: textract.create-adapter-validation-rejected.

Cause 4 of 4 — GetDocumentTextDetection's unknown-JobId refusal carries a generated sentence instead of AWS's message.
AWS: Smithy: GetDocumentTextDetection input {JobId required @length 1..64 @pattern ^[a-zA-Z0-9-_]+$, MaxResults, NextToken}; InvalidJobIdException ("An invalid job identifier was passed to an asynchronous analysis operation") is the modeled client error at HTTP 400. A JobId the service never issued is refused HTTP 400 / InvalidJobIdException with message "Request has invalid Job Id" — fixed text that does NOT contain the submitted job id.
Emulator: the code is correct but the message is '<the submitted JobId> is not a text detection job started by this service' — i.e. fixed emulator prose with the request's JobId interpolated at the front.
Why: the client's error branch is keyed on code plus message. AWS's message is constant and id-free, so a client that logs or asserts it sees a different string on every call against the emulator, and an operator comparing emulator and AWS logs cannot correlate the refusal.
Fix: the unknown-job refusal in GetDocumentTextDetection's handler (and the sibling GetDocumentAnalysis path, which must agree) — construct InvalidJobIdException with message "Request has invalid Job Id" and stop interpolating the submitted JobId.
Traces: textract.get-document-text-detection-invalid-job-id-rejected.

Related: the open wire issue 20261023T210700Z-textract-accepts-invalid-requests-with-http-200-instead.md concerns textract operations that answer 200 where AWS answers 400 (StartDocumentTextDetection, StartDocumentAnalysis, DetectDocumentText on an unsupported format, ListAdapters time range) and carries only those wire replay keys; the four causes here are refusals that DO happen but with the wrong code or message, so none of them is covered by it. The resolved issues on the textract data plane (including 20261022T192423Z-textract-analyzeexpense-and-s3-sourced-documents-rejected-async.md) concern calls that were wrongly refused outright, not these codes and messages.

## Evidence
Suite simparity-behavioral-aws-v1 replayed against the emulator booted from this checkout: 12/24 checks passed, 12 mismatched, 0 errored, 0 skipped, 0 unloadable; every check is textract, so the emulator was serving (bootstrap_signal false).

Cause 1 — [mismatch] textract.detect-document-text-corrupt-image-rejected: DetectDocumentText(Document={Bytes: good_png[:64]}) — a PNG signature followed by a truncated stream. Expected HTTP 400, Error.Code 'UnsupportedDocumentException', Error.Message 'Request has unsupported document format'. Actual: HTTP 400 with Error.Code 'BadDocumentException'; AssertionError: unexpected code: 'BadDocumentException'. The refusal happened, only its classification diverges.

Cause 2 — [mismatch] textract.analyze-document-queries-without-feature-flag-rejected: AnalyzeDocument(Document={Bytes: png}, FeatureTypes=["FORMS"], QueriesConfig={Queries:[{Text:"What is the invoice number?", Alias:"INVOICE_NUMBER"}]}). Error.Code 'InvalidParameterException' matched. AssertionError: expected message 'QueriesConfig must be used with QUERIES FeatureType.', got 'QueriesConfig was supplied, but FeatureTypes does not contain QUERIES: QueriesConfig must be used with the QUERIES FeatureType'.

Cause 3 — [mismatch] textract.create-adapter-validation-rejected: CreateAdapter(AdapterName=<a name containing spaces and '!!invalid!!'>, FeatureTypes=["QUERIES"]). Error.Code 'InvalidParameterException' matched. AssertionError: expected message 'Request has invalid parameters', got "Value 'sim2real adapter 67c9c9dca061!!invalid!!' at 'adapterName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9-_]+$" — the quoted value is the per-run name the check generated; the fixed part of the emulator's message is the "Value '…' at 'adapterName' failed to satisfy constraint: Member must satisfy regular expression pattern: …" template itself.

Cause 4 — [mismatch] textract.get-document-text-detection-invalid-job-id-rejected: GetDocumentTextDetection(JobId='sim2real-' + '0'*40). Error.Code 'InvalidJobIdException' matched. AssertionError: expected message 'Request has invalid Job Id', got 'sim2real-0000000000000000000000000000000000000000 is not a text detection job started by this service' — the leading token is the JobId from the request; the fixed part is ' is not a text detection job started by this service'. AWS's message interpolates nothing.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the simparity-behavioral-aws-v1 behavioral suite (textract group).
2. Cause 1: run textract.detect-document-text-corrupt-image-rejected — render a PNG, truncate it to its first 64 bytes (PNG signature, no IDAT/IEND) and call DetectDocumentText with those Bytes; observe Error.Code 'BadDocumentException' instead of 'UnsupportedDocumentException' with message 'Request has unsupported document format'.
3. Cause 2: run textract.analyze-document-queries-without-feature-flag-rejected — call AnalyzeDocument with QueriesConfig supplied and FeatureTypes=["FORMS"] (no QUERIES); observe InvalidParameterException with the emulator's two-clause message instead of 'QueriesConfig must be used with QUERIES FeatureType.'.
4. Cause 3: run textract.create-adapter-validation-rejected — call CreateAdapter with an AdapterName violating ^[a-zA-Z0-9-_]+$; observe InvalidParameterException whose message is a constraint report echoing the submitted name instead of 'Request has invalid parameters'.
5. Cause 4: run textract.get-document-text-detection-invalid-job-id-rejected — call GetDocumentTextDetection with a syntactically valid JobId that was never issued; observe InvalidJobIdException whose message interpolates the JobId instead of the fixed 'Request has invalid Job Id'.
6. Re-replay all four check ids in trace_ids after the fix; each must report the AWS code and the exact AWS message (and each check's positive control must still pass).
