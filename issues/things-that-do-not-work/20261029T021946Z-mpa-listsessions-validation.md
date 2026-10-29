Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 57e940d225b76eac54b5261180d73f867edef087

# An empty Filter$Value is accepted as a question on ActionName, so CONTAINS "" narrows nothing and EQ "" narrows to nothing instead of being refused

_Filed: 2026-10-29T02:19:46Z_
_Operation: ListSessions | Class: validation_

> _Note: this issue was identified against the code base at commit `57e940d225b76eac54b5261180d73f867edef087`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListSessions` compiles each `Filter` into a predicate and refuses anything it cannot apply. Two of the three fields hold their `Value` to something that can state a question: `SessionStatus` refuses a value outside its enum catalog, and `InitiationTime` refuses a value that names no instant. `ActionName` holds its `Value` to nothing at all, so the EMPTY STRING reaches `Predicate::Text` and is applied as a comparison:

* `{"FieldName":"ActionName","Operator":"CONTAINS","Value":""}` -> `subject.contains("")` is true for every session, so the page is the whole unfiltered listing;
* `{"FieldName":"ActionName","Operator":"NE","Value":""}` -> true for every session, same;
* `{"FieldName":"ActionName","Operator":"EQ","Value":""}` -> false for every session (`SessionKind::action_name()` is always one of the three operation names), so the page is empty;
* `{"FieldName":"ActionName","Operator":"NOT_CONTAINS","Value":""}` -> false for every session, same empty page.

Each of those is exactly the outcome this handler says it refuses rather than answers, and the handler states that rule twice: once as the whole point of compiling filters ("a client never reads a page as narrowed when it was not") and once as the reason a `SessionStatus` value outside the catalog is a refusal ("rather than answered with a narrowing nothing (for `EQ`) or everything (for `NE`) satisfies"). The empty string on `ActionName` is literally "a narrowing nothing or everything satisfies", and it is answered 200.

The justification `required()` carries for refusing an ABSENT member applies to the empty value word for word -- "a filter missing any of them states no question to ask" -- and the crate already has one rule for an empty optional string reaching a handler: it is treated as absent. `token_key` does `next_token.filter(|token| !token.is_empty())`, and both creates do `.filter(|token| !token.is_empty())` on `ClientToken`. `compile_filter` is the one place that reads an optional request string and skips that rule, so a request this operation's own stated rule classifies as unapplicable is accepted, applied, and reported as a narrowed page.

## Evidence

The stated rule, services/service-com-amazonaws-mpa-awsfluffycoreservice/src/ops/list_sessions.rs:34-39:
```
//! A filter this operation cannot apply is REFUSED with `ValidationException` (400)
//! rather than ignored, so a client never reads a page as narrowed when it was not: one
//! of the other `FilterField` tokens the shared enum declares, an operator the named
//! field does not take, a filter leaving out `FieldName`, `Operator` or `Value`, a
//! `SessionStatus` value outside that enum's own token catalog, and a time value that
//! names no instant are each a refusal naming the member at fault.
```

The same rule spelled out as the reason the SIBLING field refuses its value, src/ops/list_sessions.rs:461-469:
```
        SESSION_STATUS_FIELD => Ok(Predicate::Text {
            field: TextField::SessionStatus,
            comparison: text_comparison(index, field, operator, SESSION_STATUS_OPERATORS)?,
            // `SessionStatus` is an ENUM, so a value outside its token catalog names no
            // status any session can be in: refused like every other filter this
            // operation cannot apply, rather than answered with a narrowing nothing (for
            // `EQ`) or everything (for `NE`) satisfies.
            value: declared_status(index, value)?.to_string(),
        }),
```

The `ActionName` arm, which holds its value to nothing, src/ops/list_sessions.rs:455-460:
```
    match field {
        ACTION_NAME_FIELD => Ok(Predicate::Text {
            field: TextField::ActionName,
            comparison: text_comparison(index, field, operator, ACTION_NAME_OPERATORS)?,
            value: value.to_string(),
        }),
```

The comparison that then admits everything, src/ops/list_sessions.rs:414-420:
```
                let value = value.as_str();
                match comparison {
                    TextComparison::Equal => subject == value,
                    TextComparison::NotEqual => subject != value,
                    TextComparison::Contains => subject.contains(value),
                    TextComparison::NotContains => !subject.contains(value),
                }
```

The reader that lets `""` through, with the justification that covers it, src/ops/list_sessions.rs:450-454 and 494-505:
```
    // All three members are `@optional` in the model, so absence reaches here; a filter
    // missing any of them states no question to ask.
    let field = required(index, "FieldName", filter.field_name.as_deref())?;
    let operator = required(index, "Operator", filter.operator.as_deref())?;
    let value = required(index, "Value", filter.value.as_deref())?;
```
```
/// One member of a filter, or the refusal naming the one that is missing.
fn required<'a>(
    index: usize,
    member: &str,
    value: Option<&'a str>,
) -> Result<&'a str, ListSessionsError> {
    value.ok_or_else(|| {
        validation(format!(
            "Filters[{index}].{member} is required for the filter to be applied."
        ))
    })
}
```
`Option::ok_or_else` alone, so `Some("")` is an accepted question.

The crate's own empty-optional-string-is-absent rule, which this reader skips -- src/state.rs:525:
```
    let Some(token) = next_token.filter(|token| !token.is_empty()) else {
        return Ok(None);
    };
```
and src/ops/create_approval_team.rs:248-254:
```
        let client_token = input
            .client_token
            .as_deref()
            .filter(|token| !token.is_empty())
```

The model permits the value: `Filter$Value` is `{"target": "String", "length": {"max": 1000}}` (`smithy_query(query="shape",shape="com.amazonaws.mpa#Filter")`) -- a maximum only, no minimum, so the empty string passes the constraint boundary and reaches the handler.

Test coverage: tests/all/session_listing.rs pins the refusals for a MISSING `Operator` (line 692 `json!({ "FieldName": "ActionName", "Operator": "EQ" })`) and a MISSING `Value` (line 687 `json!({ "FieldName": "ActionName", "Value": ACTION })`), and the applied `ActionName` comparisons at lines 395-420, but no row sends an empty `Value`, so nothing pins the current answer (`grep -n '"Value": ""' tests/` returns nothing).

## Steps to Reproduce

1. Start the emulator, record the management account as tests/all/approval_team_lifecycle.rs:194 does, and create an approval team; wait for it to become ACTIVE.
2. `POST /approval-teams/{Arn}/baseline` to record one or more sessions for that team.
3. `POST /approval-teams/{Arn}/sessions/?List` with no `Filters` and note the sessions reported -- call that the unfiltered page.
4. `POST /approval-teams/{Arn}/sessions/?List` with body `{"MaxResults":20,"Filters":[{"FieldName":"ActionName","Operator":"CONTAINS","Value":""}]}` -> 200 with the SAME sessions as the unfiltered page: the filter was accepted and narrowed nothing, which list_sessions.rs:34-39 says cannot happen.
5. Repeat with `"Operator":"EQ","Value":""` -> 200 with an empty `Sessions` list: the filter narrowed to nothing, the other half of the outcome list_sessions.rs:464-468 says is refused rather than answered.
6. Compare with the sibling field: `{"FieldName":"SessionStatus","Operator":"EQ","Value":""}` -> 400 `ValidationException` from `declared_status` (list_sessions.rs:483-492), and `{"FieldName":"InitiationTime","Operator":"GT","Value":""}` -> 400 from `filter_instant` (list_sessions.rs:592-599).
