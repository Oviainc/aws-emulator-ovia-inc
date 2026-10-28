Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: cbb9267c7abd2d15d6f906cf892ef291750ce9f0

# CreatePolicyStore silently drops `encryptionSettings`, so the customer-managed-key state GetPolicyStore serves is unreachable

_Filed: 2026-10-28T19:13:26Z_
_Operation: CreatePolicyStore | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `cbb9267c7abd2d15d6f906cf892ef291750ce9f0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreatePolicyStoreInput$encryptionSettings` is the one request member that selects a customer-managed KMS key for a policy store and its child resources. The hand-written input type declares that member `unsupported`, which removes it from the generated input struct and schema, so a request carrying `encryptionSettings.kmsEncryptionSettings` is answered 200 and the key it named is never recorded anywhere: the handler always persists `StoreEncryption::default()` (an AWS owned key).

This is not a feature the crate never attempts — the write path is the only missing half. The state model already carries the customer-managed arm (`StoreEncryption::CustomerManagedKey { key, encryption_context }`), and `GetPolicyStore` already serves it by mapping that arm onto the modelled `EncryptionState::kmsEncryptionState` union. Because nothing can ever construct that arm, the read path is dead code and a caller who asks for a CMK gets a silent success plus a response (and later `GetPolicyStore`) that claims default AWS-owned-key encryption. The crate's own note admits exactly this gap.

## Evidence

The crate's own admission — services/.../src/ops/SKIPPED_STUBS.md:31-34: "`CreatePolicyStoreInput$encryptionSettings` — the request member that selects a customer-managed KMS key — is still declared `unsupported` on the hand-written input type, so every store this slice creates is encrypted with an AWS owned key."

The declaration that drops it — services/.../src/ops/types/create_policy_store_types.rs:23: `    unsupported = ["encryptionSettings"]` (vendor/smithy-macros/src/lib.rs:1614 / :1677 skip an `unsupported` member from codegen entirely, so it never reaches the handler).

The write path that always stores the default — services/.../src/ops/create_policy_store.rs:188-193:
```
            // A store this operation creates is encrypted with an AWS owned
            // key: the one request member that selects a customer-managed key,
            // `CreatePolicyStoreInput$encryptionSettings`, is not served by this
            // slice ...
            encryption: crate::state::StoreEncryption::default(),
```

The read path that serves the unreachable arm — services/.../src/ops/get_policy_store.rs:128-137:
```
            encryption_state: Some(match &store.encryption {
                crate::state::StoreEncryption::AwsOwnedKey => EncryptionState::Default,
                crate::state::StoreEncryption::CustomerManagedKey { key, encryption_context } =>
                    EncryptionState::KmsEncryptionState(KmsEncryptionState { ... }),
            }),
```
and the state arm it reads — src/state.rs:340-348: "/// `EncryptionState$kmsEncryptionState`: the store is encrypted with a customer-managed KMS key, under a user-defined encryption context. CustomerManagedKey { key: String, encryption_context: BTreeMap<String, String> }" — constructed nowhere in the crate.

Smithy (smithy_query shape="EncryptionSettings"): "kmsEncryptionSettings: The KMS encryption settings for this policy store to encrypt data with. It will contain the customer-managed KMS key, and a user-defined encryption context."

## Steps to Reproduce

1. CreatePolicyStore with { validationSettings: { mode: "OFF" }, encryptionSettings: { kmsEncryptionSettings: { key: "arn:aws:kms:us-east-1:000000000000:key/abcd", encryptionContext: { "team": "photos" } } } }.
2. Observed: HTTP 200 and a policyStoreId; nothing in the response or in state records the key.
3. GetPolicyStore { policyStoreId, tags: false } -> `encryptionState` is reported as the `default` (AWS owned key) arm, never `kmsEncryptionState`, for every store this service can create.
