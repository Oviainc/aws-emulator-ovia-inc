Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: c441cdb97fe099c25286b36cfc473da57b1ed5b6

# A file-upload inquiry whose title renders to nothing owns zero queries and ends FAILED with "An internal error occurred", contradicting materialise's own documented rule

_Filed: 2026-10-29T00:01:41Z_
_Operation: CreateComplianceInquiry | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `c441cdb97fe099c25286b36cfc473da57b1ed5b6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ComplianceInquiry::materialise` derives the inquiry's terminal status from its queries, and its doc states the ONE reason an inquiry can own no query: "an inquiry that owns NO query never got as far as one - a submission with nothing to process". The code under that doc breaks the rule on two sides.

(1) An upload that DID carry something to process can still end up owning zero queries. `InquiryContentRecord::work_units` returns exactly one unit for a file submission with non-empty bytes and no `fileSections` - the inquiry's own title (state.rs:852-854) - and `enumerate_queries` then DROPS that unit when `reportable_query_text` cannot render it inside `QuerySummary$query`'s bounds (create_compliance_inquiry.rs:378-382). `InquiryName` is modelled as a bare string with NO @length and NO @pattern, so `name` may legitimately be `""` (or `"<>"`, which the `<`/`>` filter at state.rs:389 reduces to the empty string). The result for such a request is an inquiry that was accepted 202/`PROCESSING` with a real document attached, and which ~`INQUIRY_PROCESSING_SPAN` later reports `status=FAILED` / `statusMessage="An internal error occurred while processing the inquiry. Try again at a later time."` (state.rs:1073-1075) and an empty `queries` list from `ListComplianceInquiryQueries` - even though nothing about the submission was empty and the handler raised no error. The emulator reports an internal failure for a submission it accepted and persisted intact.

(2) The sibling branch the same doc defends is dead. The doc bullet claims a per-query failure is what selects the inquiry-level "One or more queries encountered errors" message, "so the two completion values are told apart by the queries rather than one of them being unreachable". But `QUERY_STATUS_FAILED` is never ASSIGNED anywhere in the crate - it only appears in the read at state.rs:1072 - and `QUERY_MESSAGE_INTERNAL_ERROR` (state.rs:347) is referenced nowhere at all, so `any_query_failed` is always false and `INQUIRY_MESSAGE_COMPLETE_WITH_QUERY_ERRORS` (state.rs:1081) is exactly the unreachable completion value the comment says it is not.

Note on overlap: the fixed issue 20261024T030858Z covers file uploads owning zero queries in general (uploads WITH sections, or with a reportable title, now enumerate queries correctly). This is the residual input class that fix left behind plus the dead sibling branch; it is not a request to report a query outside the modelled bounds of `QuerySummary$query`.

## Evidence

Model (smithy_query shape=InquiryName): `{"shape_id": "com.amazonaws.artifact#InquiryName", "type": "string", "sensitive": true}` - no `length`, no `pattern`, so an empty `name` satisfies every modelled constraint; `CreateComplianceInquiryRequest$name` is only `required`.

Repo side A - the rule, src/state.rs:1040-1042 (doc on `materialise`):
  "* an inquiry that owns NO query never got as far as one - a submission with nothing to process (see [`InquiryContentRecord::work_units`]) - and ends in the modelled FAILED state."
and src/state.rs:1073-1075:
  "if self.queries.is_empty() {\n    self.status = INQUIRY_STATUS_FAILED.to_string();\n    self.status_message = INQUIRY_MESSAGE_INTERNAL_ERROR.to_string();"

Repo side A' - the code that breaks it, src/state.rs:849-856 (`work_units`):
  "Self::File { sections, content } => { if content.is_empty() { Vec::new() } else if sections.is_empty() { vec![title.to_string()] } else { sections.clone() } }"
src/ops/create_compliance_inquiry.rs:378-382:
  "let texts: Vec<String> = content\n        .work_units(title)\n        .iter()\n        .filter_map(|unit| crate::state::reportable_query_text(unit))\n        .collect();"
src/state.rs:386-392 (`reportable_query_text`): "let text: String = label.chars().filter(|character| *character != '<' && *character != '>').take(QUERY_TEXT_LIMIT).collect(); (!text.is_empty()).then_some(text)"
So a non-empty document + no `fileSections` + `name = ""` yields `queries == []`, i.e. the FAILED/internal-error branch for a submission that carried bytes.

Repo side B - the dead branch, src/state.rs:1037-1039 (doc on `materialise`):
  "* a query that reached `FAILED` is what selects the inquiry-level \"One or more queries encountered errors\" message, so the two completion values are told apart by the queries rather than one of them being unreachable;"
against the only occurrences of the constant: src/state.rs:338 (`pub const QUERY_STATUS_FAILED: &str = "FAILED";`) and src/state.rs:1069-1072 (`.any(|query| query.status == QUERY_STATUS_FAILED)`) - a read, never a write; `materialise` assigns only `QUERY_STATUS_COMPLETED` (state.rs:1061) or leaves `QUERY_STATUS_PROCESSING` (state.rs:1058-1059), and `CreateComplianceInquiry` creates every query `QUERY_STATUS_PROCESSING` (create_compliance_inquiry.rs:390). `QUERY_MESSAGE_INTERNAL_ERROR` (state.rs:347) has no reference in the crate.

Test coverage: tests/all/compliance_inquiry_lifecycle.rs:42-49 pins the `InquiryStatusMessage` enum set and lines ~301-339 pin the file-upload terminal statuses; tests/all/compliance_inquiry_file_submissions.rs covers file submissions. No case submits an empty `name`, and no case asserts the "One or more queries encountered errors" message, so neither symptom is pinned by a test.

## Steps to Reproduce

1. POST /v1/compliance-inquiry/create with body {"name":"","inquiryContent":{"fileContent":{"content":"cXVlc3Rpb25uYWlyZQ=="}}} - observe 202 with status PROCESSING and inputSource FILE, so the submission was accepted and its bytes persisted.
2. Wait past INQUIRY_PROCESSING_SPAN (~1s), then GET /v1/compliance-inquiry/getMetadata?complianceInquiryId=<id> - observe status FAILED and statusMessage "An internal error occurred while processing the inquiry. Try again at a later time." for a submission that carried a document.
3. GET /v1/compliance-inquiry/listQueries?complianceInquiryId=<id> - observe an empty `queries` list, where work_units documents one unit of work (the document as a whole) for an upload naming no fileSections.
4. Repeat step 1 with "name":"<>" - identical outcome, because reportable_query_text drops `<` and `>` and then refuses the empty remainder (state.rs:389-392).
5. grep -n 'QUERY_STATUS_FAILED' services/service-com-amazonaws-artifact-artifact/src - only the declaration (state.rs:338) and the read (state.rs:1072) appear, so the INQUIRY_MESSAGE_COMPLETE_WITH_QUERY_ERRORS branch at state.rs:1079-1081 and QUERY_MESSAGE_INTERNAL_ERROR (state.rs:347) can never be observed by any caller.
