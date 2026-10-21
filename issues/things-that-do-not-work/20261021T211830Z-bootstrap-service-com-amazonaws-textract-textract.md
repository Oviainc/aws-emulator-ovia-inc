Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 34e6e58e29d15417fc6ff05c31f4f08593fa6d9a

# Bootstrap the textract emulator

_Filed: 2026-10-21T21:18:30Z_
_Operation: service-com-amazonaws-textract-textract | Class: missing_

> _Note: this issue was identified against the code base at commit `34e6e58e29d15417fc6ff05c31f4f08593fa6d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

All 25 operations in `services/service-com-amazonaws-textract-textract/src/ops/` are unmodified scaffolds: each carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` -> HTTP 501, and the service holds no state. Implement a FIRST minimal happy-path slice of Amazon Textract's document-extraction loop (awsJson1_1, `POST /` with `X-Amz-Target: Textract.<Operation>`, per README/OPERATIONS.md) so a realistic client script works end to end.

### Happy path (implement these 6, in this order)

1. **DetectDocumentText** (sync OCR) — Input: `Document` (required; `Document.Bytes` base64 blob OR `Document.S3Object{Bucket,Name,Version}`). Output: `DocumentMetadata{Pages}`, `Blocks` (list<Block>), `DetectDocumentTextModelVersion`. Must register/derive the document in state and return a PAGE block plus LINE and WORD blocks with unique `Id`, `BlockType`, `Text`, `Confidence`, `Geometry`, `Page`, and PAGE->LINE->WORD `Relationships` of type `CHILD`. Per the Block docs, `RowIndex`/`ColumnIndex`/`RowSpan`/`ColumnSpan`/`EntityTypes` must NOT be returned by DetectDocumentText/GetDocumentTextDetection. Blocks must be derived deterministically from the stored document identity (same input -> same blocks), never a hardcoded constant body.
2. **AnalyzeDocument** (sync analysis) — Input: `Document` (required), `FeatureTypes` (required, list<FeatureType>). Output: `DocumentMetadata`, `Blocks`, `AnalyzeDocumentModelVersion`. The returned block set must depend on the requested `FeatureTypes`: `FORMS` adds `KEY_VALUE_SET` blocks with `EntityTypes` KEY/VALUE and `CHILD`/`VALUE` relationships; `TABLES` adds `TABLE` + `CELL` blocks carrying `RowIndex`/`ColumnIndex`; neither feature requested -> only PAGE/LINE/WORD. Reject an empty/unknown `FeatureTypes` with `InvalidParameterException` (HTTP 400).
3. **StartDocumentTextDetection** (async start) — Input: `DocumentLocation` (required, `S3Object`), optional `ClientRequestToken`, `JobTag`, `NotificationChannel`, `OutputConfig`, `KMSKeyId`. Output: `JobId` (1..64, `^[a-zA-Z0-9-_]+$`). Must CREATE and persist a job record keyed by the generated JobId; repeating the same `ClientRequestToken` must return the same JobId (and `IdempotentParameterMismatchException` when the token is reused with a different document).
4. **GetDocumentTextDetection** (async result) — Input: `JobId` (required), `MaxResults`, `NextToken`. Output: `DocumentMetadata`, `JobStatus` (SUCCEEDED for the happy path), `Blocks`, `Warnings`, `StatusMessage`, `NextToken`, `DetectDocumentTextModelVersion`. Must look the job up in state: unknown/ill-formed JobId -> `InvalidJobIdException` (HTTP 400), never a hollow 200. Blocks must equal the blocks computed for that job's `DocumentLocation`, and `MaxResults`/`NextToken` must actually page the stored block list (`NextToken` absent on the last page).
5. **StartDocumentAnalysis** (async start) — same as (3) plus required `FeatureTypes` and optional `QueriesConfig`/`AdaptersConfig`; persist `FeatureTypes` on the job; Output `JobId`.
6. **GetDocumentAnalysis** (async result) — Input `JobId` (required), `MaxResults`, `NextToken`; Output `DocumentMetadata`, `JobStatus`, `Blocks`, `Warnings`, `StatusMessage`, `NextToken`, `AnalyzeDocumentModelVersion`. Blocks must reflect the `FeatureTypes` stored by StartDocumentAnalysis (FORMS/TABLES blocks present only when requested), paged the same way as (4).

Every one of the six must compute its response from persisted state — no canned responses, no echoing the input back, no random JobId that is never stored. A `Start*` followed by the matching `Get*` must return results for the document that was submitted; a `Get*` for a JobId that was never started must fail.

## State Model

Store in `ComAmazonawsTextractTextractState` (src/state.rs), reached via `crate::state::service_state(&*self.engine)`:

- `jobs: HashMap<String /*JobId*/, Job>` where
  `Job { job_id: String, kind: JobKind /*TextDetection | DocumentAnalysis*/, document: DocumentRef, feature_types: Vec<FeatureType>, client_request_token: Option<String>, job_tag: Option<String>, notification_channel: Option<NotificationChannel>, output_config: Option<OutputConfig>, kms_key_id: Option<String>, status: JobStatus /*IN_PROGRESS -> SUCCEEDED*/, status_message: Option<String>, warnings: Vec<Warning>, created_at: SystemTime, document_metadata: DocumentMetadata { pages: u32 }, blocks: Vec<Block>, model_version: String }`
