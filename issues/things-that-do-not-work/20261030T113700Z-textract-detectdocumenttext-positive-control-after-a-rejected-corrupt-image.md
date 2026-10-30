Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: textract.detect-document-text-corrupt-image-rejected
Issue-Shape-Id: com.amazonaws.textract#Textract
Issue-Key: textract.detect-document-text-corrupt-image-rejected|run#10|AssertionError|assert "FINE" in line_texts and "DOCUMENT" in line_texts, (
Issue-Commit: 8a62bc5d8898770ac63ef3e09ff3986bf7746acd

# DetectDocumentText positive control after a rejected corrupt image returns garbled OCR text ('[|N[EUW[NT' for a page rendering "FINE DOCUMENT")

> _Note: this issue was identified against the code base at commit `8a62bc5d8898770ac63ef3e09ff3986bf7746acd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DetectDocumentText

## Description
This issue bundles 1 distinct root cause in the textract crate; a fixer must close it before submitting.

Cause 1 of 1 — DetectDocumentText reports Block$Text that is not the text drawn in the submitted page, so the positive control of the corrupt-image rejection check fails even though the refusal itself is now correct.

AWS: Smithy (services/service-com-amazonaws-textract-textract/smithy-model/com-amazonaws-textract-textract.json): DetectDocumentText input {Document required}, output {DocumentMetadata, Blocks: BlockList, DetectDocumentTextModelVersion}; modeled errors include UnsupportedDocumentException (client, HTTP 400). Block$Text is documented as "The word or line of text that's recognized by Amazon Textract." The requirement is general: for any PNG that renders an ASCII line, the LINE/WORD blocks AWS returns carry that same text — the characters actually drawn, for whatever string the caller renders — not one recording's particular string. The rejection half of this check is already satisfied: the emulator refuses a PNG-signature-bearing, truncated blob with error code UnsupportedDocumentException and message "Request has unsupported document format" (that message is entirely fixed text for this error class, with no variable parts), which matched.

Emulator: immediately after that correct refusal, the same check re-submits the UNCORRUPTED image as a positive control. The emulator answers HTTP 200 with a structurally valid Blocks list, but the recognized characters are wrong: for a page rendering the single line "FINE DOCUMENT" the joined LINE texts came back as '[|N[EUW[NT'. No status or error code diverges — only the content.

Why: a client calls DetectDocumentText to read a document. It gets a 200 whose shape passes its own schema validation while every string in it is corrupt, so its matching, indexing and parsing run on text that was never in the document and nothing in the response signals the failure. It is worse here than a plain content bug: a caller that validates its rejection handling against this operation sees the refusal path behave correctly and therefore trusts the success path it exercises in the same breath.

Fix: the glyph recognition in the service's OCR module (crate::state::ocr, reached from crate::state::read_submitted_document / read_content, shared by the inline-Bytes, S3Object and async Get* paths) — per-glyph classification and word/line segmentation must resolve rendered ASCII glyphs to the characters actually drawn before any block text is emitted. No change is needed to the UnsupportedDocumentException path.

Related: the open issue 20261029T184456Z-textract-document-analysis-returns-garbled-ocr-text-and.md describes the same garbling as its Cause 1, but it does NOT carry this check's replay key (its Issue-Trace-Ids/Issue-Key list names only detect-document-text-reads-rendered-text, detect-document-text-from-s3-object-source, start-document-text-detection-job-lifecycle and the four feature checks). The suite therefore did not mark this check as owned, and a fixer closing that issue would never re-replay textract.detect-document-text-corrupt-image-rejected. If the recognizer fix for that issue also fixes this check, close both; this issue exists so the check is re-replayed rather than left unrun.

Traces: textract.detect-document-text-corrupt-image-rejected.

## Evidence
Suite simparity-behavioral-aws-v1 replayed against the emulator booted from this checkout: 15/24 checks passed, 9 mismatched, 0 errored, 0 skipped, 0 unloadable. All 24 checks are textract, so bootstrap_signal was false and the emulator was serving. Eight of the nine failures are marked OWNED by 20261029T184456Z-textract-document-analysis-returns-garbled-ocr-text-and.md; this one is not.

Cause 1 — [mismatch] textract.detect-document-text-corrupt-image-rejected:
  Step 1 (PASSED): detect_document_text(Document={"Bytes": good_png[:64]}) — a PNG magic header with the rest of the stream chopped off — raised a ClientError with Error.Code == "UnsupportedDocumentException" and Error.Message == "Request has unsupported document format", matching AWS exactly (the assertions on both code and message passed).
  Step 2 (FAILED): detect_document_text(Document={"Bytes": good_png}) on the uncorrupted render of the single line "FINE DOCUMENT" returned 200 with LINE blocks whose joined, upper-cased Text was '[|N[EUW[NT'.
  AssertionError: expected 'FINE' and 'DOCUMENT' in recognized text, got '[|N[EUW[NT'
  Expected: LINE/WORD Block$Text equal to the characters rendered into the PNG (here containing the words FINE and DOCUMENT). Actual: '[|N[EUW[NT'. The glyph-for-glyph corruption pattern (ASCII letters mapped to bracket/pipe/digit lookalikes) is the same one reported for the other DetectDocumentText paths, indicating the single shared recognizer rather than this handler.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the simparity-behavioral-aws-v1 behavioral suite (textract group).
2. Run textract.detect-document-text-corrupt-image-rejected. It renders a PNG containing the line "FINE DOCUMENT", truncates a copy of it to its first 64 bytes, and calls DetectDocumentText with the truncated blob: observe the correct UnsupportedDocumentException / "Request has unsupported document format" refusal (this part already matches AWS).
3. Observe the positive control that follows: DetectDocumentText with the UNCORRUPTED PNG returns HTTP 200, and the joined LINE Block.Text values are '[|N[EUW[NT' instead of containing FINE and DOCUMENT.
4. Generalize the check by rendering a different ASCII string; the reported text is garbled for that string too, confirming the defect is in glyph recognition and not tied to one rendered value.
5. Re-replay textract.detect-document-text-corrupt-image-rejected after the fix; both the refusal and the positive control must pass.
