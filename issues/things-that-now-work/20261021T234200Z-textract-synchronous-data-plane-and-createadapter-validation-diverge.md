Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: textract.detect-document-text-reads-rendered-text textract.detect-document-text-blank-image-returns-no-words textract.block-relationship-hierarchy-integrity textract.analyze-document-forms-extracts-key-value-pairs textract.analyze-document-combined-features-tables-and-forms textract.analyze-document-queries-feature-returns-query-block textract.analyze-document-queries-without-feature-flag-rejected textract.create-adapter-validation-rejected
Issue-Shape-Id: com.amazonaws.textract#Textract
Issue-Key: textract|DetectDocumentText|UnsupportedDocumentException
Issue-Key: textract|AnalyzeDocument|UnsupportedDocumentException
Issue-Key: textract|AnalyzeDocument|NotImplemented
Issue-Key: textract.analyze-document-queries-without-feature-flag-rejected|run#5|AssertionError|assert err["Code"] == "InvalidParameterException", (
Issue-Key: textract.create-adapter-validation-rejected|run#4|AssertionError|assert err["Code"] == "InvalidParameterException", (
Issue-Commit: 5a6a25397292b69464374d16e097ea3d0febc1b1

# Textract synchronous data plane and CreateAdapter validation diverge: valid PNG documents rejected as UnsupportedDocument, QueriesConfig unimplemented, wrong rejection codes

