Issue-Source: trace-sweep-divergence
Issue-Trace: opensearchserverless-access-policy-lifecycle
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: opensearchserverless-access-policy-lifecycle opensearchserverless-network-security-policy-lifecycle opensearchserverless-encryption-security-policy-lifecycle opensearchserverless-collection-group-lifecycle opensearchserverless-create-access-policy-idempotent-clienttoken
Issue-Shape-Id: com.amazonaws.opensearchserverless#OpenSearchServerless
Issue-Key: aoss|GetAccessPolicy|2xx->2xx|$.accessPolicyDetail.description
Issue-Key: aoss|GetAccessPolicy|2xx->2xx|$.accessPolicyDetail.policyVersion
Issue-Key: aoss|GetAccessPolicy|2xx->2xx|$.accessPolicyDetail.policy[*].Rules[*]
Issue-Key: aoss|GetSecurityPolicy|2xx->2xx|$.securityPolicyDetail.description
Issue-Key: aoss|GetSecurityPolicy|2xx->2xx|$.securityPolicyDetail.policyVersion
Issue-Key: aoss|GetSecurityPolicy|2xx->2xx|$.securityPolicyDetail.policy[*].Rules[*]
Issue-Key: aoss|BatchGetCollectionGroup|2xx->2xx|$.collectionGroupDetails[*].capacityLimits.maxIndexingCapacityInOCU
Issue-Key: aoss|BatchGetCollectionGroup|2xx->2xx|$.collectionGroupDetails[*].capacityLimits.maxSearchCapacityInOCU
Issue-Key: aoss|BatchGetCollectionGroup|2xx->2xx|$.collectionGroupDetails[*].description
Issue-Key: aoss|BatchGetCollectionGroup|2xx->2xx|$.collectionGroupDetails[*].storageConfiguration
Issue-Key: aoss|GetAccessPolicy|4xx->2xx|$
Issue-Key: aoss|GetAccessPolicy|4xx->2xx|$.__type
Issue-Key: aoss|GetAccessPolicy|4xx->2xx|$.message
Issue-Key: aoss|GetAccessPolicy|4xx->2xx|status
Issue-Key: aoss|GetAccessPolicy|2xx->4xx|$.accessPolicyDetail
Issue-Key: aoss|GetAccessPolicy|2xx->4xx|status
Issue-Commit: 7f205ee96657d4151f96c4aa7296372f34590a63

# aoss writes are not reflected in stored state: updates, deletes and clientToken creates

> _Note: this issue was identified against the code base at commit `7f205ee96657d4151f96c4aa7296372f34590a63`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateAccessPolicy

## Description
This issue bundles SIX distinct causes in the opensearchserverless (`aoss`) emulator, sharing one fault: a successful mutating call returns 200 but the service's stored record is unchanged, so the following read still serves the pre-mutation state. Each cause must be closed before submitting.

Cause 1 of 6: UpdateAccessPolicy does not persist the new policy document, description or version.
AWS: UpdateAccessPolicy (Smithy: input type/name/policyVersion[required]/description/policy, output accessPolicyDetail) replaces the stored document and description and mints a NEW policyVersion; a subsequent GetAccessPolicy returns the updated description, the updated Rules array and the new policyVersion (the version the update response returned).
Emulator: GetAccessPolicy after a 200 UpdateAccessPolicy returns the ORIGINAL description, the original single-element Rules array (the added index rule is absent) and the original policyVersion.
Why: a client cannot change an access policy at all — optimistic-concurrency flows that re-read the version, and any permission change pushed through UpdateAccessPolicy, silently do nothing while reporting success.
Fix: UpdateAccessPolicy's handler must write the merged record back into crate::state (services/service-com-amazonaws-opensearchserverless-opensearchserverless/src/ops/update_access_policy.rs) and bump the stored policyVersion.
Traces: opensearchserverless-access-policy-lifecycle#task#5.

