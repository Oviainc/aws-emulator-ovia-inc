Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: f75cbb9a461527dc7539f53b0560573b007d83b1

# ListActions answers the last page with nextToken="", a value its own modelled NextToken forbids and which is refused when echoed back

_Filed: 2026-10-29T03:25:27Z_
_Operation: ListActions | Class: pagination_

> _Note: this issue was identified against the code base at commit `f75cbb9a461527dc7539f53b0560573b007d83b1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every paginated listing of this crate shares one page walk (`crate::state::page_of`), which returns `page.next_token: Option<String>` — `None` on the page that leaves nothing to resume from. Thirty-three of this crate's thirty-four listings pass that option straight through, so the final page carries no token. ListActions alone collapses the absence into the EMPTY STRING (`page.next_token.unwrap_or_default()`), because its generated output struct types the member as a non-optional `String`.

The empty string is not a value this member may take. The model documents it as "The token for the next set of results, or null if there are no additional results", and the member targets `NextToken`, whose own constraints are `length(min 1, max 4096)` and `pattern ^[A-Za-z0-9+/=]+$` — both of which the empty string violates. So the last page of every ListActions enumeration ships a token that is simultaneously (a) not the documented null, (b) outside the member's modelled value space, and (c) unusable: the SAME `NextToken` shape bounds the INPUT member, so a client that feeds the reported token back is refused by boundary validation before the handler runs, and if it did arrive the handler's own `page_of` resume lookup would not find the empty resume point and would answer 400 "The nextToken is not a token this operation issued." A paginator that treats a present token as "there are more pages" therefore never terminates cleanly on this one operation, while terminating correctly on the other thirty-three.

The crate pins the empty token in a test whose stated reason is only the member's required-ness ("The member is modelled REQUIRED, so the page with nothing left to resume from still carries it, as the empty token"). That reason does not address the member's documented null, nor the length/pattern constraints of the value it fills, which are what the emitted "" breaks. Fixing this means making the reported member absent on the final page (the output struct's `next_token` becoming optional, as every sibling listing's is), leaving the non-final pages and their tokens exactly as they are.

## Evidence

The model, which states both the documented value and the value space:
- `smithy_query(query="doc", shape="ListActionsResponse", member="nextToken")` -> "The token for the next set of results, or null if there are no additional results."
- `smithy_query(query="operation", operation="ListActions")` output member -> `"nextToken": {"target": "NextToken", "required": true, "length": {"min": 1, "max": 4096}, "pattern": "^[A-Za-z0-9+/=]+$"}` — and the INPUT member of the same operation carries the identical `"length": {"min": 1, "max": 4096}, "pattern": "^[A-Za-z0-9+/=]+$"`, so "" cannot be sent back.

The handler that emits it — services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/list_actions.rs:285-290:
```
            // "The token for the next set of results, or null if there are no
            // additional results", on a member the model marks REQUIRED: a page
            // that leaves nothing to resume from reports the empty token, which
            // is the only value a required token can take when there is no next
            // page.
            next_token: page.next_token.unwrap_or_default(),
```
and the type that forces it, src/ops/types/list_actions_types.rs:27-30:
```
pub struct ListActionsOutput {
    pub action_summaries: Vec<ActionSummary>,
    pub next_token: String,
}
```

The same page walk in its sibling listings, which report NO token on the last page (33 call sites, all of the form `next_token: page.next_token`):
- src/ops/list_executions.rs:295 — `next_token: page.next_token,` (the sibling listing over the very same action/execution records)
- src/ops/list_time_series.rs:292, src/ops/list_assets.rs:248, src/ops/list_interface_relationships.rs:220 — same line, same helper.

What happens to an echoed empty token inside this handler, src/ops/list_actions.rs:271-278:
```
        let page = crate::state::page_of(
            &selected,
            input.next_token.as_deref(),
            input.max_results,
            DEFAULT_MAX_RESULTS,
            |record| record.action_id.as_str(),
        )
        .ok_or_else(invalid_token)?;
```
with `invalid_token()` = "The nextToken is not a token this operation issued." (src/ops/list_actions.rs:305-307).

Test coverage: the behaviour is pinned, with a reason that speaks only to required-ness — tests/all/actions.rs:455-461:
```
        // The member is modelled REQUIRED, so the page with nothing left to
        // resume from still carries it, as the empty token.
        assert_eq!(
            body["nextToken"],
            json!(""),
            "case {target:?}: a complete page must carry the empty token, got {body}"
        );
```
and tests/all/actions.rs:370-380 normalises both encodings (`Some("") | None => None`), so the suite's own oracle already treats "nothing to resume from" as token-absent; only that one assertion demands the empty string. No test sends the reported token back.

## Steps to Reproduce

1. Create a computation model (CreateComputationModel) and take an action on it with ExecuteAction, so one action is recorded.
2. GET /actions?targetResourceType=COMPUTATION_MODEL&targetResourceId=<id> -> 200. Observed body: {"actionSummaries":[...],"nextToken":""} — a token on the page that has nothing left to resume from.
3. Expected, per ListActionsResponse$nextToken ("The token for the next set of results, or null if there are no additional results") and per the member's modelled NextToken constraints (length min 1, pattern ^[A-Za-z0-9+/=]+$, which the empty string fails): no nextToken at all, exactly as the 33 sibling listings of this crate answer (src/ops/list_executions.rs:295).
4. Follow the reported token the way an SDK paginator does: GET /actions?targetResourceType=COMPUTATION_MODEL&targetResourceId=<id>&nextToken= -> refused 400 (the input nextToken is the same length-1..4096 NextToken shape; were it to reach the handler, src/ops/list_actions.rs:271-278 answers InvalidRequestException "The nextToken is not a token this operation issued."). The enumeration therefore cannot be walked to completion by a client that follows a present token.
5. Compare GET /executions?targetResourceType=COMPUTATION_MODEL&targetResourceId=<id> (ListExecutions, same records, same page walk): the single page carries no nextToken and the walk terminates.
