Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: license-manager-user-subscriptions.associate-user-authorization-precedes-shape-validation license-manager-user-subscriptions.deregister-identity-provider-not-found license-manager-user-subscriptions.start-product-subscription-provider-not-registered license-manager-user-subscriptions.list-tags-for-resource-malformed-arn-rejected license-manager-user-subscriptions.register-identity-provider-directory-not-found license-manager-user-subscriptions.register-identity-provider-invalid-product-rejected license-manager-user-subscriptions.stop-product-subscription-ambiguous-identifiers-rejected
Issue-Shape-Id: com.amazonaws.licensemanagerusersubscriptions#LicenseManagerUserSubscriptions
Issue-Key: license-manager-user-subscriptions.associate-user-authorization-precedes-shape-validation|run#6|AssertionError|assert code == "UnauthorizedException", str(code)
Issue-Key: license-manager-user-subscriptions.deregister-identity-provider-not-found|run#5|AssertionError|assert code == "ValidationException", str(code)
Issue-Key: license-manager-user-subscriptions.start-product-subscription-provider-not-registered|run#4|AssertionError|assert code == "UnauthorizedException", f"expected UnauthorizedException, got {code!r}"
Issue-Key: license-manager-user-subscriptions.list-tags-for-resource-malformed-arn-rejected|run#4|AssertionError|assert code == "BadRequestException", f"expected BadRequestException for a malformed ResourceArn, got {code!r}"
Issue-Key: license-manager-user-subscriptions.register-identity-provider-directory-not-found|_error#3|AssertionError|raise AssertionError(f"real AWS rejects this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: license-manager-user-subscriptions.register-identity-provider-invalid-product-rejected|_error#4|AssertionError|raise AssertionError(f"real AWS rejects this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: license-manager-user-subscriptions.stop-product-subscription-ambiguous-identifiers-rejected|run#5|AssertionError|assert message == "IdentityProvider, Product, and Username must be provided", f"unexpected message: {message!r}"
Issue-Commit: b96b6334f7964c8f0db5f0da0179882059e96e50

# license-manager-user-subscriptions: implemented handlers accept invalid input with 200 and misclassify rejections (7 causes)

> _Note: this issue was identified against the code base at commit `b96b6334f7964c8f0db5f0da0179882059e96e50`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterIdentityProvider

## Description
Where this service's handlers ARE implemented, they refuse the wrong things with the wrong codes: RegisterIdentityProvider accepts input real AWS refuses (HTTP 200 for an unknown product and for an unresolvable DirectoryId), existence/authorization failures are classified as ResourceNotFoundException (404) where AWS answers ValidationException (400) or fails closed with UnauthorizedException, malformed tag ARNs are refused by the generic shape validator instead of the service's own BadRequestException, and AssociateUser runs modeled shape validation ahead of the authorization gate AWS evaluates first. Seven distinct causes.

--- Cause 1 of 7: AssociateUser validates the Tags map before the authorization gate ---
AWS: for a nonexistent InstanceId, AssociateUser fails closed on the instance-authorization check BEFORE it validates the request shape: an oversized Tags map gets the SAME rejection as a compliant one. Smithy: AssociateUser.Tags targets Tags, a map with @length min 0 max 50, so 51 entries IS a constraint violation — AWS nevertheless answers with code `UnauthorizedException` and the fixed sentence "Unauthorized. Please check the request and access policy and try again." (recorded behavior only; UnauthorizedException is not a modeled shape for this service). Authorization strictly precedes shape validation for this operation.
Emulator: answers `ValidationException` for the 51-tag request (the centralized deserialize->validate->handle walk enforces the modeled @length before the handler ever runs), and `NotImplemented` for the compliant-tag request.
Why: a client probing authorization sees a parameter error and "fixes" the tags instead of the permission, and the two calls diverge where AWS makes them identical.
Fix: the AssociateUser handler and this operation's validation ordering — the authorization gate must run before modeled-constraint enforcement for AssociateUser.
Traces: license-manager-user-subscriptions.associate-user-authorization-precedes-shape-validation; services/service-com-amazonaws-licensemanagerusersubscriptions-licensemanagerusersubscriptions/src/ops/associate_user.rs.

