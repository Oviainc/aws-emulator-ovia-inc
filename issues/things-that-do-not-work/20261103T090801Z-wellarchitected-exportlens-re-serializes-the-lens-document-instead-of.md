Issue-Source: trace-sweep-divergence
Issue-Trace: wellarchitected-custom-lens-import-version-lifecycle
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: wellarchitected-custom-lens-import-version-lifecycle
Issue-Shape-Id: com.amazonaws.wellarchitected#WellArchitectedApiServiceLambda
Issue-Key: wellarchitected|ExportLens|2xx->2xx|$.LensJSON
Issue-Commit: c1544a9590ded4504bea161b6ee6981cbcc184a7

# ExportLens re-serializes the lens document instead of exporting AWS's canonical form

> _Note: this issue was identified against the code base at commit `c1544a9590ded4504bea161b6ee6981cbcc184a7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExportLens

## Description
This issue has 1 cause, in service wellarchitected.

Cause 1 of 1: ExportLens returns a LensJSON string that is the emulator's own re-serialization of the stored lens document, rather than the canonical lens document AWS exports. Three things diverge at once, and all three are part of the member's value because LensJSON is a STRING, not a document: member order, separator spacing, and the per-question / per-choice `number` member.

AWS: ExportLens (Smithy: services/service-com-amazonaws-wellarchitected-wellarchitectedapiservicelambda/smithy-model/com-amazonaws-wellarchitected-wellarchitectedapiservicelambda.json - GET /lenses/{LensAlias}/export, code 200, input ExportLensInput{LensAlias @httpLabel @required @length 1..128, LensVersion @httpQuery @length 1..32}, output ExportLensOutput{LensJSON @length 2..500000, documented as "The JSON representation of a lens"}; errors AccessDeniedException 403, InternalServerException 500, ResourceNotFoundException 404, ThrottlingException 429, ValidationException 400). The model constrains LensJSON only as a bounded string, so the exact byte sequence is recorded behavior only. Recorded behavior: the exported document (a) orders the top-level members `name`, `schemaVersion`, `description`, `pillars` - the lens name FIRST, schema version second; (b) is serialized compactly, with NO space after `:` or `,`; and (c) materializes an integer `number` member on EVERY question object and EVERY choice object of every pillar, placed after that object's `id`. The values of `name`, `description`, the pillar/question/choice ids and titles, `helpfulResource`/`improvementPlan` display text and `riskRules` are the ones the lens was imported with; the `number` ordinals and the ordering/spacing are AWS's, produced by its canonical lens serializer.

Emulator: serves 200 with the right members and the right imported values, but a differently serialized document: top-level order is `schemaVersion`, `name`, `description`, `pillars` (schemaVersion hoisted ahead of name); every `:` and `,` is followed by a space (Python/serde pretty separators); and the `number` member is absent from every question and every choice object. So a byte- or string-comparison of LensJSON against AWS's export fails on every exported lens, custom or built-in, independently of the lens's content.

Why: LensJSON is a string a client handles as an opaque document - it is checksummed, diffed between lens versions, stored in source control, and fed straight back into ImportLens. A client that diffs two exports, or that compares an export against the document it imported, sees a spurious whole-document difference; a client that re-imports the export loses the `number` ordinals AWS assigns, so the round trip is not identity and question/choice ordering information is dropped. Any client that hashes the export to detect lens drift reports drift on every call.

Fix: the ExportLens handler's lens-document serializer - it must emit the canonical member order (`name` before `schemaVersion`), compact separators with no whitespace, and populate the `number` member on each question and choice from the lens record's ordering - plus whatever lens-document model the stored lens is rebuilt from, if that model has no `number` field to serialize.

Related open issues, none of which covers this step: 20261029T192731Z-wellarchitected-built-in-lens-catalog-content-and-arn-form.md (built-in lens catalog content and ARN form) and 20261029T223409Z-wellarchitected-custom-lens-publish-path-empty-selection-risk-derivation.md (custom-lens publish path). The earlier wire issue 20261029T192731Z-wellarchitected-custom-lens-import-publish-version-and-export-chain.md, which made this export path reachable at all, is RESOLVED - ExportLens now answers 200, and this is the content divergence that answer exposes.

Traces: wellarchitected-custom-lens-import-version-lifecycle#task#3.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator (12 fixtures, 94 steps, 61 passed, 33 mismatched, score 64.9%). This cluster: fixture wellarchitected-custom-lens-import-version-lifecycle, step task#3, operation ExportLens, verdict mismatch, divergence_key service=wellarchitected|ExportLens|status=200->200|keys=$.LensJSON. Status agrees (200 expected, 200 actual); the sole diverging key is $.LensJSON.

$.LensJSON: value_mismatch.

Expected (recorded AWS export, compact, `name` first, `number` present):
{"name":"<lensName>","schemaVersion":"2021-11-01","description":"<description>","pillars":[{"id":"<pillarId>","name":"<pillarName>","questions":[{"id":"<questionId>","number":0,"title":"<title>","description":"<description>","choices":[{"id":"<choiceId>","number":0,"title":"<choiceTitle>","helpfulResource":{"displayText":"<text>"},"improvementPlan":{"displayText":"<text>"}}, ...],"riskRules":[{"condition":"<choiceId>","risk":"NO_RISK"},{"condition":"default","risk":"HIGH_RISK"}]}]}]}

Actual (emulator, spaced separators, `schemaVersion` first, no `number`):
{"schemaVersion": "2021-11-01", "name": "<lensName>", "description": "<description>", "pillars": [{"id": "<pillarId>", "name": "<pillarName>", "questions": [{"id": "<questionId>", "title": "<title>", "description": "<description>", "choices": [{"id": "<choiceId>", "title": "<choiceTitle>", "helpfulResource": {"displayText": "<text>"}, "improvementPlan": {"displayText": "<text>"}}, ...], "riskRules": [{"condition": "<choiceId>", "risk": "NO_RISK"}, {"condition": "default", "risk": "HIGH_RISK"}]}]}]}

The three structural differences - member order, separator spacing, and the missing `number` on each question and each choice - are the requirement here. The concrete lens name, description, pillar/question/choice ids and display strings in the recording are the probe fixture's own per-run values filling that template and are NOT requirements; the recorded run used a generated lens name and the schemaVersion the import carried.

This cluster is neither blocked by an unimplemented operation (stub_path empty) nor owned by an open issue (owned_by empty) - it is the only net-new cluster in the 27 reported for this service.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay wire fixture wellarchitected-custom-lens-import-version-lifecycle and look at step task#3 (ExportLens).
3. Equivalently, by hand: PUT /importLens with a custom lens document (schemaVersion 2021-11-01, a name, a description and one pillar carrying one question with two choices and two riskRules), take LensArn from the response, publish a version with POST /lenses/{LensArn}/versions, then GET /lenses/{LensAlias}/export.
4. Compare the LensJSON string byte for byte with the document AWS exports: the emulator puts `schemaVersion` before `name`, writes `": "` / `", "` separators where AWS writes `":"` / `","`, and omits the `number` member that AWS emits on every question object and every choice object. The status (200) and every other member of the document agree.
5. Re-replay wellarchitected-custom-lens-import-version-lifecycle#task#3 to confirm a fix.
