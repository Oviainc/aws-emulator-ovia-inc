Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 047f25c8e675653717aa743423e962efe8d38250

# Default ListFlows page size is 1000 instead of the documented 10

_Filed: 2026-10-24T01:05:42Z_
_Operation: ListFlows | Class: pagination_

> _Note: this issue was identified against the code base at commit `047f25c8e675653717aa743423e962efe8d38250`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

ListFlows with no `maxResults` query parameter must page with a maximum of 10 flows per page and return a `nextToken` when more flows exist. The emulator instead returns up to 1000 flows in one page and omits `nextToken` for any account holding fewer than 1000 flows, so a client that relies on the service default never paginates and sees a page size AWS never produces.

Expected: `GET /v1/flows` (no maxResults) on an account with 12 flows -> `flows` holds the first 10 entries and `nextToken` names the ARN the 11th page entry resumes at.
Actual: `flows` holds all 12 entries and `nextToken` is absent.

The sibling list operations in this same crate already implement the rule from the identical model sentence (ListBridges, ListGateways both default to 10), so ListFlows is the lone outlier.

## Evidence

Spec: `smithy_query(query="doc",shape="ListFlowsRequest",member="MaxResults")` — "The service might return fewer results than the MaxResults value. If MaxResults is not included in the request, the service defaults to pagination with a maximum of 10 results per page." ListFlows is `@paginated(inputToken=NextToken, outputToken=NextToken, items=Flows, pageSize=MaxResults)`.

Handler: `src/ops/list_flows.rs:89` declares `const MAX_PAGE_SIZE: i32 = 1000;` documented as "the largest page ListFlows returns, AND the page size it uses when the request names none", and line 196-202 computes `page_size = input.max_results.unwrap_or(MAX_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE)` — i.e. the default page is 1000.

Contrast: `src/ops/list_bridges.rs:101` and `src/ops/list_gateways.rs:98` each declare `const DEFAULT_PAGE_SIZE: i32 = 10;` and use it in `unwrap_or(DEFAULT_PAGE_SIZE)`, and `tests/all/gateway_lifecycle.rs:507-525` asserts the no-MaxResults page holds exactly the first ten gateways plus a nextToken, quoting the same model sentence.

Test coverage: none. Every ListFlows pagination assertion passes an explicit `maxResults` (`tests/all/flow_lifecycle.rs:594-640` uses page sizes 1,2,3,7,1000; `tests/all/flow_lifecycle.rs:479,554,583` call `list_flows("")` only with 0-1 stored flows), so no test pins the default page size.

## Steps to Reproduce

1. Start the emulator and create 12 flows: POST /v1/flows with {"name":"flow-<i>","source":{"protocol":"rtp","whitelistCidr":"10.0.0.0/16"}} for i in 0..12
2. GET /v1/flows with no query string
3. Observe that the response lists all 12 flows and carries no `nextToken`, where AWS returns the first 10 flows plus a `nextToken`
4. GET /v1/bridges or /v1/gateways with no query string after creating 12 of those resources and observe that those operations do cap the default page at 10 — the inconsistency
