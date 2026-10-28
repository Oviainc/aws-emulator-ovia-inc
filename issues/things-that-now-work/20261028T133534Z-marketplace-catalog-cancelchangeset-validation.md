Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 87b02e9e0446b34956525b9761a3171af51589ac

# CancelChangeSet cancels a change set that has already reached APPLYING, which the operation's own documentation forbids

_Filed: 2026-10-28T13:35:34Z_
_Operation: CancelChangeSet | Class: validation_

> _Note: this issue was identified against the code base at commit `87b02e9e0446b34956525b9761a3171af51589ac`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CancelChangeSet`'s modelled documentation states that the cancel must be sent before the change set's status changes to `APPLYING`. The handler instead accepts a cancel for any change set that `is_running()`, and `is_running()` is true for both `PREPARING` and `APPLYING`. So a cancel arriving while the change set is in `APPLYING` is served 200, the record is flipped to `CANCELLED`, its `schedule` is cleared and its changes are never applied — the entities the change set was mid-way through applying are silently never created.

The crate contradicts the model in its own prose as well: `ChangeSetRecord::is_running`'s doc asserts that a change set in either running status "can still be cancelled", which is exactly the claim the operation's documentation denies for `APPLYING`. The window is reachable rather than theoretical: this crate schedules a distinct `APPLYING` phase of `CHANGE_SET_APPLYING` (250 ms) after `CHANGE_SET_PREPARING`, and the test suite already observes the `APPLYING` status (tests/all/change_set_lifecycle.rs:421). `ResourceInUseException` (HTTP 423, "The resource is currently in use") is the modelled error the handler already answers for a change set past cancelling.

## Evidence

Smithy, `CancelChangeSet` (smithy_query doc): "Used to cancel an open change request. Must be sent before the status of the request changes to APPLYING, the final stage of completing your change request."

The handler accepts APPLYING — services/service-com-amazonaws-marketplacecatalog-awsmpseymour/src/ops/cancel_change_set.rs:183-200:
```
        // Only a change set that is still running can be stopped: one that has
        // completed is in use by nobody, and its result stands.
        if !record.is_running() {
            let status = record.status.clone();
            return Err(CancelChangeSetErrorModeled::ResourceInUseException(
...
        record.status = crate::state::STATUS_CANCELLED.to_string();
        record.end_time = Some(crate::state::iso8601(now));
        // Cancelling is what the clock no longer owes anything after: the
        // changes this change set carried are never applied.
        record.schedule.clear();
```

`is_running` includes APPLYING — state.rs:726-730:
```
    /// True while the change set is still running, so its entities are locked
    /// and it can still be cancelled.
    pub fn is_running(&self) -> bool {
        self.status == STATUS_PREPARING || self.status == STATUS_APPLYING
    }
```

The APPLYING phase is a real, observable window — state.rs:56-59: "How long a change set then stays `APPLYING` before the clock owes it `SUCCEEDED` ... pub const CHANGE_SET_APPLYING: Duration = Duration::from_millis(250);"

Test coverage: tests/all/change_set_lifecycle.rs:664-737 (`cancelling_a_running_change_set_stops_it`) cancels immediately after the start — i.e. in PREPARING — and then asserts 423 only for a COMPLETED change set (lines 726-737). `grep -n APPLYING tests/` shows the status is only ever asserted as one of ["PREPARING", "APPLYING"] (change_set_lifecycle.rs:421, change_set_failures.rs:201); no test cancels a change set in APPLYING, so nothing pins the current behaviour.

## Steps to Reproduce

1. POST /StartChangeSet with {"Catalog":"AWSMarketplace","ChangeSet":[{"ChangeType":"CreateProduct","Entity":{"Type":"AmiProduct"},"Details":"{\"ProductTitle\":\"t\"}"}]} and keep the ChangeSetId.
2. Sleep CHANGE_SET_PREPARING + ~50 ms (past PREPARING, inside the APPLYING span) and GET /DescribeChangeSet?catalog=AWSMarketplace&changeSetId=<id> — confirm Status is "APPLYING".
3. PATCH /CancelChangeSet?catalog=AWSMarketplace&changeSetId=<id> — observe 200 with the ChangeSetId/ChangeSetArn, where the operation's documentation ("Must be sent before the status of the request changes to APPLYING") requires the modelled 423 ResourceInUseException.
4. GET /DescribeChangeSet again and sleep past CHANGE_SET_PREPARING + CHANGE_SET_APPLYING — Status is CANCELLED and POST /ListEntities for AmiProduct returns an empty EntitySummaryList: the change set that was already in its final applying stage applied nothing.
