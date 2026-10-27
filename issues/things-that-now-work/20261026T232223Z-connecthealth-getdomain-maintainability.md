Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: f2163892bf40cea6da7d1662f28c14ec75c797c6

# The EncryptionContext projection is built twice (create_domain.rs and get_domain.rs) while every sibling projection is single-homed in state.rs

_Filed: 2026-10-26T23:22:23Z_
_Operation: GetDomain | Class: maintainability_

> _Note: this issue was identified against the code base at commit `f2163892bf40cea6da7d1662f28c14ec75c797c6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The rule that turns a stored `Domain` into the modelled `EncryptionContext` — report the stored `EncryptionType`'s wire value, and carry the domain's stored `kmsKeyArn` beside it — is written out twice, once in `CreateDomain`'s response and once in `GetDomain`'s. The two copies are the only place that projection exists, so a later change to how encryption is reported (another `EncryptionType`, or dropping `kmsKeyArn` for an AWS-owned key) has to be made in both or the create response and the read response start disagreeing about the same stored record.

This is the one projection in the crate that is NOT single-homed: the parallel projections all live on the state record and are called from each handler — `WebAppConfigurationRecord::projected` (called from both the same two handlers) and `Subscription::description` (called from four handlers). Moving the `EncryptionContext` build onto `Domain` the same way leaves every response byte-identical and every existing test unchanged.

## Evidence

Copy 1 — `services/service-com-amazonaws-connecthealth-connecthealth/src/ops/create_domain.rs:376-379`:
```
                encryption_context: Some(EncryptionContext {
                    encryption_type: stored.encryption_type.as_wire().to_string(),
                    kms_key_arn: stored.kms_key_arn.clone(),
                }),
```
Copy 2 — `src/ops/get_domain.rs:221-224`:
```
                encryption_context: Some(EncryptionContext {
                    encryption_type: domain.encryption_type.as_wire().to_string(),
                    kms_key_arn: domain.kms_key_arn.clone(),
                }),
```

The convention the crate follows for every other projection, `src/state.rs:288-305`: "/// The record as the modelled `WebAppConfiguration` every Domain response reports" ... "pub fn projected(&self) -> WebAppConfiguration {" — and `src/state.rs:626-627`: "/// This record as the modelled `SubscriptionDescription` four operations return.\n    pub fn description(&self) -> SubscriptionDescription {". Both handlers above already call `crate::state::WebAppConfigurationRecord::projected` for the neighbouring member (`create_domain.rs:382-385`, `get_domain.rs:227-230`), so the shared-projection seam exists and only this member bypasses it. `src/state.rs:5-8` states the intent: "Every field a response carries is\n//! read back OUT of these records" and "Helpers several operations share ... live here rather than being copied\n//! into each `src/ops/*.rs`".

Test coverage that pins the responses the refactor must not change: `tests/all/domain_subscription_lifecycle.rs:362-374` asserts the create's `encryptionContext.encryptionType` follows `kmsKeyArn`, and `tests/all/domain_subscription_lifecycle.rs:418-434` asserts `GetDomain` reports the identical `encryptionContext` member ("case {case:?}: GetDomain must report the stored {member}").

## Steps to Reproduce

1. Open src/ops/create_domain.rs:376-379 and src/ops/get_domain.rs:221-224 and observe the same `EncryptionContext { encryption_type: ..as_wire().to_string(), kms_key_arn: ..clone() }` build written twice.
2. Open src/state.rs:296 (`WebAppConfigurationRecord::projected`) and src/state.rs:627 (`Subscription::description`) and observe that every other response projection in this crate is defined once on the state record and called from each handler.
3. Note that both handlers already call `WebAppConfigurationRecord::projected` for the adjacent member, so the two `EncryptionContext` copies are the only duplicated projection; hoisting it onto `Domain` leaves tests/all/domain_subscription_lifecycle.rs:362-434 passing unchanged.