--- Cause 2 of 7: DeregisterIdentityProvider misclassifies an unresolvable ARN as ResourceNotFoundException ---
AWS: DeregisterIdentityProvider given only a well-formed but never-registered IdentityProviderArn answers `ValidationException` (HTTP 400; Smithy: DeregisterIdentityProvider errors include ValidationException @httpError 400 and ResourceNotFoundException @httpError 404) with recorded message "Identity provider is not valid" — the ARN is treated as an invalid parameter, not a missing resource.
Emulator: `ResourceNotFoundException` (HTTP 404). The handler's state layer maps StateError::NotFound onto ResourceNotFoundException for provider lookups.
Why: 404 vs 400 changes retry/branching semantics and makes "this ARN is not a provider" look like "it was deleted".
Fix: the DeregisterIdentityProvider handler's provider-resolution error mapping.
Traces: license-manager-user-subscriptions.deregister-identity-provider-not-found; src/ops/deregister_identity_provider.rs.

--- Cause 3 of 7: StartProductSubscription answers ResourceNotFoundException instead of failing closed ---
AWS: StartProductSubscription for a user whose identity provider was never registered for the requested Product fails closed on authorization: code `UnauthorizedException` with "Unauthorized. Please check the request and access policy and try again." (recorded behavior only).
Emulator: `ResourceNotFoundException` — the handler documents "an identity provider that is not registered for the requested product is a ResourceNotFoundException", which is the wrong class for this path.
Why: the emulator discloses existence where AWS deliberately does not, and clients that treat ResourceNotFoundException as "create it first" take a path AWS never offers.
Fix: the StartProductSubscription handler's unregistered-provider branch.
Traces: license-manager-user-subscriptions.start-product-subscription-provider-not-registered; src/ops/start_product_subscription.rs.

--- Cause 4 of 7: ListTagsForResource refuses a malformed ResourceArn with ValidationException instead of BadRequestException ---
AWS: ListTagsForResource rejects a ResourceArn that does not match the service ARN form AND a well-formed but never-created ARN with the same code, `BadRequestException` — recorded messages "Invalid input resource arn: bad syntax" for the malformed one and "Invalid input resource arn" for the unresolvable one. Recorded behavior only: BadRequestException is not in this service's Smithy model (ListTagsForResource models InternalServerException, ResourceNotFoundException, ValidationException), so the model does not predict the code AWS actually sends.
Emulator: `ValidationException`, produced by the modeled @pattern on ListTagsForResourceRequest.ResourceArn (^arn:([a-z0-9-.]{1,63}):...$) in the generic request-validation step, before the handler.
Why: tag-management clients branch on BadRequestException for "this ARN is not usable"; a ValidationException is a different code and a different retry class.
Fix: the ListTagsForResource (and TagResource/UntagResource) ARN validation — the service's own ARN check must own this refusal rather than the generic pattern validator.
Traces: license-manager-user-subscriptions.list-tags-for-resource-malformed-arn-rejected; src/ops/list_tags_for_resource.rs.

--- Cause 5 of 7: RegisterIdentityProvider accepts a nonexistent DirectoryId with HTTP 200 ---
AWS: RegisterIdentityProvider resolves the ActiveDirectoryIdentityProvider.DirectoryId against AWS Directory Service as part of validating the request and rejects an id that does not resolve (or is not active) with `ValidationException` (HTTP 400), recorded message of the form "<DirectoryId> directory either cannot be found or is not in active state" — the message is built from the request, so each distinct DirectoryId appears in its own rejection.
Emulator: HTTP 200 with a fabricated IdentityProviderSummary (Status REGISTERING, a minted identity-provider ARN) for DirectoryId d-0123456789; the handler persists the registration without ever resolving the directory.
Why: this is the overly-permissive failure mode — a client's registration appears to succeed, later product-subscription flows build on a provider AWS would never have created, and no error is ever surfaced.
Fix: the RegisterIdentityProvider handler's validation — resolve the DirectoryId against directory state and refuse with ValidationException naming the requested id.
Traces: license-manager-user-subscriptions.register-identity-provider-directory-not-found; src/ops/register_identity_provider.rs:138-211 (handler validates only Settings presence and (provider,product) uniqueness before inserting).

