Issue-Source: trace-sweep-divergence
Issue-Trace: textract-sync-text-detection-and-document-analysis
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: textract-sync-text-detection-and-document-analysis textract-sync-expense-and-id-analysis
Issue-Shape-Id: com.amazonaws.textract#Textract
Issue-Key: textract|DetectDocumentText|2xx->4xx|$.Blocks
Issue-Key: textract|DetectDocumentText|2xx->4xx|$.DetectDocumentTextModelVersion
Issue-Key: textract|DetectDocumentText|2xx->4xx|$.DocumentMetadata
Issue-Key: textract|DetectDocumentText|2xx->4xx|status
Issue-Key: textract|AnalyzeDocument|2xx->4xx|$.AnalyzeDocumentModelVersion
Issue-Key: textract|AnalyzeDocument|2xx->4xx|$.Blocks
Issue-Key: textract|AnalyzeDocument|2xx->4xx|$.DocumentMetadata
Issue-Key: textract|AnalyzeDocument|2xx->4xx|status
Issue-Key: textract|AnalyzeExpense|2xx->4xx|$.DocumentMetadata
Issue-Key: textract|AnalyzeExpense|2xx->4xx|$.ExpenseDocuments
Issue-Key: textract|AnalyzeExpense|2xx->4xx|status
Issue-Commit: dcb14db06d6eb8eb19e965c6ac371e7edd3d0be4

# Textract synchronous data plane rejects supported documents with 400 instead of returning Blocks

