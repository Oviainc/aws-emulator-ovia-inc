Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 34e6e58e29d15417fc6ff05c31f4f08593fa6d9a

# Bootstrap the Amazon Translate emulator

_Filed: 2026-10-21T21:18:38Z_
_Operation: service-com-amazonaws-translate-awsshinefrontendservice-20170701 | Class: stub_

> _Note: this issue was identified against the code base at commit `34e6e58e29d15417fc6ff05c31f4f08593fa6d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

All 19 operations in `services/service-com-amazonaws-translate-awsshinefrontendservice-20170701/src/ops/` are unmodified scaffolds: they carry the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and answer HTTP 501 `NotImplemented`, and `src/state.rs` holds no fields (`// TODO: add service-specific state fields`). No client workflow works.

Implement a FIRST, SMALL happy-path slice: the custom-terminology lifecycle plus the core synchronous translate call. Protocol is `awsJson1_1` — `POST /` with `X-Amz-Target: AWSShineFrontendService_20170701.<OperationName>` (see README/OPERATIONS.md).

Happy path, in order:

1. **ImportTerminology** — input `Name` (required, len 1..256, pattern `^([A-Za-z0-9-]_?)+$`), `MergeStrategy` (required, `OVERWRITE`), `TerminologyData` (required: `File` blob, `Format` one of CSV|TMX|TSV, `Directionality` UNI|MULTI), `Description`, `EncryptionKey`, `Tags`. Must PARSE the uploaded file and STORE a terminology record; return `TerminologyProperties` computed from what was stored: `Name`, `Description`, `Arn` (`arn:aws:translate:<region>:<account>:terminology/<Name>/<revision>`), `SourceLanguageCode`, `TargetLanguageCodes`, `Format`, `Directionality`, `SizeBytes` (actual byte length of `File`), `TermCount` (actual parsed term count), `CreatedAt`/`LastUpdatedAt` timestamps. Re-importing the same `Name` with `MergeStrategy=OVERWRITE` must replace the terms and bump `LastUpdatedAt`, not create a duplicate.
2. **GetTerminology** — input `Name` (required), optional `TerminologyDataFormat`. Must return the STORED `TerminologyProperties` for that name (same values ImportTerminology reported, including `TermCount`/`SizeBytes`) plus a `TerminologyDataLocation` (`RepositoryType`, `Location` presigned-style URL). Unknown `Name` → `ResourceNotFoundException` (HTTP 404), not 501 and not an empty 200.
3. **ListTerminologies** — input `NextToken`, `MaxResults` (1..500). Must return `TerminologyPropertiesList` containing every imported terminology, honouring `MaxResults` and returning an opaque `NextToken` that resumes exactly where the page ended (and omitting `NextToken` on the last page).
4. **TranslateText** — input `Text` (required, len 1..10000), `SourceLanguageCode`/`TargetLanguageCode` (required, len 2..5), optional `TerminologyNames`, `Settings`. Must return `TranslatedText` plus the echoed-back-but-resolved `SourceLanguageCode` (when `auto` is requested, resolve to a concrete detected code), `TargetLanguageCode`, `AppliedSettings` (the `Settings` actually applied), and `AppliedTerminologies` derived from the STORED terminology: for each named terminology that exists, the terms from it that actually matched `Text` (`AppliedTerminology{Name, Terms:[{SourceText, TargetText}]}`), with those target terms substituted into `TranslatedText`. A `TerminologyNames` entry that was never imported → `ResourceNotFoundException` (404). Source == target language or an unsupported pair → `UnsupportedLanguagePairException` (400). The translation itself may be a deterministic synthetic transform (e.g. a stable marker plus the source text) — that is acceptable for the emulator — but the terminology substitution and the applied-terminology report MUST come from persisted state.
5. **DeleteTerminology** — input `Name` (required); empty output. Must remove the record from state; deleting an unknown name → `ResourceNotFoundException` (404). After deleting, `GetTerminology` must 404 and `ListTerminologies` must no longer list it, and a `TranslateText` naming it must 404.

NON-NEGOTIABLE: every one of these five computes its response from persisted state. No canned bodies, no echoed-input-only responses, no generated ARN/`TermCount` without a stored record behind it. A create followed by a read must return what was created.