Cause 2 of 6: UpdateSecurityPolicy does not persist changes to a network policy.
AWS: after a 200 UpdateSecurityPolicy, GetSecurityPolicy(type='network', name=...) returns the updated description, the updated Rules array (including an added dashboard rule) and the new policyVersion.
Emulator: returns the original description, the original single rule and the original policyVersion.
Why: network exposure cannot be changed through the API; a client that widens or narrows dashboard/collection access sees success and no effect.
Fix: UpdateSecurityPolicy's handler/state write (update_security_policy.rs), same as Cause 1.
Traces: opensearchserverless-network-security-policy-lifecycle#task#5.

Cause 3 of 6: UpdateSecurityPolicy does not persist changes to an encryption policy's description.
AWS: after a 200 UpdateSecurityPolicy, GetSecurityPolicy(type='encryption', name=...) returns the updated description.
Emulator: returns the description the policy was created with.
Why: same mechanism as Cause 2 on the encryption policy type — the update is a no-op.
Fix: same handler/state write; this cause confirms the defect is not specific to one SecurityPolicyType.
Traces: opensearchserverless-encryption-security-policy-lifecycle#task#5.

Cause 4 of 6: UpdateCollectionGroup does not persist capacityLimits or description.
AWS: after a 200 UpdateCollectionGroup, BatchGetCollectionGroup returns collectionGroupDetails[0] with the updated description and the updated capacityLimits (maxIndexingCapacityInOCU / maxSearchCapacityInOCU as set by the update; Smithy models capacityLimits as CollectionGroupCapacityLimits on CollectionGroupDetail).
Emulator: returns the creation-time description and the creation-time capacity limits.
Why: OCU capacity — the thing collection groups exist to control — cannot be changed; a client scaling a group up sees 200 and keeps the old ceiling. (The same step also shows storageConfiguration missing from the detail; that omission is filed separately under the CollectionGroup detail-field issue for this service.)
Fix: UpdateCollectionGroup's handler/state write (update_collection_group.rs).
Traces: opensearchserverless-collection-group-lifecycle#task#4.

Cause 5 of 6: DeleteAccessPolicy does not remove the policy.
AWS: after DeleteAccessPolicy, GetAccessPolicy for that type/name is refused — recorded response HTTP 400 with __type com.amazon.aoss#ResourceNotFoundException (Smithy: GetAccessPolicy models ResourceNotFoundException; message example "Policy with name <name> and type data is not found").
Emulator: GetAccessPolicy keeps answering 200 with the full accessPolicyDetail for the deleted policy, through 15 polling attempts.
Why: a deleted access policy keeps granting the access it described as far as the API is concerned, and any teardown/recreate cycle sees a stale resource it cannot clear.
Fix: DeleteAccessPolicy's handler must remove the record from crate::state (delete_access_policy.rs) so the read path raises ResourceNotFoundException.
Traces: opensearchserverless-access-policy-lifecycle#teardown#7.

Cause 6 of 6: CreateAccessPolicy carrying a clientToken does not store the policy.
AWS: CreateAccessPolicy's clientToken is an @idempotencyToken; a create that carries one still creates the policy (and a repeat with the same token returns the same policy rather than a conflict), so a following GetAccessPolicy returns 200 with accessPolicyDetail for the requested name.
Emulator: GetAccessPolicy for the just-created name answers HTTP 404 with no accessPolicyDetail — the policy was never stored (status divergence 404-vs-400 for not-found is the separate error-status issue; the defect here is that the resource is absent at all).
Why: any client that passes an idempotency token on create — the SDK does this automatically for @idempotencyToken members — ends up with no policy while the create reported success.
Fix: CreateAccessPolicy's handler/idempotency path (create_access_policy.rs): persist the record and key the idempotent replay off the clientToken instead of dropping the write.
Traces: opensearchserverless-create-access-policy-idempotent-clienttoken#task#4.

