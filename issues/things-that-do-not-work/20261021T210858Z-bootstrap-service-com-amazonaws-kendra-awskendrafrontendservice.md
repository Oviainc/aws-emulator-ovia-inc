Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 7b44f3f32d1f58d662874eb8ca0f546e296cee00

# Bootstrap the kendra emulator

_Filed: 2026-10-21T21:08:58Z_
_Operation: service-com-amazonaws-kendra-awskendrafrontendservice | Class: missing_

> _Note: this issue was identified against the code base at commit `7b44f3f32d1f58d662874eb8ca0f546e296cee00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 66 operations in `services/service-com-amazonaws-kendra-awskendrafrontendservice/src/ops/` is an untouched scaffold: each carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))`, i.e. HTTP 501 `NotImplemented`, and the crate holds no state. There is no working emulator at all.

This issue scopes the FIRST buildable slice only: the Amazon Kendra core index-and-search loop, implemented statefully end-to-end. Protocol is `awsJson1_1`, `POST /` with `X-Amz-Target: AWSKendraFrontendService.<OperationName>` (per OPERATIONS.md "Wire protocol").

Happy path to implement, in order:

1. **CreateIndex** — input `Name` (required, 1..1000, `^[a-zA-Z0-9][a-zA-Z0-9_-]*$`), `RoleArn` (required), optional `Edition` (DEVELOPER_EDITION | ENTERPRISE_EDITION | GEN_AI_ENTERPRISE_EDITION), `Description`, `Tags`, `UserContextPolicy`, `ClientToken` (@idempotencyToken). Output `Id` — must be a freshly generated 36-char id matching `^[a-zA-Z0-9][a-zA-Z0-9-]*$` (a UUID works) that is STORED, not just returned. Record `CreatedAt`/`UpdatedAt` and `Status`.
2. **DescribeIndex** — input `Id` (required). Output must be read back from the stored index: `Id`, `Name`, `Edition`, `RoleArn`, `Description`, `Status`, `CreatedAt`, `UpdatedAt`, `UserContextPolicy`, and `IndexStatistics` (`FaqStatistics`, `TextDocumentStatistics` — the text-document count must reflect documents actually put via BatchPutDocument). Unknown `Id` -> `ResourceNotFoundException` (HTTP 404), not 501 and not an empty 200.
3. **ListIndices** — paginated (`NextToken` in/out, `MaxResults` 1..100). Output `IndexConfigurationSummaryItems` of `IndexConfigurationSummary{Name, Id, Edition, CreatedAt, UpdatedAt, Status}` built from the stored index map; every created index appears exactly once, deleted ones do not. `MaxResults` must truncate and emit a `NextToken` that resumes without duplicating or dropping entries.
4. **BatchPutDocument** — input `IndexId` (required), `Documents` (required, 1..10 of `Document{Id (required), Title, Blob, S3Path, Attributes, AccessControlList, ContentType}`), optional `RoleArn`. Documents must be persisted into the named index keyed by `Document.Id` (same id replaces the previous version, per the model's "each document ID must be unique per index"). Output `FailedDocuments` — empty on success, and populated with `{Id, ErrorCode, ErrorMessage}` for documents that cannot be accepted. Unknown `IndexId` -> `ResourceNotFoundException` (404).
5. **Query** — input `IndexId` (required), `QueryText`, `PageNumber`, `PageSize`, `RequestedDocumentAttributes`, `QueryResultTypeFilter`. Must SEARCH the documents stored by BatchPutDocument (a simple case-insensitive substring/term match over `Title` + decoded `Blob` text is acceptable for the first slice) and return `ResultItems` of `QueryResultItem{Id, Type: DOCUMENT, DocumentId, DocumentTitle (TextWithHighlights), DocumentExcerpt (TextWithHighlights), DocumentAttributes, ScoreAttributes}`, plus a generated `QueryId` and a `TotalNumberOfResults` equal to the number of matches before `PageSize`/`PageNumber` slicing. A query for a term present in an indexed document must return that document; a query for an absent term must return zero results — never a canned result list.
6. **DeleteIndex** — input `Id` (required). Must remove the index and its documents from state; a subsequent DescribeIndex/Query on that `Id` must fail with `ResourceNotFoundException` (404), and ListIndices must no longer list it.

NEVER STUBS: each of these six must compute its response from persisted state held in the shared engine reached via `crate::state::service_state(&*self.engine)` (see README "State"). Hardcoded bodies, echoing the request back, or returning a random `Id` without storing anything are failures of this issue. Create-then-read must return what was created.

## State Model

In `src/state.rs` (`ComAmazonawsKendraAwskendrafrontendserviceState`):

- `indices: HashMap<String /* IndexId (36-char uuid) */, Index>`
- `Index { id: String, name: String, edition: String /* IndexEdition, default DEVELOPER_EDITION */, role_arn: String, description: Option<String>, status: String /* IndexStatus; ACTIVE for the emulator */, created_at: SystemTime, updated_at: SystemTime, user_context_policy: Option<String>, tags: Vec<Tag>, documents: HashMap<String /* Document.Id */, StoredDocument>, faq_count: u64 }`
- `StoredDocument { id: String, title: Option<String>, body_text: String /* decoded Blob */, content_type: Option<String>, attributes: Vec<DocumentAttribute>, access_control_list: Vec<Principal>, s3_path: Option<S3Path>, indexed_at: SystemTime }`
- `queries: HashMap<String /* QueryId */, QueryRecord { index_id, query_text, result_document_ids: Vec<String>, created_at }>` — so SubmitFeedback and paging can later resolve a QueryId.
- Index name uniqueness: a secondary `name -> index_id` lookup so a duplicate `Name` can raise `ResourceAlreadyExistException` (HTTP 400), and the `ClientToken` of CreateIndex can be recorded per index for idempotent retries.
- `IndexStatistics` is DERIVED on read: `TextDocumentStatistics.IndexedTextDocumentsCount = index.documents.len()`, byte size summed from `body_text`; `FaqStatistics.IndexedQuestionAnswersCount = index.faq_count`.

Everything else (data sources, FAQs, thesauri, experiences, featured results, query suggestions, principal mapping, Retrieve, tagging) stays 501 for now and will be filed as individual gap issues.

## Evidence

OPERATIONS.md for this crate (66 operations, `awsJson1_1`, `X-Amz-Target: AWSKendraFrontendService.<Op>`): CreateIndex (line 99) input `Name`/`RoleArn` required, output `Id` 36..36; DescribeIndex (line 195) input `Id`, output includes `Status`, `CreatedAt`, `UpdatedAt`, `IndexStatistics`; ListIndices (line 312) `@paginated(inputToken=NextToken, outputToken=NextToken)` with `MaxResults` 1..100 and `IndexConfigurationSummaryItems`; BatchPutDocument (line 57) `IndexId` + `Documents` 1..10 required, output `FailedDocuments`; Query (line 345) `IndexId` required, output `QueryId`/`ResultItems`/`TotalNumberOfResults`; DeleteIndex (line 141) input `Id`, empty output, `ResourceNotFoundException (HTTP 404)`. Shape detail from the Smithy model: `Document` requires `Id` (1..2048, documented unique per index) with `Title`/`Blob`/`S3Path`/`Attributes`/`ContentType`; `QueryResultItem` members `Id`/`Type`/`DocumentId`/`DocumentTitle`/`DocumentExcerpt`/`DocumentAttributes`/`ScoreAttributes`; `IndexConfigurationSummary` requires `CreatedAt`/`UpdatedAt`/`Status`; `IndexStatistics` requires `FaqStatistics` + `TextDocumentStatistics`. All-stub census: `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-kendra-awskendrafrontendservice/src/ops/` matches every one of the 66 handler files (e.g. `src/ops/create_index.rs:4`, `src/ops/query.rs:4`, `src/ops/batch_put_document.rs:4`). No tests exist beyond the generated `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK Kendra client at it.
2. CreateIndex(Name="demo-index", Edition="DEVELOPER_EDITION", RoleArn="arn:aws:iam::123456789012:role/kendra") -> expect 200 with a 36-char `Id`; today it returns HTTP 501 NotImplemented.
3. DescribeIndex(Id=<id>) -> expect `Name`="demo-index", `Edition`="DEVELOPER_EDITION", `Status`="ACTIVE", `CreatedAt`/`UpdatedAt` set, `IndexStatistics.TextDocumentStatistics.IndexedTextDocumentsCount`=0.
4. ListIndices() -> expect exactly one summary whose `Id` and `Name` match what CreateIndex returned; repeat with MaxResults=1 across two indices and verify the NextToken round-trip returns the second index without repeats.
5. BatchPutDocument(IndexId=<id>, Documents=[{Id:"doc-1", Title:"Kendra bootstrap guide", Blob: base64("the quick brown fox jumps over the lazy dog"), ContentType:"PLAIN_TEXT"}]) -> expect 200 with empty `FailedDocuments`.
6. DescribeIndex(Id=<id>) again -> expect `IndexStatistics.TextDocumentStatistics.IndexedTextDocumentsCount`=1.
7. Query(IndexId=<id>, QueryText="brown fox") -> expect a `QueryId`, `TotalNumberOfResults`>=1, and a `ResultItems` entry with `DocumentId`="doc-1" and a `DocumentExcerpt` drawn from the stored body.
8. Query(IndexId=<id>, QueryText="zzz-no-such-term") -> expect `ResultItems` empty and `TotalNumberOfResults`=0 (proves results are computed, not canned).
9. BatchPutDocument with IndexId="00000000-0000-0000-0000-000000000000" -> expect ResourceNotFoundException (HTTP 404).
10. DeleteIndex(Id=<id>) -> expect 200; then DescribeIndex(Id=<id>) -> expect ResourceNotFoundException (HTTP 404) and ListIndices() -> expect the index absent.
