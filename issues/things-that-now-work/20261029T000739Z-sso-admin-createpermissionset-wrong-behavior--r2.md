Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: f10a9c1d4031e9ac500bbdeee80ff5cb5da13a14

# CreatePermissionSet reads the clock twice, so the stored CreatedDate is dated after the state the call resolved against

_Filed: 2026-10-29T00:07:39Z_
_Operation: CreatePermissionSet | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `f10a9c1d4031e9ac500bbdeee80ff5cb5da13a14`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every other create in this crate captures ONE instant per request and uses it both for the clock-materialisation sweep (`on_request`) and for the record's own date, and the crate states that rule explicitly. CreatePermissionSet instead calls `SystemTime::now()` a second time when it builds the record, so the `CreatedDate` it stores (and answers with, and that every later DescribePermissionSet reports) comes from an instant AFTER the one the instance resolution, the account-instance check and the name-uniqueness check were decided at — exactly what the stated rule forbids: "the stored status cannot be dated before or after the state it describes". The same double reading appears in UpdateInstance (the encryption-configuration record's due instant) and DescribeInstance (the encryption status), so one response can report state settled at two different instants.

## Evidence

The rule, stated in this crate — services/service-com-amazonaws-ssoadmin-swbexternalservice/src/ops/delete_account_assignment.rs:186-189:
```
// ONE instant for the whole call: the instance resolution below and the deletion's
// own `CreatedDate` are the same moment, so the stored status cannot be dated
// before or after the state it describes.
let now = ::std::time::SystemTime::now();
```
Siblings that follow it: src/ops/create_account_assignment.rs:207 (`let now = ::std::time::SystemTime::now();`) used at 212 (`store.on_request(&region, now);`) and 265 (`let created_date = ::aws_smithy_types::DateTime::from(now);`); src/ops/create_application.rs:222 `let now = …` used at 227 (`store.on_request(&region, now);`) and 420 (`created_date: ::aws_smithy_types::DateTime::from(now),`); src/ops/create_instance.rs:213 `let now = …` used at 220 and in `Instance::creating(…, now, …)`.

The outlier — src/ops/create_permission_set.rs:186 and :266:
```
186:        store.on_request(&region, ::std::time::SystemTime::now());
…
266:            created_date: ::aws_smithy_types::DateTime::from(::std::time::SystemTime::now()),
```
Same pattern in src/ops/update_instance.rs:298 (`store.on_request(&region, ::std::time::SystemTime::now());`) and :334 (`::std::time::SystemTime::now(),` passed to `EncryptionConfigurationRecord::new`), and src/ops/describe_instance.rs:212 vs :254 (`.to_shape(::std::time::SystemTime::now())`).

Test coverage of the created date: tests/all/permission_sets.rs / permission_set_describe-style suites assert the create's `PermissionSet.CreatedDate` round-trips through DescribePermissionSet, which it still does — the defect is which instant it is taken at, not that it is stored.

## Steps to Reproduce

1. Read src/ops/create_permission_set.rs: line 186 passes a fresh `SystemTime::now()` to `store.on_request`, and line 266 takes a SECOND `SystemTime::now()` for the stored `created_date`.
2. Compare with src/ops/create_account_assignment.rs:207/212/265, src/ops/create_application.rs:222/227/420 and src/ops/create_instance.rs:213/220, which all bind `let now` once and reuse it.
3. Compare with the rule stated in src/ops/delete_account_assignment.rs:186-189 ("ONE instant for the whole call … so the stored status cannot be dated before or after the state it describes").
4. Call CreatePermissionSet: the returned CreatedDate is later than the instant at which the instance state (expiry/activation sweep) used to accept the call was evaluated.
