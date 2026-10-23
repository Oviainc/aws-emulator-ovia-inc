Issue-Source: trace-sweep-divergence
Issue-Trace: sso-admin-instance-describe-update-and-quota-r2
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sso-admin-instance-describe-update-and-quota-r2 sso-admin-bootstrap-instance-if-missing-r2 sso-admin-describe-list-edge-cases-r2 sso-admin-tagging-round-trip-r2 sso-admin-trusted-token-issuer-lifecycle
Issue-Shape-Id: com.amazonaws.ssoadmin#SWBExternalService
Issue-Key: sso-admin|DescribeInstance|2xx->2xx|$.Instance
Issue-Key: sso-admin|DescribeInstance|2xx->2xx|$.PrimaryRegion
Issue-Key: sso-admin|DescribeInstance|2xx->2xx|$.Regions[RegionName="us-west-2"]
Issue-Key: sso-admin|CreateInstance|4xx->4xx|$.Message
Issue-Key: sso-admin|CreateInstance|4xx->4xx|status
Issue-Key: sso-admin|ListInstances|2xx->2xx|$.Instances[Name="IdentityCenter"]
Issue-Key: sso-admin|ListInstances|2xx->2xx|$.Instances[Name="sim2real-wire-idc-a2a07929"]
Issue-Key: sso-admin|ListInstances|2xx->2xx|$.NextToken
Issue-Key: sso-admin|ListInstances|2xx->2xx|$.Instances[Name="IdentityCenter"].PrimaryRegion
Issue-Key: sso-admin|ListInstances|2xx->2xx|$.Instances[Name="IdentityCenter"].Regions[*]
Issue-Key: sso-admin|ListInstances|2xx->2xx|$.Instances[Name="IdentityCenter"].State
Issue-Key: sso-admin|ListInstances|2xx->2xx|$.Instances[*]
Issue-Key: sso-admin|DescribeTrustedTokenIssuer|4xx->2xx|$.Message
Issue-Key: sso-admin|DescribeTrustedTokenIssuer|4xx->2xx|$.__type
Issue-Key: sso-admin|DescribeTrustedTokenIssuer|4xx->2xx|status
Issue-Commit: 0b023ce22a329ff2896e758c2b0769a212a04448

# sso-admin instance state not tracked: ambient us-east-1 instance blocks creates, deletes ignored, State and NextToken omitted

> _Note: this issue was identified against the code base at commit `0b023ce22a329ff2896e758c2b0769a212a04448`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInstance

## Description
The sso-admin instance surface answers from a fixed seeded instance rather than from tracked state: the seeded instance is pinned to us-east-1, instances created in the session never appear, the quota rejection carries the wrong HTTP status, modeled/recorded response members are omitted, and a deleted trusted token issuer is still resolvable. SIX distinct causes are bundled here; all six must be closed.

Cause 1 of 6 — DescribeInstance reports PrimaryRegion/Regions from a seed pinned to us-east-1 instead of the instance's actual primary Region, and omits State.
AWS: Smithy DescribeInstance -> DescribeInstanceResponse{InstanceArn, IdentityStoreId, IdentityStoreArn, OwnerAccountId, Name, CreatedDate, Status, StatusReason, PrimaryRegion (RegionName, pattern ^([a-z]+-){2,3}\\d$), Regions (RegionMetadataList), EncryptionConfigurationDetails, PermissionSetsEnabled}. PrimaryRegion is documented as 'the primary Region where the IAM Identity Center instance was originally enabled'; Regions is 'the list of Regions enabled in the instance'. A State member is NOT in the model — recorded behavior only: the recorded response carries State {'Name': 'ACTIVE'} beside Status, and PrimaryRegion/Regions[0].RegionName equal to the Region the instance was enabled in (us-west-2 in the recording, which is the Region the fixture drives).
Emulator: PrimaryRegion 'us-east-1' and Regions [{RegionName 'us-east-1', Status 'ACTIVE', AddedDate <now>, IsPrimaryRegion true}] regardless of the Region the request is made in, and no State member at all. (Specific recorded CreatedDate/IdentityStoreId values are environment detail, not requirements; the Region values and the presence of State are.)
Why: clients that read PrimaryRegion to build identitystore/permission-set endpoints or to decide where to call next are sent to the wrong Region, and clients that gate on State.Name see nothing and treat the instance as not-ready.
Fix: DescribeInstance handler + the instance state seed in the ssoadmin service crate — derive PrimaryRegion/Regions from the instance record (the Region it was created in) and serialize State.
Traces: sso-admin-instance-describe-update-and-quota-r2#task#3 (and #task#7, same cluster).

