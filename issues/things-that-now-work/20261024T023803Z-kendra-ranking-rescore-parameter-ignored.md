Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: 047af81d647cd24bcec0ef3f34fd68e55311879a

# Rescore ignores Document$GroupId: grouped documents are scored and returned individually instead of being processed as one document

_Filed: 2026-10-24T02:38:03Z_
_Operation: Rescore | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `047af81d647cd24bcec0ef3f34fd68e55311879a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The modeled `Document` structure carries an optional `GroupId` member whose documentation states: "The optional group identifier of the document from the search service. Documents with the same group identifier are grouped together and processed as one document within the service." The emulator deserializes `GroupId` (it exists as `group_id: Option<String>` on the generated input type) but never reads it anywhere in the handler or state: scoring is done strictly per-document from that document's own `Title`/`TokenizedTitle` and `Body`/`TokenizedBody` text plus its own `OriginalScore`, and one `RescoreResultItem` is emitted per submitted document id with an independently computed `Score`.

Expected: when two or more submitted documents carry the same `GroupId`, the service treats them as a single document for ranking purposes — their text contributes to one scoring unit and the relevancy outcome for members of the group is consistent with being one document (i.e. group members do not get independent, text-divergent scores, and a group's position in the descending ordering is one position, not one per member).

Actual: `GroupId` is inert. A document whose body text alone carries no query terms but whose group sibling does is scored as if the sibling did not exist, so group members come back with unrelated scores and the descending ranking is computed over individual documents rather than groups. The parameter is silently dropped — a 200 is returned that is indistinguishable from the request having omitted `GroupId` entirely.

## Evidence

Spec/model grounding: `com.amazonaws.kendraranking#Document$GroupId` (length 1..2048) — doc: "Documents with the same group identifier are grouped together and processed as one document within the service." `RescoreResult$ResultItems` is documented as "A list of result items for documents with new relevancy scores. The results are in descending order."

Handler: services/service-com-amazonaws-kendraranking-awskendrarerankingfrontendservice/src/ops/rescore.rs — `handle` validates `Documents` (batch ceiling, Body/TokenizedBody exclusivity, Title/TokenizedTitle exclusivity) and then maps each document through `rescored_score(&query_terms, document)` (lines ~297-314); `rescored_score`, `field_terms`, `coverage` (lines ~364-397) read only `title`/`tokenized_title`/`body`/`tokenized_body`/`original_score`. No reference to `group_id` exists in the crate: `grep -r "group_id|GroupId"` matches only the generated type `src/ops/types/rescore_types.rs:26` and a test that passes `GroupId` purely as a bystander field in an invalid-document case (tests/all/wire_error_status.rs:351-352, which asserts the body-required refusal, not grouping).

Test coverage: none. No test in tests/all/ asserts any grouping behavior; the only `GroupId` occurrence is the refusal case above. The existing open Rescore issue (20261023T004037Z, gap_class wrong-behavior) is about the score blend magnitude/stopword dilution and explicitly assumes "one ResultItem per submitted DocumentId" — it does not mention `GroupId`, so this is a distinct (operation, class) gap.

## Steps to Reproduce

1. CreateRescoreExecutionPlan(Name='group-test') and poll DescribeRescoreExecutionPlan until Status == ACTIVE.
2. Call Rescore(RescoreExecutionPlanId=<plan>, SearchQuery='electric car batteries', Documents=[{Id:'chunk-1', GroupId:'doc-A', Title:'Electric vehicle batteries', Body:'Lithium ion batteries power electric cars.', OriginalScore:0.2}, {Id:'chunk-2', GroupId:'doc-A', Title:'Appendix', Body:'Table of contents.', OriginalScore:0.2}, {Id:'other', Title:'Unrelated', Body:'Gardening tips.', OriginalScore:0.2}]).
3. Inspect ResultItems: the two documents sharing GroupId 'doc-A' come back with different Scores computed only from their own text (chunk-2 scores like an unrelated document), showing the group was never processed as one document.
4. Repeat the identical request with GroupId omitted from both chunks: the ResultItems are byte-for-byte identical, proving the member is dropped and never affects the response or any persisted/derived state.
5. Cleanup: DeleteRescoreExecutionPlan once the plan is ACTIVE.
