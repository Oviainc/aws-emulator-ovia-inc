Issue-Severity: LOW
Issue-Gap-Class: parameter-ignored
Issue-Commit: 8d66e3013bd632dac86bbefa8efd5ff8af0fdc07

# ListRecommendations silently ignores orderBy.order when orderBy carries no dimension

_Filed: 2026-10-28T15:08:54Z_
_Operation: ListRecommendations | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `8d66e3013bd632dac86bbefa8efd5ff8af0fdc07`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler states that an `orderBy` it cannot honour is refused "rather than silently served in another order", and it does refuse an `orderBy.dimension` that names no `Recommendation` member. But when the request sends an `orderBy` that names NO dimension at all while setting `order` (e.g. `{"orderBy": {"order": "Desc"}}` — both `OrderBy` members are optional in the model), the same `match` falls into the `None => None` arm: no refusal is raised and no sort is applied, so the request is served in catalog order, i.e. exactly the "silently served in another order" outcome the comment above the code rules out. The caller's `order` value is dropped without any signal.

## Evidence

`src/ops/list_recommendations.rs:189-200`:
```
// An `orderBy` that names no `Recommendation` member cannot be honoured,
// so the request is refused rather than silently served in another order.
let order = match &input.order_by {
    None => None,
    Some(order_by) => match order_by.dimension.as_deref() {
        None => None,
        Some(dimension) => match crate::state::Dimension::parse(dimension) {
            Some(parsed) => Some((parsed, order_by.order.as_deref() == Some(ORDER_DESC))),
            None => return Err(unknown_dimension(dimension)),
        },
    },
};
```
The inner `None => None` arm is an `orderBy` that names no member, which the comment two lines above says "cannot be honoured, so the request is refused"; instead the handler proceeds to `let page = crate::state::paginate(...)` at `:225` with `order` unset, serving catalog order.

Model: `smithy_query(query="shape", shape="OrderBy")` declares `dimension` and `order` with no `@required`, and documents `order` as "The order that's used to sort the data" — so `{"order": "Desc"}` alone is input the model allows.

No test covers it: the only `orderBy` cases in the crate send a dimension (`tests/all/cost_optimization_hub.rs:908`, `:938`, `:959`, and the refusal case `tests/all/validation_exception_fields.rs:427-430` sends `{"dimension": "NotADimension", "order": "Asc"}`).

## Steps to Reproduce

1. Enrol the caller and wait for the analysis to come due (as `enrolled_router()` does).
2. Call `ListRecommendations` with `{"includeAllRecommendations": true, "orderBy": {"order": "Desc"}}`.
3. Observe 200 with the items in plain catalog order — the requested `order` was neither honoured nor refused, while `{"orderBy": {"dimension": "NotADimension", "order": "Desc"}}` on the same call is refused with 400.
