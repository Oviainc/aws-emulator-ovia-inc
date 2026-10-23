Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 145ab011fad305b9f8c024d3a43ce6653fc244ab

# UpdateCluster treats the reserved `AWS_OWNED_KMS_KEY` value as a customer-managed key instead of reverting to the AWS-owned key

_Filed: 2026-10-23T23:29:20Z_
_Operation: UpdateCluster | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `145ab011fad305b9f8c024d3a43ce6653fc244ab`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

UpdateCluster's `kmsEncryptionKey` documentation states: "To switch to the key owned by Amazon Web Services, specify the reserved value AWS_OWNED_KMS_KEY." The value satisfies the member's pattern `^[a-zA-Z0-9:/_-]+$`, so it reaches the handler.

Expected: UpdateCluster(kmsEncryptionKey="AWS_OWNED_KMS_KEY") moves the cluster back to the Amazon-owned key, so GetCluster reports `encryptionDetails.encryptionType = AWS_OWNED_KMS_KEY` with no `kmsKeyArn`.

Actual: the handler passes the string straight into `crate::state::encryption_details(&region, Some(key))`, whose `Some(..)` arm unconditionally produces `encryptionType = CUSTOMER_MANAGED_KMS_KEY` and expands the value into a bogus key ARN `arn:aws:kms:{region}:{account}:key/AWS_OWNED_KMS_KEY` (`state.rs` lines 833-857). The reserved value is also stored verbatim in `cluster.kms_encryption_key`, so the cluster can never be returned to AWS-owned encryption through the API.

## Evidence

Spec: UpdateCluster `kmsEncryptionKey` member documentation in the Smithy model (reproduced verbatim in `src/ops/update_cluster.rs` lines 89-93); `EncryptionType` enum contains both AWS_OWNED_KMS_KEY and CUSTOMER_MANAGED_KMS_KEY. Handler: `src/ops/update_cluster.rs` lines 275-281 plus `src/state.rs::encryption_details` lines 833-846 (no special case for the reserved value). Tests: `tests/all/cluster_lifecycle.rs::update_mutates_only_the_members_the_request_carries` covers only `alias/rotated`; `grep AWS_OWNED` over tests shows no case supplying the reserved value as input.

## Steps to Reproduce

1. POST /cluster {"deletionProtectionEnabled": false}; wait for ACTIVE.
2. POST /cluster/{id} with {"kmsEncryptionKey": "alias/rotated"}; wait for ACTIVE; GET /cluster/{id} shows encryptionType CUSTOMER_MANAGED_KMS_KEY.
3. POST /cluster/{id} with {"kmsEncryptionKey": "AWS_OWNED_KMS_KEY"}; wait for ACTIVE.
4. GET /cluster/{id} → encryptionDetails is {encryptionType: CUSTOMER_MANAGED_KMS_KEY, kmsKeyArn: "arn:aws:kms:us-east-1:<account>:key/AWS_OWNED_KMS_KEY"}. Expected {encryptionType: AWS_OWNED_KMS_KEY} with no kmsKeyArn.
