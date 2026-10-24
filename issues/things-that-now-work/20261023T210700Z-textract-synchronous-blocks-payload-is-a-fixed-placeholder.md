Issue-Source: trace-sweep-divergence
Issue-Trace: textract-sync-text-detection-and-document-analysis
Issue-Step: task#1
Issue-Severity: MEDIUM
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
Issue-Commit: 89d5e152ff27d848f663b8ae7517a0827a99e56e

# Textract synchronous Blocks payload is a fixed placeholder instead of the document's blocks

> _Note: this issue was identified against the code base at commit `89d5e152ff27d848f663b8ae7517a0827a99e56e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DetectDocumentText

## Description
Both synchronous read operations answer HTTP 200 but the Blocks list they return is a constant placeholder rather than a block tree derived from the submitted document: the PAGE block's geometry is degenerate, child blocks carry constant Confidence and a fixed small geometry, and the list is shorter than the document's block tree (whole Blocks entries are absent). This issue bundles 2 distinct root causes, both in the textract crate; a fixer must close BOTH before submitting.

Cause 1 of 2 - DetectDocumentText returns a placeholder Blocks list.
AWS: Smithy (services/service-com-amazonaws-textract-textract/smithy-model/com-amazonaws-textract-textract.json): DetectDocumentText input {Document required}, output {DocumentMetadata, Blocks: BlockList, DetectDocumentTextModelVersion}. Recorded behavior: HTTP 200 with a PAGE block whose Geometry.BoundingBox and 4-point Polygon bound the detected content (neither the full unit square nor all-0/all-1 corners), followed by one LINE/WORD block per detected text region, each with its own Text, Confidence and Geometry; the number of Blocks is a property of the document, not a constant.
Emulator: HTTP 200, but Blocks[0] (the PAGE) has BoundingBox Left/Top 0.0 and Width/Height 1.0 with a Polygon whose points are all exactly 0.0/1.0; every subsequent block carries a constant Confidence and an identical fixed BoundingBox Height 0.018 / Width 0.105; and the list is truncated relative to the recording, so trailing Blocks entries are missing entirely. The values do not vary with the document, so the payload is synthesized from a template rather than from the decoded document.
Why: a client that reads Geometry to crop, order or lay out detected text gets a degenerate full-page box and identically sized child boxes, so any geometry-dependent logic (reading order, column detection, overlap tests, cropping) produces wrong results that look plausible. A client that iterates Blocks also misses the trailing blocks outright, and a confidence threshold applied to a constant Confidence either accepts or rejects everything.
Fix: the DetectDocumentText handler's block-synthesis layer - derive the PAGE geometry and the per-block Text/Confidence/Geometry from the decoded document so the Blocks list has the document's block count and per-block geometry rather than template constants.
Traces: textract-sync-text-detection-and-document-analysis#task#1.

Cause 2 of 2 - AnalyzeDocument returns the same placeholder Blocks list.
AWS: Smithy: AnalyzeDocument input {Document required, FeatureTypes required, HumanLoopConfig, QueriesConfig, AdaptersConfig}, output {DocumentMetadata, Blocks: BlockList, HumanLoopActivationOutput, AnalyzeDocumentModelVersion}. Recorded behavior: HTTP 200 with a PAGE block bounding the content plus the document's own blocks, each with document-derived Text, Confidence and Geometry.
Emulator: HTTP 200 with the same template payload as Cause 1 - PAGE BoundingBox 0/0/1/1 with an all-0/1 Polygon, constant child Confidence and a fixed child BoundingBox - and Blocks[3] through Blocks[6] missing relative to the recording, so the analysis path shares the placeholder synthesis rather than producing its own block tree.
Why: same as Cause 1, and additionally any FORMS/TABLES consumer walking block relationships sees a block set that is both too small and geometrically meaningless, so table/key-value reconstruction cannot be exercised against the emulator.
Fix: the AnalyzeDocument handler's block-synthesis layer, sharing the document-derived block builder fixed for Cause 1 so both synchronous operations emit the document's blocks with real geometry and block count.
Traces: textract-sync-text-detection-and-document-analysis#task#2.

The exact coordinate and confidence values of any one recording are examples; the requirements are that Blocks is derived from the submitted document (non-degenerate PAGE geometry, per-block geometry that varies between blocks, and the document's block count) rather than a constant template.

## Evidence
Corpus simparity-wire-aws-v1 replayed against the emulator booted from this checkout: 38 steps, 17 passed, 12 divergence clusters; these two are the status-200 value-mismatch clusters.

Cause 1 - fixture textract-sync-text-detection-and-document-analysis, step task#1, operation DetectDocumentText, expected and actual both HTTP 200. Blocks[0].Geometry.BoundingBox expected Left 0.011839513666927814 / Top 0.10043054074048996 / Width 0.8971883058547974 / Height 0.875543475151062, actual 0.0 / 0.0 / 1.0 / 1.0; Blocks[0].Geometry.Polygon expected four distinct interior points, actual all coordinates exactly 0.0 or 1.0. Blocks[1].Confidence expected 99.9255599975586, actual 81.9842300415039; Blocks[1].Geometry.BoundingBox expected Left 0.03252305090427399 / Top 0.21599966287612915 / Width 0.27597329020500183 / Height 0.1828515976667404, actual 0.06499999761581421 / 0.05000000074505806 / 0.10499999672174454 / 0.017999999225139618. The cluster's key list continues through Blocks[1].Text, Blocks[2].* (including Geometry.RotationAngle) and further Blocks[3]+ entries.

Cause 2 - fixture textract-sync-text-detection-and-document-analysis, step task#2, operation AnalyzeDocument, expected and actual both HTTP 200. Blocks[0].Geometry.BoundingBox expected Left 0.009066186845302582 / Top 0.011760788038372992 / Width 0.9776000380516052 / Height 0.9818877577781676, actual 0.0 / 0.0 / 1.0 / 1.0, Polygon again all 0.0/1.0. Blocks[1].Confidence expected 94.70218658447266, actual 82.48684692382812; Blocks[1].Geometry.BoundingBox expected Left 0.032841674983501434 / Top 0.2324480265378952 / Width 0.3802343904972077 / Height 0.1839762181043625, actual 0.057500001043081284 / 0.05000000074505806 / 0.10499999672174454 / 0.017999999225139618. The cluster's key list ends with $.Blocks[3], $.Blocks[4], $.Blocks[5], $.Blocks[6] - whole blocks present in the recording and absent from the emulator's response.

The identical actual child-block geometry (Width 0.105, Height 0.018) and the identical degenerate PAGE box across two different documents and two different operations is the evidence that the payload is a template rather than document-derived. state_reset_failures was 0.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the simparity-wire-aws-v1 wire corpus.
2. Cause 1: replay fixture textract-sync-text-detection-and-document-analysis step task#1 - call textract.DetectDocumentText with the fixture's Document; observe Blocks[0].Geometry.BoundingBox = {Left 0.0, Top 0.0, Width 1.0, Height 1.0} with an all-0/1 Polygon, a constant Confidence and a fixed 0.105 x 0.018 BoundingBox on every child block, and fewer Blocks than the recording.
3. Cause 2: replay the same fixture's step task#2 - call textract.AnalyzeDocument with the fixture's Document and FeatureTypes; observe the same degenerate PAGE geometry and constant child geometry, and Blocks[3]..Blocks[6] absent from the response.
4. Compare the two responses: the child-block BoundingBox Width/Height are identical across both operations and both documents, showing the Blocks payload is not derived from the submitted document.
5. Re-replay both fixture steps after the fix; each must return a PAGE block bounding the content, per-block geometry that varies between blocks, and the document's full block count.
