Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 0b0649dc1b525c86072f831fd9c0b574c3cd2f61

# UpdateInstance stamps the encryption record from a SECOND clock read, so the stored "accepted at" instant is later than the state the call resolved against

_Filed: 2026-10-29T01:26:52Z_
_Operation: UpdateInstance | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `0b0649dc1b525c86072f831fd9c0b574c3cd2f61`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every other write in this crate reads the wall clock ONCE, before taking the store lock's materialisation step, and reuses that one instant both for resolving what the clock already owes the store and for any instant it stores. `UpdateInstance` does not: it passes a fresh `::std::time::SystemTime::now()` to `on_request` (the materialisation that decides which instance this call resolves) and then, when the request carries an `EncryptionConfiguration`, calls `::std::time::SystemTime::now()` a SECOND time to stamp the stored `EncryptionConfigurationRecord`.

The record's own documentation says that stamp is "when the update is accepted" and that the whole `UPDATING` → `ENABLED` walk is derived from it (`effective_at: Some(accepted_at + ENCRYPTION_UPDATE_DELAY)`), so the stored instant — and with it the window in which `DescribeInstance` reports `UPDATING` — is dated strictly after the state the call was judged against, by however long the handler spent between the two reads (lock acquisition, materialisation, four validation branches, the instance lookup and the name write). Nothing in the handler or in the crate states a reason for taking a second reading; the sibling instance-level configuration write does the opposite.

The same double reading sits in `DescribeInstance` (src/ops/describe_instance.rs:212 and :254), which stores nothing, so the defect that outlives the call is this one.

## Evidence

THE RULE, stated by a sibling write — services/service-com-amazonaws-ssoadmin-swbexternalservice/src/ops/delete_account_assignment.rs:186-189:
```
        // ONE instant for the whole call: the instance resolution below and the deletion's
        // own `CreatedDate` are the same moment, so the stored status cannot be dated
        // before or after the state it describes.
        let now = ::std::time::SystemTime::now();
```

THE SAME RULE, implemented by the instance-level configuration write that is this handler's closest sibling — src/ops/create_instance_access_control_attribute_configuration.rs:170-176 and :255-257:
```
        let now = ::std::time::SystemTime::now();
        let region = crate::state::request_region(ctx.parts());
        let mut store = state.lock();
        // What the CLOCK already owes every instance, plus the account's ambient instance
        // if nothing has enabled one yet, so this create resolves the same instance a
        // read at this instant would resolve.
        store.on_request(&region, now);
```
```
        instance.access_control_attributes = Some(
            crate::state::AccessControlAttributeConfigurationRecord::new(attributes, now),
        );
```
(`CreateInstance` src/ops/create_instance.rs:213 and `ProvisionPermissionSet` src/ops/provision_permission_set.rs:198 capture `now` once the same way.)

THE OUTLIER — src/ops/update_instance.rs:293-298:
```
        let region = crate::state::request_region(ctx.parts());
        let mut store = state.lock();
        // What the CLOCK already owes every instance, plus the account's ambient instance
        // if nothing has enabled one yet, so this update resolves the same instance a read
        // at this instant would resolve.
        store.on_request(&region, ::std::time::SystemTime::now());
```
and src/ops/update_instance.rs:326-336, a SECOND reading for the value it STORES:
```
        if let Some(configuration) = input.encryption_configuration {
            // REPLACES the configuration the instance already had (the Amazon Web
            // Services owned default, until a caller opts into a key of its own): the key
            // change is asynchronous, so the new record reports `UPDATING` until it is
            // due.
            instance.encryption = crate::state::EncryptionConfigurationRecord::new(
                configuration.key_type,
                configuration.kms_key_arn,
                ::std::time::SystemTime::now(),
            );
        }
```

WHAT THE STORED INSTANT MEANS — src/state.rs:1060-1062, :1078-1085:
```
/// stored, so a read never has to write to move the status on: the instant a change
/// becomes effective is fixed when the update is accepted, and every later read computes
/// the status the clock implies at the moment it is asked.
```
```
    /// The configuration an `UpdateInstance` accepted at `accepted_at` writes.
    pub fn new(key_type: String, kms_key_arn: Option<String>, accepted_at: SystemTime) -> Self {
        Self {
            key_type,
            kms_key_arn,
            effective_at: Some(accepted_at + ENCRYPTION_UPDATE_DELAY),
        }
    }
```
with the span that instant is measured against — src/state.rs:472:
```
pub const ENCRYPTION_UPDATE_DELAY: Duration = Duration::from_millis(500);
```

TEST COVERAGE: tests/all/instance_update.rs and tests/all/instance_encryption_default.rs exercise the `UPDATING` → `ENABLED` walk, and tests/all/instance_create_in_progress_writes.rs pins the three `UpdateInstance` rows; none of them asserts anything about WHICH clock reading the stamp comes from, so no test states a reason for the second reading.

## Steps to Reproduce

1. POST / with `X-Amz-Target: SWBExternalService.ListInstances` and read the `InstanceArn`.
2. Call `UpdateInstance` with `{"InstanceArn":"<instance>","EncryptionConfiguration":{"KeyType":"CUSTOMER_MANAGED_KEY","KmsKeyArn":"arn:aws:kms:us-east-1:123456789012:key/0a1b2c3d-4e5f-6a7b-8c9d-0e1f2a3b4c5d"}}`.
3. Read the stored record: `effective_at` is `ENCRYPTION_UPDATE_DELAY` after the SECOND `SystemTime::now()` (src/ops/update_instance.rs:334), not after the instant the call resolved the instance with (src/ops/update_instance.rs:298).
4. Compare with `CreateInstanceAccessControlAttributeConfiguration`, whose `accepted_at` is the very instant it passed to `on_request` (src/ops/create_instance_access_control_attribute_configuration.rs:170 and :256), and with the rule `DeleteAccountAssignment` states at src/ops/delete_account_assignment.rs:186-189.
