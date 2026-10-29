Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: cd4f62d4622d99c848b6778d8f7c3e7dcd5cf5b1

# The Status filter's ValueList is never judged against ChangeStatus, so a mistyped status silently answers an empty page

_Filed: 2026-10-29T02:42:04Z_
_Operation: ListChangeSets | Class: validation_

> _Note: this issue was identified against the code base at commit `cd4f62d4622d99c848b6778d8f7c3e7dcd5cf5b1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

ListChangeSets judges a filter's NAME (via the shared `filter_predicate`) and, for the four before/after filters, judges the filter's VALUE — refusing a value that is not an ISO-8601 instant with the stated reason that "anything else would silently select everything or nothing". The `Status` filter gets no equivalent treatment: its `ValueList` entries are passed straight to `ChangeSetRecord::matches_filter`, which compares them to the record's status, so a value outside `ChangeStatus` (a lower-cased `"Succeeded"`, a typo, or a value from another API's status vocabulary) matches no change set and the call answers 200 with an empty `ChangeSetSummaryList` instead of telling the caller the filter value names no status.

The closed set is already in the crate: `CHANGE_STATUS_VALUES` is generated from the model's `ChangeStatus` enum into `ops/types/shared_types.rs` and is referenced nowhere in `src/`. The crate also states the opposite principle everywhere else it judges a value naming nothing the catalog has — an `EntityType` or a `ChangeType` outside the published set "is refused rather than listed as empty" — and it even applies that reasoning to a free-form details member (`Visibility` is checked against `ENTITY_VISIBILITY_VALUES`). The `Status` filter value is the one such value left unjudged.

## Evidence

The model names the value set for this filter — Smithy `Filter$ValueList` doc (`smithy_query(query="shape", shape="Filter")`): "`Status` - The supported `ValueList` is a list of statuses for all change set requests."

The sibling check in the SAME handler, `services/service-com-amazonaws-marketplacecatalog-awsmpseymour/src/ops/list_change_sets.rs:189`:
```
            if crate::state::CHANGE_SET_TIME_FILTER_NAMES.contains(&name.as_str()) {
                // A before/after filter compares against one instant, and only
                // another `DateTimeISO8601` is an instant this list can compare
                // its records' timestamps with: anything else would silently
                // select everything or nothing.
                ...
                if !crate::state::is_iso8601(&values[0]) {
                    return Err(ListChangeSetsErrorModeled::ValidationException(
```

The unjudged branch, `src/state.rs:807`:
```
            "Status" => values.iter().any(|value| value == &self.status),
```

The value set the crate already carries and never uses, `src/ops/types/shared_types.rs:139`:
```
pub const CHANGE_STATUS_VALUES: &[&str] = &["APPLYING", "CANCELLED", "FAILED", "PREPARING", "SUCCEEDED"];
```
(grep for `CHANGE_STATUS_VALUES` across the crate matches only this definition — no `src/` use.)

The principle the crate states for exactly this situation, `src/state.rs:281`:
```
/// A type the model names either way is one this catalog serves, so listing it
/// answers a page (empty while the catalog holds nothing of it) and a change set
/// may create one. A letters-only value naming NO type of the model names no
/// entity type at all, so it is refused rather than listed as empty.
```

Test coverage: the only Status-filter cases send modelled values — `tests/all/filter_validation.rs:214` (`"Status" => vec!["SUCCEEDED".to_string()]`) and `tests/all/change_set_lifecycle.rs:1374` (`[{"Name": "Status", "ValueList": ["SUCCEEDED", "FAILED"]}]`) — so no test pins acceptance of a value outside `ChangeStatus`.

## Steps to Reproduce

1. POST /StartChangeSet with {"Catalog":"AWSMarketplace","ChangeSet":[{"ChangeType":"CreateProduct","Entity":{"Type":"AmiProduct"},"Details":"{\"ProductTitle\":\"p\"}"}]} and wait for it to reach SUCCEEDED
2. POST /ListChangeSets {"Catalog":"AWSMarketplace","FilterList":[{"Name":"Status","ValueList":["Succeeded"]}]} — answered 200 with an empty ChangeSetSummaryList, as if the account had no change set in that state
3. POST /ListChangeSets {"Catalog":"AWSMarketplace","FilterList":[{"Name":"BeforeStartTime","ValueList":["Succeeded"]}]} — answered 422 ValidationException, because the sibling branch judges its filter value for exactly the reason that applies here
4. Compare: the Status row's value names no `ChangeStatus` member, and the crate already carries that member list in CHANGE_STATUS_VALUES
