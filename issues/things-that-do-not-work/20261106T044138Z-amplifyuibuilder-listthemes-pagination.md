Issue-Severity: LOW
Issue-Gap-Class: pagination
Issue-Commit: 1be7cfa8c8ca9e4bb16e8074ed05071db555de6e

# The page-token cursor-bound suite claims to drive every paginated operation but has rows for only four of the seven

_Filed: 2026-11-06T04:41:38Z_
_Operation: ListThemes | Class: pagination_

> _Note: this issue was identified against the code base at commit `1be7cfa8c8ca9e4bb16e8074ed05071db555de6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`tests/all/page_token_cursor_bounds.rs` is the crate's one suite for the rule that a `nextToken` names a page only when the cursor it carries is one the service actually handed out (`crate::state::parse_page_token`'s `(FIRST_CURSOR..=issued).contains(&cursor)` gate). Its module doc states as fact that this crate has FOUR paginated operations and that "each case here is driven against every one of them", and its one test is named `a_cursor_outside_the_span_handed_out_is_refused_by_every_paginated_operation`. The crate contradicts that count: the model binds SEVEN `@paginated` operations, and `paginated_operations()` returns rows for only ListComponents, ExportComponents, ExportForms and ExportThemes. ListThemes, ListForms and ListCodegenJobs are left out, even though each of them serves its page from the same `crate::state::page_of` walk and therefore honours (or fails to honour) exactly the bound the suite claims to pin everywhere.

The consequence is a blind spot in precisely the place the crate has already regressed once: the cursor-floor rule ("a page token whose cursor is 0 — a cursor the service never hands out — is served as the first page instead of refused") was a filed and fixed defect, and this suite is what guards it. Reintroducing it on the ListThemes / ListForms / ListCodegenJobs path — for instance by letting one of those handlers pass a different `issued` value, or by bypassing `parse_page_token` — leaves this suite green while its own doc and test name assert the opposite. The defect is the false claim plus the three uncovered operations, not the behaviour of the shared validator, which is correct today.

Filed against ListThemes because it is the entity list whose cursor-bound handling no suite exercises; the same row is missing for ListForms (whose absence from the OTHER page-walk suite is already filed separately) and for ListCodegenJobs, and all three belong in the same fix.

## Evidence

The suite's claim about how many paginated operations this crate has, and that every one is driven:

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/tests/all/page_token_cursor_bounds.rs:12-15
    //! All four paginated operations of this crate (`ListComponents`, `ExportComponents`,
    //! `ExportForms`, `ExportThemes`) share one token validator, so each case here is driven
    //! against every one of them: the cursor bound is a property of the token, not of the
    //! operation that reads it.

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/tests/all/page_token_cursor_bounds.rs:241-249
    /// A token whose cursor is outside the span the service hands out is refused by every
    /// paginated operation, while one carrying a cursor it did hand out — the FIRST one as
    /// much as the highest — is served.
    ...
    async fn a_cursor_outside_the_span_handed_out_is_refused_by_every_paginated_operation() {

The rows it actually returns — four, with no ListThemes, ListForms or ListCodegenJobs:

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/tests/all/page_token_cursor_bounds.rs:170-189
    fn paginated_operations() -> Vec<Paginated> {
        vec![
            Paginated { operation: "ListComponents", uri: collection_uri("components") },
            Paginated { operation: "ExportComponents", uri: export_uri("components") },
            Paginated { operation: "ExportForms", uri: export_uri("forms") },
            Paginated { operation: "ExportThemes", uri: export_uri("themes") },
        ]
    }

The crate has seven operations carrying `@paginated`, not four:

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/OPERATIONS.md:231-233
    ## ListThemes
    - **Route:** `GET /app/{appId}/environment/{environmentName}/themes`
    - **Traits:** @paginated(inputToken=nextToken, outputToken=nextToken, items=entities), @readonly

  (the other six: OPERATIONS.md:97 ListCodegenJobs, :137 ListComponents, :145 ExportComponents, :185 ListForms, :193 ExportForms, :241 ExportThemes)

ListThemes reaches the very validator the suite says it covers everywhere, so it is inside the claim's scope:

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/list_themes.rs:184-191
        let page = crate::state::page_of(
            store.themes_in(&input.app_id, &input.environment_name),
            &key,
            store.cursor_seq,
            input.next_token.as_deref(),
            page_size,
            crate::state::StoredTheme::to_summary,
        )

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/state.rs:1504-1521 (the bound the suite pins)
    pub fn parse_page_token(token: &str, key: &EnvironmentKey, issued: u64) -> Option<u64> {
        ...
        (FIRST_CURSOR..=issued).contains(&cursor).then_some(cursor)

Test coverage today: `tests/all/shared_page_walk.rs` drives ListThemes and ListCodegenJobs for the resume/unknown-token rules, but no suite in the crate presents either of them (or ListForms) a well-formed, correctly scoped token whose cursor lies OUTSIDE the span handed out — the case `page_token_cursor_bounds.rs` exists for. Adding the three missing rows changes no response and no existing assertion, since the validator is already shared.

## Steps to Reproduce

1. Read services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/tests/all/page_token_cursor_bounds.rs:170-189 and list the `operation` field of every row: ListComponents, ExportComponents, ExportForms, ExportThemes — four.
2. Read OPERATIONS.md and count the operations carrying `@paginated`: ListCodegenJobs, ListComponents, ExportComponents, ListForms, ExportForms, ListThemes, ExportThemes — seven.
3. Observe the suite's doc (lines 12-15) asserts 'All four paginated operations of this crate ... each case here is driven against every one of them', and its test is named `a_cursor_outside_the_span_handed_out_is_refused_by_every_paginated_operation` (line 249), while ListThemes, ListForms and ListCodegenJobs have no row.
4. Confirm ListThemes reaches the bound the suite pins: src/ops/list_themes.rs:184-191 calls `crate::state::page_of`, which gates the cursor through `parse_page_token` (src/state.rs:1504-1521).
5. Blind-spot check: make ListThemes accept an out-of-span cursor (e.g. hand `page_of` a `u64::MAX` `issued` instead of `store.cursor_seq`, so `GET /app/d1234abcd/environment/staging/themes?nextToken=c-<scope>0000000000000000` is served as a page instead of refused 400) and run the crate's tests — page_token_cursor_bounds.rs still passes, because no row drives ListThemes.
