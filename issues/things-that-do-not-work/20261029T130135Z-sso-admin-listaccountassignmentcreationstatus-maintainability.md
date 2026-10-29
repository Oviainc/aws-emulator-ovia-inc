Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 4cfc65c0defa14c543cb5de0c0380dc37d9ef00f

# Single-home the "a status filter value must be one of the modelled enum's values" refusal: four handlers rebuild it, a fifth sibling reading the same enum-typed member relies on the constraint walk instead

_Filed: 2026-10-29T13:01:35Z_
_Operation: ListAccountAssignmentCreationStatus | Class: maintainability_

> _Note: this issue was identified against the code base at commit `4cfc65c0defa14c543cb5de0c0380dc37d9ef00f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Five list handlers of this crate read a status-filter member whose target is a modelled `enum` shape, and they answer the question "who refuses a value outside that enum?" two contradictory ways.

FOUR of them rebuild the same rule in handler code — match the filter, test membership against the generated value list, and refuse with the crate's own `…_unknown_message` on the operation's `ValidationException`:

* `list_account_assignment_creation_status.rs` (`Filter.Status` -> `StatusValues`)
* `list_account_assignment_deletion_status.rs` (`Filter.Status` -> `StatusValues`, the same block word for word apart from one noun)
* `list_accounts_for_provisioned_permission_set.rs` (`ProvisioningStatus` -> `ProvisioningStatus`)
* `list_permission_sets_provisioned_to_account.rs` (`ProvisioningStatus` -> `ProvisioningStatus`, again word for word)

The FIFTH, `list_permission_set_provisioning_status.rs`, reads the *same* `OperationStatusFilter$Status` member as the first two and documents the opposite decision — the constraint walk owns the refusal, so the handler takes the filter through untested. That is the drift this class is for: one rule, four copies, and a fifth reader that has already moved off them.

The fifth reader is the one that is right, and the crate's own test says so: `tests/all/permission_set_provisioning_status_list.rs` sends four out-of-enum spellings to `ListPermissionSetProvisioningStatus`, asserts 400 `ValidationException` for each, and then asserts "A refused request reached no handler". So the four handler-side copies sit behind a walk that already refuses, i.e. they are unreachable — which is also why no test anywhere exercises `crate::state::operation_status_unknown_message` or `crate::state::provisioning_status_unknown_message` (grep over `tests/` finds neither message, and neither spelling of the refusal, anywhere).

The crate has already single-homed the *message* of this refusal in `state.rs` (two helpers built from the generated value lists, each documented as built "exactly as" the other) but not the *decision*, and it has a settled idiom for exactly this shape of rule: `state.rs` owns the decision and answers a `Result<_, String>`/refusal enum that each handler maps onto its own error variant (`permission_set_surface_gate`, `cursor_after`, `tag_target`). The status-filter guard is the one rule of that kind still copied into the handlers.

Fix: fold it into one `state.rs` entry point the status-filtering enumerations share (or drop the four dead copies in favour of the walk the fifth sibling and its test already rely on), so the crate states once whether the enum guard belongs in a handler at all. No response changes: the guard is unreachable behind the constraint walk, so neither the pages these four enumerations answer nor the 400 an out-of-enum filter already earns moves.

## Evidence

MODEL — all five filters go through an enum-typed member, so the same walk covers them (`smithy_query(query="shape", shape="StatusValues,OperationStatusFilter,ProvisioningStatus")`):
```
"StatusValues": {"type": "enum", "values": ["IN_PROGRESS", "FAILED", "SUCCEEDED"]}
"OperationStatusFilter": {"type": "structure", "members": {"Status": {"target": "StatusValues", "type": "enum", ...}}}
"ProvisioningStatus": {"type": "enum", "values": ["LATEST_PERMISSION_SET_PROVISIONED", "LATEST_PERMISSION_SET_NOT_PROVISIONED"]}
```
`ListAccountAssignmentCreationStatus`, `ListAccountAssignmentDeletionStatus` and `ListPermissionSetProvisioningStatus` all declare `"Filter": "OperationStatusFilter"` — the same member.

COPY 1 — services/service-com-amazonaws-ssoadmin-swbexternalservice/src/ops/list_account_assignment_creation_status.rs:173-198:
```
        // The `StatusValues` the request filters by, as one of the values the model's enum
        // declares (`STATUS_VALUES_VALUES`, the one source for their spelling). Reaching
        // this handler means the modelled constraints already hold, so anything else
        // cannot arrive here; it is still refused rather than answered with an empty page,
        // which would read as "no creation is in that state".
        let filter = match input
            .filter
            .as_ref()
            .and_then(|filter| filter.status.as_deref())
        {
            None => None,
            Some(status) if STATUS_VALUES_VALUES.contains(&status) => Some(status),
            Some(status) => {
                return Err(
                    ListAccountAssignmentCreationStatusErrorModeled::ValidationException(
                        ValidationException {
                            message: Some(crate::state::operation_status_unknown_message(status)),
```

COPY 2 — .../src/ops/list_account_assignment_deletion_status.rs:173-198, the same block (`"no deletion is in that state"` is the only change):
```
            Some(status) if STATUS_VALUES_VALUES.contains(&status) => Some(status),
            Some(status) => {
                return Err(
                    ListAccountAssignmentDeletionStatusErrorModeled::ValidationException(
                        ValidationException {
                            message: Some(crate::state::operation_status_unknown_message(status)),
```

COPY 3 — .../src/ops/list_accounts_for_provisioned_permission_set.rs:185-208:
```
        // The `ProvisioningStatus` the request filters by, as one of the values the
        // model's enum declares (`PROVISIONING_STATUS_VALUES`, the one source for their
        // spelling). Reaching this handler means the modelled constraints already hold, so
        // anything else cannot arrive here; it is still refused rather than answered with
        // an empty page, which would read as "no account is in that state".
        let filter = match input.provisioning_status.as_deref() {
            None => None,
            Some(status) if PROVISIONING_STATUS_VALUES.contains(&status) => Some(status),
```

COPY 4 — .../src/ops/list_permission_sets_provisioned_to_account.rs:185-208, the same block (`"no permission set is in that state"`):
```
            Some(status) if PROVISIONING_STATUS_VALUES.contains(&status) => Some(status),
            Some(status) => {
                return Err(
                    ListPermissionSetsProvisionedToAccountErrorModeled::ValidationException(
                        ValidationException {
                            message: Some(crate::state::provisioning_status_unknown_message(
```

THE FIFTH SIBLING, ON THE SAME MODELLED MEMBER, DECIDED THE OPPOSITE — .../src/ops/list_permission_set_provisioning_status.rs:174-182:
```
        // The `StatusValues` the request filters by ("filters the list operations result
        // based on the status attribute"). The model declares `Filter$Status` as that
        // enum, so a value outside it is refused by the constraint walk before this
        // handler runs; a value inside it that no tracked provisioning is in selects no
        // row, which is an empty page rather than a refusal.
        let filter = input
            .filter
            .as_ref()
            .and_then(|filter| filter.status.as_deref());
```

THE CRATE'S OWN TEST PROVES THE WALK REFUSES FIRST, SO THE FOUR COPIES ARE UNREACHABLE — .../tests/all/permission_set_provisioning_status_list.rs:524-562:
```
/// RULE 2, continued — a `Filter$Status` outside the `StatusValues` enum the model declares
/// is refused as a `ValidationException`, never answered with an empty page.
#[tokio::test]
async fn a_filter_status_outside_the_modelled_enum_is_refused() {
...
    for status in ["in_progress", "INPROGRESS", "LATEST_PERMISSION_SET_PROVISIONED", ""] {
        let (code, body) = list(&router, &instance_arn, Some(status), None, None).await;
        assert_eq!(code, 400, ...);
        assert_eq!(error_code(&body), "ValidationException", ...);
    }

    // A refused request reached no handler, so the history is untouched and still answers.
```

THE MESSAGE IS ALREADY SINGLE-HOMED, THE DECISION IS NOT — .../src/state.rs:571-594:
```
pub fn provisioning_status_unknown_message(provisioning_status: &str) -> String {
    format!("ProvisioningStatus must be one of {}, got {provisioning_status}", ...)
}
...
/// earns, as the message of the caller's own `ValidationException`. BUILT from the member
/// the request got wrong and the values the model declares for it
/// ([`crate::ops::types::STATUS_VALUES_VALUES`]), exactly as
/// [`provisioning_status_unknown_message`] is, ...
pub fn operation_status_unknown_message(status: &str) -> String {
```
and the idiom the crate uses for every other shared rule — .../src/state.rs:3140-3156:
```
    /// This step has ONE refusal, so it answers that message directly and needs no refusal
    /// enum — the division [`TagWriteRefusal`] keeps for the reporting side of the tag
    /// surface. The same rule either way: [`Self::permission_set_surface`] asks exactly
    /// this first, so the gate has one implementation and an operation that takes both
    /// steps together cannot answer it differently from one that takes them apart.
    pub fn permission_set_surface_gate(&self, instance_arn: &str) -> Result<(), String> {
```

TESTS THAT COVER THESE PATHS AND MUST STAY GREEN UNCHANGED:
* tests/all/account_assignment_creation_status_list.rs:385 `the_status_filter_selects_exactly_the_creations_in_that_status`
* tests/all/account_assignment_deletion_status_list.rs:367 `the_status_filter_selects_exactly_the_rows_in_that_status`
* tests/all/accounts_for_provisioned_permission_set.rs:500 `the_provisioning_status_filter_selects_by_the_definition_an_account_holds`
* tests/all/permission_sets_provisioned_to_account.rs:548 `the_provisioning_status_filter_selects_by_the_definition_the_account_holds`
* tests/all/permission_set_provisioning_status_list.rs:527 `a_filter_status_outside_the_modelled_enum_is_refused` (the one that pins who refuses)
No test sends an out-of-enum status to any of the four copying handlers, and no test asserts either `…_unknown_message` text — so none of them observes the copies at all.

## Steps to Reproduce

1. Open src/ops/list_account_assignment_creation_status.rs:173-198 and src/ops/list_account_assignment_deletion_status.rs:173-198 side by side: the same `match`/`contains`/`ValidationException` block, differing only in one noun of the comment and the error enum's name.
2. Open src/ops/list_accounts_for_provisioned_permission_set.rs:185-208 and src/ops/list_permission_sets_provisioned_to_account.rs:185-208: the same block again, against PROVISIONING_STATUS_VALUES.
3. Open src/ops/list_permission_set_provisioning_status.rs:174-182 — the same modelled member (`OperationStatusFilter$Status`) as step 1, with no guard and a comment stating the opposite rule: "a value outside it is refused by the constraint walk before this handler runs".
4. Confirm the fifth reader is right by running tests/all/permission_set_provisioning_status_list.rs::a_filter_status_outside_the_modelled_enum_is_refused — four out-of-enum spellings each answer 400 ValidationException and the test asserts "A refused request reached no handler".
5. grep tests/ for `operation_status_unknown`, `provisioning_status_unknown`, `must be one of` — no hit, so neither of the four copies' refusals is reachable or observed by any test.
6. Fold the guard into one state.rs entry point (or delete the four dead copies in favour of the walk), then re-run the five tests named in the evidence: every page and every 400 is unchanged.
