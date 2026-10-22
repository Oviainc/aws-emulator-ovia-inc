Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: license-manager-user-subscriptions.associate-user-instance-not-found license-manager-user-subscriptions.create-license-server-endpoint-provider-not-found license-manager-user-subscriptions.delete-license-server-endpoint-not-found license-manager-user-subscriptions.disassociate-user-ambiguous-identifiers-rejected license-manager-user-subscriptions.untag-resource-not-found-rejected license-manager-user-subscriptions.update-identity-provider-settings-overlapping-subnets-rejected license-manager-user-subscriptions.list-instances-missing-permission-rejected
Issue-Shape-Id: com.amazonaws.licensemanagerusersubscriptions#LicenseManagerUserSubscriptions
Issue-Key: license-manager-user-subscriptions.associate-user-instance-not-found|run#4|AssertionError|assert code == "UnauthorizedException", f"expected UnauthorizedException, got {code!r}"
Issue-Key: license-manager-user-subscriptions.create-license-server-endpoint-provider-not-found|run#5|AssertionError|assert code == "ValidationException", str(code)
Issue-Key: license-manager-user-subscriptions.delete-license-server-endpoint-not-found|run#5|AssertionError|assert code == "UnauthorizedException", str(code)
Issue-Key: license-manager-user-subscriptions.disassociate-user-ambiguous-identifiers-rejected|run#4|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code!r}"
Issue-Key: license-manager-user-subscriptions.untag-resource-not-found-rejected|run#5|AssertionError|assert code == "BadRequestException", f"expected BadRequestException from UntagResource, got {code!r}"
Issue-Key: license-manager-user-subscriptions.update-identity-provider-settings-overlapping-subnets-rejected|run#5|AssertionError|assert overlap_code == "ValidationException", f"expected ValidationException, got {overlap_code!r}"
Issue-Key: license-manager-user-subscriptions.list-instances-missing-permission-rejected|run#14|AssertionError|assert code == "AccessDeniedException", f"expected AccessDeniedException, got {code!r}"
Issue-Commit: b96b6334f7964c8f0db5f0da0179882059e96e50

# license-manager-user-subscriptions: 7 operations answer HTTP 501 NotImplemented instead of AWS's rejection (7 causes)

> _Note: this issue was identified against the code base at commit `b96b6334f7964c8f0db5f0da0179882059e96e50`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateUser

## Description
Seven License Manager User Subscriptions operations are still scaffolded stubs: their handlers return `crate::scaffolded::Scaffolded::not_implemented(..)`, which the emulator serializes as HTTP 501 with error code `NotImplemented` and the message "operation <Name> is scaffolded but not implemented in this emulator". Real AWS answers each of these calls with a specific client rejection, so every negative-path client behaviour (retry classification, error-code branching, fail-closed authorization) is wrong. Seven distinct causes, one per operation/handler file.

Note: the suite's auto-filed stub issues cover `ListInstances` and `ListLicenseServerEndpoints` as stubs; Cause 7 below is filed for the IAM-enforcement behaviour that the ListInstances check asserts and that the stub issue does not describe.

--- Cause 1 of 7: AssociateUser answers 501 instead of failing closed on authorization ---
AWS: AssociateUser against a syntactically valid but nonexistent EC2 instance id (and an unregistered identity provider) is rejected on the instance-authorization gate, before any existence lookup. Recorded behavior only: the code is `UnauthorizedException` (not a modeled shape — Smithy models only AccessDeniedException/ConflictException/InternalServerException/ResourceNotFoundException/ServiceQuotaExceededException/ThrottlingException/ValidationException for AssociateUser), with the fixed sentence "Unauthorized. Please check the request and access policy and try again."
Emulator: HTTP 501, code `NotImplemented`.
Why: a client that branches on UnauthorizedException (or treats 501 as a transport/retryable server fault) never sees the authorization refusal, and nothing about user association can be exercised at all.
Fix: the AssociateUser handler — implement the fail-closed instance-authorization gate and the operation itself.
Traces: license-manager-user-subscriptions.associate-user-instance-not-found; services/service-com-amazonaws-licensemanagerusersubscriptions-licensemanagerusersubscriptions/src/ops/associate_user.rs:4 (SCAFFOLDED marker) and :176 (`not_implemented`).