Cause 2 of 6 — ListInstances repeats the same wrong Region data, omits State, and never emits NextToken.
AWS: Smithy ListInstances -> ListInstancesResponse{Instances: InstanceList (length 0..10) of InstanceMetadata, NextToken: Token}; @paginated{inputToken: NextToken, outputToken: NextToken, pageSize: MaxResults, items: Instances}. Recorded behavior: each InstanceMetadata carries PrimaryRegion/Regions for the Region the instance is enabled in plus State {'Name': 'ACTIVE'}, and the response carries a NextToken even for a single-instance account.
Emulator: Instances[Name='IdentityCenter'].PrimaryRegion 'us-east-1' (recorded 'us-west-2'), Regions[0] pinned to us-east-1, State absent, NextToken absent.
Why: same wrong-Region consequence as Cause 1 on the enumeration path, plus a paginating client sees no continuation token and cannot distinguish 'no more pages' from a truncated list. The recorded token's text is an opaque value and is not a requirement — its presence is.
Fix: ListInstances handler/serializer and the shared InstanceMetadata serializer; emit a continuation token from the same pagination layer.
Traces: sso-admin-instance-describe-update-and-quota-r2#task#4.

Cause 3 of 6 — CreateInstance's one-instance-per-account rejection is returned as HTTP 402 where the wire returns HTTP 400.
AWS: Smithy CreateInstance errors include ServiceQuotaExceededException, which the model marks @httpError(402); the recorded wire response for creating a second instance is HTTP 400 (the error code itself matched on replay — only the status diverged). Recorded message, as an example of the rule 'at most one instance per account': "You have exceeded IAM Identity Center quotas. Cannot create more than 1 instance for <accountId>. Please refer to https://docs.aws.amazon.com/singlesignon/latest/userguide/limits.html" — i.e. AWS reports the one-instance quota naming the account, not a resource conflict.
Emulator: HTTP 402 with "An instance of IAM Identity Center already exists in this account. Only one instance can be created per account, across all Amazon Web Services Regions."
Why: 402 is not a status AWS SDK error classifiers expect from this API; bootstrap code that branches on a 400 quota rejection (the common 'create the instance only if absent' pattern) instead surfaces an unclassified transport-level failure and aborts. The exact message text is an example, but the status is a requirement.
Fix: the CreateInstance handler's quota/pre-existing-instance rejection path and the ssoadmin error status mapping for ServiceQuotaExceededException.
Traces: sso-admin-bootstrap-instance-if-missing-r2#task#3 (cluster also covers sso-admin-instance-describe-update-and-quota-r2#task#5).

Cause 4 of 6 — an instance created in the session never appears in ListInstances; only the seeded instance is ever returned.
AWS: ListInstances returns the instances of the calling account; an instance created by CreateInstance (which returns its InstanceArn) is subsequently listed, with the Name the create request supplied.
Emulator: after the fixture's CreateInstance step, ListInstances returns only {'Name': 'IdentityCenter', 'InstanceArn': 'arn:aws:sso:::instance/ssoins-...'} — the created instance (recorded under the fixture's created InstanceArn and its requested Name) is missing from the list.
Why: any workflow that creates an instance and then discovers it by listing (rather than holding the ARN) never finds it, so every follow-on call is made against the wrong instance ARN. Combined with Cause 3 this makes the whole create-then-use path unusable.
Fix: the CreateInstance handler must insert the instance into the shared instance state that ListInstances/DescribeInstance read, instead of ListInstances answering from a fixed seed.
Traces: sso-admin-describe-list-edge-cases-r2#task#4.

Cause 5 of 6 — ListInstances answers from the seeded 'IdentityCenter' instance even when the session's instance is the one under test.
AWS: the listed instance is the account's instance, with the Name, IdentityStoreId and Region it was enabled with.
Emulator: $.Instances[0] is always the seeded {'Name': 'IdentityCenter', PrimaryRegion 'us-east-1', CreatedDate <now>} record; the fixture's own instance is absent. (The recorded Name/IdentityStoreId/CreatedDate are environment-specific values and are examples only — the defect is that the listed instance is not the session's instance and its Region/State are seed constants.)
Why: a client that sets up an instance and then lists to obtain its ARN for tagging or permission-set work operates on an instance it did not create, and tag/permission-set calls then target the wrong resource.
Fix: same instance state store as Cause 4 — ListInstances must project tracked state, not a literal seed.
Traces: sso-admin-tagging-round-trip-r2#setup#1.

Cause 6 of 6 — DescribeTrustedTokenIssuer still returns 200 for a trusted token issuer that was deleted.
AWS: Smithy DescribeTrustedTokenIssuer errors include ResourceNotFoundException; after DeleteTrustedTokenIssuer the ARN no longer resolves. Recorded behavior: HTTP 400 with __type 'ResourceNotFoundException' and a message naming the ARN, e.g. 'Could not find TrustedTokenIssuer with arn <arn>.' (the status and the error code are the requirement; the message is an example, though its rule — name the missing TTI ARN — is visible in the recording).
Emulator: HTTP 200 with a normal describe payload and no __type, i.e. the delete left the issuer resolvable.
Why: a client that deletes an issuer and confirms removal by describing it concludes the delete failed, or worse keeps using an issuer it believes is gone; teardown/idempotency logic loops.
Fix: DeleteTrustedTokenIssuer must remove the record from trusted-token-issuer state, and DescribeTrustedTokenIssuer must raise the not-found error for an unknown ARN (with the same 400 status this surface uses for not-found).
Traces: sso-admin-trusted-token-issuer-lifecycle#task#8.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from mainline (emulator_up: true; state_reset_failures: 0, so each fixture ran against a clean state and these are not cross-fixture leakage).

