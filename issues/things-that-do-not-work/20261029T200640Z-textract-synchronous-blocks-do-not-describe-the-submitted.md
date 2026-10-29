Issue-Source: trace-sweep-divergence
Issue-Trace: textract-sync-text-detection-and-document-analysis
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: textract-sync-text-detection-and-document-analysis
Issue-Shape-Id: com.amazonaws.textract#Textract
Issue-Key: textract|DetectDocumentText|2xx->2xx|$.Blocks[*]
Issue-Key: textract|DetectDocumentText|2xx->2xx|$.Blocks[*].Confidence
Issue-Key: textract|DetectDocumentText|2xx->2xx|$.Blocks[*].Geometry.BoundingBox.Height
Issue-Key: textract|DetectDocumentText|2xx->2xx|$.Blocks[*].Geometry.BoundingBox.Left
Issue-Key: textract|DetectDocumentText|2xx->2xx|$.Blocks[*].Geometry.BoundingBox.Top
Issue-Key: textract|DetectDocumentText|2xx->2xx|$.Blocks[*].Geometry.BoundingBox.Width
Issue-Key: textract|DetectDocumentText|2xx->2xx|$.Blocks[*].Geometry.Polygon[*].X
Issue-Key: textract|DetectDocumentText|2xx->2xx|$.Blocks[*].Geometry.Polygon[*].Y
Issue-Key: textract|DetectDocumentText|2xx->2xx|$.Blocks[*].Geometry.RotationAngle
Issue-Key: textract|DetectDocumentText|2xx->2xx|$.Blocks[*].Text
Issue-Key: textract|AnalyzeDocument|2xx->2xx|$.Blocks[*]
Issue-Key: textract|AnalyzeDocument|2xx->2xx|$.Blocks[*].Confidence
Issue-Key: textract|AnalyzeDocument|2xx->2xx|$.Blocks[*].Geometry.BoundingBox.Height
Issue-Key: textract|AnalyzeDocument|2xx->2xx|$.Blocks[*].Geometry.BoundingBox.Left
Issue-Key: textract|AnalyzeDocument|2xx->2xx|$.Blocks[*].Geometry.BoundingBox.Top
Issue-Key: textract|AnalyzeDocument|2xx->2xx|$.Blocks[*].Geometry.BoundingBox.Width
Issue-Key: textract|AnalyzeDocument|2xx->2xx|$.Blocks[*].Geometry.Polygon[*].X
Issue-Key: textract|AnalyzeDocument|2xx->2xx|$.Blocks[*].Geometry.Polygon[*].Y
Issue-Key: textract|AnalyzeDocument|2xx->2xx|$.Blocks[*].Text
Issue-Commit: 78295efc92da5c3ab6dc723b39af1e4944870efd

# Textract synchronous Blocks do not describe the submitted document

