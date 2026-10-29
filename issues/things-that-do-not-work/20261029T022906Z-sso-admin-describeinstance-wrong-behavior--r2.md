Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 77670ec594ee56220d3fb04f56c8d32fdb5a5e9f

# DescribeInstance derives EncryptionStatus from a SECOND clock read, so one response reports the instance's state and its encryption state at two different instants

_Filed: 2026-10-29T02:29:06Z_
_Operation: DescribeInstance | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `77670ec594ee56220d3fb04f56c8d32fdb5a5e9f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every other clock-dependent READ of this crate captures ONE instant before taking the store lock and derives both the state it resolves and every status it reports from that instant. `DescribeInstance` does not: it reads the clock once to settle what the clock owes the instances (`on_request`), resolves the instance from the state that read produced, and then reads the clock a SECOND time to derive `EncryptionConfigurationDetails$EncryptionStatus`.

The status is a pure function of the instant it is asked at (`EncryptionConfigurationRecord::status`: `UPDATING` while `effective_at` is in the future, `ENABLED` once it is not), so the response can carry a `Status`/`Regions` view computed at T1 beside an `EncryptionStatus` computed at T2 > T1 — the encryption key change reported as settled at an instant the rest of the response was not computed at. The handler's own comment on the second read claims the opposite ("so this stays a pure READ"), and the crate states the single-instant rule explicitly where it is kept.

The fix is local: bind `let now = ::std::time::SystemTime::now();` before the lock and pass that `now` to both `on_request` and `to_shape`, exactly as the five sibling status readers do. No response value and no existing test changes (the encryption tests sleep past `ENCRYPTION_UPDATE_DELAY` before asserting).

This is NOT the already-fixed DescribeInstance gap (`20261024T151922Z-sso-admin-describeinstance-wrong-behavior.md`, which was about the member being omitted until UpdateInstance set one); the member is now always present and this is about the instant its status is derived at.

## Evidence

The two clock reads in one handler — services/service-com-amazonaws-ssoadmin-swbexternalservice/src/ops/describe_instance.rs:210-212:
```rust
        let region = crate::state::request_region(ctx.parts());
        let mut guard = state.lock();
        guard.on_request(&region, ::std::time::SystemTime::now());
```
and describe_instance.rs:249-255:
```rust
            // is derived from the instant a replacement becomes effective, so this stays
            // a pure READ.
            encryption_configuration_details: Some(
                instance
                    .encryption
                    .to_shape(::std::time::SystemTime::now()),
            ),
```

The reported status IS a function of that argument — state.rs:1101-1107:
```rust
    pub fn status(&self, now: SystemTime) -> &'static str {
        if self.effective_at.is_some_and(|effective_at| now < effective_at) {
            KMS_KEY_STATUS_UPDATING
        } else {
            KMS_KEY_STATUS_ENABLED
        }
    }
```

The rule this crate states for itself — services/service-com-amazonaws-ssoadmin-swbexternalservice/src/ops/delete_account_assignment.rs:186-189:
```rust
        // ONE instant for the whole call: the instance resolution below and the deletion's
        // own `CreatedDate` are the same moment, so the stored status cannot be dated
        // before or after the state it describes.
        let now = ::std::time::SystemTime::now();
```

The sibling status READERS that keep it, each with a single `let now` passed to both `on_request` and the projection:
* describe_account_assignment_creation_status.rs:171 `let now = ::std::time::SystemTime::now();` → `store.on_request(&region, now);` (:177) → `record.to_shape(now)` (:236)
* describe_account_assignment_deletion_status.rs:164
* describe_permission_set_provisioning_status.rs:168
* describe_instance_access_control_attribute_configuration.rs:167
* list_permission_set_provisioning_status.rs:199 (`store.on_request(..., now)` at :206, `record.status(now)` at :248)

Test coverage that stays green under the fix: tests/all/instance_encryption_default.rs:233 and tests/all/instance_update.rs:348 both `sleep(ENCRYPTION_UPDATE_DELAY + 100ms)` before asserting `EncryptionStatus`, and tests/all/instance_describe.rs:225-253 asserts the already-in-effect default.

## Steps to Reproduce

1. Read services/service-com-amazonaws-ssoadmin-swbexternalservice/src/ops/describe_instance.rs:212 — the clock is read to settle the store and resolve the instance.
2. Read describe_instance.rs:251-255 — the clock is read AGAIN and that second instant is what `EncryptionConfigurationDetails$EncryptionStatus` is derived from.
3. Compare with describe_account_assignment_creation_status.rs:171/177/236, where one `now` is captured before the lock and used for both the settle and the projection.
4. Call UpdateInstance with an `EncryptionConfiguration`, then call DescribeInstance so that the request is served across the `ENCRYPTION_UPDATE_DELAY` boundary: the instance's own state is resolved from the first clock read while `EncryptionStatus` reflects the second, so the one response mixes two instants.