--- Cause 2 of 7: CreateLicenseServerEndpoint answers 501 instead of ValidationException ---
AWS: CreateLicenseServerEndpoint with a well-formed but never-registered IdentityProviderArn resolves the ARN as part of request validation and rejects it with `ValidationException` (HTTP 400; Smithy: CreateLicenseServerEndpoint errors include ValidationException @httpError 400), recorded message "Identity provider is not valid" — not ResourceNotFoundException.
Emulator: HTTP 501, code `NotImplemented`.
Why: callers cannot distinguish an unusable identity provider from a server outage, and RDS SAL license-server endpoint creation is unavailable.
Fix: the CreateLicenseServerEndpoint handler — resolve IdentityProviderArn against service state and refuse with ValidationException when it does not resolve.
Traces: license-manager-user-subscriptions.create-license-server-endpoint-provider-not-found; src/ops/create_license_server_endpoint.rs.

--- Cause 3 of 7: DeleteLicenseServerEndpoint answers 501 instead of failing closed ---
AWS: DeleteLicenseServerEndpoint on a well-formed but never-created LicenseServerEndpointArn fails closed on the ownership/authorization check. Recorded behavior only: code `UnauthorizedException` with "Unauthorized. Please check the request and access policy and try again." (not ResourceNotFoundException).
Emulator: HTTP 501, code `NotImplemented`.
Why: delete-of-unknown-ARN is a routine idempotency path; a 501 makes clients retry a call that AWS refuses deterministically.
Fix: the DeleteLicenseServerEndpoint handler — resolve the endpoint ARN and fail closed when the caller cannot be shown to own it.
Traces: license-manager-user-subscriptions.delete-license-server-endpoint-not-found; src/ops/delete_license_server_endpoint.rs.

--- Cause 4 of 7: DisassociateUser answers 501 instead of validating its identifier combination ---
AWS: DisassociateUser requires either InstanceUserArn or the full Username/InstanceId/IdentityProvider triple. Smithy marks none of them @required (DisassociateUserRequest members are all optional), so the service itself must refuse a request naming neither with `ValidationException` (HTTP 400), recorded message "IdentityProvider, InstanceId, and Username must be provided". Supplying a well-formed but never-created InstanceUserArn instead fails closed with `UnauthorizedException` / "Unauthorized. Please check the request and access policy and try again." (recorded behavior only).
Emulator: HTTP 501, code `NotImplemented` for both forms.
Why: an empty DisassociateUser request is an argument error the client must fix, not a server fault; the two distinct refusals also tell a client whether the identifiers or the resource were the problem.
Fix: the DisassociateUser handler — identifier-combination validation first, then the owned-resource authorization gate.
Traces: license-manager-user-subscriptions.disassociate-user-ambiguous-identifiers-rejected; src/ops/disassociate_user.rs.

--- Cause 5 of 7: UntagResource (and ListTagsForResource) answer 501 for an unresolvable resource ARN ---
AWS: UntagResource against a well-formed but never-created identity-provider ARN is rejected with `BadRequestException` and the recorded message "Invalid input resource arn"; ListTagsForResource against the same ARN is rejected identically. Recorded behavior only — BadRequestException is not in this service's Smithy model (UntagResource models only InternalServerException and ResourceNotFoundException), so the model does not predict it.
Emulator: HTTP 501, code `NotImplemented` (both tag handlers are scaffolded stubs).
Why: tag read/write on this service's resources is entirely unavailable, and the "ARN does not resolve" refusal clients key on is replaced by a server-fault code.
Fix: the UntagResource and ListTagsForResource handlers — resolve ResourceArn against service state and refuse unresolvable ARNs with the recorded BadRequestException form.
Traces: license-manager-user-subscriptions.untag-resource-not-found-rejected; src/ops/untag_resource.rs, src/ops/list_tags_for_resource.rs:4 (SCAFFOLDED marker).

--- Cause 6 of 7: UpdateIdentityProviderSettings answers 501 instead of resolving the ARN first ---
AWS: UpdateIdentityProviderSettings resolves IdentityProviderArn before it evaluates the UpdateSettings payload: a nonexistent ARN is rejected with `ValidationException` (HTTP 400; Smithy: UpdateIdentityProviderSettings errors are AccessDeniedException, ThrottlingException, ValidationException), recorded message "Identity provider is not valid", and an otherwise-identical call whose AddSubnets/RemoveSubnets do NOT overlap produces the same code and message — ARN resolution short-circuits before any subnet-overlap check.
Emulator: HTTP 501, code `NotImplemented`.
Why: clients cannot update provider settings at all, and get a retryable-looking 501 where AWS gives a terminal 400.
Fix: the UpdateIdentityProviderSettings handler — resolve the ARN before validating the settings payload.
Traces: license-manager-user-subscriptions.update-identity-provider-settings-overlapping-subnets-rejected; src/ops/update_identity_provider_settings.rs.

