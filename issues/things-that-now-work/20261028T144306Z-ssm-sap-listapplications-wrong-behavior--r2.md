Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: a13c4bd7833d49550a5f70ddd7b2f501243c6c8c

# Filter Name/Value bounds are re-stated in BYTES while the model (and the boundary validator) count characters

_Filed: 2026-10-28T14:43:06Z_
_Operation: ListApplications | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `a13c4bd7833d49550a5f70ddd7b2f501243c6c8c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::filters_within_bounds` re-states the model's `@length` bounds on `FilterName` (1..32) and `FilterValue` (1..64) using `String::len()`, which is the UTF-8 BYTE count. The modelled bound is a character count: the framework's own validator, which has already accepted the request before the handler runs, counts `chars().count()` for a string `@length`. So a filter whose `Name` or `Value` is within the modelled bound in characters but longer in bytes — i.e. any value carrying non-ASCII characters, which the model admits because `FilterValue` constrains only length — is accepted by the boundary validator and then refused by this crate with a `ValidationException` whose message states the byte count as though it were a character count ("Filter Value is at most 64 characters, but 80 were given").

The helper is the ONE validation every filtered listing of this service shares, so all four of them turn the same model-legal filter into a 400: `ListApplications`, `ListOperations`, `ListOperationEvents` and `ListConfigurationCheckOperations` each call `validate_filters`, which calls `filters_within_bounds` first.

This crate counts CHARACTERS everywhere else it re-states a modelled string bound — `rule_metadata` cuts a `RuleResultMetadata` value with `.chars().take(..)` and says so explicitly, and `database_id_from_name` cuts a `DatabaseId` the same way — so the byte count here is the one copy of the rule that disagrees with the rest of the crate.

## Evidence

Byte count (the defect), `services/service-com-amazonaws-ssmsap-ssmsap/src/state.rs:3991-4003`:
```rust
    for filter in filters {
        if filter.name.len() > FILTER_NAME_MAX_LENGTH {
            return Err(validation(format!(
                "Filter Name is at most {FILTER_NAME_MAX_LENGTH} characters, but {} were given",
                filter.name.len()
            )));
        }
        if filter.value.len() > FILTER_VALUE_MAX_LENGTH {
            return Err(validation(format!(
                "Filter Value is at most {FILTER_VALUE_MAX_LENGTH} characters, but {} were given",
                filter.value.len()
            )));
```
Its own doc claims it re-states the MODEL's bound (`state.rs:3951-3962`): "The model gives `FilterValue` `@length(min: 1, max: 64)` ... so the boundary validator already refuses a longer value; [`filters_within_bounds`] re-states the bound HERE" / "The longest `Filter$Name` a `ListApplications` request may carry: the model gives `FilterName` `@length(min: 1, max: 32)`."

The model's unit, `vendor/smithy-schema-server/src/constraints.rs:1046-1047`:
```rust
        // `@length` on a string counts characters, not bytes (`chars().count()`).
        let len = value.chars().count();
```

The same crate counting characters for the same kind of bound, `services/service-com-amazonaws-ssmsap-ssmsap/src/state.rs:1891-1906`:
```rust
/// ... and a longer one is cut to the first
/// [`MAX_RULE_RESULT_METADATA_VALUE_LENGTH`] CHARACTERS — the unit the constraint is
/// declared in, so a multi-byte name is neither split nor counted twice.
...
            let within_bound: String = value
                .chars()
                .take(MAX_RULE_RESULT_METADATA_VALUE_LENGTH)
```
and `state.rs:3474-3485` (`database_id_from_name` ... `.take(MAX_DATABASE_ID_LENGTH)` over `.chars()`).

Shared by all four filtered listings: `src/ops/list_applications.rs:161-166`, `src/ops/list_operations.rs:169-174`, `src/ops/list_operation_events.rs:182-187`, `src/ops/list_configuration_check_operations.rs:192-197` all call `crate::state::validate_filters`, whose first step is `filters_within_bounds(filters)?` (`state.rs:4064`).

Test coverage: `tests/all/filter_validation.rs` covers refused operators and refused filter names only; no test in the crate exercises a multi-byte `Filter$Name`/`Filter$Value`, so nothing pins the byte count.

## Steps to Reproduce

1. Register any application: POST /register-application {"ApplicationId":"HDB-FILTER","ApplicationType":"HANA","Instances":["i-0123456789abcdef0"],"Sid":"HDB","SapInstanceNumber":"00","Credentials":[{"DatabaseName":"SYSTEMDB","CredentialType":"ADMIN","SecretId":"sap/hdb/system"}]}
2. POST /list-applications with a filter whose Value is 40 non-ASCII characters (80 UTF-8 bytes), e.g. {"Filters":[{"Name":"Id","Value":"<40 Cyrillic characters>","Operator":"Equals"}]}
3. Observe HTTP 400 ValidationException "Filter Value is at most 64 characters, but 80 were given" — the value is 40 characters, inside the modelled `@length(min: 1, max: 64)` the framework validator already accepted, and the message reports a byte count as a character count
4. Repeat against POST /list-operations, POST /list-operation-events and POST /list-configuration-check-operations with the same filter: each refuses it identically, since all four share `validate_filters` -> `filters_within_bounds`