> _Note: this issue was identified against the code base at commit `dcb14db06d6eb8eb19e965c6ac371e7edd3d0be4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DetectDocumentText

## Description
The textract service answers on the wire (it is bootstrapped), but every synchronous document operation refuses a supported, well-formed document with an HTTP 400 instead of producing a result body. This issue bundles 3 distinct root causes, one per synchronous operation. A fixer must close ALL THREE before submitting.

Cause 1 of 3 - DetectDocumentText returns 400 for a supported single-page raster instead of 200 with Blocks.
AWS: Smithy (services/service-com-amazonaws-textract-textract/smithy-model/com-amazonaws-textract-textract.json): DetectDocumentText input {Document required}, output {DocumentMetadata, Blocks: BlockList, DetectDocumentTextModelVersion}; UnsupportedDocumentException is a modeled client error (HTTP 400) reserved for document formats that are not supported. The recorded response for this step is HTTP 200 carrying DocumentMetadata, DetectDocumentTextModelVersion and a Blocks list with a PAGE block whose CHILD relationships reach LINE blocks, each LINE reaching WORD blocks with Text/TextType/Confidence. The structural requirement is the 200 plus those members and the PAGE->LINE->WORD hierarchy; the particular geometry floats, block Ids, confidences and recognised strings in the recording are per-document example values, not requirements.
Emulator: HTTP 400. The replay reports $.Blocks, $.DocumentMetadata and $.DetectDocumentTextModelVersion all missing from the response. The rejection does not depend on the document being an unsupported format - the fixture submits the document the recording was made against.
Why: the core text-detection call is unusable. A client that submits a supported document gets a client error where AWS returns 200 with blocks, so document pipelines fail closed and mis-attribute the failure to their own input format rather than to the service.
Fix: the DetectDocumentText handler and the shared document-decode / block-synthesis layer it calls - decode the supported raster formats, emit a PAGE block plus its LINE/WORD children derived from the submitted bytes, and raise UnsupportedDocumentException only for bytes that are genuinely not a supported format.
Traces: textract-sync-text-detection-and-document-analysis#task#1.

Cause 2 of 3 - AnalyzeDocument returns 400 for the same supported document instead of 200 with Blocks and AnalyzeDocumentModelVersion.
AWS: Smithy: AnalyzeDocument input {Document required, FeatureTypes required, HumanLoopConfig, QueriesConfig, AdaptersConfig}, output {DocumentMetadata, Blocks, HumanLoopActivationOutput, AnalyzeDocumentModelVersion}; UnsupportedDocumentException modeled at HTTP 400 for unsupported formats only. The recorded response is HTTP 200 with AnalyzeDocumentModelVersion ('1.0' in the recording), DocumentMetadata and a Blocks list rooted at a PAGE block with CHILD relationships to LINE and WORD blocks for the requested feature set.
Emulator: HTTP 400; $.AnalyzeDocumentModelVersion, $.Blocks and $.DocumentMetadata are all missing.
Why: the whole AnalyzeDocument feature surface (FORMS/TABLES/etc.) is unreachable over the wire, and the client cannot distinguish 'unsupported format' from 'this operation never succeeds'.
Fix: the AnalyzeDocument handler plus the same shared document-decode / block-synthesis layer as Cause 1.
Traces: textract-sync-text-detection-and-document-analysis#task#2.

Cause 3 of 3 - AnalyzeExpense returns 400 instead of 200 with ExpenseDocuments.
AWS: Smithy: AnalyzeExpense input {Document required}, output {DocumentMetadata, ExpenseDocuments: ExpenseDocumentList}; UnsupportedDocumentException modeled at HTTP 400 for unsupported formats. The recorded response is HTTP 200 with DocumentMetadata (Pages: 1 in the recording) and one ExpenseDocument carrying ExpenseIndex, a Blocks list with a PAGE block and Geometry, a LineItemGroups entry with LineItemGroupIndex, and SummaryFields. The requirement is the 200 and the presence/shape of DocumentMetadata and ExpenseDocuments with those members; the recorded ids, page count and empty field lists are examples.
Emulator: HTTP 400; $.DocumentMetadata and $.ExpenseDocuments are missing.
Why: expense extraction returns a client error for a document AWS processes, so any invoice/receipt flow built on the emulator cannot run at all.
Fix: the AnalyzeExpense handler and the shared expense-document synthesis path (it must produce at least the PAGE-level ExpenseDocument skeleton AWS returns, with ExpenseIndex, LineItemGroups and SummaryFields present even when empty).
Traces: textract-sync-expense-and-id-analysis#task#1.

Related open issue, which does NOT cover these: 20261021T234200Z-textract-synchronous-data-plane-and-createadapter-validation-diverge.md is a BEHAVIORAL issue (corpus simparity-behavioral-aws-v1) describing the same unconditional document rejection for DetectDocumentText and AnalyzeDocument. It carries no wire fixture/step key, so none of these replay steps are gated by it, and it says nothing about AnalyzeExpense (Cause 3). The wire bindings listed here are what a fixer must re-replay.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator booted from this checkout (textract: 2/38 steps passed).

Cause 1 - fixture textract-sync-text-detection-and-document-analysis step task#1, DetectDocumentText: expected status 200, got 400. Divergence detail: $.Blocks missing (expected a PAGE block with CHILD relationships to LINE blocks and WORD blocks such as Text 'SIMPARITY', TextType 'PRINTED'), $.DocumentMetadata missing, $.DetectDocumentTextModelVersion missing.

Cause 2 - fixture textract-sync-text-detection-and-document-analysis step task#2, AnalyzeDocument: expected status 200, got 400. Detail: $.AnalyzeDocumentModelVersion missing (expected '1.0'), $.Blocks missing (expected PAGE/LINE/WORD blocks, e.g. LINE Text 'Account Number: 48213'), $.DocumentMetadata missing.

Cause 3 - fixture textract-sync-expense-and-id-analysis step task#1, AnalyzeExpense: expected status 200, got 400. Detail: $.DocumentMetadata missing (expected {'Pages': 1}), $.ExpenseDocuments missing (expected one document with ExpenseIndex 1, a PAGE Block with full-page BoundingBox/Polygon, LineItemGroups [{LineItemGroupIndex: 1, LineItems: []}] and SummaryFields []).

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. Replay fixture textract-sync-text-detection-and-document-analysis and watch step task#1 (DetectDocumentText with the fixture's document): the emulator answers 400 where the recording has 200 with Blocks.
3. Watch step task#2 of the same fixture (AnalyzeDocument with the fixture's FeatureTypes): 400 instead of 200 with Blocks and AnalyzeDocumentModelVersion.
4. Replay fixture textract-sync-expense-and-id-analysis step task#1 (AnalyzeExpense): 400 instead of 200 with DocumentMetadata and ExpenseDocuments.
5. All three must return the recorded success shapes before the fixtures pass.
