Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 5f1c460e5544105d602e680f733bf81495110ec8

# The optional `description` length rule is implemented twice, once per Apache Airflow entity parser

_Filed: 2026-10-28T16:04:28Z_
_Operation: InvokeRestApi | Class: maintainability_

> _Note: this issue was identified against the code base at commit `5f1c460e5544105d602e680f733bf81495110ec8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`InvokeRestApi` parses the two Apache Airflow entities a write can carry in two sibling functions in the same module, `requested_variable` (a variable's `key`/`value`/`description`) and `requested_pool` (a pool's `name`/`slots`/`description`/`include_deferred`). Both have to apply the same rule to the SAME member: the optional `description`, when the body carries one, must fit `crate::state::MAX_AIRFLOW_VALUE_LENGTH`.

That rule is written out twice, as two byte-identical `if let Some(description) = description.as_deref() { bounded(description, "description", crate::state::MAX_AIRFLOW_VALUE_LENGTH)?; }` blocks — the branch, the member name that goes into the refusal sentence, and the constant that bounds it. The crate single-homes every other cross-writer rule it has exactly to stop this: `crate::state::first_reserved_tag_key` ("The one place the tag writers ask the question, so a key is judged the same way whichever operation carries it", src/state.rs:90-98), `EnvironmentRecord::change_refusal` ("Every writer that reaches a stored record asks THIS, so the condition and its wording are decided in one place and no writer can drift from the others", src/state.rs:940-946) and `EnvironmentRecord::tags_member` ("The decision ... is made HERE and nowhere else, so the `Environment` shape and the `ListTagsForResource` response cannot drift apart", src/state.rs:901-911). The two `description` copies are the one place that single-homing is missing, so a change to the bound, to the member name in the message, or to the "a `null` description carries no bound" branch can be made in one parser and missed in the other — and a variable write and a pool write would then disagree about what a `description` may be.

The module already has the single-member helpers this would reduce to (`optional_text`, `bounded`); a `bounded` variant that takes the `Option<&str>` (or an `optional_bounded_text(body, name, longest)` that reads and bounds in one step) collapses both copies with no change to any refusal status, code or sentence.

## Evidence

Copy 1 — services/service-com-amazonaws-mwaa-amazonmwaa/src/ops/invoke_rest_api.rs:777-783, inside `requested_variable`:
```
    if let Some(description) = description.as_deref() {
        bounded(
            description,
            "description",
            crate::state::MAX_AIRFLOW_VALUE_LENGTH,
        )?;
    }
```

Copy 2 — services/service-com-amazonaws-mwaa-amazonmwaa/src/ops/invoke_rest_api.rs:809-815, inside `requested_pool`:
```
    if let Some(description) = description.as_deref() {
        bounded(
            description,
            "description",
            crate::state::MAX_AIRFLOW_VALUE_LENGTH,
        )?;
    }
```
The two blocks are byte-identical, and both are reached from the same dispatcher: src/ops/invoke_rest_api.rs:368-388 routes `POST /variables` -> `set_variable` -> `requested_variable` and `PATCH /variables/{key}` -> `change_variable` -> `requested_variable`, and `POST /pools` -> `create_pool` -> `requested_pool` and `PATCH /pools/{name}` -> `change_pool` -> `requested_pool`.

The one rule both copies implement is stated once in the state module — src/state.rs:131-134: "The longest Apache Airflow variable value (or description) this store keeps, in characters. With the counts above this bounds what one environment's Apache Airflow metadata can grow to." `pub const MAX_AIRFLOW_VALUE_LENGTH: usize = 4 * 1024;`

The crate's own single-homing standard, which these two copies are the exception to — src/state.rs:940-946:
```
    /// The sentence refusing a change this environment cannot take, or `None` when it can
    /// take one. Every writer that reaches a stored record asks THIS, so the condition and
    /// its wording are decided in one place and no writer can drift from the others; see
    /// [`change_not_allowed_message`].
```
and src/state.rs:901-911:
```
    /// The decision — the emptiness test and
    /// the conversion of the stored `BTreeMap` into the generated `HashMap` — is made HERE
    /// and nowhere else, so the `Environment` shape and the `ListTagsForResource` response
    /// cannot drift apart about what an environment's tags are.
```

Test coverage of the two paths (both must keep answering exactly as they do now): services/service-com-amazonaws-mwaa-amazonmwaa/tests/all/airflow_rest_api.rs:392-464 (`a_write_is_visible_to_the_next_read`) round-trips a variable `description` through both a present value and `Value::Null` and asserts the stored description reads back (lines 433-464); tests/all/airflow_rest_api.rs:260-296 covers a pool `description` (`json!({"name": "etl", "slots": 7, "description": "the ETL pool"})`) and the seeded `default_pool`'s `"description": "Default pool"` through `a_served_call_reports_the_airflow_status_and_its_document`; tests/all/airflow_rest_api.rs:513 (`an_airflow_refusal_is_the_modelled_client_exception`) pins that a body the parsers refuse comes back as the modelled `RestApiClientException`. No test asserts the over-length `description` refusal for either entity, so nothing currently pins the two copies to each other.

## Steps to Reproduce

1. Open services/service-com-amazonaws-mwaa-amazonmwaa/src/ops/invoke_rest_api.rs and read `requested_variable` (lines 769-789) and `requested_pool` (lines 791-831).
2. Observe that lines 777-783 and 809-815 are the same four-line branch applying the same bound to the same member name `"description"` with the same constant `crate::state::MAX_AIRFLOW_VALUE_LENGTH`.
3. Change the bound, the member name in the refusal sentence, or the `null`-description branch in one of the two copies only (for example, bound a pool description by `MAX_AIRFLOW_KEY_LENGTH`), and observe that the crate still builds and the existing tests in tests/all/airflow_rest_api.rs still pass, while `POST /variables` and `POST /pools` now disagree about what a `description` may be.
4. Contrast with `crate::state::first_reserved_tag_key` (src/state.rs:90-98) and `EnvironmentRecord::change_refusal` (src/state.rs:940-946), where the crate routes every writer through one function precisely so the copies cannot drift.
