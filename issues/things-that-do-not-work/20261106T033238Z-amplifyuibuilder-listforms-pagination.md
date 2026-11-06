Issue-Severity: LOW
Issue-Gap-Class: pagination
Issue-Commit: 15ef1adb3fb7a9db5ce7069e30860cdce3c8580a

# The shared-page-walk suite claims to cover every paginated operation but has no row for ListForms

_Filed: 2026-11-06T03:32:37Z_
_Operation: ListForms | Class: pagination_

> _Note: this issue was identified against the code base at commit `15ef1adb3fb7a9db5ce7069e30860cdce3c8580a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`tests/all/shared_page_walk.rs` is the crate's one end-to-end suite for the shared page walk, and it states that it drives that walk from EVERY operation of the service that serves a page. It does not: `paginated_operations()` returns six rows (ListComponents, ExportComponents, ListThemes, ExportThemes, ExportForms, ListCodegenJobs) while the model binds SEVEN `@paginated` operations — ListForms is the one left out, even though the comment above the row list explicitly claims the "form" pages are among those now sharing the walk (only ExportForms is). ListForms does serve its page from `crate::state::page_of` (list_forms.rs:181-193), so it is in scope of the claim; it is simply the only paginated operation whose token handling — resume at the cursor, refusal of a token this operation never issued, the `maxResults` page bound, the resume-past-a-deleted-record rule — no row exercises. The defect is the false claim and the resulting blind spot: a regression in ListForms' page walk (for instance a different page bound, or serving an unknown token instead of refusing it) passes the suite that says it covers it.

## Evidence

The suite's own claim, both in its module doc and above the row list:

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/tests/all/shared_page_walk.rs:6
    //! The rule under test, which all six of them implement: ...

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/tests/all/shared_page_walk.rs:16-19
    //! Each row drives the walk through a different operation — `ListComponents`,
    //! `ExportComponents`, `ListThemes`, `ExportThemes`, `ExportForms` and `ListCodegenJobs` —
    //! over the record kind that operation pages and the projection it serves, so the shared
    //! walk is exercised from every operation that reaches it.

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/tests/all/shared_page_walk.rs:232-235
    /// Every operation of this crate that serves a page, one row each: the two component
    /// listings the duplicated walk lived in, and the theme, form and codegen-job pages that
    /// now share it too.
    fn paginated_operations() -> Vec<Paginated> {

The rows it actually returns (shared_page_walk.rs:236-273) name ListComponents, ExportComponents, ListThemes, ExportThemes, ExportForms and ListCodegenJobs — six, with no ListForms row.

The operations that serve a page are seven, per the model:

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/OPERATIONS.md:185-187
    ## ListForms
    - **Route:** `GET /app/{appId}/environment/{environmentName}/forms`
    - **Traits:** @paginated(inputToken=nextToken, outputToken=nextToken, items=entities), @readonly

  (the other six: OPERATIONS.md:97 ListCodegenJobs, :137 ListComponents, :145 ExportComponents, :193 ExportForms, :233 ListThemes, :241 ExportThemes)

And ListForms is served from the very walk the suite claims to drive from every such operation:

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/list_forms.rs:181-193
        let page = crate::state::page_of(
            store.forms_in(&input.app_id, &input.environment_name),
            &key,
            store.cursor_seq,
            input.next_token.as_deref(),
            page_size,
            crate::state::StoredForm::to_summary,
        )
        .map_err(|unknown| {
            ListFormsErrorModeled::InvalidParameterException(InvalidParameterException {
                message: Some(unknown.message(Self::NAME)),
            })
        })?;

Existing form pagination coverage is tests/all/form_list.rs only; no row of the shared-walk suite drives ListForms.

## Steps to Reproduce

1. Read services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/tests/all/shared_page_walk.rs:232-273 and list the `operation` field of every row: ListComponents, ExportComponents, ListThemes, ExportThemes, ExportForms, ListCodegenJobs — six.
2. Read OPERATIONS.md and count the operations carrying `@paginated`: ListCodegenJobs, ListComponents, ExportComponents, ListForms, ExportForms, ListThemes, ExportThemes — seven.
3. Observe that ListForms is the missing one, while shared_page_walk.rs:232-234 claims the row list is "Every operation of this crate that serves a page, one row each" and names the form pages among them.
4. Confirm ListForms reaches the shared walk (src/ops/list_forms.rs:181-193 calls `crate::state::page_of`), so it is inside the claim's scope and is nonetheless unexercised by that suite.
