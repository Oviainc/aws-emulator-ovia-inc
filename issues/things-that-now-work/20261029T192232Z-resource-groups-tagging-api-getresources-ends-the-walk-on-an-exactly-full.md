Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: resourcegroupstaggingapi.get-resources-out-of-range-page-size-rejected
Issue-Shape-Id: com.amazonaws.resourcegroupstaggingapi#ResourceGroupsTaggingAPI_20170126
Issue-Key: resourcegroupstaggingapi.get-resources-out-of-range-page-size-rejected|run#20|AssertionError|assert full["PaginationToken"], (
Issue-Commit: 907b6e75f03592bc638357f3d5c791839011739f

# GetResources ends the walk on an exactly-full page: no PaginationToken is minted when the page consumes the last matching resource

> _Note: this issue was identified against the code base at commit `907b6e75f03592bc638357f3d5c791839011739f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResources

## Description
Cause 1 of 1: GetResources omits the continuation token on an exactly-full page (check resource-groups-tagging-api.get-resources-out-of-range-page-size-rejected).

AWS: GetResources' walk terminates on an EMPTY page, not on a full one. When a page returns exactly ResourcesPerPage items and that page happened to consume the last matching resource, the response still carries a non-empty PaginationToken; following that token returns a response whose ResourceTagMappingList is empty and whose PaginationToken is the empty string, which is what ends the walk. Only an UNDER-full page (fewer items than ResourcesPerPage) ends the walk immediately with an empty PaginationToken. Smithy: GetResources is @paginated {inputToken: PaginationToken, outputToken: PaginationToken, items: ResourceTagMappingList, pageSize: ResourcesPerPage}; GetResourcesOutput.PaginationToken is documented only as "A string that indicates that there is more data available than this response contains", and the model does not state the full-page rule — so the termination rule above is recorded/measured behavior only (the check documents it as measured against real AWS). Status is 200 throughout; no error is involved.

Emulator: mints a continuation token only when strictly more matching resources remain than the page took. With one matching resource and ResourcesPerPage=1 it returns that one resource and PaginationToken "", ending the walk one page early and never serving the empty terminal page. The under-full case (ResourcesPerPage=100 over 1 resource -> PaginationToken "") is already correct.

Why: a client calibrated on AWS cannot distinguish "this page is full and there may be more" from "this is the last page" the way AWS lets it. Any consumer that treats a full page as implying another request (a hand-rolled pager, a page-count assertion, a resumable scan that persists the token after each full page, or a diff of page boundaries) gets a different page sequence against the emulator than against AWS: against AWS a 1-of-1 scan is two responses (one full page + one empty terminal page), against the emulator it is one. Conversely a scan resumed from a persisted token will fail to resume because no token was ever issued.

Fix: the shared paging helper used by this operation's handler, and the handler's token emission — mint the resume token whenever the page was filled to its item cap, not only when further matches are known to remain, and serve the following empty page with an empty token.

Traces: resource-groups-tagging-api.get-resources-out-of-range-page-size-rejected. Code sites: src/state.rs:588-626 (crate::state::page — `let truncated = rest.len() > taken;` gates token minting, so next is None on an exactly-full page) and src/ops/get_resources.rs:406-422 (the page() call and `pagination_token: Some(next.unwrap_or_default())`).

Note: everything else this check grades passed — ResourcesPerPage=0 and ResourcesPerPage=101 are both refused with InvalidParameterException and the message "ResourcesPerPage is not in the valid range of 1 and 100" (src/state.rs:479-481), and both in-range edges return the tagged queue — so only the pagination-termination rule is implicated.

## Evidence
Behavioral replay of simparity-behavioral-aws-v1 (23/32 passed, emulator_up=true, bootstrap_signal=false). Verdict 'mismatch' on service resource-groups-tagging-api; not marked ALREADY FILED and not OWNED by any open issue.

check_resource_groups_tagging_api_get_resources_out_of_range_page_size_rejected.py, line 92:
  assert full["PaginationToken"]
  AssertionError: an exactly-full page (ResourcesPerPage=1 over 1 resource) ended the walk with an empty token; real AWS returns a continuation token here

Sequence the check executed before that point, all passing:
  1. create one SQS queue, TagResources it with a unique key, poll GetResources(TagFilters=[{Key:key}]) until the index reports exactly that one ARN.
  2. GetResources(..., ResourcesPerPage=0)   -> 400 InvalidParameterException, Message "ResourcesPerPage is not in the valid range of 1 and 100" (matches AWS).
  3. GetResources(..., ResourcesPerPage=101) -> same refusal (matches AWS).
  4. GetResources(..., ResourcesPerPage=1) and (..., ResourcesPerPage=100) both return exactly the one tagged queue ARN (matches AWS).
  5. GetResources(TagFilters=[{Key:key}], ResourcesPerPage=1) -> expected a non-empty PaginationToken (1 item filling a 1-item page); observed PaginationToken "" (empty).
The two assertions behind the failure were therefore never reached: that following the token yields ResourceTagMappingList == [] and PaginationToken == "".

Model evidence (services/service-com-amazonaws-resourcegroupstaggingapi-resourcegroupstaggingapi-20170126/smithy-model/com-amazonaws-resourcegroupstaggingapi-resourcegroupstaggingapi-20170126.json): GetResources paginated {inputToken PaginationToken, outputToken PaginationToken, items ResourceTagMappingList, pageSize ResourcesPerPage}; GetResourcesOutput.PaginationToken (PaginationToken, len 0..2048, pattern ^[\s\S]*$).

Source evidence:
  src/state.rs:616-623 — `let truncated = rest.len() > taken; ... let next = truncated.then(|| ...encode_pagination_token(scope, key))` — no token unless items remain beyond the page.
  src/ops/get_resources.rs:406-418 — the page() call, then `pagination_token: Some(next.unwrap_or_default())`, so a None next is serialized as the terminating empty string.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Create one SQS queue and read its QueueArn.
3. POST TagResources (X-Amz-Target ResourceGroupsTaggingAPI_20170126.TagResources) with {"ResourceARNList":["<queue arn>"],"Tags":{"<unique-key>":"sized"}}.
4. POST GetResources with {"TagFilters":[{"Key":"<unique-key>"}]} and confirm exactly one ResourceTagMapping is returned (the index has converged).
5. POST GetResources with {"TagFilters":[{"Key":"<unique-key>"}],"ResourcesPerPage":1}. Expect 200 with one mapping AND a NON-EMPTY PaginationToken; observe 200 with one mapping and PaginationToken "".
6. (AWS behavior to reproduce once fixed) POST GetResources again with the same filter, ResourcesPerPage 1 and that PaginationToken: expect 200 with ResourceTagMappingList == [] and PaginationToken == "".
7. Control that already matches AWS: POST GetResources with the same filter and ResourcesPerPage=100 — one mapping and PaginationToken "".

Replay check id: resource-groups-tagging-api.get-resources-out-of-range-page-size-rejected