## Evidence
Cause 1 — opensearchserverless-access-policy-lifecycle#task#5 GetAccessPolicy (200): $.accessPolicyDetail.description expected 'updated to include index rules', got 'initial data access policy'; $.accessPolicyDetail.policy[0].Rules[1] expected the added index rule {'ResourceType':'index','Resource':['index/<col>/*'],'Permission':[...]}, got None; $.accessPolicyDetail.policyVersion expected the version returned by the update, got the creation version.
Cause 2 — opensearchserverless-network-security-policy-lifecycle#task#5 GetSecurityPolicy (200): description expected 'public network policy with index rule', got 'public network policy'; policy[0].Rules[1] expected {'ResourceType':'dashboard','Resource':['collection/<col>']}, got None; policyVersion expected the post-update version, got '000001a166bdfa4900000001'.
Cause 3 — opensearchserverless-encryption-security-policy-lifecycle#task#5 GetSecurityPolicy (200): $.securityPolicyDetail.description expected 'updated description', got 'AWS owned key encryption policy'.
Cause 4 — opensearchserverless-collection-group-lifecycle#task#4 BatchGetCollectionGroup (200): capacityLimits.maxIndexingCapacityInOCU expected 4.0, got 2.0; capacityLimits.maxSearchCapacityInOCU expected 4.0, got 2.0; description expected 'updated group description', got 'initial group description'.
Cause 5 — opensearchserverless-access-policy-lifecycle#teardown#7 GetAccessPolicy: expected status 400 with $.__type 'com.amazon.aoss#ResourceNotFoundException' and message 'Policy with name <name> and type data is not found'; got 200 with the full accessPolicyDetail, and the poll reports "error code None not in ['ResourceNotFoundException'] after 15 attempt(s)".
Cause 6 — opensearchserverless-create-access-policy-idempotent-clienttoken#task#4 GetAccessPolicy: expected status 200 with accessPolicyDetail for name 'sim2real-probe-idem-fcc8755bf4' (type data, the clientToken-created policy); got 404 and no accessPolicyDetail.

## Steps to Reproduce
Boot the emulator and replay the aoss fixtures.
1 (Cause 1): CreateAccessPolicy(type='data', name=N, policy=P1, description=D1); UpdateAccessPolicy(type='data', name=N, policyVersion=<returned>, policy=P2 with an extra index rule, description=D2); GetAccessPolicy(type='data', name=N) -> expect D2, both rules and a new policyVersion; observe D1, one rule, the old version. Fixture opensearchserverless-access-policy-lifecycle#task#5.
2 (Cause 2): same shape with CreateSecurityPolicy/UpdateSecurityPolicy(type='network') then GetSecurityPolicy -> expect the updated description, the added dashboard rule and the new version. Fixture opensearchserverless-network-security-policy-lifecycle#task#5.
3 (Cause 3): UpdateSecurityPolicy(type='encryption', description='updated description') then GetSecurityPolicy -> expect the new description. Fixture opensearchserverless-encryption-security-policy-lifecycle#task#5.
4 (Cause 4): CreateCollectionGroup(capacityLimits 2/2, description='initial group description'); UpdateCollectionGroup(capacityLimits 4/4, description='updated group description'); BatchGetCollectionGroup(ids=[id]) -> expect 4/4 and the new description; observe 2/2 and the old description. Fixture opensearchserverless-collection-group-lifecycle#task#4.
5 (Cause 5): DeleteAccessPolicy(type='data', name=N) then poll GetAccessPolicy(type='data', name=N) -> expect ResourceNotFoundException; observe 200 with the policy, indefinitely. Fixture opensearchserverless-access-policy-lifecycle#teardown#7.
6 (Cause 6): CreateAccessPolicy(type='data', name=N, policy=P, clientToken='<token>') then GetAccessPolicy(type='data', name=N) -> expect 200 with accessPolicyDetail; observe not-found. Fixture opensearchserverless-create-access-policy-idempotent-clienttoken#task#4.