> _Note: this issue was identified against the code base at commit `78295efc92da5c3ab6dc723b39af1e4944870efd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DetectDocumentText

## Description
The two synchronous textract read paths answer HTTP 200 with a structurally valid Blocks list, but the CONTENT of that list is not a description of the document that was submitted: the emulator returns fewer blocks than the document contains, the Text it reports is not the text drawn in the pixels, Confidence values are unrelated to the recorded ones, and every Geometry it emits is the same small box regardless of what the block is - in particular the PAGE block's BoundingBox covers only the first recognized line rather than the page content, and WORD-level RotationAngle is not reported. This issue bundles 2 distinct root causes, both in the textract crate's shared document-reading/block-derivation path; a fixer must close BOTH before submitting.

Note on what is and is not the requirement here: individual coordinate floats produced by an OCR engine are not a wire contract and no fix should chase a recording's 15-digit values. What IS required, and what diverges, is (a) the NUMBER and kind of blocks - one block per page, per detected line and per detected word of the submitted document - (b) Block$Text equal to the characters actually drawn, (c) a Geometry per block that encloses THAT block's own pixels rather than a constant box shared by all blocks, with the PAGE block spanning the page's content, and (d) Geometry$RotationAngle on WORD blocks where the document's words are rotated.

Cause 1 of 2 - DetectDocumentText returns a short, constant-geometry block set that does not match the submitted document.
AWS: Smithy (services/service-com-amazonaws-textract-textract/smithy-model/com-amazonaws-textract-textract.json): DetectDocumentText input {Document required}, output {DocumentMetadata, Blocks: BlockList, DetectDocumentTextModelVersion}. BlockType documents, for text detection, PAGE ("Contains a list of the LINE Block objects that are detected on a document page"), LINE ("A string of space-delimited, contiguous words") and WORD ("A word detected on a document page"). Block$Text is "The word or line of text that's recognized by Amazon Textract"; Block$Confidence is "the confidence score that Amazon Textract has in the accuracy of the recognized text and the accuracy of the geometry points around the recognized text" (@range 0..100). Geometry$BoundingBox is "an axis-aligned coarse representation of the location of the recognized item on the document page", whose Left/Top/Width/Height are documented as ratios of the overall page width/height with the origin at the upper-left; Geometry$Polygon is "within the bounding box, a fine-grained polygon around the recognized item"; Geometry$RotationAngle is "a numerical value corresponding to the rotation of the WORD block. Possible values are 0, 90, 180, and 270". So AWS emits one block per detected page/line/word, each with its OWN box around its own pixels, and the recorded response for this step contains blocks beyond index 3 that the emulator does not emit at all.
Emulator: HTTP 200 with a Blocks list that is SHORTER than the recorded one (the recorded Blocks[3] and beyond have no counterpart), whose Text differs from the drawn text, whose Confidence differs, and whose Geometry is effectively one repeated rectangle: Blocks[0] (the page) and Blocks[1] carry an identical BoundingBox, and every Polygon vertex of Blocks[0] takes only two distinct X and two distinct Y values equal to that box's edges - i.e. the page block is given the first line's extent instead of the page's content extent. Geometry$RotationAngle is absent where the recording reports one.
Why: a client calls DetectDocumentText to read a document. It gets a 200 whose shape passes its schema checks, so nothing signals failure, yet the strings are not the document's text, blocks for parts of the document are simply missing, and every block claims to live in the same place on the page - so any consumer that crops, highlights, orders text by position or counts lines/words operates on data that was never in the document.
Fix: the shared document-reading and block-derivation path in the textract crate's state module that DetectDocumentText's handler calls (glyph recognition, word/line segmentation, and the per-block geometry assignment) - it must emit a block per detected page/line/word of the submitted raster, set each block's BoundingBox and Polygon from that block's own pixel extent (the PAGE block spanning the page's detected content, not the first line), and populate WORD RotationAngle for rotated words.
Traces: textract-sync-text-detection-and-document-analysis#task#1.

Cause 2 of 2 - AnalyzeDocument returns the same deficient block set on the analysis path, dropping four of the recorded blocks.
AWS: Smithy: AnalyzeDocument input {Document required, FeatureTypes required, HumanLoopConfig, QueriesConfig, AdaptersConfig}, output {DocumentMetadata, Blocks, HumanLoopActivationOutput, AnalyzeDocumentModelVersion}. For text analysis BlockType additionally documents PAGE ("Contains a list of child Block objects that are detected on a document page"), KEY_VALUE_SET, LINE, WORD and TABLE, and Block carries the analysis-only members (EntityTypes, RowIndex/ColumnIndex, Relationships). The recorded response for this step contains seven blocks (indices 0..6).
Emulator: HTTP 200 with only three blocks - the comparison reports $.Blocks[3], $.Blocks[4], $.Blocks[5] and $.Blocks[6] as diverging because they are present in the recording and absent from the emulator - plus the same Text, Confidence and constant-geometry divergences as Cause 1 on the blocks it does return (Blocks[0]'s box again equals Blocks[1]'s, so the PAGE block does not span the page content).
Why: a client that asked for document analysis receives a successful response in which more than half the document's blocks do not exist, so its key/value, table and relationship traversal finds nothing to traverse and it concludes the document has no such content rather than that the service under-reported. Because the response is a 200 with a valid shape, the loss is silent.
Fix: the same block-derivation path, reached from AnalyzeDocument's handler - the analysis path must derive its full block set (page, lines, words and the feature blocks requested through FeatureTypes) from the submitted document with per-block geometry, rather than the truncated set it shares with the detection path.
Traces: textract-sync-text-detection-and-document-analysis#task#2.

Related open issue, which does NOT cover these two wire steps: 20261029T184456Z-textract-document-analysis-returns-garbled-ocr-text-and.md is a BEHAVIORAL issue about the same subsystem (garbled OCR text and empty FORMS/TABLES/QUERIES/expense extraction), but it carries only behavioral check ids (textract.*) as its replay keys, so a fixer closing it never re-replays this wire corpus; it also says nothing about the missing blocks, the constant per-block Geometry, the PAGE block's extent or the absent RotationAngle reported here. 20261023T210700Z-textract-accepts-invalid-requests-with-http-200-instead.md concerns refusal paths that answer 200, not the content of a legitimately successful read.

## Evidence
Trace set simparity-wire-aws-v1 replayed against the emulator booted from this checkout: emulator_up true, bootstrap_signal false, 6 fixtures replayed, 38 steps total, 19 passed, 19 mismatched, 0 errored, 0 blocked, score 50.0%, 10 divergence clusters, all service 'textract'. Neither cluster below carries a stub_path or an owned_by.

Cause 1 - [mismatch] textract-sync-text-detection-and-document-analysis#task#1, DetectDocumentText, expected status 200 and got 200, so the divergence is entirely in the body. divergence_key service=textract|DetectDocumentText|status=200->200 lists the diverging keys: $.Blocks[0].Geometry.BoundingBox.{Height,Left,Top,Width}, all eight $.Blocks[0].Geometry.Polygon[0..3].{X,Y}, $.Blocks[1].Confidence, $.Blocks[1].Geometry.BoundingBox.{Height,Left,Top,Width}, all of $.Blocks[1].Geometry.Polygon[0..3].{X,Y}, $.Blocks[1].Text, $.Blocks[2].Confidence, $.Blocks[2].Geometry.BoundingBox.*, $.Blocks[2].Geometry.Polygon[0..3].*, $.Blocks[2].Geometry.RotationAngle, $.Blocks[2].Text, $.Blocks[3] and beyond.
  The structural facts those values show (not the floats themselves, which are OCR output): the emulator's $.Blocks[0] BoundingBox (Left 0.0356, Top 0.2333, Width 0.2733, Height 0.1667) is IDENTICAL to its $.Blocks[1] BoundingBox, and its $.Blocks[0].Polygon uses only X in {0.0356, 0.3089} and Y in {0.2333, 0.4000} - the four corners of that same small box - whereas the recorded PAGE block spans the page (Left 0.0118, Top 0.1004, Width 0.8972, Height 0.8755). $.Blocks[2].Geometry.RotationAngle is reported in the recording and diverges. $.Blocks[1].Text and $.Blocks[2].Text diverge, i.e. the reported text is not the document's. $.Blocks[3] (and further indices) appear as whole-block divergences: the recorded response has blocks the emulator does not return. Confidence also diverges ($.Blocks[1].Confidence recorded 99.9256, emulator 81.9842) - both are in @range 0..100, so this is reported as corroboration of the content defect, not as a standalone requirement on the exact float.

Cause 2 - [mismatch] textract-sync-text-detection-and-document-analysis#task#2, AnalyzeDocument, expected status 200 and got 200. divergence_key service=textract|AnalyzeDocument|status=200->200 ends with $.Blocks[3], $.Blocks[4], $.Blocks[5], $.Blocks[6] - four whole blocks present in the recording and missing from the emulator's response, which therefore returns 3 blocks where 7 were recorded. Alongside them: $.Blocks[0].Geometry.BoundingBox.* and Polygon[0..3].*, $.Blocks[1].* (Confidence, BoundingBox, Polygon, Text), $.Blocks[2].* (Confidence, BoundingBox, Polygon, Text). Again the emulator's $.Blocks[0] box (Left 0.0333, Top 0.2333, Width 0.3778, Height 0.1733) equals its $.Blocks[1] box, while the recorded PAGE block spans the page (Left 0.0091, Top 0.0118, Width 0.9776, Height 0.9819), and the emulator's Blocks[0] Polygon uses only the two X values {0.0333, 0.4111} and two Y values {0.2333, 0.4067} of that box.

## Steps to Reproduce
1. Clone this checkout, build and boot the emulator from it, and replay the simparity-wire-aws-v1 wire corpus (replay_wire_corpus with no arguments).
2. Cause 1: replay fixture textract-sync-text-detection-and-document-analysis, step task#1 - DetectDocumentText over the fixture's document. Compare the response's Blocks against the recorded ones: count them (the recording has blocks at index 3 and beyond that the emulator omits), read Blocks[*].Text against the text drawn in the document, and check whether Blocks[0] (PAGE) has its own page-spanning BoundingBox or merely repeats Blocks[1]'s box; also check whether WORD blocks carry Geometry.RotationAngle.
3. Cause 2: replay the same fixture's step task#2 - AnalyzeDocument over its document and FeatureTypes. Observe only 3 blocks returned where 7 were recorded (Blocks[3]..Blocks[6] missing entirely), the same Text divergences, and Blocks[0]'s BoundingBox again equal to Blocks[1]'s instead of spanning the page.
4. After fixing, re-replay the representative step (textract-sync-text-detection-and-document-analysis#task#1) AND the fixture#step token in trace_ids (textract-sync-text-detection-and-document-analysis#task#2). Each must return a block per detected page/line/word of the submitted document, with Text equal to the drawn characters, a per-block Geometry enclosing that block's own pixels (PAGE spanning the page content), and RotationAngle on rotated WORD blocks; the 19 steps that already pass must still pass.
