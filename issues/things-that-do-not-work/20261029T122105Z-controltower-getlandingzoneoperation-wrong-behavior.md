Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 83aee2831731f4d9adb4a4e165486d477c375438

# Operation details resolve forever: the documented 90-day availability window is applied to the listing but not to GetLandingZoneOperation/GetControlOperation

_Filed: 2026-10-29T12:21:05Z_
_Operation: GetLandingZoneOperation | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `83aee2831731f4d9adb4a4e165486d477c375438`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

GetLandingZoneOperation's own modelled documentation states how long an operation's details are available, and the crate implements that sentence for the sibling LISTING while leaving the by-identifier read with no age bound at all.

The model says: "Returns the status of the specified landing zone operation. Details for an operation are available for 90 days." GetControlOperation carries the same sentence ("Details for an operation are available for 90 days.").

The handler, however, answers from the raw map with no test on the record's age: `store.landing_zone_operations.get(&input.operation_identifier)`. A record whose `start_time` is a year old is still reported with full details. The ONLY thing that ever drops a record is `enforce_log_bound`, a COUNT bound (`MAX_LANDING_ZONE_OPERATIONS = 1024`), and the comment that justifies that constant asserts the opposite of what the model says: "AWS documents no retention for this log, so the value is CHOSEN". That premise is false — the operation's own doc sentence, reproduced verbatim in this very crate at `src/ops/get_landing_zone_operation.rs:49-50`, documents a 90-day availability window.

That the crate treats these doc sentences as normative is not in doubt: `ListLandingZoneOperations` derives a 90-day constant from its own one-line doc and filters the listing by it, and the test suite pins that bound with explicit past-the-window cases. The result is a crate that answers two contradictory ways about the same stored record: ListLandingZoneOperations says a 91-day-old operation is outside the window it reports, while GetLandingZoneOperation (and GetControlOperation) report it as though no window existed.

The defect is the missing age bound on the by-identifier reads (and the false premise comment that stands in for it), not the count bound, which the crate justifies separately as a store bound.

## Evidence

Smithy model (smithy_query query="doc" shape="GetLandingZoneOperation"):
  "<p>Returns the status of the specified landing zone operation. Details for an operation are available for 90 days.</p>"
and (shape="GetControlOperation"):
  "<p>Returns the status of a particular <code>EnableControl</code> or <code>DisableControl</code> operation. Displays a message in case of error. Details for an operation are available for 90 days. ...</p>"

The same sentence is already reproduced inside the crate:
  services/service-com-amazonaws-controltower-awscontroltowerapis/src/ops/get_landing_zone_operation.rs:49-50
    "//! Returns the status of the specified landing zone operation. Details for an operation are"
    "//! available for 90 days."
  services/service-com-amazonaws-controltower-awscontroltowerapis/src/ops/get_control_operation.rs:57
    "//! message in case of error. Details for an operation are available for 90 days. For usage"

The handler applies no age bound:
  services/service-com-amazonaws-controltower-awscontroltowerapis/src/ops/get_landing_zone_operation.rs:167-170
    "        let Some(operation) = store"
    "            .landing_zone_operations"
    "            .get(&input.operation_identifier)"
    "        else {"
  services/service-com-amazonaws-controltower-awscontroltowerapis/src/ops/get_control_operation.rs:180
    "        let Some(operation) = store.control_operations.get(&input.operation_identifier) else {"

The only retention rule is a COUNT, justified by a premise the model contradicts:
  services/service-com-amazonaws-controltower-awscontroltowerapis/src/state.rs:110-116
    "/// A record stays resolvable through `GetLandingZoneOperation` until the log is full, at"
    "/// which point the OLDEST terminal record is evicted to make room. Without this cap a"
    "/// caller repeating create/delete would grow the store, and the `settle` scan held under"
    "/// the state lock, without limit. AWS documents no retention for this log, so the value"
    "/// is CHOSEN: large enough that no lifecycle an emulator client drives reaches it, small"
    "/// enough to bound the store."
    "pub const MAX_LANDING_ZONE_OPERATIONS: usize = 1024;"

The sibling listing DOES implement the 90-day sentence, and says so:
  services/service-com-amazonaws-controltower-awscontroltowerapis/src/ops/list_landing_zone_operations.rs:122-127
    "/// How far back a listing reaches."
    "/// From this operation's own docs — it \"lists all landing zone operations from the past 90"
    "/// days\" — so an older record is not listed, even while `GetLandingZoneOperation` still"
    "/// resolves it by identifier for as long as the log retains it."
    "const LISTED_WINDOW: ::std::time::Duration = ::std::time::Duration::from_secs(90 * 24 * 60 * 60);"
  and applies it at line 214: "&& earliest_listed.is_none_or(|earliest| record.start_time >= earliest)"

Test coverage: the listing bound is pinned, the by-identifier read is not.
  services/service-com-amazonaws-controltower-awscontroltowerapis/tests/all/landing_zone_operations.rs:65-66
    "/// operations from the past 90 days\" (the operation's model docs)."
    "const LISTED_WINDOW: Duration = Duration::from_secs(90 * 24 * 60 * 60);"
  and lines 712-713 pin the out-of-window cases:
    "        (\"a day past the window\", LISTED_WINDOW + DAY, false),"
    "        (\"a month past the window\", LISTED_WINDOW + 30 * DAY, false),"
  No test asserts anything about a GetLandingZoneOperation/GetControlOperation record past 90 days.

## Steps to Reproduce

1. Seed the service store with one landing zone operation record whose `start_time`/`end_time` are 91 days in the past, the way tests/all/landing_zone_operations.rs already seeds records at `LISTED_WINDOW + DAY` (see tests/all/landing_zone_pending_transitions.rs for the direct-seeding helper).
2. Call ListLandingZoneOperations with no filter: the seeded record is absent, because list_landing_zone_operations.rs:214 drops anything older than LISTED_WINDOW (90 days).
3. Call GetLandingZoneOperation with that same `operationIdentifier`: HTTP 200 with the full `operationDetails`, although GetLandingZoneOperation's own modelled doc says "Details for an operation are available for 90 days."
4. Repeat with a control operation record and GetControlOperation (same doc sentence, same missing bound at get_control_operation.rs:180).
5. Observe the contradiction inside the crate: src/state.rs:113 states "AWS documents no retention for this log" while src/ops/get_landing_zone_operation.rs:49-50 reproduces the model sentence that documents exactly such a window.
