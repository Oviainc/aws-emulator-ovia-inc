Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 930ad079f132337074cf049827d554fb10a3d200

# A PENDING_SETUP cluster accepts deletionProtectionEnabled and kmsEncryptionKey changes when the request also carries multiRegionProperties

_Filed: 2026-10-28T13:16:55Z_
_Operation: UpdateCluster | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `930ad079f132337074cf049827d554fb10a3d200`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateCluster` picks which status gate to apply from the PRESENCE of `multiRegionProperties` alone, and then applies every member the request carries. The crate exempts a `PENDING_SETUP` cluster from the status gate for exactly one reason — it is waiting for the configuration call that completes its peering — but the exemption is granted to the whole request, so a caller can attach `deletionProtectionEnabled` and/or `kmsEncryptionKey` to that same call and have them written to a cluster that, on its own, refuses both with the gate's 409.

Concretely: `if input.multi_region_properties.is_some() { stored.multi_region_configuration_conflict() } else { stored.mutation_conflict("be updated") }` selects the lenient refusal, and the three member assignments below it are unconditional. The result is that the gate the crate describes as the one question every write of a cluster record asks is bypassed for arbitrary members by adding one more member to the request — the opposite of the crate's own statement that a waiting cluster "takes no other mutation".

## Evidence

The rule the crate states for the exemption — `services/service-com-amazonaws-dsql-dsql/src/state.rs:318-331`:
```
    /// The refusal an update that CONFIGURES this cluster's multi-Region setup
    /// answers with, or `None` when the cluster takes it.
    ///
    /// A cluster waiting in [`STATUS_PENDING_SETUP`] takes no other mutation, but
    /// it does take this one: the peering it is waiting for is established by
    /// configuring BOTH sides, so the configuration change is the very call the
    /// wait is for. Every other status answers exactly as it does to any other
    /// mutation ([`Self::mutation_conflict`]).
    pub fn multi_region_configuration_conflict(&self) -> Option<ConflictException> {
        if self.status == STATUS_PENDING_SETUP {
            return None;
        }
        self.mutation_conflict("be updated")
    }
```
and the gate it belongs to — `src/state.rs:297-306`: "Every operation that writes a cluster record asks this one question, so none of them can state the condition differently from [`Self::accepts_mutation`]".

The code that breaks it — `src/ops/update_cluster.rs:302-342`:
```
        // A cluster the clock still owes a transition, or one on its way out, cannot
        // take a configuration change — with the one exception a request CONFIGURING
        // the multi-Region setup gets: a cluster waiting in `PENDING_SETUP` is
        // waiting for exactly that call.
        let refusal = if input.multi_region_properties.is_some() {
            stored.multi_region_configuration_conflict()
        } else {
            stored.mutation_conflict("be updated")
        };
        ...
        if let Some(deletion_protection_enabled) = input.deletion_protection_enabled {
            cluster.deletion_protection_enabled = deletion_protection_enabled;
        }
        ...
        if let Some(kms_encryption_key) = input.kms_encryption_key.as_deref() {
            let customer_key = crate::state::customer_managed_key(Some(kms_encryption_key));
            cluster.encryption_details = Some(crate::state::encryption_details(
                &cluster_region,
                customer_key,
            ));
            cluster.kms_encryption_key = customer_key.map(str::to_string);
        }
```
Nothing between the refusal selection and these assignments restricts the applied members to the multi-Region configuration.

Test coverage states the rule but leaves the combination untested — `tests/all/multi_region_cluster_status.rs:341-380`:
"/// RULE: while a cluster waits in `PENDING_SETUP` it accepts the configuration
/// change that completes its peering — the call the wait is for — and refuses every
/// other mutation with the status gate's 409."
its rows are `("deletionProtectionEnabled", json!({"deletionProtectionEnabled": true}))`, `("kmsEncryptionKey", json!({"kmsEncryptionKey": "alias/other"}))` and `("an empty request", json!({}))` — each asserted 409 — and the accepted row is `json!({"multiRegionProperties": {"witnessRegion": WITNESS_REGION}})` alone. No row sends `multiRegionProperties` TOGETHER with another member, which is the path that writes them. `tests/all/cluster_mutation_status_gate.rs:82-84` drives `Mutation::UpdateCluster` as `dsql.update(identifier, json!({"deletionProtectionEnabled": true}))`, i.e. also without `multiRegionProperties`.

## Steps to Reproduce

1. In region us-east-1, `CreateCluster` with `{"deletionProtectionEnabled": false, "multiRegionProperties": {"witnessRegion": "us-west-2"}}` and wait for the clock, as `tests/all/multi_region_cluster_status.rs:346-359` does; `GetCluster` reports `PENDING_SETUP`.
2. Call `UpdateCluster` with `{"kmsEncryptionKey": "alias/other"}` alone — refused 409 ConflictException naming PENDING_SETUP (the behaviour the test pins).
3. Call `UpdateCluster` with `{"multiRegionProperties": {"witnessRegion": "us-west-2"}, "kmsEncryptionKey": "alias/other", "deletionProtectionEnabled": true}`.
4. Observe 200, and `GetCluster` now reports `encryptionDetails.kmsKeyArn` for `alias/other` and `deletionProtectionEnabled: true` — two writes the cluster's status refuses on their own, applied because the request also carried the member the exemption is for.
