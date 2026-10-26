Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: f2163892bf40cea6da7d1662f28c14ec75c797c6

# A CreateCluster clientToken replay reports the cluster's CURRENT deletionProtectionEnabled, encryptionDetails and multiRegionProperties instead of the values the original call answered

_Filed: 2026-10-26T23:24:48Z_
_Operation: CreateCluster | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `f2163892bf40cea6da7d1662f28c14ec75c797c6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CreateCluster's idempotency replay renders its whole answer off the LIVE cluster record and only overrides `status` from the ledger. Three of the remaining output members are mutable after creation — `deletionProtectionEnabled`, `encryptionDetails` and `multiRegionProperties` are all written by UpdateCluster — so once an UpdateCluster has changed any of them, a retry carrying the original `clientToken` answers 200 with the UPDATED configuration rather than with the result the first call produced. The handler's own comment asserts the opposite ("Every other member of the answer is a field of the cluster this call minted and cannot change"), the state module's RecordedCall doctrine says a value that can be superseded must be kept in the record, and the model's `clientToken` documentation says a retry returns the result of the original request. The replay is reachable regardless of the cluster's current state, because the fingerprint is computed from the REQUEST body only (`create_fingerprint`), not from stored state.

This is a distinct gap from the fixed CreateCluster PENDING_SETUP issue and from the fixed PutClusterPolicy replay issue — it is the same class of defect as the latter, on the one output member set nobody recorded.

## Evidence

Model (quote): CreateClusterInput.clientToken — "With an idempotent request, if the original request completes successfully, the subsequent retries with the same client token return the result from the original successful request and they have no additional effect." (smithy_query doc, reproduced in services/service-com-amazonaws-dsql-dsql/src/ops/create_cluster.rs:82-88).

Handler side (both halves of the repo contradiction):
- services/service-com-amazonaws-dsql-dsql/src/ops/create_cluster.rs:274-285 — "crate::state::TokenVerdict::Replay(recorded) => { if let Some(existing) = state.clusters.get(&recorded.resource) { let mut answer = created_cluster_output(existing); // Every other member of the answer is a field of the cluster this call minted and cannot change, but its status can have moved on since — through the clock or a later `UpdateCluster` ... if let Some(status) = recorded.answered(crate::state::STATUS_MEMBER) { answer.status = status.to_string(); } return Ok(answer);"
- services/service-com-amazonaws-dsql-dsql/src/ops/create_cluster.rs:393-404 — `created_cluster_output` reads `multi_region_properties: cluster.multi_region_properties.clone()`, `encryption_details: cluster.encryption_details.clone()`, `deletion_protection_enabled: cluster.deletion_protection_enabled` straight off the stored record.
- services/service-com-amazonaws-dsql-dsql/src/ops/update_cluster.rs:320-335 — those very three fields ARE changed after creation: "if let Some(deletion_protection_enabled) = input.deletion_protection_enabled { cluster.deletion_protection_enabled = deletion_protection_enabled; }" … "cluster.encryption_details = Some(crate::state::encryption_details(&region, customer_key));" … "if let Some(multi_region_properties) = input.multi_region_properties { cluster.multi_region_properties = Some(multi_region_properties); }" — so the comment at create_cluster.rs:277-281 ("cannot change") is false.
- services/service-com-amazonaws-dsql-dsql/src/state.rs:474-480 states the crate's own rule the handler breaks: "A retry has to answer with the result of the ORIGINAL call, and the state the service holds when the retry arrives need not still determine that result: a value the first call MINTED can be superseded by a later call or dropped altogether. Such a value is therefore kept here, keyed by the output member it belongs to, so the retry is served from the record rather than re-derived." Only `STATUS_MEMBER` is ever recorded (create_cluster.rs:362-371).
- Sibling that does it right: services/service-com-amazonaws-dsql-dsql/src/ops/put_cluster_policy.rs:204-227 serves the replay from `recorded.answered(POLICY_VERSION_MEMBER)` because "a later write supersedes the version".

Test coverage: services/service-com-amazonaws-dsql-dsql/tests/all/idempotent_replay_result.rs states the rule — "a retry the service's idempotency ledger recognises is served from the record of the original call — it answers with the result that call produced" (lines 5-9) — but `a_minting_call_retry_answers_the_status_the_first_call_answered` asserts only identity and `status` (lines 504-515). No case changes `deletionProtectionEnabled`/`kmsEncryptionKey`/`multiRegionProperties` between the first CreateCluster and its retry, so nothing pins the current behaviour.

## Steps to Reproduce

1. POST /cluster with body {"deletionProtectionEnabled": true, "clientToken": "t"} — note the answer: deletionProtectionEnabled=true, encryptionDetails={encryptionType: AWS_OWNED_KMS_KEY}, no multiRegionProperties.
2. Wait out the settle span so the cluster reaches ACTIVE.
3. POST /cluster/{identifier} (UpdateCluster) with {"deletionProtectionEnabled": false, "kmsEncryptionKey": "alias/other", "multiRegionProperties": {"witnessRegion": "us-west-2"}} → 200.
4. Replay the create: POST /cluster with the identical body from step 1 ({"deletionProtectionEnabled": true, "clientToken": "t"}) → 200, but the answer now reports deletionProtectionEnabled=false, encryptionDetails={encryptionType: CUSTOMER_MANAGED_KMS_KEY, kmsKeyArn: arn:aws:kms:...:alias/other} and multiRegionProperties={witnessRegion: us-west-2}.
5. Expected (per the clientToken contract and state.rs:474-480): the retry answers exactly what the first call answered — deletionProtectionEnabled=true, AWS_OWNED_KMS_KEY, no multiRegionProperties.
