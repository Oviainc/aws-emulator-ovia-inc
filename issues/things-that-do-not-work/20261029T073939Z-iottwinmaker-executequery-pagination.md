Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 2b09bad8439b0b0ac599f9d638bea059710099b4

# ExecuteQuery resumes at row offsets it never minted ("+N", leading-zero spellings, and positions past the selected row set), answering 200 instead of the unminted-token refusal its own doc promises

_Filed: 2026-10-29T07:39:39Z_
_Operation: ExecuteQuery | Class: pagination_

> _Note: this issue was identified against the code base at commit `2b09bad8439b0b0ac599f9d638bea059710099b4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

ExecuteQuery's `nextToken` resumes at a row OFFSET (unlike this service's key-based listings). The envelope, the workspace and the statement are all verified, but the offset itself is accepted as whatever `usize::from_str` will parse, so three spellings/positions the handler never mints are honoured as a page position:

* a leading `+` (`usize::from_str` accepts `"+1"`), so a hand-built token carrying `+1` resumes at row 1;
* leading zeros (`"0000001"` parses to 1), likewise;
* any magnitude, including `0` and offsets at or past the number of rows the query selects. `encode_next_token` is reached only on the `(served_through < selected)` branch, so a minted offset is always a plain decimal with no sign, no leading zero, at least 1, and strictly less than the selected row count. An offset past the end makes `bindings.iter().skip(resume_at)` yield nothing, `served_through = resume_at`, and `(served_through < selected)` false — so the call answers HTTP 200 with `rows: []` and no `nextToken`, i.e. a successful "you have reached the end" for a cursor the service never issued.

The handler's own module doc states the rule this breaks: a `nextToken` not minted for this query is a 400 `ValidationException`. `crate::state::invalid_next_token_message()` is the one refusal every paginated operation of this service answers for such a token, and `crate::state::decode_page_token`'s doc states the same principle on the key side (an empty key is rejected precisely because "a listing never mints one"). Here the key IS decoded and then widened by `.parse::<usize>().ok()`, which admits a value set strictly larger than the one `encode_next_token` writes.

The same `.parse().ok()` widening sits in the two other ordinal-resuming readers of this crate (`get_property_value.rs::decode_row_token`, `get_property_value_history.rs::decode_token`), so a fix belongs beside the shared token helpers rather than in this handler alone.

## Evidence

THE RULE THE HANDLER'S OWN DOC STATES — services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/ops/execute_query.rs:42-48:
```
//! What the handler refuses, and with which modelled error:
//!
//! * a statement the grammar does not accept, a `FROM` naming anything but `EntityGraph`,
//!   a variable no `MATCH` declares, an unknown workspace (the model declares no
//!   `ResourceNotFoundException` here, so an unknown workspace is a bad request like it
//!   is for `ListEntities`) and a `nextToken` not minted for this query — a 400
//!   `ValidationException`;
```
and the handler's comment at src/ops/execute_query.rs:247-255:
```
        // A `nextToken` carries the row offset the page resumes at, and is bound to BOTH
        // the workspace and the statement it was minted for: the row set is recomputed
        // from state on every call, so resuming one query's paging inside another would
        // report rows from a listing the caller never asked for.
        let resume_at = match input.next_token.as_deref() {
            None | Some("") => 0,
            Some(token) => decode_next_token(token, &input.workspace_id, &input.query_statement)
                .ok_or_else(|| invalid(crate::state::invalid_next_token_message()))?,
        };
```

THE CODE THAT BREAKS IT — src/ops/execute_query.rs:2085-2091:
```
/// The row offset inside a `nextToken` minted for this workspace and statement, or
/// `None` when the token is not one of ours or belongs to another query.
fn decode_next_token(token: &str, workspace_id: &str, statement: &str) -> Option<usize> {
    crate::state::decode_page_token(NEXT_TOKEN_PREFIX, &[workspace_id, statement], token)?
        .parse::<usize>()
        .ok()
}
```
Nothing compares the parsed offset against what this operation can have written.

WHAT IT CAN HAVE WRITTEN — src/ops/execute_query.rs:299-305 and 345-352:
```
            let rows: Vec<Row> = bindings
                .iter()
                .skip(resume_at)
                .take(page_size)
