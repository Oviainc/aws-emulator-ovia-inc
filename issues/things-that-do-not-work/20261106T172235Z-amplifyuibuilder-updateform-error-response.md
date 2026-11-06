Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: aced2d7a7a8791505dae5681e5c5c62e3a68d2ec

# UpdateForm's modeled-contract block omits the 404 it widens in and answers, where every other widening handler of this crate documents it

_Filed: 2026-11-06T17:22:35Z_
_Operation: UpdateForm | Class: error-response_

> _Note: this issue was identified against the code base at commit `aced2d7a7a8791505dae5681e5c5c62e3a68d2ec`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateForm` widens this service's `ResourceNotFoundException` into its error channel and answers it (HTTP 404) for an `id` that names no form in the addressed app environment — but its `## Errors` contract block lists only the three errors the model declares and says nothing about that widening. Every other handler in this crate that widens an error documents it in that same block with an explicit "widened in through `extra_errors`" bullet: `UpdateComponent`, `UpdateTheme`, `GetMetadata`, `PutMetadataFlag`, `RefreshToken` and `ExchangeCodeForToken` all do. `update_form.rs` is the single exception, so a reader of its stated contract concludes the handler can only answer 500/400/409, while the code three sections below answers 404 — the exact doc-vs-handler mismatch already fixed once for `UpdateComponent` (issue 20261030T045001Z-amplifyuibuilder-updatecomponent-error-response). No wire behaviour is wrong; the handler's 404 is the right answer and is pinned by tests. The defect is that the operation's own written contract contradicts it, so the next reader or editor of this file has no way to know the 404 is deliberate.

## Evidence

The two sibling update handlers document their widening inside the `## Errors` block:

services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/update_theme.rs:77-78
    //! * `ResourceNotFoundException` (HTTP 404) — widened in through `extra_errors` (see below):
    //!   the shape is this service's own, declared for the same id lookup by `GetTheme`.

services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/update_component.rs:80-82
    //! * `ResourceNotFoundException` (HTTP 404) — widened in through `extra_errors` (see below):
    //!   the shape is this service's own, declared for the same id lookup by `GetComponent` and
    //!   `DeleteComponent`.

And so does the non-update precedent:

services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/get_metadata.rs:65-67
    //! * `InternalServerException` (HTTP 500) — widened in through `extra_errors` (see below):
    //!   the shape is this service's own, and it is how an unreachable shared store is
    //!   reported, as in every sibling handler of this crate.

`UpdateForm`'s block stops at three bullets — no 404 anywhere:

services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/update_form.rs:66-73
    //! ## Errors
    //!
    //! * `InternalServerException` (HTTP 500) — An internal error has occurred. Please retry your
    //!   request.
    //! * `InvalidParameterException` (HTTP 400) — An invalid or out-of-range value was supplied
    //!   for the input parameter.
    //! * `ResourceConflictException` (HTTP 409) — The resource specified in the request conflicts
    //!   with an existing resource.

Yet the same file widens the shape in and answers it:

services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/update_form.rs:90-93
    #[smithy_macros::operation_error(
        "com.amazonaws.amplifyuibuilder#UpdateForm",
        extra_errors = ["ResourceNotFoundException"]
    )]

services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/update_form.rs:188-191
            return Err(UpdateFormErrorModeled::ResourceNotFoundException(
                crate::state::item_not_found(),
            )
            .into());

Test coverage of the behaviour the block fails to state: tests/all/update_absent_id_refusal_class.rs:12-17 — "`UpdateComponent`, `UpdateForm` and `UpdateTheme` each declare only `InternalServerException`, `InvalidParameterException` and `ResourceConflictException` ... so each widens this service's own `ResourceNotFoundException` in through `extra_errors` to answer the absent case the way its `Get…` sibling does" — i.e. the suite asserts the 404 for all three updates, so correcting the doc changes no response and no test.

## Steps to Reproduce

1. Open services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/update_form.rs and read the `## Errors` section (lines 66-73): it names InternalServerException (500), InvalidParameterException (400) and ResourceConflictException (409), and no 404.
2. Read on to line 92 of the same file: `extra_errors = ["ResourceNotFoundException"]` — a fourth error is widened into the channel.
3. Read lines 188-191 of the same file: the handler returns `UpdateFormErrorModeled::ResourceNotFoundException(crate::state::item_not_found())` for an id that names no form in the addressed app environment, i.e. HTTP 404.
4. Compare with update_theme.rs:77-78 and update_component.rs:80-82, which each carry a `ResourceNotFoundException (HTTP 404) — widened in through extra_errors` bullet in the identical block, and with get_metadata.rs:65-67, which does the same for its widened 500.
5. Confirm the 404 is intended and tested: tests/all/update_absent_id_refusal_class.rs:12-17 states the rule for all three update operations, so only update_form.rs's contract block is out of step.
