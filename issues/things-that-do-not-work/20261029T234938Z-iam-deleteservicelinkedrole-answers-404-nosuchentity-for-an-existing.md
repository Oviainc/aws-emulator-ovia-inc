Issue-Source: trace-sweep-divergence
Issue-Trace: license-manager-user-subscriptions-register-identity-provider-unknown-directory
Issue-Step: teardown#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: license-manager-user-subscriptions-register-identity-provider-unknown-directory
Issue-Shape-Id: com.amazonaws.licensemanagerusersubscriptions#LicenseManagerUserSubscriptions
Issue-Key: iam|DeleteServiceLinkedRole|2xx->4xx|status
Issue-Commit: c1b8e21acf710dd592fea1840f63adefca5c3a99

# iam DeleteServiceLinkedRole answers 404 NoSuchEntity for an existing service-linked role

> _Note: this issue was identified against the code base at commit `c1b8e21acf710dd592fea1840f63adefca5c3a99`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteServiceLinkedRole

## Description
Cause 1 of 1 - DeleteServiceLinkedRole cannot delete the service-linked role established earlier in the same fixture.

AWS: Smithy (services/service-com-amazonaws-iam-awsidentitymanagementv20100508): DeleteServiceLinkedRole takes a required RoleName (roleNameType, @length 1-64, @pattern ^[\w+=,.@-]+$) and returns DeleteServiceLinkedRoleResponse with a required DeletionTaskId (@length 1-1000) - i.e. a successful HTTP 200 that starts an asynchronous deletion task. Its modeled errors are NoSuchEntityException (httpError 404, awsQueryError code NoSuchEntity / 404), LimitExceededException (409, LimitExceeded) and ServiceFailureException (500, ServiceFailure). For a service-linked role that exists in the account - including the AWSServiceRoleFor<Service> role created by CreateServiceLinkedRole or minted on a caller's behalf when a service is first used - the recorded response is HTTP 200; NoSuchEntity is reserved for a RoleName that names no role.
Emulator: HTTP 404 - the NoSuchEntity class - for exactly that role, so the delete never succeeds and no DeletionTaskId is returned.
Why: a client cleaning up (or a stack/teardown path) can never remove a service-linked role: it gets a not-found it cannot act on while the role is still present, so repeated attempts keep failing and any subsequent assertion that the role is gone, or any re-create of it, is left in an inconsistent state. The 404 also misleads a client into believing the role was never created.
Hypothesis (neither the model nor the recording states the mechanism): the role is stored under a name or path that the delete path's lookup does not derive the same way - e.g. created from AWSServiceName as AWSServiceRoleFor<Service> under /aws-service-role/ but looked up as a plain role name - or the delete path only considers ordinary roles and skips the service-linked role store.
Fix: the iam crate's DeleteServiceLinkedRole handler and the role store lookup it uses - resolve the service-linked role by the same name/path under which CreateServiceLinkedRole (or the implicit creation path) recorded it, delete it, and answer 200 with a DeletionTaskId; keep 404 NoSuchEntity only for a RoleName that resolves to no role.
Traces: license-manager-user-subscriptions-register-identity-provider-unknown-directory#teardown#5 (only the status is asserted on this step: expected 200, got 404).

This divergence belongs to the iam service even though it was exercised from a license-manager-user-subscriptions fixture's teardown. The only existing iam issue naming this operation (20261025T000319Z-iam-deleteservicelinkedrole-stub.md) is already resolved and covered the unimplemented scaffold, not this lookup failure.

## Evidence
Replay of trace set simparity-wire-aws-v1 (9 fixtures, 37 steps, 14 mismatched, score 62.2%). Divergence: service iam, operation DeleteServiceLinkedRole, fixture license-manager-user-subscriptions-register-identity-provider-unknown-directory, step teardown#5, verdict mismatch, detail 'status: expected 200, got 404'. No body members were asserted on the step, so the finding is the status/error class: a 404 NoSuchEntity where the recording has a 200 success carrying DeletionTaskId. Model evidence: DeleteServiceLinkedRoleResponse.DeletionTaskId is required on the success path; NoSuchEntityException is httpError 404 with awsQueryError code NoSuchEntity.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus).
3. Fixture license-manager-user-subscriptions-register-identity-provider-unknown-directory, step teardown#5: call iam DeleteServiceLinkedRole with the RoleName of the service-linked role the fixture's earlier steps established.
4. Observe HTTP 404 (NoSuchEntity). AWS recorded HTTP 200 with a DeletionTaskId.
