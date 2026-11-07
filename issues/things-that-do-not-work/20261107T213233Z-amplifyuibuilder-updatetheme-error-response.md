Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 7f27151474f220c715589d81820913dcd4b7b62b

# UpdateTheme's summary doc calls the 404 it answers "modelled", while the same file says the operation's own errors list omits it

_Filed: 2026-11-07T21:32:33Z_
_Operation: UpdateTheme | Class: error-response_

> _Note: this issue was identified against the code base at commit `7f27151474f220c715589d81820913dcd4b7b62b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`update_theme.rs` states its 404 refusal three times and the three statements disagree about where the shape comes from.

The module summary (lines 6-9) tells the reader the absent-id refusal is `the modelled ResourceNotFoundException (HTTP 404)`. But `UpdateTheme`'s model declares only `InternalServerException`, `InvalidParameterException` and `ResourceConflictException` (plus the service-common `InvalidParameterException`): the 404 reaches the error channel only because this file widens it in through `extra_errors`, which the same file says in two other places — the `## Errors` bullet (lines 77-78, "widened in through `extra_errors`") and the comment above the enum (lines 95-102, "this operation's own `errors` list omits it").

No wire behaviour is wrong — the 404 is the right answer and is pinned by `tests/all/update_absent_id_refusal_class.rs` — but a reader or editor of the summary concludes the model declares the shape for this operation and that no widening is needed, which is exactly the doc-vs-code drift already corrected once for `UpdateComponent` (fixed issue 20261030T045001Z) and still open for `UpdateForm` (20261106T172235Z, filed against the omitted bullet). `UpdateComponent`, the handler this one says it mirrors, words the same sentence correctly: it calls the shape "the shape this operation's own `errors` list omits and which is widened in through `extra_errors`". The fix is to the sentence only; every response and every existing test stays unchanged.

## Evidence

The model does NOT declare ResourceNotFoundException for UpdateTheme — smithy_query(query="digest"):

    "UpdateTheme": {"input": "UpdateThemeRequest", "output": "UpdateThemeResponse",
      "errors": ["InternalServerException", "ResourceConflictException"],
      "http": {"method": "PATCH", "uri": "/app/{appId}/environment/{environmentName}/themes/{id}"}}

(plus the service-common "InvalidParameterException" — `common_errors`.)

Yet the module summary calls the 404 modelled:

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/update_theme.rs:6-9
    //! theme the request names — the `@required` `values` replaces what was stored, an
    //! optional member the request carries replaces what was stored and one it leaves out
    //! keeps it, the members that identify the record never move, `modifiedAt` moves to the
    //! instant of the call — and an id that
    //! names no theme in that app environment answers the modelled `ResourceNotFoundException`
    //! (HTTP 404) — the same miss `GetTheme` and `DeleteTheme` report.

The same file, 70 lines later, says the opposite:

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/update_theme.rs:77-78
    //! * `ResourceNotFoundException` (HTTP 404) — widened in through `extra_errors` (see below):
    //!   the shape is this service's own, declared for the same id lookup by `GetTheme`.

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/update_theme.rs:95-102
    // `ResourceNotFoundException` is widened in through `extra_errors`, exactly as
    // `UpdateComponent` widens it: ... and this operation's
    // own `errors` list omits it.

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/update_theme.rs:103-106
    #[smithy_macros::operation_error(
        "com.amazonaws.amplifyuibuilder#UpdateTheme",
        extra_errors = ["ResourceNotFoundException"]
    )]

The handler's own inline comment also calls it widened-in, not modelled:

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/update_theme.rs:201-204
        // The target is resolved before anything is merged: an id that names no
        // theme in the app environment the request addressed is absent (the
        // widened-in `ResourceNotFoundException`, 404), and nothing is created —
        // an update is not an upsert.

The sibling that words it correctly:

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/update_component.rs:6-9
    //! `ResourceNotFoundException` (HTTP 404) — the shape this operation's own `errors` list
    //! omits and which is widened in through `extra_errors` (see below), the same miss
    //! `GetComponent` and `DeleteComponent` report

Test coverage of the behaviour (unchanged by a doc fix): tests/all/update_absent_id_refusal_class.rs:12-17 states that `UpdateComponent`, `UpdateForm` and `UpdateTheme` "each declare only `InternalServerException`, `InvalidParameterException` and `ResourceConflictException` ... so each widens this service's own `ResourceNotFoundException` in through `extra_errors`", and asserts the 404 for all three.

## Steps to Reproduce

1. Run smithy_query(query="operation", operation="UpdateTheme") against services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/smithy-model/com-amazonaws-amplifyuibuilder-amplifyuibuilder.json: the `errors` list is InternalServerException (500), InvalidParameterException (400, service-common) and ResourceConflictException (409) — no ResourceNotFoundException.
2. Open services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/update_theme.rs and read the summary at lines 6-9: it calls the absent-id refusal "the modelled `ResourceNotFoundException` (HTTP 404)".
3. Read lines 77-78 and 95-106 of the same file: both say the shape is widened in through `extra_errors` and that "this operation's own `errors` list omits it".
4. Read lines 201-217 of the same file: the handler returns `UpdateThemeErrorModeled::ResourceNotFoundException(crate::state::item_not_found())`, and its comment calls it "the widened-in `ResourceNotFoundException`".
5. Compare update_component.rs:6-9, which words the identical sentence as "the shape this operation's own `errors` list omits and which is widened in through `extra_errors`".
