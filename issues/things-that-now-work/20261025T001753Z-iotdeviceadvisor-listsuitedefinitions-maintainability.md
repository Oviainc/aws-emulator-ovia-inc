Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 9d4ea5e78ce612465906f9a67fc5f9d068061c14

# Deduplicate the pagination token/page-size rule shared by the two listings

_Filed: 2026-10-25T00:17:53Z_
_Operation: ListSuiteDefinitions | Class: maintainability_

> _Note: this issue was identified against the code base at commit `9d4ea5e78ce612465906f9a67fc5f9d068061c14`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

ListSuiteDefinitions and ListSuiteRuns each carry their own verbatim copy of the same two paging rules: (1) "an absent or empty nextToken means start from the beginning, any token that is not a UUID-shaped id this service mints is refused with the modelled ValidationException carrying the message `Invalid nextToken`", and (2) "an absent maxResults defaults to crate::state::MAX_RESULTS_MAX and the page size is clamped to at least 1". The rule itself already lives in crate::state (`is_page_token_shape`, `MAX_RESULTS_MAX`), but the branch that APPLIES it, including the empty-token special case, the refusal message and the `usize::try_from(..).unwrap_or(1).max(1)` computation, is written twice. The two copies can drift: a change to the empty-token handling, to the default page size, or to the refusal text in one listing silently leaves the other behaving differently, and nothing in the crate ties them together. The logic is decidable once — e.g. a helper in `crate::state` that turns `(next_token, max_results)` into `(Option<String>, usize)` or an `Err(String)` the caller lifts into its own modelled ValidationException variant — leaving both responses and every existing test byte-identical (each operation keeps its own error enum and its own message, which is the same string today).

## Evidence

Copy 1 — services/service-com-amazonaws-iotdeviceadvisor-iotsenateservice/src/ops/list_suite_definitions.rs:150-161 and :169-171:
```
        let resume_at = match input.next_token.as_deref() {
            None | Some("") => None,
            Some(token) if crate::state::is_page_token_shape(token) => Some(token.to_string()),
            Some(_) => {
                return Err(ListSuiteDefinitionsErrorModeled::ValidationException(
                    ValidationException {
                        message: Some("Invalid nextToken".to_string()),
                    },
                )
                .into());
            }
        };
...
        let page_size = usize::try_from(input.max_results.unwrap_or(crate::state::MAX_RESULTS_MAX))
            .unwrap_or(1)
            .max(1);
```
Copy 2 — services/service-com-amazonaws-iotdeviceadvisor-iotsenateservice/src/ops/list_suite_runs.rs:179-188 and :194-196:
```
        let resume_at = match input.next_token.as_deref() {
            None | Some("") => None,
            Some(token) if crate::state::is_page_token_shape(token) => Some(token.to_string()),
            Some(_) => {
                return Err(ListSuiteRunsErrorModeled::ValidationException(ValidationException {
                    message: Some("Invalid nextToken".to_string()),
                })
                .into());
            }
        };
...
        let page_size = usize::try_from(input.max_results.unwrap_or(crate::state::MAX_RESULTS_MAX))
            .unwrap_or(1)
            .max(1);
```
The crate already centralises every other shared rule in state.rs rather than restating it per handler — e.g. `src/state.rs:682` "The rule belongs to the SHAPE, so every operation that accepts a `SuiteDefinitionConfiguration` asks here rather than restating it." (`root_group_violation`), and `src/state.rs:662` `pub fn is_page_token_shape`.

Test coverage that must stay unchanged: `tests/all/suite_runs_listing_and_update.rs:434` `a_next_token_this_service_never_issued_is_refused_by_every_listing` (drives BOTH `/suiteRuns` and `/suiteDefinitions` with 8 malformed tokens, asserting 400 `ValidationException` whose message contains `nextToken`, plus the empty token being accepted), `tests/all/suite_runs_listing_and_update.rs:335` `listing_suite_runs_pages_to_the_end`, and `tests/all/suite_lifecycle.rs:571` `listing_projects_every_definition_and_pages_to_the_end` (both exercise the default and explicit `maxResults` paging loops).

## Steps to Reproduce

1. Open services/service-com-amazonaws-iotdeviceadvisor-iotsenateservice/src/ops/list_suite_definitions.rs and read lines 150-171.
2. Open services/service-com-amazonaws-iotdeviceadvisor-iotsenateservice/src/ops/list_suite_runs.rs and read lines 179-196 — the token branch and the page-size computation are the same code with only the error enum name changed.
3. Change one of them (e.g. make an empty nextToken a refusal, or change the default page size) and observe that `cargo test -p` still passes for the other listing: nothing forces the two paging rules to agree.