- `idempotency: HashMap<String /*ClientRequestToken*/, String /*JobId*/>` — backs the Start* idempotency and `IdempotentParameterMismatchException`.
- `DocumentRef { s3: Option<S3Ref { bucket: String, name: String, version: Option<String> }>, bytes_digest: Option<String> }` — the stable key used to derive blocks deterministically for both sync and async paths.
- `page_tokens: HashMap<String /*NextToken*/, PageCursor { job_id: String, offset: usize }>` — opaque cursors for GetDocumentTextDetection/GetDocumentAnalysis paging.
- `Block { id: String, block_type: BlockType, text: Option<String>, confidence: f32, text_type: Option<TextType>, geometry: Geometry { bounding_box: BoundingBox, polygon: Vec<Point> }, page: u32, relationships: Vec<Relationship { kind, ids: Vec<String> }>, entity_types: Vec<EntityType>, row_index/column_index/row_span/column_span: Option<u32> }` — one shared block model used by all four read paths so sync and async results agree.

The remaining 19 operations (AnalyzeExpense, AnalyzeID, Start/GetExpenseAnalysis, Start/GetLendingAnalysis(+Summary), the Adapter CRUD set CreateAdapter/CreateAdapterVersion/GetAdapter/GetAdapterVersion/ListAdapters/ListAdapterVersions/UpdateAdapter/DeleteAdapter/DeleteAdapterVersion, and TagResource/UntagResource/ListTagsForResource) stay stubbed at 501 and will be filed as individual gap issues later.

## Evidence

OPERATIONS.md (crate root) lists 25 operations with protocol `awsJson1_1`, target prefix `Textract.`; it gives DetectDocumentText (Input `Document` required; Output `DocumentMetadata`/`Blocks`/`DetectDocumentTextModelVersion`), AnalyzeDocument (Input `Document`+`FeatureTypes` required), StartDocumentTextDetection and StartDocumentAnalysis (Input `DocumentLocation` required, Output `JobId` len 1..64 pattern ^[a-zA-Z0-9-_]+$, errors include `IdempotentParameterMismatchException`), and GetDocumentTextDetection/GetDocumentAnalysis (Input `JobId` required + `MaxResults`/`NextToken`, Output `JobStatus`/`Blocks`/`NextToken`, errors include `InvalidJobIdException`). `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-textract-textract/src/ops/` matches all 25 handler files, so every operation currently answers HTTP 501 and no state exists. Smithy shapes confirm the field names used above: `Document{Bytes,S3Object}`, `S3Object{Bucket,Name,Version}`, `DocumentLocation{S3Object}`, `DocumentMetadata{Pages}`, and `Block{BlockType,Text,Confidence,Geometry,Id,Relationships,EntityTypes,RowIndex,ColumnIndex,RowSpan,ColumnSpan,Page}` whose docs state RowIndex/ColumnIndex/EntityTypes are not returned by DetectDocumentText/GetDocumentTextDetection.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK Textract client at it.
2. Call DetectDocumentText with {"Document": {"Bytes": "<base64 png>"}} — expect HTTP 200 with DocumentMetadata.Pages >= 1 and Blocks containing a PAGE block plus LINE/WORD blocks with unique Ids and CHILD relationships; actual: HTTP 501 NotImplemented.
3. Call DetectDocumentText again with the identical document — expect the same block set (deterministic from state).
4. Call AnalyzeDocument with the same Document and FeatureTypes ["FORMS","TABLES"] — expect KEY_VALUE_SET (EntityTypes KEY/VALUE) and TABLE/CELL blocks in addition to PAGE/LINE/WORD; call it again with FeatureTypes [] — expect InvalidParameterException (HTTP 400).
5. Call StartDocumentTextDetection with {"DocumentLocation":{"S3Object":{"Bucket":"docs","Name":"invoice.pdf"}},"ClientRequestToken":"tok1"} — expect HTTP 200 with a JobId matching ^[a-zA-Z0-9-_]{1,64}$; repeat the same call and expect the SAME JobId.
6. Call GetDocumentTextDetection with that JobId — expect JobStatus SUCCEEDED, DocumentMetadata, and the same Blocks computed for docs/invoice.pdf; then call it with MaxResults=2 and follow NextToken until absent, expecting the pages to concatenate to the full block list with no duplicates.
7. Call GetDocumentTextDetection with JobId "doesnotexist" — expect InvalidJobIdException (HTTP 400), not a 200 with empty Blocks.
8. Call StartDocumentAnalysis with the same DocumentLocation and FeatureTypes ["TABLES"], then GetDocumentAnalysis with the returned JobId — expect JobStatus SUCCEEDED, AnalyzeDocumentModelVersion set, and TABLE/CELL blocks present (and absent when only ["FORMS"] was requested).
