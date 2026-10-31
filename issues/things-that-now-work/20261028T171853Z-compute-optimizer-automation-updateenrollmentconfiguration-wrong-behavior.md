Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 68e4a9e742a6835063083b9abc795c05ac8a482e

# A repeat of the same enrollment status re-stamps the account's lastUpdatedTimestamp, although the two sibling writers of that same field stamp only when the stored configuration changed

_Filed: 2026-10-28T17:18:53Z_
_Operation: UpdateEnrollmentConfiguration | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `68e4a9e742a6835063083b9abc795c05ac8a482e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateEnrollmentConfiguration` writes `enrollment.last_updated_timestamp = Some(stored_at)` unconditionally, without comparing the requested status against the stored one. So calling `UpdateEnrollmentConfiguration {"status": "Active"}` twice (with no `clientToken`, or with two different ones, so no replay is involved) moves the account's `lastUpdatedTimestamp` on the second call even though nothing about the enrollment configuration changed: the status is the same, `statusReason` was already absent, and `ensure_recommended_action_inventory` is idempotent. `GetEnrollmentConfiguration` and `ListAccounts` then both report an update instant at which nothing was updated.

That contradicts the crate in two places:

1. The only other two writers of this exact field — `associate_account` and `disassociate_account` in `state.rs` — deliberately stamp it ONLY when the stored configuration changed, and say so, and that no-op rule is pinned by a test that explicitly avoids the client-token replay path (`tests/all/account_association.rs:360-388`, "A second association of the same account, under a DIFFERENT client token (so no replay is involved) … nothing changes — the stored timestamp included").

2. The handler's own replay arm states the rule it then fails to apply outside the ledger: "A retry of the same call: report the enrollment as stored, without re-stamping its timestamp." So an identical retry is a no-op when the caller happened to send a `clientToken` and a re-stamp when it did not — the same request, two different stored outcomes, decided by an optional member.

A fixer should note the one edge the current unconditional write papers over: the very first call for an account that the store has no entry for yet creates the entry, which IS a change (and `UpdateEnrollmentConfigurationResponse$lastUpdatedTimestamp` is `@required`), so "changed" has to include entry creation — exactly as `associate_account` treats a fresh `or_default()` entry whose status differs from `ENROLLED_STATUS`.

## Evidence

The unconditional stamp — `src/ops/update_enrollment_configuration.rs:251-259`:
```
        let stored_at = crate::state::now();
        {
            let enrollment = guard.enrollments.entry(account_id.clone()).or_default();
            enrollment.status = input.status.clone();
            // The reason belongs to the status it explains; a status the caller
            // set successfully has none.
            enrollment.status_reason = None;
            enrollment.last_updated_timestamp = Some(stored_at);
        }
```

The same handler's own statement of the no-re-stamp rule — `src/ops/update_enrollment_configuration.rs:215-226`:
```
            // A retry of the same call: report the enrollment as stored, without
            // re-stamping its timestamp.
            crate::state::TokenOutcome::Replay(_) => {
                let stored = guard.enrollment(&account_id);
                return Ok(UpdateEnrollmentConfigurationOutput {
                    status: stored.status,
                    status_reason: stored.status_reason,
                    last_updated_timestamp: stored
                        .last_updated_timestamp
                        .unwrap_or_else(crate::state::now),
                });
            }
```

The two sibling writers of the SAME field, which stamp only on change — `src/state.rs:3169-3192`:
```
    /// Answers whether the stored configuration CHANGED. `at` is stamped as the
    /// account's `lastUpdatedTimestamp` ("The timestamp when the account's
    /// Automation enrollment status was last updated") only when it did, so
    /// re-associating an account that is already associated and enrolled leaves
    /// its stored configuration, timestamp included, exactly where it was.
    pub fn associate_account(&mut self, account_id: &str, at: ::aws_smithy_types::DateTime) -> bool {
...
        if changed {
            enrollment.last_updated_timestamp = Some(at);
        }
        changed
    }
```
and `src/state.rs:3215-3233`:
```
    /// Answers whether the stored configuration CHANGED. `at` is stamped as the
    /// account's `lastUpdatedTimestamp` only when it did, so disassociating an
    /// account that carries no association leaves its stored configuration,
    /// timestamp included, exactly where it was.
    pub fn disassociate_account(
...
        if enrollment.reported_organization_rule_mode() == ORGANIZATION_RULE_MODE_NONE_ALLOWED {
            return false;
        }
```

The modelled meaning of the member — `smithy_query(query="doc",shape="UpdateEnrollmentConfigurationResponse",member="lastUpdatedTimestamp")`:
```
<p> The timestamp when the enrollment configuration was last updated. </p>
```
and `smithy_query(query="doc",shape="AccountInfo",member="lastUpdatedTimestamp")`:
```
<p>The timestamp when the account's Automation enrollment status was last updated. </p>
```

Test coverage: the sibling no-op rule IS pinned, deliberately outside the replay path — `tests/all/account_association.rs:360-388`:
```
    // A second association of the same account, under a DIFFERENT client token
    // (so no replay is involved): the account is already associated, so nothing
    // changes — the stored timestamp included.
...
    assert_eq!(
        member(&read_back, "lastUpdatedTimestamp"),
        &stamped,
        "case \"after-re-association\": re-associating an associated account changes nothing, \
         so its update time stands, got {read_back}"
    );
```
(`tests/all/account_disassociation.rs:457-483` pins the same for the removal.) The enrollment side is pinned only across STATUS CHANGES — `tests/all/automation_rule_lifecycle.rs:226-258` walks the cases `("Active", true), ("Inactive", true), …` and asserts `assert_ne!(stamped, expected_timestamp, "case {status_value:?}: each update must stamp its own store time")` — every settable case in that table changes the status, so no test sends the SAME status twice and stamping only on change leaves every existing assertion green.

## Steps to Reproduce

1. Mount the emulator.
2. Call UpdateEnrollmentConfiguration with {"status": "Active", "clientToken": "enroll-1"} — 200. Record lastUpdatedTimestamp as S1.
3. Call GetEnrollmentConfiguration with {} — status Active, lastUpdatedTimestamp == S1.
4. Wait a moment, then call UpdateEnrollmentConfiguration with {"status": "Active", "clientToken": "enroll-2"} — a DIFFERENT token, so src/ops/update_enrollment_configuration.rs:213 classifies it Fresh and the replay arm is not taken. Observe 200 with lastUpdatedTimestamp S2 > S1, although the stored configuration is byte-for-byte what it was (status Active, no statusReason).
5. Call GetEnrollmentConfiguration and ListAccounts: both now report S2 as the instant the enrollment status was last updated, for an update that changed nothing.
6. Re-run step 4 with the SAME token "enroll-1" instead: the replay arm returns S1 unchanged (src/ops/update_enrollment_configuration.rs:215-226). The identical request therefore has two different effects on stored state depending on an optional member.
7. Compare the sibling writer of the same field: AssociateAccounts {"accountIds": ["111111111111"], "clientToken": "a1"} then AssociateAccounts {"accountIds": ["111111111111"], "clientToken": "a2"} — the member's lastUpdatedTimestamp in ListAccounts does NOT move, as tests/all/account_association.rs:360-388 pins.
