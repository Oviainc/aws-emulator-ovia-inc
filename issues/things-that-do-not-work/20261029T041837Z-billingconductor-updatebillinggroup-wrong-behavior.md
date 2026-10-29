Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 1fd13cac8c9d59f908c3acc103dd89ab8a3a1d61

# An UpdateBillingGroup naming the Status the group already reports (ACTIVE) is treated as a change: LastModifiedTime moves and a generation is frozen

_Filed: 2026-10-29T04:18:37Z_
_Operation: UpdateBillingGroup | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `1fd13cac8c9d59f908c3acc103dd89ab8a3a1d61`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateBillingGroup` decides whether the call modified anything by comparing the merged record with the stored one, and the handler states the rule it is implementing: `LastModifiedTime` "moves only when this call actually modified the group: a request that names no member, or names the values the group already holds, leaves it where it was."

The `Status` term of that comparison is `updated.requested_status != stored.requested_status` — the INTERNAL nullable field, not the status the group reports. The crate documents `requested_status: None` as reading back as `ACTIVE`, and `status_of` maps `None` and `Some("ACTIVE")` to the identical reported `ACTIVE`. So for a group that has never had a status set — every group straight out of `CreateBillingGroup`, which stores `requested_status: None` — a request carrying `Status: "ACTIVE"` names exactly the value every read of the group already reports, yet `changed` is true.

Two observable consequences, both the ones the rule exists to prevent:
1. `LastModifiedTime` moves (it is what the 200 response and every later `ListBillingGroups` read report for the group), although no member any read reports differs before and after.
2. The write goes through `put_billing_group`, which freezes the current generation as a superseded one whenever the write lands in a billing period later than the one that generation started in. So a call that changed nothing consumes a slot in the bounded version history and makes a period-scoped read be answered from a generation minted by a no-op.

The two values can never become distinguishable later: both members `status_of` derives from (`primary_account_id`, `responsibility_transfer_arn`) are immutable — `UpdateBillingGroupInput` carries neither, and the handler refuses any change to `ResponsibilityTransferArn` outright — so `None` and `Some("ACTIVE")` are the same group forever.

The fix is the comparison the comment already describes: compare the status the group REPORTS (`status_of`) rather than the raw `requested_status`, leaving the `PENDING` cases and both refusal paths exactly as they are.

## Evidence

The rule and the comparison that breaks it — `src/ops/update_billing_group.rs:360-384`:
```
        // `LastModifiedTime` is "the most recent time when the billing group was
        // modified", so it moves only when this call actually modified the group: a
        // request that names no member, or names the values the group already holds,
        // leaves it where it was.
        let changed = updated.name != stored.name
            || updated.description != stored.description
            || updated.pricing_plan_arn != stored.pricing_plan_arn
            || updated.requested_status != stored.requested_status
            || updated.auto_associate != stored.auto_associate;
        if changed {
            updated.last_modified_time_millis = crate::state::now_millis();
...
            state.put_billing_group(updated.clone());
        }
```
with the assignment that makes `Some("ACTIVE")` out of a request — `src/ops/update_billing_group.rs:349-351`:
```
        if let Some(status) = input.status.clone() {
            updated.requested_status = Some(status);
        }
```

That `None` and `Some("ACTIVE")` are the SAME reported status — `src/state.rs:1285-1289`:
```
    /// The operational `BillingGroupStatus` a caller asked this group to hold, one
    /// of [`CALLER_SETTABLE_BILLING_GROUP_STATUSES`]. `None` for a group no caller
    /// has set a status on, which reads back as `ACTIVE` — the status a group with a
    /// billing owner holds until something says otherwise.
    pub requested_status: Option<String>,
```
and `src/state.rs:1390-1398`:
```
    pub fn status_of(&self, version: &BillingGroupVersion) -> &'static str {
        if !self.has_billing_owner() {
            return "PRIMARY_ACCOUNT_MISSING";
        }
        match version.requested_status.as_deref() {
            Some("PENDING") => "PENDING",
            _ => "ACTIVE",
        }
    }
```
and that a create always leaves `None` — `src/ops/create_billing_group.rs:434-436`:
```
            // `CreateBillingGroup` takes no `Status`: a created group holds the status
            // its billing owner implies until an `UpdateBillingGroup` asks for one.
            requested_status: None,
```

The extra generation — `src/state.rs:3054-3062`:
```
    pub fn put_billing_group(&mut self, mut updated: BillingGroupRecord) {
        let arn = updated.arn.clone();
        let superseded = self
            .billing_groups
            .get(&arn)
            .filter(|previous| {
                previous.version_start_billing_period() != updated.version_start_billing_period()
            })
            .map(BillingGroupRecord::current_version);
```

Test coverage: `tests/all/update_billing_group.rs:1119-1147` is `last_modified_time_moves_only_when_the_group_changed`, whose own premise is "Rows differ along whether the request asks for anything the group does not already hold: no member at all, the values it holds, and each kind of real change". Its rows cover `Name`, `Description` and the pricing plan the group already holds (expected `false`) and `"Status": "PENDING"` as a real change (expected `true`) — there is NO row for the status the group already reports, so nothing pins the current answer:
```
        (
            "the Name it already holds",
            |_| json!({ "Name": "ClockGroup" }),
            false,
        ),
...
        ("a new Status", |_| json!({ "Status": "PENDING" }), true),
```

## Steps to Reproduce

1. Designate the emulated account as the payer so the control plane is reachable; create a pricing plan and a billing group G with a PrimaryAccountId (so it has a billing owner).
2. POST /list-billing-groups and note G's Status ("ACTIVE") and LastModifiedTime (call it T0).
3. Wait a few milliseconds, then POST /update-billing-group with {"Arn":"<G>","Status":"ACTIVE"} — the value the previous read reported.
4. Observe the 200 response's LastModifiedTime > T0, and a following POST /list-billing-groups reporting the moved value, although Status, StatusReason and every other reported member are unchanged.
5. Compare: POST /update-billing-group with {"Arn":"<G>","Name":"<its current name>"} leaves LastModifiedTime at its previous value, and a second {"Arn":"<G>","Status":"ACTIVE"} call also leaves it alone (requested_status is now Some("ACTIVE")), so only the first restatement is treated as a change.
6. To see the extra generation: make the no-op Status="ACTIVE" call in a billing period later than the one G was created in, then read G back with POST /list-billing-groups naming the creation period — it is now answered from a generation frozen by the call that changed nothing.
