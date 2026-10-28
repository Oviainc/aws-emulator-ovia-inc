Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: c15dfa091c78a47417078a336c74dcf21189cb0f

# ListEntities serves 50 entities a page when MaxResults is absent, not the documented default of 20

_Filed: 2026-10-27T09:08:03Z_
_Operation: ListEntities | Class: pagination_

> _Note: this issue was identified against the code base at commit `c15dfa091c78a47417078a336c74dcf21189cb0f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListEntitiesRequest$MaxResults` is documented as defaulting to 20 when the request does not provide it, and the member carries no `@default` trait, so nothing applies that default before the handler runs — the handler owns it. The handler instead falls back to `LIST_ENTITIES_PAGE_LIMIT` (50), i.e. it uses the modelled MAXIMUM as the default. A caller that lists entities without naming `MaxResults` therefore receives up to 50 `EntitySummary` items on one page and NO `NextToken` where the documented behaviour serves 20 items plus a `NextToken`. A client that pages until `NextToken` is absent sees a different page partitioning than documented, and a client that sizes buffers from the documented default can be handed 2.5x the elements it expects.

The sibling list in the same crate shows the intended treatment: `ListChangeSetsRequest$MaxResults` is documented "By default, this value is 20" and `LIST_CHANGE_SETS_PAGE_LIMIT` is 20, so for that operation the max and the documented default coincide and the same `None => <page limit>` fallback is correct. For `ListEntities` the two numbers differ (max 50, documented default 20) and the constant conflates them: its doc comment cites only the `@range`, never a default. `LIST_ASSESSMENTS_PAGE_LIMIT`'s comment in the same file even quotes the identical sentence ("If a value isn't provided, the default value is 20") for its own member, showing the crate reads that sentence as behaviour it must honour — it just relies on the boundary applying the modelled `@default` there, which `ListEntities$MaxResults` does not have.

## Evidence

Smithy model, `ListEntitiesRequest$MaxResults` (smithy_query query="doc" shape="ListEntitiesRequest" member="MaxResults"): "Specifies the upper limit of the elements on a single page. If a value isn't provided, the default value is 20." The same query over the operation shows no `@default` on the member: `"MaxResults": {"target": "ListEntitiesMaxResultInteger", "range": {"min": 1, "max": 50}, "type": "integer"}` (contrast `ListAssessments`/`DescribeAssessment`, whose `MaxResults` are `required` with a declared default per OPERATIONS.md:80 and :51).

Handler, services/service-com-amazonaws-marketplacecatalog-awsmpseymour/src/ops/list_entities.rs:375-376:
```
        let page_size = match input.max_results {
            None => crate::state::LIST_ENTITIES_PAGE_LIMIT,
```
with the comment above it at :370-374 claiming "an absent member serves the largest page the model allows".

Constant, services/service-com-amazonaws-marketplacecatalog-awsmpseymour/src/state.rs:77-79:
```
/// The same bound for `ListEntities`: `ListEntitiesRequest$MaxResults` is
/// `@range(min: 1, max: 50)`.
pub const LIST_ENTITIES_PAGE_LIMIT: i32 = 50;
```
(no default is cited — only the range).

Sibling, services/service-com-amazonaws-marketplacecatalog-awsmpseymour/src/state.rs:72-75:
```
/// The largest page `ListChangeSets` serves, and the page it serves when the
/// request names no `MaxResults`: `ListChangeSetsRequest$MaxResults` is
/// `@range(min: 1, max: 20)`.
pub const LIST_CHANGE_SETS_PAGE_LIMIT: i32 = 20;
```
whose value matches that member's documented default ("The maximum number of results returned by a single call. ... By default, this value is 20.").

Test coverage: no test pins the absent-`MaxResults` page size for `ListEntities`. tests/all/change_set_lifecycle.rs:1067-1135 pages both lists with explicit `MaxResults` ([1,2,3,7,20]) over 7 records; tests/all/entity_reads.rs:554-562 and tests/all/entity_type_projections.rs:178,482 always name `MaxResults`; tests/all/request_validation.rs:1103-1175 only checks that a value outside the modelled range is refused and a published bound accepted.

## Steps to Reproduce

1. Start enough APPLY change sets to create 25 `AmiProduct` entities and wait for them to reach SUCCEEDED (so 25 entities are stored).
2. POST /ListEntities with body {"Catalog": "AWSMarketplace", "EntityType": "AmiProduct"} — naming no MaxResults.
3. Observe `EntitySummaryList` holds 25 items and no `NextToken` is returned.
4. Expected per `ListEntitiesRequest$MaxResults` ("If a value isn't provided, the default value is 20"): 20 items plus a `NextToken` that resumes at the 21st.
5. Compare POST /ListChangeSets with {"Catalog": "AWSMarketplace"} and 25 change sets stored: it serves 20 and a NextToken, because LIST_CHANGE_SETS_PAGE_LIMIT equals that member's documented default.
