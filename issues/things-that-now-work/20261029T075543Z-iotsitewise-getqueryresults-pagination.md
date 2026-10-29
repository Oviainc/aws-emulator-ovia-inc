Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 97cefcb07ab7801eeb37fff053a06a3892425e6e

# A GetQueryResults nextToken is a bare row offset bound to no query, so a token minted for one query is honoured against another and silently hides that query's leading rows

_Filed: 2026-10-29T07:55:43Z_
_Operation: GetQueryResults | Class: pagination_

> _Note: this issue was identified against the code base at commit `97cefcb07ab7801eeb37fff053a06a3892425e6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

GetQueryResults pages a query's rows with `crate::ops::execute_query::decode_row_offset`, which decodes nothing but a number. The only check the handler then makes is a RANGE check against the result it is about to slice (`resume >= held`). Nothing in the token identifies the enumeration it came from, so a token minted while paging query A is accepted when replayed against a DIFFERENT query B of the same workspace — provided B's result holds more rows than the offset. B is then served from that offset onwards: B's leading rows appear in no page of B's enumeration and the caller is never told. The same token form is issued by ExecuteQuery (the codec is shared and documented as shared), so an ExecuteQuery token is honoured here too.

The crate's third paged reader does exactly the missing check and states the rule for the whole crate: BatchGetAssetPropertyValue carries a FINGERPRINT of the request beside the position "so a token is only honoured against the request that produced it", and a test pins the refusal. GetQueryResults' own refusal message claims "The nextToken is not a token this operation issued." while the check it guards cannot tell such a token apart.

This is the half the open ExecuteQuery pagination issue does not reach: that issue cites GetQueryResults as the stricter sibling for the at-the-end offset (which GetQueryResults does refuse), and its fix — binding ExecuteQuery's token to the statement — leaves GetQueryResults' token bound to nothing, since here the enumeration is identified by the (workspaceName, queryId) pair rather than by a statement in the request.

## Evidence

The whole of this operation's token validation, services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/get_query_results.rs:235-255:
    "        // `nextToken` is the index of the first row of the next page, in the one
        // opaque form the engine that issues it owns ([`encode_row_offset`] /
        // [`decode_row_offset`]), so the two operations that page a query's rows
        // cannot drift about the form they emit or the text they read back. WHICH
        // offsets this operation accepts is its own question, answered below: a
        // token is issued only for a row this result HAS, so an offset at or past
        // the end is one it never issued ...
        let start = match input.next_token.as_deref() {
            Some(token) => {
                let resume = decode_token(token)?;
                if resume >= held {
                    return Err(invalid_token());
                }
                resume
            }
            None => 0,
        };"
and the decoder it uses, src/ops/get_query_results.rs:335-336 — "fn decode_token(token: &str) -> Result<usize, GetQueryResultsError> { crate::ops::execute_query::decode_row_offset(token).ok_or_else(invalid_token) }" — i.e. any hex of a number inside the result's row count is accepted, whatever query minted it. The refusal it would otherwise raise is worded src/ops/get_query_results.rs:342 `invalid_token()` -> "The nextToken is not a token this operation issued."

The sibling of this crate that binds a page token to the request that produced it, src/ops/batch_get_asset_property_value.rs:386-401:
    "/// The token carries the FINGERPRINT of the entry list beside the position, so a
     /// token is only honoured against the request that produced it: replayed against a
     /// different list it names no page, rather than silently re-paging that other list
     /// and reporting its leading entries as already processed."
    ... "fn decode_page_token(token: &str, entries: &[BatchGetAssetPropertyValueEntry]) -> Option<usize> { ... (fingerprint == entries_fingerprint(entries)).then_some(())?; at.parse().ok() }"
and the crate-wide rule the 34 shared-page-walk listings follow, src/state.rs (the `page_of` doc) — "`None` for a `next_token` this service never issued — text that is not the hex of a resume key, or a key no entry of this enumeration carries. Its caller answers that with the modelled refusal for an invalid `nextToken`, rather than with an empty page, which would report absence as success."

Test coverage: tests/all/query_results.rs:586-625 is this operation's invalid-token table and is purely STRUCTURAL — "A `nextToken` this operation never issued: text that is not a token at all, one that decodes to no row index, one at the end of the result" with rows ("hexadecimal of no number", "a negative offset", at_the_end, past_the_end) and the empty-result rows. No row replays a token minted for another query, so the cross-query acceptance is unpinned and binding the token to the query keeps every existing row passing. The sibling refusal IS pinned: tests/all/batch_get_latest_values.rs:645-675 ("A page token belongs to the request that produced it: replayed against a different entry list it names no page, instead of re-paging that other list") expects 400.

## Steps to Reproduce

1. CreateWorkspace W; create an asset model and at least three assets so a SELECT over the asset view answers several rows.
2. StartQuery in W with a statement whose result holds >= 2 rows -> query A; poll DescribeQuery until COMPLETED.
3. GET /workspaces/W/queries/A/results?maxResults=1 -> 200 with one row and a nextToken (the hex of "1").
4. StartQuery in W with a DIFFERENT statement whose result also holds >= 2 rows -> query B; poll until COMPLETED.
5. GET /workspaces/W/queries/B/results carrying query A's nextToken.
6. Observed: 200, and B's rows are served from index 1 — B's first row is reported in no page of this enumeration and no error is raised.
7. Expected, per the rule the sibling batch reader states (src/ops/batch_get_asset_property_value.rs:386-389) and a test pins (tests/all/batch_get_latest_values.rs:661-675): a token replayed against a different enumeration names no page and is refused with InvalidRequestException — the very message this handler already carries, src/ops/get_query_results.rs:342.
8. The same acceptance holds for a token minted by ExecuteQuery (the codec is shared, src/ops/get_query_results.rs:336).