> _Note: this issue was identified against the code base at commit `5a6a25397292b69464374d16e097ea3d0febc1b1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DetectDocumentText

## Description
The emulator's textract service answers requests (it is bootstrapped), but its synchronous data-plane operations refuse well-formed documents and its parameter rejections use the wrong error codes. This issue bundles 5 distinct root causes, all in the textract crate. A fixer must close ALL FIVE before submitting.

Cause 1 of 5 - DetectDocumentText rejects a well-formed PNG instead of returning Blocks.
AWS: Smithy (services/service-com-amazonaws-textract-textract): DetectDocumentText input Document (required), output {DocumentMetadata, Blocks: BlockList, DetectDocumentTextModelVersion}; UnsupportedDocumentException is documented as "The format of the input document isn't supported. Documents for operations can be in PNG, JPEG, PDF, or TIFF format." A PNG is a supported format, so AWS answers HTTP 200 with a PAGE block and the LINE/WORD blocks for the text in the raster (and a PAGE with no WORD children for a blank page). The real unsupported-format case is UnsupportedDocumentException, HTTP 400.
Emulator: every PNG - including a plain white 800x400 image with no text at all - is rejected with UnsupportedDocumentException, HTTP 400 (message example: "the document's bytes are not text, and this emulator runs no OCR engine, so no text can be detected in it"; the text is an example, the status+code are the requirement). The rejection is unconditional on document content, not a format decision.
Why: the core text-detection path is unusable. A caller submitting a supported image gets a 400 client error where AWS returns 200 with Blocks, so document pipelines fail closed and mis-attribute the failure to their own input format. The blank-page case requires no text recognition at all and still fails.
Fix: the DetectDocumentText handler and the document-decode/block-synthesis layer it calls (services/service-com-amazonaws-textract-textract/src/ops/detect_document_text.rs plus the shared document/blocks module) - decode the supported raster formats, emit a PAGE block plus LINE/WORD blocks derived from the submitted bytes, and reserve UnsupportedDocumentException for bytes that are not a supported format.
Traces: textract.detect-document-text-reads-rendered-text, textract.detect-document-text-blank-image-returns-no-words, textract.block-relationship-hierarchy-integrity.

Cause 2 of 5 - AnalyzeDocument rejects a well-formed PNG the same way, so FORMS/TABLES never produce blocks.
AWS: Smithy: AnalyzeDocument input {Document required, FeatureTypes required (FeatureType enum: TABLES, FORMS, QUERIES, SIGNATURES, LAYOUT), HumanLoopConfig, QueriesConfig, AdaptersConfig}, output {DocumentMetadata, Blocks, HumanLoopActivationOutput, AnalyzeDocumentModelVersion}. With FeatureTypes=[FORMS] AWS returns 200 and KEY_VALUE_SET blocks carrying EntityTypes KEY/VALUE with CHILD relationships to WORD blocks; with [TABLES] it returns TABLE blocks with CELL children; with both features, both kinds in one response.
Emulator: UnsupportedDocumentException, HTTP 400 - the same unconditional "bytes are not text" rejection as Cause 1, for any image.
Why: the entire AnalyzeDocument feature surface (forms, tables) is unreachable; clients get a 400 instead of a 200 with blocks and cannot distinguish unsupported format from unimplemented feature.
Fix: the AnalyzeDocument handler and the shared document-decode/block-synthesis layer (services/service-com-amazonaws-textract-textract/src/ops/analyze_document.rs).
Traces: textract.analyze-document-forms-extracts-key-value-pairs, textract.analyze-document-combined-features-tables-and-forms.

Cause 3 of 5 - AnalyzeDocument with QueriesConfig returns an unmodeled "NotImplemented" error code.
AWS: Smithy: AnalyzeDocument accepts QueriesConfig (structure with required Queries, @length min 1) and FeatureTypes may include QUERIES; AWS answers 200 and includes one QUERY block per query with Query.Text and Query.Alias echoed from the request. "NotImplemented" is NOT in AnalyzeDocument's modeled error set (AccessDeniedException, BadDocumentException, DocumentTooLargeException, HumanLoopQuotaExceededException, InternalServerError, InvalidParameterException, InvalidS3ObjectException, ProvisionedThroughputExceededException, ThrottlingException, UnsupportedDocumentException).
Emulator: error code "NotImplemented" with message "operation AnalyzeDocument with QueriesConfig is scaffolded but not implemented in this emulator".
Why: an SDK cannot model that code, so it surfaces as a generic ClientError instead of a modeled Textract exception, and the QUERIES feature returns nothing usable.
Fix: the QueriesConfig branch of the AnalyzeDocument handler - synthesize QUERY blocks echoing each query's Text/Alias (services/service-com-amazonaws-textract-textract/src/ops/analyze_document.rs).
Traces: textract.analyze-document-queries-feature-returns-query-block.

Cause 4 of 5 - QueriesConfig supplied without QUERIES in FeatureTypes is not validated.
AWS: Smithy: InvalidParameterException ("An input parameter violated a constraint") is a modeled error of AnalyzeDocument; recorded behavior: AnalyzeDocument with QueriesConfig but FeatureTypes=[FORMS] is rejected with code InvalidParameterException, HTTP 400 (recorded message example: "QueriesConfig must be used with QUERIES FeatureType."). The requirement is the status and code; the message rule is request-derived (the member supplied and the feature missing from FeatureTypes).
Emulator: code "NotImplemented" - the request reaches the unimplemented QueriesConfig branch, so the cross-member validation never runs.
Why: a caller with an inconsistent request gets an unmodeled error and cannot tell that its FeatureTypes/QueriesConfig combination is the problem; validation must precede feature dispatch.
Fix: AnalyzeDocument input validation, ordered before feature dispatch (services/service-com-amazonaws-textract-textract/src/ops/analyze_document.rs validation path).
Traces: textract.analyze-document-queries-without-feature-flag-rejected.

Cause 5 of 5 - CreateAdapter pattern violation is rejected with ValidationException instead of InvalidParameterException.
AWS: Smithy: CreateAdapter.AdapterName @length 1..128 @pattern ^[a-zA-Z0-9-_]+$; both InvalidParameterException and ValidationException are modeled on the operation, and recorded behavior for an AdapterName containing spaces and '!' is code InvalidParameterException, HTTP 400 (recorded message example: "Request has invalid parameters").
Emulator: code ValidationException for the same input.
Why: a client catching textract's InvalidParameterException for bad adapter names does not catch ValidationException, so its validation/retry branch takes the wrong path.
Fix: CreateAdapter input validation - map AdapterName constraint violations to InvalidParameterException (services/service-com-amazonaws-textract-textract/src/ops/create_adapter.rs). The separate already-filed issue for UNIMPLEMENTED CreateAdapter covers making the operation work at all; this cause is specifically the rejection code and must be satisfied too.
Traces: textract.create-adapter-validation-rejected.

## Evidence
Suite simparity-behavioral-aws-v1, textract 0/24 passed.

Cause 1 - expected HTTP 200 with Blocks (PAGE + LINE/WORD, or a PAGE with no WORD children for a blank page). Actual: botocore.errorfactory.UnsupportedDocumentException: An error occurred (UnsupportedDocumentException) when calling the DetectDocumentText operation: the document's bytes are not text, and this emulator runs no OCR engine, so no text can be detected in it. textract.detect-document-text-blank-image-returns-no-words submits a plain white 800x400 PNG and asserts only success plus zero WORD blocks, and still gets this 400.

Cause 2 - expected HTTP 200 with KEY_VALUE_SET blocks (EntityTypes KEY/VALUE) and/or TABLE+CELL blocks. Actual: the same UnsupportedDocumentException on AnalyzeDocument with FeatureTypes=[FORMS] and with FeatureTypes=[TABLES, FORMS].

Cause 3 - expected HTTP 200 with one QUERY block per query (Query.Text/Query.Alias echoed). Actual: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the AnalyzeDocument operation: operation AnalyzeDocument with QueriesConfig is scaffolded but not implemented in this emulator.

Cause 4 - expected Error.Code == "InvalidParameterException" (HTTP 400). Actual: AssertionError: expected InvalidParameterException for QueriesConfig without QUERIES in FeatureTypes, got 'NotImplemented'.

Cause 5 - expected Error.Code == "InvalidParameterException" (HTTP 400) for AdapterName 'sim2real adapter <uniq>!!invalid!!'. Actual: AssertionError: expected InvalidParameterException, got 'ValidationException'.

## Steps to Reproduce
1. Build and boot the emulator from the checkout and replay the textract behavioral checks.
2. Cause 1: render any PNG (even an all-white one) and call textract.DetectDocumentText with Document={'Bytes': png} - observe UnsupportedDocumentException/400 instead of 200 with Blocks (checks detect-document-text-reads-rendered-text, detect-document-text-blank-image-returns-no-words, block-relationship-hierarchy-integrity).
3. Cause 2: call textract.AnalyzeDocument with the same bytes and FeatureTypes=['FORMS'], then ['TABLES','FORMS'] - same UnsupportedDocumentException.
4. Cause 3: call AnalyzeDocument with FeatureTypes=['QUERIES'] and QueriesConfig={'Queries':[{'Text':'What is the invoice number?','Alias':'INVOICE_NUMBER'}]} - observe code NotImplemented.
5. Cause 4: repeat step 4 with FeatureTypes=['FORMS'] - observe NotImplemented instead of InvalidParameterException/400.
6. Cause 5: call textract.CreateAdapter(AdapterName='sim2real adapter x!!invalid!!', FeatureTypes=['QUERIES']) - observe ValidationException instead of InvalidParameterException/400.