## State Model

Add to `ComAmazonawsTranslateAwsshinefrontendservice20170701State`:

- `terminologies: BTreeMap<String /* Name */, Terminology>` — insertion/name-ordered so `ListTerminologies` pagination is stable.
- `Terminology { name: String, description: Option<String>, arn: String, source_language_code: String, target_language_codes: Vec<String>, format: TerminologyDataFormat /* CSV|TMX|TSV */, directionality: Directionality /* UNI|MULTI */, encryption_key: Option<EncryptionKey>, size_bytes: i32, term_count: i32, skipped_term_count: i32, created_at: Timestamp, last_updated_at: Timestamp, raw_file: Vec<u8>, terms: Vec<TermEntry>, tags: Vec<Tag> }`
- `TermEntry { source_text: String, target_text: String, target_language_code: String }` — the parsed rows used both by `GetTerminology` reporting and by `TranslateText` substitution.
- `list_tokens: BTreeMap<String /* NextToken */, usize /* offset */>` (or an offset encoded in the opaque token) so `ListTerminologies` resumes correctly.

The remaining 14 operations (parallel-data CRUD, the async `StartTextTranslationJob`/`DescribeTextTranslationJob`/`ListTextTranslationJobs`/`StopTextTranslationJob` family, `TranslateDocument`, `ListLanguages`, and the Tag/Untag/ListTagsForResource trio) stay at 501 for this slice and are filed as individual gap issues later. Keep `terminologies[*].tags` and the ARN populated now so the tagging ops can be layered on without a state migration.

Done = an integration test running the workflow below passes with correct, stateful response bodies — not merely 2xx.

## Evidence

Spec: `smithy-model/com-amazonaws-translate-awsshinefrontendservice-20170701.json` via OPERATIONS.md — ImportTerminology (lines 63-67), GetTerminology (57-61), ListTerminologies (89-94, `@paginated(inputToken=NextToken, outputToken=NextToken)`), TranslateText (127-131), DeleteTerminology (39-43), including the declared errors `ResourceNotFoundException (HTTP 404)` and `UnsupportedLanguagePairException (HTTP 400)`. Shape detail from smithy_query: `TerminologyProperties` (Name, Description, Arn, SourceLanguageCode, TargetLanguageCodes, SizeBytes, TermCount, SkippedTermCount, CreatedAt, LastUpdatedAt, Directionality, Format, Message), `TerminologyData` (File required blob, Format required, Directionality), `AppliedTerminology` (Name, Terms), `TranslationSettings` (Formality, Profanity, Brevity). Implementation: `grep -rn '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches all 19 handler files, including `src/ops/import_terminology.rs`, `get_terminology.rs`, `list_terminologies.rs`, `translate_text.rs`, `delete_terminology.rs`; `src/state.rs` has zero fields. No tests exist beyond the generated `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI `translate` client at it.
2. ImportTerminology Name=demo-terms MergeStrategy=OVERWRITE TerminologyData={File: CSV bytes 'en,fr\nhello,bonjour\n', Format: CSV, Directionality: UNI} — expect 200 with TerminologyProperties.Name=demo-terms, TermCount=1, SizeBytes equal to the uploaded byte length, an ARN and CreatedAt. Currently: HTTP 501 NotImplemented.
3. GetTerminology Name=demo-terms — expect the same stored properties back plus a TerminologyDataLocation.
4. ListTerminologies MaxResults=1 — expect demo-terms in TerminologyPropertiesList; add a second terminology and confirm MaxResults=1 pages with a usable NextToken.
5. TranslateText Text='hello world' SourceLanguageCode=en TargetLanguageCode=fr TerminologyNames=[demo-terms] — expect 200 with TranslatedText containing 'bonjour' and AppliedTerminologies=[{Name: demo-terms, Terms: [{SourceText: hello, TargetText: bonjour}]}].
6. TranslateText with TerminologyNames=[no-such-terms] — expect ResourceNotFoundException (HTTP 404).
7. DeleteTerminology Name=demo-terms — expect 200 (empty body); then GetTerminology Name=demo-terms must return ResourceNotFoundException (404) and ListTerminologies must no longer include it.
8. DeleteTerminology Name=demo-terms a second time — expect ResourceNotFoundException (404).
