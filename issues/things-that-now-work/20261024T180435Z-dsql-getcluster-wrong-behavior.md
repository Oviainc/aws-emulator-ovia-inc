Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: b3076e7c0c2ebb8fac474fe10cf99e18802cbe88

# encryptionDetails.encryptionStatus is hardcoded ENABLED, so ENABLING/UPDATING/KMS_KEY_INACCESSIBLE are unreachable and a key change looks instantly complete

_Filed: 2026-10-24T18:04:35Z_
_Operation: GetCluster | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `b3076e7c0c2ebb8fac474fe10cf99e18802cbe88`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`EncryptionStatus` models four values — ENABLED, ENABLING, UPDATING, KMS_KEY_INACCESSIBLE — and `encryptionDetails` is the member through which a client learns whether a cluster's encryption configuration has actually taken effect. The emulator derives `EncryptionDetails` in one place (`crate::state::encryption_details`) and sets `encryption_status` to the ENABLED constant on every path, so:

* a cluster that is still CREATING reports encryption as already ENABLED rather than ENABLING;
* an `UpdateCluster` that changes `kmsEncryptionKey` rewrites `encryptionDetails` synchronously to the new key with status ENABLED, while the cluster itself is reported as UPDATING until the clock settles it — so GetCluster simultaneously says "the cluster is being reconfigured" and "its encryption change is finished". Expected: encryptionStatus reports UPDATING for the window the key change is in flight (the same window the cluster's own status is UPDATING) and only then ENABLED, which is what lets a client poll for the re-encryption to complete;
* KMS_KEY_INACCESSIBLE can never be reported for a customer-managed key that is disabled or deleted.

The observable effect on every GetCluster/CreateCluster/UpdateCluster response is a constant where the model carries a state machine: three of the four modeled values are unreachable on all code paths.

## Evidence

Spec: `smithy_query(query="shape", shape="EncryptionStatus")` -> [ENABLED, UPDATING, KMS_KEY_INACCESSIBLE, ENABLING]; `EncryptionDetails.encryptionStatus` is @required and documented as "The status of encryption for the cluster"; `encryptionDetails` is an output member of CreateCluster and GetCluster (`OPERATIONS.md` lines 54, 62).

State/handlers: `src/state.rs` `encryption_details` (both match arms set `encryption_status: ENCRYPTION_STATUS_ENABLED`) and the single constant `ENCRYPTION_STATUS_ENABLED` ("EncryptionStatus value for a cluster whose encryption is in place"); `src/ops/create_cluster.rs` line 303 stores it while queueing the CREATING -> ACTIVE transition; `src/ops/update_cluster.rs` lines 307-312 replace `cluster.encryption_details` immediately, then lines 319-323 set the cluster to UPDATING with a pending ACTIVE transition; `src/ops/get_cluster.rs` line 197 reports the stored value verbatim.

Tests: `tests/all/encryption_key_selection.rs` asserts only `encryptionType` and `kmsKeyArn` (the `KeyCase` table has no status field), and a grep of `tests/` for `encryptionStatus`/`ENABLED` returns no matches — the status member is entirely unpinned, so no test asserts the current constant.

## Steps to Reproduce

1. POST /cluster {"deletionProtectionEnabled": false, "kmsEncryptionKey": "1234abcd-12ab-34cd-56ef"} -> response reports status CREATING with encryptionDetails.encryptionStatus = ENABLED; expected ENABLING while the cluster is still being created.
2. Wait for the cluster to settle to ACTIVE.
3. POST /cluster/{identifier} {"kmsEncryptionKey": "alias/another-key"} -> 200 with status UPDATING.
4. GET /cluster/{identifier} immediately -> emulator reports status UPDATING but encryptionDetails already showing the NEW kmsKeyArn with encryptionStatus ENABLED; expected encryptionStatus UPDATING until the key change completes, then ENABLED.
5. Observe that no request to any operation can produce encryptionStatus ENABLING, UPDATING or KMS_KEY_INACCESSIBLE.