--- Cause 7 of 7: no IAM authorization is enforced on ListInstances (501 served to every caller) ---
AWS: a caller with no license-manager-user-subscriptions permissions is rejected with `AccessDeniedException` (Smithy: ListInstances models AccessDeniedException @httpError 400) and the standard not-authorized sentence naming the principal, the action license-manager-user-subscriptions:ListInstances and the resource; a permissioned caller's ListInstances succeeds and returns InstanceSummaries.
Emulator: HTTP 501, code `NotImplemented`, for the unprivileged caller and for the permissioned one alike — no IAM evaluation happens anywhere on this service's request path.
Why: an emulator that never refuses an unauthorized principal silently passes tests of least-privilege policies that would fail against AWS. (The auto-filed `ListInstances` scaffold-stub issue covers implementing the operation; it does not cover the authorization gate asserted here.)
Fix: the ListInstances handler plus the service's shared authorization layer — evaluate the caller's identity before serving the list.
Traces: license-manager-user-subscriptions.list-instances-missing-permission-rejected; src/ops/list_instances.rs.

## Evidence
Suite: license-manager-user-subscriptions 1/18 passed (6%), 17 failed.

AssociateUser: expected code 'UnauthorizedException' (HTTP 400-class, message "Unauthorized. Please check the request and access policy and try again."); got 'NotImplemented' (HTTP 501).
CreateLicenseServerEndpoint: expected 'ValidationException' / "Identity provider is not valid"; got 'NotImplemented'.
DeleteLicenseServerEndpoint: expected 'UnauthorizedException'; got 'NotImplemented'.
DisassociateUser (no identifiers): expected 'ValidationException' / "IdentityProvider, InstanceId, and Username must be provided"; got 'NotImplemented'.
UntagResource (never-created identity-provider ARN): expected 'BadRequestException' / "Invalid input resource arn"; got 'NotImplemented'.
UpdateIdentityProviderSettings (nonexistent ARN): expected 'ValidationException' / "Identity provider is not valid"; got 'NotImplemented'.
ListInstances (unprivileged IAM user): expected 'AccessDeniedException'; got 'NotImplemented'.

Raw stub response observed by the harness on a sibling operation, showing the 501 channel: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListLicenseServerEndpoints operation: operation ListLicenseServerEndpoints is scaffolded but not implemented in this emulator.

Handler evidence in the checkout: associate_user.rs line 4 carries "SCAFFOLDED, NOT IMPLEMENTED" and line 176 returns Scaffolded::not_implemented("AssociateUser"); list_tags_for_resource.rs line 4 carries the same marker.

## Steps to Reproduce
1. Boot the emulator and point a botocore client at it with region us-west-2.
2. AssociateUser(Username='u', InstanceId='i-0123456789abcdef0', IdentityProvider={'ActiveDirectoryIdentityProvider':{'DirectoryId':'d-0123456789'}}) -> observe 501 NotImplemented instead of UnauthorizedException.
3. CreateLicenseServerEndpoint(IdentityProviderArn='arn:aws:license-manager-user-subscriptions:us-west-2:<acct>:identity-provider/never-registered', LicenseServerSettings={'ServerType':'RDS_SAL','ServerSettings':{'RdsSalSettings':{'RdsSalCredentialsProvider':{'SecretsManagerCredentialsProvider':{'SecretId':'s'}}}}}) -> 501 instead of ValidationException.
4. DeleteLicenseServerEndpoint(LicenseServerEndpointArn='arn:aws:license-manager-user-subscriptions:us-west-2:<acct>:license-server-endpoint/never-created', ServerType='RDS_SAL') -> 501 instead of UnauthorizedException.
5. DisassociateUser() with no members -> 501 instead of ValidationException; DisassociateUser(InstanceUserArn=<well-formed, never-created>) -> 501 instead of UnauthorizedException.
6. UntagResource(ResourceArn=<well-formed, never-created identity-provider ARN>, TagKeys=['env']) -> 501 instead of BadRequestException.
7. UpdateIdentityProviderSettings(IdentityProviderArn=<nonexistent>, UpdateSettings={'AddSubnets':['subnet-0123456789abcdef0'],'RemoveSubnets':['subnet-0123456789abcdef0']}) -> 501 instead of ValidationException.
8. Create an IAM user with no policies, sign ListInstances with its access key -> 501 instead of AccessDeniedException; the permissioned client's ListInstances also returns 501 instead of InstanceSummaries.
9. Or replay the check ids in trace_ids.