```
```
        let served_through = resume_at.saturating_add(rows.len());
        let next_token = (served_through < selected).then(|| {
            encode_next_token(
                served_through,
                &input.workspace_id,
                &input.query_statement,
            )
        });
```
`encode_next_token` renders the offset with `&offset.to_string()` (src/ops/execute_query.rs:2077-2083), so a minted offset carries no sign, no leading zero, is >= 1, and is < `selected`.

THE CRATE'S STATED PRINCIPLE FOR A KEY A LISTING NEVER MINTS — src/state.rs:4317-4336:
```
/// The key is split off first and each scope part is compared in turn, so a
/// token whose envelope, arity or scope disagrees is rejected rather than
/// resumed; the final part absorbs the remainder, so the last scope part may
/// itself contain newlines. A token carrying an EMPTY key is rejected too: a
/// listing never mints one, since a page is only resumed at a row that exists.
pub fn decode_page_token(envelope: &str, scope: &[&str], token: &str) -> Option<String> {
```
and the one refusal it earns, src/state.rs:4457-4468:
```
/// The message for a `nextToken` [`decode_page_token`] does not recognise: the
/// ONE refusal every paginated operation of this service reports for a token it
/// did not mint, so the same failure reads the same way whichever page was
/// asked for.
...
pub fn invalid_next_token_message() -> String {
    "nextToken is not a token this listing minted".to_string()
}
```

THE SIBLING COPIES OF THE SAME WIDENING — src/ops/get_property_value.rs:1246-1254 (`decode_row_token`) and src/ops/get_property_value_history.rs:1096-1104 (`decode_token`), both ending `.parse()\n    .ok()`.

TEST COVERAGE: nothing pins the un-minted offset. tests/all/next_token_refusal.rs:15-23 enumerates exactly which forged tokens the refusal rule is asserted over — "text that is not hex at all, hex of the wrong length, hex that decodes to other bytes, hex that decodes to no UTF-8, punctuation and markup, non-ASCII text, a token at the member's published maximum length, and a REAL token another listing minted" — none of which is a well-formed token of this envelope and scope carrying an offset the handler never wrote. The only ExecuteQuery token-refusal row, tests/all/execute_query.rs:780-803, varies the STATEMENT ("a token minted for one statement does not resume another"), not the offset, so a narrower offset check leaves it passing.

## Steps to Reproduce

1. CreateWorkspace(workspaceId="ws"), then CreateEntity three times (entityId="a", "b", "c", each with an entityName) so the query below selects three rows.
2. ExecuteQuery(workspaceId="ws", queryStatement="SELECT e.entityId FROM EntityGraph MATCH (e)", maxResults=1) -> 200 with one row and a nextToken. Hex-decoding it shows the body "iottwinmaker:query:1:1\nws\nSELECT e.entityId FROM EntityGraph MATCH (e)" — offset "1".
3. Hex-encode the same body with the offset spelled "+1" instead of "1" and call ExecuteQuery(workspaceId="ws", queryStatement=<the same statement>, nextToken=<that hex>). Expected: 400 ValidationException "nextToken is not a token this listing minted", the refusal the module doc at execute_query.rs:44-48 promises for a token not minted for this query. Actual: 200, resumed at row 1.
4. Repeat with the offset spelled "0000001" — again 200, resumed at row 1.
5. Repeat with the offset spelled "99999" (past the three rows the query selects). Actual: 200 with rows=[] and no nextToken — a successful end-of-pagination answer for a cursor the service never issued — instead of the 400 refusal.
6. Contrast a key-resuming sibling: ListScenes with a hand-built token whose envelope/scope match but whose key is empty is refused, because crate::state::decode_page_token rejects a key "a listing never mints" (src/state.rs:4323-4335).