--- Cause 6 of 7: RegisterIdentityProvider accepts an unknown Product with HTTP 200 ---
AWS: Product is restricted to the user-based subscription product names. Smithy: RegisterIdentityProviderRequest.Product is @required and its documentation states "Valid values: VISUAL_STUDIO_ENTERPRISE | VISUAL_STUDIO_PROFESSIONAL | OFFICE_PROFESSIONAL_PLUS | OFFICE_STANDARD | REMOTE_DESKTOP_SERVICES". A value outside that set is rejected with `ValidationException` (HTTP 400) and a message naming the rejected value, recorded as "Invalid product NOT_A_REAL_PRODUCT_XYZ" — i.e. the rule is "name the invalid Product value".
Emulator: HTTP 200, persisting an identity provider whose Product is the literal 'NOT_A_REAL_PRODUCT_XYZ'. Because Product is modeled as a plain String (no enum), nothing in the generic validation layer catches it and the handler does not check it either.
Why: callers can register providers for products that do not exist, and every downstream Start/StopProductSubscription and ListProductSubscriptions result keyed on that product is fiction.
Fix: the RegisterIdentityProvider handler's validation — enforce the documented Product value set before minting the registration.
Traces: license-manager-user-subscriptions.register-identity-provider-invalid-product-rejected; src/ops/register_identity_provider.rs:138-211.

--- Cause 7 of 7: StopProductSubscription's missing-identifiers ValidationException does not name the required triple ---
AWS: StopProductSubscription requires either ProductUserArn or the full Username/IdentityProvider/Product triple; Smithy marks none of them @required (StopProductSubscriptionRequest members are all optional), so the service refuses a request naming neither with `ValidationException`. The rejection message is a fixed sentence naming the members that must be supplied — recorded as "IdentityProvider, Product, and Username must be provided", the same "<members> must be provided" form DisassociateUser uses ("IdentityProvider, InstanceId, and Username must be provided"). The requirement is that rule: the message names the required triple.
Emulator: the code and status are right (`ValidationException`), but the message is "address a product user either by ProductUserArn or by Username, IdentityProvider and Product", which does not follow that form. LOW severity: code and status match; only the sentence diverges.
Why: tooling and tests that assert AWS's refusal sentence (and humans reading it) see a different contract for the same refusal.
Fix: the StopProductSubscription handler's identifier-combination validation message.
Traces: license-manager-user-subscriptions.stop-product-subscription-ambiguous-identifiers-rejected; src/ops/stop_product_subscription.rs.

## Evidence
Suite: license-manager-user-subscriptions 1/18 passed (6%), 17 failed.

Cause 1 — AssociateUser with 51 Tags entries against i-0123456789abcdef0: expected 'UnauthorizedException'; AssertionError: ValidationException.
Cause 2 — DeregisterIdentityProvider(IdentityProviderArn=<never-registered>): expected 'ValidationException' / "Identity provider is not valid"; AssertionError: ResourceNotFoundException.
Cause 3 — StartProductSubscription(Username=..., IdentityProvider={'ActiveDirectoryIdentityProvider':{'DirectoryId':'d-0123456789'}}, Product='VISUAL_STUDIO_ENTERPRISE'): expected 'UnauthorizedException'; AssertionError: expected UnauthorizedException, got 'ResourceNotFoundException'.
Cause 4 — ListTagsForResource(ResourceArn='not-an-arn'): expected 'BadRequestException' / "Invalid input resource arn: bad syntax"; AssertionError: expected BadRequestException for a malformed ResourceArn, got 'ValidationException'.
Cause 5 — RegisterIdentityProvider(DirectoryId='d-0123456789', Product='OFFICE_STANDARD', Settings={'Subnets':['subnet-0123456789abcdef0'],'SecurityGroupId':'sg-0123456789abcdef0'}): expected ValidationException "d-0123456789 directory either cannot be found or is not in active state"; the call SUCCEEDED with HTTPStatusCode 200 and body {'IdentityProviderSummary': {'IdentityProvider': {'ActiveDirectoryIdentityProvider': {'DirectoryId': 'd-0123456789'}}, 'Settings': {...}, 'Product': 'OFFICE_STANDARD', 'Status': 'REGISTERING', 'IdentityProviderArn': 'arn:aws:license-manager-user-subscriptions:us-west-2:...:identity-provider/ip-...', 'OwnerAccountId': '...'}}.
Cause 6 — RegisterIdentityProvider with Product='NOT_A_REAL_PRODUCT_XYZ': expected ValidationException "Invalid product NOT_A_REAL_PRODUCT_XYZ"; the call SUCCEEDED with HTTPStatusCode 200 and the summary echoing 'Product': 'NOT_A_REAL_PRODUCT_XYZ' with Status 'REGISTERING'.
Cause 7 — StopProductSubscription() with no members: code 'ValidationException' matched; AssertionError: unexpected message: 'address a product user either by ProductUserArn or by Username, IdentityProvider and Product' (AWS: "IdentityProvider, Product, and Username must be provided").

