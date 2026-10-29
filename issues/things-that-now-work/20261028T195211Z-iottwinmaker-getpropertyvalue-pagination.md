Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: a67556f919df77bd586efc9ebe2e6b88c4f9f9e7

# A tabular nextToken is resumed by a query with different tabularConditions, against the claim the handler makes two lines above the decode that the token "is bound to the query that produced it"

_Filed: 2026-10-28T19:52:11Z_
_Operation: GetPropertyValue | Class: pagination_

> _Note: this issue was identified against the code base at commit `a67556f919df77bd586efc9ebe2e6b88c4f9f9e7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GetPropertyValue`'s tabular answer pages with an ORDINAL token: `table_response` reads an integer out of the token and uses it as `skip(resume_at)` / `served < total` arithmetic over the row sequence that this request's own `tabularConditions` produced (the `propertyFilters` passes and the `orderBy` sorts applied by `Table::sift`).

An ordinal only means something inside ONE sequence, and the handler says so and says the token is bound accordingly — `table_response`'s own comment at src/ops/get_property_value.rs:989-992 states both halves of the rule: "which rows there are and in what order is what this request's own filters and ordering say — so the token resumes at the row's ORDINAL in that sequence, and is bound to the query that produced it so a token from another one is refused rather than resumed."

The code under that comment does not do the second half. `token_scope` (src/ops/get_property_value.rs:1160-1173) binds six members — `entityId`, `componentName`, `componentPath`, `componentTypeId`, `propertyGroupName` and the sorted selection — and deliberately binds NOTHING from `tabularConditions`, which its own doc at lines 1156-1159 admits. But `tabularConditions` is exactly what decides which rows there are and in what order:

* `Table::sift` drops every row no filter selects (`rows.retain(...)`, src/ops/get_property_value.rs:841-853), so adding, removing or changing a `propertyFilters` entry changes both which rows are in the sequence and how many there are;
* `Table::sift` then re-sorts the rows per `orderBy` key (lines 860-877), so adding, dropping or flipping an `orderBy` entry permutes the sequence wholesale.

So a token minted by one tabular query is accepted by a materially different tabular query and its ordinal is applied to a sequence it was never counted in. A caller that follows `nextToken` while changing `orderBy` (or adding a `propertyFilters` entry) is handed rows silently skipped or repeated — or an empty `tabularPropertyValues` with no `nextToken`, which looks like a completed walk — instead of the refusal the same handler gives for the identical mistake on any of the six members it DOES bind (changing `componentTypeId`, `propertyGroupName` or the selection between two pages is correctly refused by `decode_row_token`).

The already-open sibling issue on `GetPropertyValueHistory` (20261028T180932Z-iottwinmaker-getpropertyvaluehistory-pagination) excuses this handler in a parenthetical on the grounds that it "binds NO sequence-deciding member at all, so it is self-consistent". That is not so: lines 989-992 claim the binding, and lines 1156-1159 withdraw it, so the handler contradicts itself; and the six members it does bind are checked while the two that reorder and shorten the sequence are not — the same asymmetry the sibling issue is about.

## Evidence

THE RULE THE HANDLER STATES, two lines above the decode — services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/ops/get_property_value.rs:989-997:
```
    // A row has no key of its own — which rows there are and in what order is
    // what this request's own filters and ordering say — so the token resumes
    // at the row's ORDINAL in that sequence, and is bound to the query that
    // produced it so a token from another one is refused rather than resumed.
    let resume_at = match input.next_token.as_deref() {
        None | Some("") => 0,
        Some(token) => decode_row_token(input, token)
            .ok_or_else(|| invalid(crate::state::invalid_next_token_message()))?,
    };
```

WHAT THE TOKEN IS ACTUALLY BOUND TO — src/ops/get_property_value.rs:1156-1173 (no `propertyFilters`, no `orderBy`):
```
/// The filters and the ordering of a TABULAR query are NOT part of this: a
/// tabular token resumes at a row's ordinal in the sequence the request's own
/// `tabularConditions` produce, so a caller resumes by repeating the whole
/// request with the token, which is what an SDK paginator does.
fn token_scope(input: &GetPropertyValueInput) -> String {
    let mut selected = input.selected_properties.clone();
    selected.sort();
    selected.dedup();
    format!(
        "{}|{}|{}|{}|{}|{}",
        input.entity_id.as_deref().unwrap_or(""),
        input.component_name.as_deref().unwrap_or(""),
        input.component_path.as_deref().unwrap_or(""),
        input.component_type_id.as_deref().unwrap_or(""),
        input.property_group_name.as_deref().unwrap_or(""),
        selected.join(","),
    )
}
```

