Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: textract.analyze-document-combined-features-tables-and-forms
Issue-Shape-Id: com.amazonaws.textract#Textract
Issue-Key: textract.analyze-document-combined-features-tables-and-forms|run#14|AssertionError|assert any(expected in t for t in cell_texts), (
Issue-Commit: 03df40548cc183621db41b1021feb6ebc20165db

# Textract AnalyzeDocument TABLES: CELL blocks carry garbled text and the table's ruling lines are read as word text, so no cell reports the rendered cell content

> _Note: this issue was identified against the code base at commit `03df40548cc183621db41b1021feb6ebc20165db`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AnalyzeDocument

## Description
A single AnalyzeDocument call with FeatureTypes=["TABLES","FORMS"] over a rasterised page that draws a 2x2 ruled grid (header row "Item" / "Price", data row "Widget" / "10.00") plus a "Name: Test User" line now does produce a TABLE block with at least four CELL children — the TABLE-detection failure that the open issue 20261029T184456Z-textract-document-analysis-returns-garbled-ocr-text-and.md describes as its Cause 3 no longer reproduces on this check, and that issue's recorded replay key for this check (`assert table_blocks, "no TABLE block detected in the rendered grid"`) is no longer the assertion that fails, so it does not cover what fails now. The check now fails further on, on the CONTENT of the cells. This issue bundles 2 distinct root causes, both in the textract crate; a fixer must close BOTH before submitting.

Cause 1 of 2 — the glyph recogniser misreads rendered ASCII, so a CELL's CHILD WORD blocks do not spell the text drawn in that cell.
AWS: Smithy (services/service-com-amazonaws-textract-textract/smithy-model/com-amazonaws-textract-textract.json): AnalyzeDocument input {Document required, FeatureTypes required, HumanLoopConfig, QueriesConfig, AdaptersConfig}, output {DocumentMetadata, Blocks, HumanLoopActivationOutput, AnalyzeDocumentModelVersion}. Block$Text is documented as "The word or line of text that's recognized by Amazon Textract"; BlockType enum contains TABLE, CELL, WORD, LINE; RelationshipType contains CHILD; Block$RowIndex/$ColumnIndex are documented 1-based. AnalyzeDocument's documentation states "A TABLE Block object contains information about a detected table. A CELL Block object is returned for each cell in a table." The requirement is therefore that for ANY text drawn into a grid cell, the WORD blocks reached through that CELL's CHILD relationship spell exactly the characters drawn there — not the particular strings of one recording.
Emulator: the structure is right (a TABLE with CELL children, 1-based indices, Geometry and Confidence present) but every recognised string is corrupt. The cells' concatenated CHILD-word texts came back as ['| .`..', '| .[..', '|', '| .....`', '| ]O.OO', '|'] where the drawn cells are "Item", "Price", "Widget", "10.00": "Item" was read as ".`..", "Price" as ".[..", "Widget" as ".....`", "10.00" as "]O.OO". This is the same per-glyph misclassification the open issue above describes for the plain-text paths, reached here through the TABLES path; it is one shared recogniser, so the cells inherit it.
Why: a client that asks for TABLES does so to read the cells. It gets HTTP 200 with a structurally valid TABLE/CELL tree that passes its schema and index checks, yet every cell string is characters that were never on the page, so its row parsing, column matching and numeric extraction silently operate on garbage. Nothing in the response signals a failure.
Fix: the glyph classification and word segmentation in the service's OCR module (crate::state::ocr, reached from crate::state::read_submitted_document / read_content), which is upstream of the block derivation in crate::state::blocks — rendered ASCII glyphs must resolve to the characters actually drawn before any WORD text is emitted.
Traces: textract.analyze-document-combined-features-tables-and-forms.