1) sso-admin-instance-describe-update-and-quota-r2#task#3 DescribeInstance (same cluster: #task#7) — expected 200, got 200; $.PrimaryRegion: value_mismatch (expected 'us-west-2', got 'us-east-1'); $.Regions[RegionName="us-west-2"]: missing; $.Instance (the recorded instance object incl. State {'Name': 'ACTIVE'}): missing.

2) sso-admin-instance-describe-update-and-quota-r2#task#4 ListInstances — expected 200, got 200; $.Instances[Name="IdentityCenter"].PrimaryRegion: expected 'us-west-2', got 'us-east-1'; $.Instances[Name="IdentityCenter"].Regions[0]: expected {'RegionName': 'us-west-2', 'IsPrimaryRegion': True, 'Status': 'ACTIVE', ...}, got {'RegionName': 'us-east-1', ...}; $.Instances[Name="IdentityCenter"].State: missing (expected {'Name': 'ACTIVE'}); $.NextToken: missing (a token was recorded).

3) sso-admin-bootstrap-instance-if-missing-r2#task#3 CreateInstance (same cluster: sso-admin-instance-describe-update-and-quota-r2#task#5) — status: expected 400, got 402; $.Message: expected 'You have exceeded IAM Identity Center quotas. Cannot create more than 1 instance for 000000000000. ...', got 'An instance of IAM Identity Center already exists in this account. Only one instance can be created per account, across all Amazon Web Services Regions.'

4) sso-admin-describe-list-edge-cases-r2#task#4 ListInstances — expected 200, got 200; $.Instances[Name="sim2real-wire-idc-a2a07929"]: missing (the instance created earlier in the fixture, recorded with InstanceArn {{created_instance_arn}}, State {'Name': 'ACTIVE'}, PrimaryRegion 'us-west-2'); $.Instances[Name="IdentityCenter"]: extra (the seeded instance, PrimaryRegion 'us-east-1'); $.NextToken: missing.

5) sso-admin-tagging-round-trip-r2#setup#1 ListInstances — expected 200, got 200; $.Instances[0]: expected the fixture's instance (InstanceArn {{instance_arn_tags}}, PrimaryRegion 'us-west-2', State {'Name': 'ACTIVE'}), got {'Name': 'IdentityCenter', 'InstanceArn': 'arn:aws:sso:::instance/ssoins-7223b1d1ac5e1f0e', 'PrimaryRegion': 'us-east-1', 'Regions': [{'RegionName': 'us-east-1', ...}]} with no State.

6) sso-admin-trusted-token-issuer-lifecycle#task#8 DescribeTrustedTokenIssuer — status: expected 400, got 200; $.__type: missing (expected 'ResourceNotFoundException'); $.Message: missing (expected 'Could not find TrustedTokenIssuer with arn arn:aws:sso::000000000000:trustedTokenIssuer/ssoins-790789ec69b6e68d/tti-48a13360-70d0-708e-2c1a-f933a3381e84.').

Model consulted: Smithy com.amazonaws.ssoadmin — DescribeInstance/ListInstances output signatures (PrimaryRegion RegionName, Regions RegionMetadataList, Instances length 0..10, @paginated NextToken), shape InstanceMetadata (no State member — State is recorded behavior only), CreateInstance errors (ServiceQuotaExceededException @httpError 402, ConflictException 409), DescribeTrustedTokenIssuer errors (ResourceNotFoundException).

## Steps to Reproduce
1. git clone the repo at mainline and boot the emulator built from it.
2. Replay trace set simparity-wire-aws-v1.
3. Fixture sso-admin-instance-describe-update-and-quota-r2, step task#3: DescribeInstance for the account's instance — observe PrimaryRegion 'us-east-1' (recording: the Region the instance was enabled in, us-west-2), Regions pinned to us-east-1, and no State member.
4. Same fixture, step task#4: ListInstances — observe the same us-east-1 Region data, no State, and no NextToken.
5. Fixture sso-admin-bootstrap-instance-if-missing-r2, step task#3: CreateInstance when an instance already exists — observe HTTP 402 where the recording has HTTP 400 with the one-instance quota error.
6. Fixture sso-admin-describe-list-edge-cases-r2, step task#4: after the fixture's CreateInstance, call ListInstances — observe only the seeded 'IdentityCenter' instance and not the created one.
7. Fixture sso-admin-tagging-round-trip-r2, step setup#1: ListInstances — observe the seeded 'IdentityCenter' instance instead of the fixture's instance.
8. Fixture sso-admin-trusted-token-issuer-lifecycle, step task#8: DescribeTrustedTokenIssuer for an ARN deleted earlier in the fixture — observe HTTP 200 with a describe payload instead of the 400 ResourceNotFoundException.