PROOF THAT THE TWO OMITTED MEMBERS DECIDE THE SEQUENCE — src/ops/get_property_value.rs:841-853 (filters remove rows):
```
        for filter in &filters {
            rows.retain(|row| {
                row.values
                    .get(&filter.property_name)
                    .and_then(|value| crate::state::compare_data_values(value, &filter.value))
                    .is_some_and(|ordering| {
                        crate::state::comparison_satisfies(&filter.operator, ordering)
                    })
            });
        }
```
and src/ops/get_property_value.rs:860-877 (orderBy permutes them):
```
        for order in orders.iter().rev() {
            let mut keyed: Vec<(OrderKey, TableRow)> = rows
...
            keyed.sort_by(|(left, _), (right, _)| {
                let ordering = left.cmp(right);
                if order.descending {
                    ordering.reverse()
...
            rows = keyed.into_iter().map(|(_, row)| row).collect();
```

THE ORDINAL ARITHMETIC THE TOKEN FEEDS — src/ops/get_property_value.rs:998-1011:
```
    let total = rows.len();
    let page: Vec<::std::collections::HashMap<String, DataValue>> = rows
        .into_iter()
        .skip(resume_at)
        .take(page_size)
...
    let served = resume_at.saturating_add(page.len());
    let next_token = (served < total).then(|| encode_row_token(input, served));
```

THE INTENT THE SHARED DECODER STATES — src/state.rs:4251-4253 (quoted in the sibling issue):
```
/// The key is split off first and each scope part is compared in turn, so a
/// token whose envelope, arity or scope disagrees is rejected rather than
/// resumed;
```

TEST COVERAGE: `a_tabular_answer_is_paged_over_its_rows` (tests/all/property_value_read.rs:562-609) pins the walk when the WHOLE request — `tabularConditions: {"orderBy": [{"propertyName": "Threshold"}]}` included — is repeated unchanged on every page (lines 570-578), so binding the conditions into the token scope leaves it green. The refusal table at tests/all/property_value_read.rs:789-795 has only the row "a nextToken this query did not mint" with the hand-made token `"6e6f742d612d746f6b656e"`; no test sends a token minted under one `tabularConditions` back with another, so nothing pins the current acceptance.

## Steps to Reproduce

1. CreateWorkspace(workspaceId="ws"); CreateComponentType(workspaceId="ws", componentTypeId="com.example.base", propertyDefinitions={"Tag": {"dataType": {"type": "STRING"}}, "Threshold": {"dataType": {"type": "DOUBLE"}}}).
2. Create four entities e1..e4, each with a component "C" of componentTypeId "com.example.base", with Threshold values 1, 2, 3, 4 and distinct Tag values.
3. POST /workspaces/ws/entity-properties/value with {"componentTypeId":"com.example.base","selectedProperties":["Tag","Threshold"],"tabularConditions":{"orderBy":[{"propertyName":"Threshold"}]},"maxResults":2} -> 200 with the rows for Threshold 1 and 2 and a nextToken whose ordinal is 2.
4. Repeat the request with that same nextToken but change the ordering to {"orderBy":[{"propertyName":"Threshold","order":"DESCENDING"}]}. The token is accepted and `skip(2)` is applied to the DESCENDING sequence, so the page served is Threshold 2 and 1 — rows the caller already has, while Threshold 3 and 4 are never reported.
5. Repeat step 3's token with {"propertyFilters":[{"propertyName":"Threshold","operator":">","value":{"doubleValue":3.5}}]} added. The filtered sequence holds one row, so resume_at=2 is past total=1: the answer is 200 with an empty tabularPropertyValues and no nextToken — a completed walk that reported nothing — instead of refusing a token this query did not mint.
6. Contrast a member that IS bound: repeat step 3's token with "propertyGroupName" added, or with a different "selectedProperties" — `decode_row_token` fails and the request is correctly refused with a ValidationException. The same class of change to `tabularConditions` is not.