Cause 2 of 2 — the table's vertical ruling lines are recognised as word text ('|') and are segmented as table content, so each row gains a spurious column and every cell's text is polluted by the rule.
AWS: Smithy: BlockType includes TABLE, CELL and MERGED_CELL; Block$ColumnIndex is documented "The column in which a table cell appears. The first column position is 1." AWS's documentation of Block describes a Block as "items that are recognized in a document" and the recogniser reports detected WORDs — a drawn rule is table STRUCTURE, used to delimit cells, and is not reported as a WORD or as a cell's text. For a 2-column by 2-row ruled grid AWS returns four CELLs with ColumnIndex in {1,2} and RowIndex in {1,2}, each cell's CHILD words being only the content words inside that cell. The emulator's own module documentation agrees that a rule is page content that is not a character (services/service-com-amazonaws-textract-textract/src/state.rs, module docs: the reader "reports nothing for ink it could not match to a glyph" and a PAGE's geometry covers "a rule, a border or a logo").
Emulator: the drawn vertical rules are matched to the '|' glyph and emitted as word text, and that text is then fed into the row/cell segmentation. Each row is segmented into three columns instead of two, two of the six returned cells have '|' as their ENTIRE text ('|' with no content words at all), and every populated cell's text begins with '| '. Column segmentation in crate::state is driven by tab characters in the recognised line text (table_runs: "each one a contiguous run of tab-separated lines"; cell_texts: `line.text.split('\t')`; LineLayout::cells documented as "Word counts per tab-separated cell"), so the rule glyphs land inside cells as ordinary words and a leading rule creates an extra leading column. The same segmentation feeds AnalyzeExpense's line_item_groups / line_item, so a ruled receipt table is mis-columned there too.
Why: a client reading a scanned ruled table gets the wrong number of columns, cells whose only content is a border artefact, and a border character prefixed to every cell value — so even with correct glyph recognition its header-to-column mapping and per-cell values would be wrong, and ColumnIndex no longer identifies the column a human sees. Again this is reported as a successful 200.
Fix: two components — the ink classification in crate::state::ocr must not emit a long thin rule as a '|' character glyph, and the table segmentation in crate::state (table_runs / cell_texts / cell_words / cell_boxes / table_blocks, shared with crate::state::line_item_groups) must derive a row's columns from the ruling lines and the recognised words' geometry on a raster page rather than from tab characters in the line text, emitting one CELL per drawn cell with 1-based RowIndex/ColumnIndex and CHILD relationships to only that cell's content words.
Traces: textract.analyze-document-combined-features-tables-and-forms.

## Evidence
Suite simparity-behavioral-aws-v1 replayed against the emulator booted from this checkout: 16/26 passed, 8 mismatched, 2 errored, 0 skipped, 0 unloadable. Per-service: textract 16/24 (67%), cloudformation__textract 0/2. bootstrap_signal was false — the emulator was serving and textract answered HTTP 200 on this call.

[mismatch] textract.analyze-document-combined-features-tables-and-forms
Request: AnalyzeDocument(Document={"Bytes": <PNG>}, FeatureTypes=["TABLES","FORMS"]). The PNG renders "Name: Test User" and a 2-column x 2-row grid drawn with 2px ruling lines, header cells "Item" and "Price", data cells "Widget" and "10.00".
Response: HTTP 200. The assertions that PASSED: a TABLE block exists; the TABLE's CHILD relationship resolves to >= 4 CELL blocks.
The assertion that FAILED (check_textract_analyze_document_combined_features_tables_and_forms.py line 46):
  AssertionError: expected a table cell containing 'ITEM' rendered into the grid, got ['| .`..', '| .[..', '|', '| .....`', '| ]O.OO', '|']
The list is each CELL's CHILD WORD texts joined with ' ' and upper-cased, in TABLE CHILD order. Reading it against the drawn page:
  - six cells are returned for a four-cell grid (three columns per row instead of two);
  - cells 3 and 6 contain only '|' — the row's trailing ruling line, with no content word;
  - every populated cell is prefixed by '| ' — a ruling line read as a word;
  - the content strings are garbled: 'Item' -> '.`..', 'Price' -> '.[..', 'Widget' -> '.....`', '10.00' -> ']O.OO'.
None of 'ITEM', 'PRICE', 'WIDGET' or '10.00' appears in any cell. The failure is in response content only: no error code, no non-2xx status, and the block structure/index/geometry assertions did not fail. (Values such as the exact garbled strings are one run's output; the requirement is that a cell's words spell whatever was drawn in that cell and that rules are not reported as words.)

Still open and NOT covering this: 20261029T184456Z-textract-document-analysis-returns-garbled-ocr-text-and.md lists this check id, but its replay key for it is `assert table_blocks, "no TABLE block detected in the rendered grid"`, an assertion that now passes; the cell-content assertion above is a different failure point and is not carried by any open issue.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the simparity-behavioral-aws-v1 behavioral suite, textract group.
2. Run textract.analyze-document-combined-features-tables-and-forms: render a PNG containing a line "Name: Test User" and a 2x2 table drawn with ruling lines whose cells contain "Item", "Price" (header row) and "Widget", "10.00" (data row), then call AnalyzeDocument(Document={"Bytes": png}, FeatureTypes=["TABLES","FORMS"]).
3. Collect the TABLE block, follow its CHILD relationship to the CELL blocks, and for each CELL follow its CHILD relationship to the WORD blocks and join their Text.
4. Cause 2: observe six CELL blocks instead of four, two of them whose only word text is '|', and a '| ' prefix on every other cell — the drawn ruling lines are being reported as words and as a column. Expected: exactly one CELL per drawn cell, RowIndex/ColumnIndex in 1..2, and each cell's CHILD words limited to that cell's content words.
5. Cause 1: observe the content strings are not the drawn text ('.`..' for 'Item', ']O.OO' for '10.00'). Expected: each cell's joined CHILD-word text equals the characters drawn in that cell, for any text drawn.
6. Re-replay textract.analyze-document-combined-features-tables-and-forms after the fix; it must pass, including the later KEY_VALUE_SET / 'NAME' key assertions in the same check.
