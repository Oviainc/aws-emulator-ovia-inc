Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 83df1cdc32a5bdf510e34eb14380f1c3f1e573c9

# Four paginated operations copy the caller's raw nextToken into the refusal message, the one thing seven sibling handlers state they must not do

_Filed: 2026-10-27T14:56:24Z_
_Operation: ListEntities | Class: error-response_

> _Note: this issue was identified against the code base at commit `83df1cdc32a5bdf510e34eb14380f1c3f1e573c9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Seven of this crate's paginated handlers refuse an unrecognised `nextToken` with a fixed sentence and carry a comment stating why the token must NOT be reflected: it is opaque to the caller and modelled up to 17880 characters, so the refusal names the member and what is wrong with it rather than copying an arbitrary request string into the response.

Four operations — `ListEntities`, `ListTagsForResource`, `ExecuteQuery` and `GetPropertyValue`/`GetPropertyValueHistory` — do exactly what that comment forbids: they interpolate the caller-supplied token straight into the `ValidationException` message. A caller may therefore push up to 17880 characters of arbitrary text (the `NextToken` shape is `@length(0..17880)`, `@pattern(".*")`, so any bytes that survive the body decode) through the service and get them echoed back in an error body, and the same failure reads differently depending on which listing was asked.

No test pins either message, so the rule the seven siblings state can be applied to the four outliers without changing any assertion.

## Evidence

THE RULE, stated in seven handlers (quoted from src/ops/list_workspaces.rs:188-192, repeated verbatim at src/ops/list_component_types.rs:262, src/ops/list_components.rs:268, src/ops/list_properties.rs:241, src/ops/list_scenes.rs:225, src/ops/list_sync_jobs.rs:215, src/ops/list_metadata_transfer_jobs.rs:280):
  "// The token itself is NOT echoed back: it is opaque to the
   // caller and modelled up to 17880 characters, so the refusal
   // names the member and what is wrong with it rather than
   // copying an arbitrary request string into the response.
   invalid(\"nextToken is not a token this listing minted\".to_string())"

THE HANDLERS THAT BREAK IT:
- src/ops/list_entities.rs:220: `.ok_or_else(|| invalid(format!("nextToken {token} is not a valid token")))?,`
- src/ops/list_tags_for_resource.rs:204: `.ok_or_else(|| invalid(format!("nextToken {token} is not a valid token")))?,`
- src/ops/get_property_value.rs:705 and src/ops/get_property_value.rs:999: `.ok_or_else(|| invalid(format!("nextToken {token} is not a valid token")))?,`
- src/ops/get_property_value_history.rs:1008: `.ok_or_else(|| invalid(format!("nextToken {token} is not a valid token")))?,`
- src/ops/execute_query.rs:255: `invalid(format!("nextToken {token} is not a valid token for this query"))`

MODEL (what the echoed member may carry, `smithy_query(query="operation",operation="ListEntities")`):
  "nextToken": {"target": "NextToken", "length": {"min": 0, "max": 17880}, "pattern": ".*"}

TEST COVERAGE: no test asserts either construction — `grep -n 'is not a valid token|not a token this listing minted' tests/` returns "No matches." The bad-token cases only assert status and error type, e.g. tests/all/workspace_entity_lifecycle.rs:1409 (`Some(json!({"nextToken": "not-a-token"}))`), tests/all/component_listing.rs:550, tests/all/property_listing.rs:711, tests/all/property_value_read.rs:789-795, tests/all/metadata_transfer_job_listing.rs:644-645.

## Steps to Reproduce

1. CreateWorkspace `w1`; CreateEntity `e1` in it.
2. POST /workspaces/w1/entities-list with `{"nextToken":"<4000 characters of caller-chosen text>"}`.
3. Observe HTTP 400 ValidationException whose `message` contains all 4000 characters of the request string.
4. Compare with the sibling that states the rule: POST /workspaces-list with the same forged `nextToken` answers HTTP 400 with the fixed message "nextToken is not a token this listing minted" and echoes nothing.
5. Repeat for ListTagsForResource, ExecuteQuery, GetPropertyValue and GetPropertyValueHistory — all four echo.