## Steps to Reproduce
1. Boot the emulator and point a botocore license-manager-user-subscriptions client at it (region us-west-2).
2. AssociateUser(Username='u', InstanceId='i-0123456789abcdef0', IdentityProvider={'ActiveDirectoryIdentityProvider':{'DirectoryId':'d-0123456789'}}, Tags={f'k{i}':'x' for i in range(51)}) -> ValidationException; AWS answers UnauthorizedException, identical to the same call with one tag.
3. DeregisterIdentityProvider(IdentityProviderArn='arn:aws:license-manager-user-subscriptions:us-west-2:<acct>:identity-provider/never-registered') -> ResourceNotFoundException (404); AWS: ValidationException (400) "Identity provider is not valid".
4. StartProductSubscription(Username='u', IdentityProvider={'ActiveDirectoryIdentityProvider':{'DirectoryId':'d-0123456789'}}, Product='VISUAL_STUDIO_ENTERPRISE') -> ResourceNotFoundException; AWS: UnauthorizedException.
5. ListTagsForResource(ResourceArn='not-an-arn') -> ValidationException; AWS: BadRequestException.
6. RegisterIdentityProvider(IdentityProvider={'ActiveDirectoryIdentityProvider':{'DirectoryId':'d-0123456789'}}, Product='OFFICE_STANDARD', Settings={'Subnets':['subnet-0123456789abcdef0'],'SecurityGroupId':'sg-0123456789abcdef0'}) -> HTTP 200; AWS: ValidationException naming d-0123456789. Repeat with DirectoryId 'd-9876543210' — AWS's message names that id instead.
7. Same call with Product='NOT_A_REAL_PRODUCT_XYZ' -> HTTP 200; AWS: ValidationException "Invalid product NOT_A_REAL_PRODUCT_XYZ".
8. StopProductSubscription() with no members -> ValidationException with the wrong sentence.
9. Or replay the check ids in trace_ids.

## Resolution

All seven causes are fixed in the license-manager-user-subscriptions crate, each by a
stated rule rather than by the recorded instance: the documented `Product` value set
(every operation with a `Product` member), a `DirectoryId` resolved against the account's
directory fleet in the service's own state, a per-plane answer for an identity provider
that is
addressed but not registered (an invalid parameter to the registration plane, a fail-closed
`UnauthorizedException` to the product-subscription plane), `BadRequestException` for a
`ResourceArn` this service cannot use (from the boundary refusal as well as the handler),
one builder for the "<members> must be provided" sentence, and the AssociateUser
authorization gate hoisted ahead of modelled-constraint enforcement. `tests/all/refusals.rs`
drives each rule over the wire across a table of inputs.

### Known limitations, tracked for a later change

The account's directories are held in this service's own state behind a seed seam
(`state::seed_account_directories`), exactly as the account's EC2 fleet is
(`state::seed_account_instances`): a registration resolves its `DirectoryId` against that
fleet. Point both seams at the owning services' stores once there is a shared one to read
— reading another service crate's state struct directly would couple the two crates.

A provider the request marks `IsSharedActiveDirectory`, or whose `DirectoryId` takes the
`sd-` form the member's `@pattern` admits beside `d-`, names a directory SHARED into this
account. `ds:DescribeSharedDirectories` is what reports those and is still a scaffolded 501
in the Directory Service emulator, so there is nothing to resolve such an id against: those
registrations are accepted without resolution, and a shared id naming no directory
therefore still mints a provider. Resolve them — and tighten this carve-out — once
Directory Service records shared directories.
