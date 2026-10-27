Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront.delete-public-key-in-use-by-key-group-fails cloudfront.concurrent-update-key-group-conflicting-etag cloudfront.field-level-encryption-profile-delete-in-use-rejected cloudfront.distribution-trusted-key-group-validation-rejected cloudfront.list-conflicting-aliases-detects-overlap cloudfront.get-anycast-ip-list-malformed-id-invalid-argument-rejected cloudfront.get-resource-policy-invalid-argument-rejected cloudfront.op-contract-getResourcePolicy cloudfront.op-contract-createKeyGroup cloudfront.op-contract-get-public-key-wrong-resource-type-id-rejected cloudfront.op-contract-get-connection-group-by-routing-endpoint
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront.delete-public-key-in-use-by-key-group-fails|run#10|AssertionError|assert code == "PublicKeyInUse", f"expected PublicKeyInUse while a key group references it, got {code}"
Issue-Key: cloudfront.concurrent-update-key-group-conflicting-etag|run#16|AssertionError|assert len(oks) == 1, f"expected exactly one winner, got {results}"
Issue-Key: cloudfront.field-level-encryption-profile-delete-in-use-rejected|run#17|AssertionError|assert code == "FieldLevelEncryptionProfileInUse", (
Issue-Key: cloudfront.distribution-trusted-key-group-validation-rejected|_error#13|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: cloudfront.list-conflicting-aliases-detects-overlap|_try_attach_alias#7|AssertionError|raise AssertionError(
Issue-Key: cloudfront.get-anycast-ip-list-malformed-id-invalid-argument-rejected|run#9|AssertionError|assert c2 == "InvalidArgument", c2
Issue-Key: cloudfront.get-resource-policy-invalid-argument-rejected|run#5|AssertionError|assert malformed_message == "ARNs must start with 'arn:'", (
Issue-Key: cloudfront.op-contract-getResourcePolicy|run#5|AssertionError|assert malformed_msg == "ARNs must start with 'arn:'", f"unexpected message for a malformed ResourceArn: {malformed_msg!r}"
Issue-Key: cloudfront.op-contract-createKeyGroup|run#23|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: cloudfront.op-contract-get-public-key-wrong-resource-type-id-rejected|run#16|AssertionError|assert message == "The specified CloudFront public key does not exist.", (
Issue-Key: cloudfront.op-contract-get-connection-group-by-routing-endpoint|_error#18|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded with {result!r}")
Issue-Commit: 281440c6fd2fb349634dcc135fbec4e15edad452

# cloudfront: nine control-plane divergences — DeletePublicKey/UpdateKeyGroup/DeleteFieldLevelEncryptionProfile/DeleteConnectionGroup answer 501, CreateDistribution validates no TrustedKeyGroups or ViewerCertificate, and four refusals carry the wrong code or message

> _Note: this issue was identified against the code base at commit `281440c6fd2fb349634dcc135fbec4e15edad452`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDistribution

## Description
This issue bundles NINE root causes in the cloudfront service. ALL NINE must be closed before submitting; each is numbered below with its own failing check ids.

Cause 1 of 9: DeletePublicKey answers 501 NotImplemented, so the PublicKeyInUse refusal never happens.
AWS: Smithy: DeletePublicKey -> DELETE /2020-05-31/public-key/{Id}, success http code 204, no output body; modeled errors AccessDenied(403), InvalidIfMatchVersion(400), NoSuchPublicKey(404), PreconditionFailed(412), PublicKeyInUse(409). Deleting a public key that a key group still lists must be refused with `PublicKeyInUse` (HTTP 409); the message's fixed text is "The specified public key is in use." with no variable part.
Emulator: the handler returns 501 with code `NotImplemented` for every call, so no public key can ever be deleted and the in-use rule is unobservable.
Why: a client cannot delete a public key at all (its teardown leaks), and an operator relying on CloudFront to protect a key group's referential integrity gets a 501 server fault instead of the 409 that says "detach it from the key group first". A 501 is also classified as a retryable server error by the SDKs, so clients retry a request that can never succeed.
Fix: the DeletePublicKey handler, services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/delete_public_key.rs (still carrying the SCAFFOLDED marker), plus the public-key/key-group reference state it must consult to raise PublicKeyInUse.
Traces: cloudfront.delete-public-key-in-use-by-key-group-fails.

Cause 2 of 9: UpdateKeyGroup answers 501 NotImplemented, so key groups are immutable and optimistic locking is unenforceable.
AWS: Smithy: UpdateKeyGroup -> PUT /2020-05-31/key-group/{Id}, success 200, output KeyGroup payload + ETag header; IfMatch is sent as the If-Match header; modeled errors InvalidArgument(400), InvalidIfMatchVersion(400), KeyGroupAlreadyExists(409), NoSuchResource(404), PreconditionFailed(412), TooManyPublicKeysInKeyGroup(400). Two concurrent updates from the same ETag must produce exactly one winner; the loser is refused on the If-Match precondition (InvalidIfMatchVersion/PreconditionFailed), never silently applied.
Emulator: both concurrent calls are answered 501 `NotImplemented`, so there is no winner and no loser.
Why: the live set of admissible signers for signed URLs/cookies can never be changed — a compromised public key cannot be revoked from a key group — and a client cannot tell a lost-update conflict from a server outage.
Fix: the UpdateKeyGroup handler, services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/update_key_group.rs (still carrying the SCAFFOLDED marker), including the If-Match/ETag version check against the stored key-group record and a fresh ETag on success.
Traces: cloudfront.concurrent-update-key-group-conflicting-etag.

Cause 3 of 9: DeleteFieldLevelEncryptionProfile answers 501 NotImplemented, so the FieldLevelEncryptionProfileInUse refusal never happens.
AWS: Smithy: DeleteFieldLevelEncryptionProfile -> DELETE /2020-05-31/field-level-encryption-profile/{Id}, success 204; modeled errors AccessDenied(403), FieldLevelEncryptionProfileInUse(409), InvalidIfMatchVersion(400), NoSuchFieldLevelEncryptionProfile(404), PreconditionFailed(412). Deleting a profile a field-level-encryption config still references must be refused with `FieldLevelEncryptionProfileInUse` (HTTP 409).
Emulator: 501 `NotImplemented` for every call.
Why: profiles can never be deleted, and the referential-integrity rule that keeps an FLE config from pointing at a deleted profile is absent, so a client cannot distinguish "detach it first" from an emulator outage.
Fix: the DeleteFieldLevelEncryptionProfile handler, services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/delete_field_level_encryption_profile.rs (still carrying the SCAFFOLDED marker), plus the config->profile reference state it must consult.
Traces: cloudfront.field-level-encryption-profile-delete-in-use-rejected.

Cause 4 of 9: CreateDistribution performs no validation of a cache behavior's TrustedKeyGroups block and creates the distribution instead.
AWS: Smithy: CreateDistribution -> POST /2020-05-31/distribution, success 201; its modeled errors include TrustedKeyGroupDoesNotExist(400) and InconsistentQuantities(400) and InvalidArgument(400). A TrustedKeyGroups entry naming a well-formed key-group id that does not exist in the account is refused with `TrustedKeyGroupDoesNotExist`, message "One or more of your trusted keygroups do not exist." (fixed text, no variable part). A Quantity that disagrees with the Items length is refused FIRST, before any id is resolved, with `InconsistentQuantities` whose message names the member and both counts — "The specified quantity of TrustedKeyGroups (<quantity sent>) does not match the actual quantity supplied (<len(Items)>)." (the member name is fixed, the two numbers come from the request). Enabled=true with no items is `InvalidArgument` "The parameter TrustedKeyGroups must contain items if enabled."; Enabled=false with items is `InvalidArgument` "The parameter TrustedKeyGroups must not contain items if disabled." — two distinct messages for the two directions of one cross-field rule. None of the four creates a distribution.
Emulator: the first leg (Enabled=true, Quantity=1, Items=[a nonexistent key-group id]) is ACCEPTED: HTTP 201 with a Location header, an ETag and a full Distribution body whose ActiveTrustedKeyGroups echoes the nonexistent id back as an active signer group (Quantity 1, KeyPairIds Quantity 0). No TrustedKeyGroups validation runs at all, so the ordering rule and the other three refusals are unreachable too.
Why: TrustedKeyGroups is an authorization boundary — a behavior that lists it only serves signed requests. A distribution created from a dangling or malformed block LOOKS protected and is not: every URL under it is public while its config claims a trusted signer group, and the echoed ActiveTrustedKeyGroups entry with zero key pairs cannot admit any signature. A client also loses the validation ORDER that tells it whether its shape is wrong or its id is wrong.
Fix: CreateDistribution's input validation (and the same validation on UpdateDistribution/CreateDistributionWithTags), in the cloudfront crate's distribution-config validation path: check each TrustedKeyGroups/TrustedSigners block's Quantity-vs-Items consistency and Enabled-vs-Items rule before resolving ids, then resolve each id against key-group state. Related but distinct open issues: 20261022T025800Z-cloudfront-createdistribution-accepts-configurations-aws-rejects.md and Cause-level notes in 20261024T182555Z-cloudfront-ten-fidelity-divergences-distribution-policy-tenant-validation.md — neither carries this check or the TrustedKeyGroups rules.
Traces: cloudfront.distribution-trusted-key-group-validation-rejected.

Cause 5 of 9: CreateDistributionWithTags accepts an Aliases entry whose ViewerCertificate cannot be validated for the domain, instead of refusing it with InvalidViewerCertificate.
AWS: Smithy: CreateDistributionWithTags shares CreateDistribution's error set, including InvalidViewerCertificate(400). Attaching an alias requires a certificate that is valid for that domain and in us-east-1; otherwise the create is refused with `InvalidViewerCertificate` and the message "The specified SSL certificate doesn't exist, isn't in us-east-1 region, isn't valid, or doesn't include a valid certificate chain." (fixed text, no variable part), and no distribution is created.
Emulator: the create SUCCEEDS and returns a distribution id, so an alias is attached with an unvalidated certificate.
Why: alias ownership and certificate validity are the gate that stops one tenant claiming another's domain; an emulator that skips it lets a client build a distribution AWS would reject, and the alias-conflict discovery surface (ListConflictingAliases) can never be set up because the wildcard alias is attached without the refusal that real AWS raises.
Fix: CreateDistributionWithTags/CreateDistribution ViewerCertificate validation in the cloudfront crate — an Aliases block must be refused unless the referenced ACMCertificateArn/IAMCertificateId exists, is us-east-1 and covers the alias.
Traces: cloudfront.list-conflicting-aliases-detects-overlap.

Cause 6 of 9: GetAnycastIpList does not validate the Id's shape, so a malformed Id is answered EntityNotFound instead of InvalidArgument.
AWS: Smithy: GetAnycastIpList -> GET /2020-05-31/anycast-ip-list/{Id}, success 200; modeled errors AccessDenied(403), EntityNotFound(404), InvalidArgument(400), UnsupportedOperation(400). The two errors track different conditions: an Id that is well formed ("aip_" + 22 alphanumerics) but absent is `EntityNotFound`, while an Id containing characters the format does not allow is refused up front with `InvalidArgument` — the shape is validated before the lookup.
Emulator: the malformed Id is answered `EntityNotFound`, the same code as the absent-but-well-formed Id, collapsing the two conditions into one.
Why: a client cannot tell "you typed the id wrong" from "that list is gone" — the first is a 400 it must fix in its own code, the second a 404 it may legitimately tolerate — and retry/alarm logic keyed on 404-vs-400 behaves wrongly.
Fix: GetAnycastIpList's input validation in services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/get_anycast_ip_list.rs: reject an Id that does not match the anycast-ip-list id format with InvalidArgument before consulting state.
Traces: cloudfront.get-anycast-ip-list-malformed-id-invalid-argument-rejected.

Cause 7 of 9: GetResourcePolicy's malformed-ARN refusal carries the wrong message — the resource-type message is returned for input that is not an ARN at all.
AWS: Smithy: GetResourcePolicy -> POST /2020-05-31/get-resource-policy with a required ResourceArn; modeled errors AccessDenied(403), EntityNotFound(404), InvalidArgument(400), UnsupportedOperation(400). `InvalidArgument` covers two distinct causes with two distinct messages: a structurally malformed ResourceArn (one that fails ARN syntax parsing, e.g. "this-is-not-an-arn") is refused with "ARNs must start with 'arn:'" — fixed text, no variable part — whereas a well-formed ARN of a resource type the operation does not support is refused with "The specified resource type isn't valid. Specify a VPC origin and try again."
Emulator: BOTH inputs get the resource-type message: a ResourceArn of "this-is-not-an-arn" is refused with "The specified resource type isn't valid. Specify a VPC origin and try again." The code is InvalidArgument (correct); only the message is wrong, so the two causes are indistinguishable.
Why: the message is the only thing that attributes an InvalidArgument to its cause here. A client told to "specify a VPC origin" when its real defect is a syntactically broken ARN is sent to fix the wrong thing, and an emulator-driven test that asserts on the message cannot pin either branch.
Fix: GetResourcePolicy's ResourceArn validation in services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/get_resource_policy.rs: parse ARN syntax first and refuse a non-ARN with the ARN-syntax message, then (only for a syntactically valid ARN) check the resource type and refuse an unsupported type with the resource-type message. PutResourcePolicy needs the same two-stage order.
Traces: cloudfront.get-resource-policy-invalid-argument-rejected, cloudfront.op-contract-getResourcePolicy.

Cause 8 of 9: refusal messages are rendered from the Smithy error shape's DOCUMENTATION text instead of AWS's runtime message.
AWS: an AWS error message is the service's runtime text for that condition; the model's error-shape doc string is prose about the error, not the wire message. Measured: CreateKeyGroup with a Name that already exists answers `KeyGroupAlreadyExists` (HTTP 409) with the message "Another key group with the same name already exists within the aws account." (fixed text, no variable part). GetPublicKey for an id that is not a public-key id answers `NoSuchPublicKey` (HTTP 404) with "The specified CloudFront public key does not exist."
Emulator: CreateKeyGroup returns "A key group with this name already exists. You must provide a unique name. To modify an existing key group, use UpdateKeyGroup." — verbatim the KeyGroupAlreadyExists shape's model documentation; GetPublicKey returns "The specified public key doesn't exist." — verbatim the NoSuchPublicKey shape's model documentation. The codes and statuses are right; the message text is the doc string in both cases, which is why these two are one cause and not two.
Why: error messages are part of the contract clients and tooling match on; a message that is the SDK's own documentation rather than the service's response makes emulator-recorded goldens diverge from AWS and breaks any consumer that keys on the message text.
Fix: the cloudfront crate's error construction for these refusals (CreateKeyGroup's duplicate-name path and GetPublicKey's not-found path), and the shared helper that populates a modeled error's message — it must use the service's runtime message, not the shape's documentation trait. Audit the other modeled cloudfront errors for the same substitution.
Traces: cloudfront.op-contract-createKeyGroup, cloudfront.op-contract-get-public-key-wrong-resource-type-id-rejected.

Cause 9 of 9: DeleteConnectionGroup answers 501 NotImplemented, so a connection group can never be removed and GetConnectionGroupByRoutingEndpoint keeps resolving a deleted group.
AWS: Smithy: GetConnectionGroupByRoutingEndpoint -> GET /2020-05-31/connection-group?RoutingEndpoint=..., success 200 with a ConnectionGroup payload and ETag header; modeled errors AccessDenied(403) and EntityNotFound(404). After the group is disabled and deleted, the identical lookup must be refused `EntityNotFound` with "The specified connection group does not exist." (fixed text, no variable part). DeleteConnectionGroup is the operation that must make that true.
Emulator: GetConnectionGroupByRoutingEndpoint still answers HTTP 200 with the full ConnectionGroup (Id, Arn, RoutingEndpoint, Status InProgress, Enabled true) after the delete sequence, because both operations that sequence needs answer 501 `NotImplemented`: UpdateConnectionGroup (already filed as 20261026T092638Z-cloudfront-updateconnectiongroup-stub.md) and DeleteConnectionGroup, whose handler is still a scaffold — so nothing is ever deleted. Hypothesis: the lookup itself is correct and only reports state that the missing delete never removed.
Why: connection groups accumulate forever and a routing endpoint that a client believes it released still resolves, so a caller cannot reuse the name, cannot confirm a teardown, and reads a deleted resource as live.
Fix: the DeleteConnectionGroup handler, services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/delete_connection_group.rs (still carrying the SCAFFOLDED marker) — honour If-Match, remove the record from connection-group state, and make the routing-endpoint index drop with it so GetConnectionGroupByRoutingEndpoint answers EntityNotFound.
Traces: cloudfront.op-contract-get-connection-group-by-routing-endpoint.

## Evidence
Cause 1 — cloudfront.delete-public-key-in-use-by-key-group-fails [mismatch]:
  AssertionError: expected PublicKeyInUse while a key group references it, got NotImplemented
  Expected: HTTP 409, code PublicKeyInUse. Actual: HTTP 501, code NotImplemented.

Cause 2 — cloudfront.concurrent-update-key-group-conflicting-etag [mismatch]:
  AssertionError: expected exactly one winner, got [('a', 'NotImplemented', None), ('b', 'NotImplemented', None)]
  Expected: one call HTTP 200 with a new ETag, the other HTTP 400 InvalidIfMatchVersion / HTTP 412 PreconditionFailed. Actual: both HTTP 501 NotImplemented.

Cause 3 — cloudfront.field-level-encryption-profile-delete-in-use-rejected [mismatch]:
  AssertionError: expected FieldLevelEncryptionProfileInUse, got 'NotImplemented'
  Expected: HTTP 409, code FieldLevelEncryptionProfileInUse. Actual: HTTP 501, code NotImplemented.

Cause 4 — cloudfront.distribution-trusted-key-group-validation-rejected [mismatch]:
  AssertionError: expected the call to be rejected, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 201, ...}, 'Location': ..., 'ETag': ..., 'Distribution': {'Status': 'InProgress', 'ActiveTrustedKeyGroups': {'Enabled': True, 'Quantity': 1, 'Items': [{'KeyGroupId': '8f4d1c2b-6a3e-4f5d-9b8c-1e2f3a4b5c6d', 'KeyPairIds': {'Quantity': 0}}]}, ...}}
  The request was DefaultCacheBehavior.TrustedKeyGroups = {Enabled: true, Quantity: 1, Items: [<a well-formed key-group id that does not exist>]}.
  Expected: HTTP 400, code TrustedKeyGroupDoesNotExist, message "One or more of your trusted keygroups do not exist.", no distribution created. Actual: HTTP 201 and a distribution. (Ids, ARNs, domain names, ETags and timestamps in the recorded body are per-run values, not requirements; the 201-instead-of-400 and the echoed dangling KeyGroupId are.)
  Three further legs of the same cause are unreachable because leg 1 does not refuse: Quantity 2 with 1 item -> expected InconsistentQuantities, "The specified quantity of TrustedKeyGroups (2) does not match the actual quantity supplied (1)." (the two numbers come from the request); Enabled true + 0 items -> InvalidArgument "The parameter TrustedKeyGroups must contain items if enabled."; Enabled false + 1 item -> InvalidArgument "The parameter TrustedKeyGroups must not contain items if disabled."

Cause 5 — cloudfront.list-conflicting-aliases-detects-overlap [mismatch]:
  AssertionError: expected InvalidViewerCertificate, but CreateDistributionWithTags SUCCEEDED and created 'E7IYCGPKTANSF3'
  Expected: HTTP 400, code InvalidViewerCertificate, message "The specified SSL certificate doesn't exist, isn't in us-east-1 region, isn't valid, or doesn't include a valid certificate chain.", nothing created. Actual: HTTP 201 and a distribution id (the id itself is a per-run value).

Cause 6 — cloudfront.get-anycast-ip-list-malformed-id-invalid-argument-rejected [mismatch]:
  AssertionError: EntityNotFound  (assert c2 == "InvalidArgument")
  Expected for an Id containing characters the anycast-ip-list id format disallows: HTTP 400, code InvalidArgument. Actual: HTTP 404, code EntityNotFound — the same answer the well-formed-but-absent Id gets.

Cause 7 — cloudfront.get-resource-policy-invalid-argument-rejected and cloudfront.op-contract-getResourcePolicy [mismatch]:
  AssertionError: expected AWS's exact malformed-ARN message, got "The specified resource type isn't valid. Specify a VPC origin and try again."
  AssertionError: unexpected message for a malformed ResourceArn: "The specified resource type isn't valid. Specify a VPC origin and try again."
  Request: GetResourcePolicy(ResourceArn="this-is-not-an-arn"). Expected: code InvalidArgument with message "ARNs must start with 'arn:'". Actual: code InvalidArgument with the unsupported-resource-type message.

Cause 8 — cloudfront.op-contract-createKeyGroup [mismatch]:
  AssertionError: expected message 'Another key group with the same name already exists within the aws account.', got 'A key group with this name already exists. You must provide a unique name. To modify an existing key group, use UpdateKeyGroup.'
  cloudfront.op-contract-get-public-key-wrong-resource-type-id-rejected [mismatch]:
  AssertionError: expected AWS's exact not-found message for a key-group id, got "The specified public key doesn't exist."  (expected "The specified CloudFront public key does not exist.")
  In both cases the emulator's text is the error shape's model documentation string verbatim.

Cause 9 — cloudfront.op-contract-get-connection-group-by-routing-endpoint [mismatch]:
  AssertionError: expected the call to be refused, but it succeeded with {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'ETag': ..., 'ConnectionGroup': {'Id': 'cg_...', 'RoutingEndpoint': '<d...>.cloudfront.net', 'Status': 'InProgress', 'Enabled': True, 'IsDefault': False, ...}}
  This is the lookup issued AFTER the group was disabled and deleted. Expected: HTTP 404, code EntityNotFound, message "The specified connection group does not exist.". Actual: HTTP 200 and the live group. The delete sequence it depends on is refused 501 NotImplemented by UpdateConnectionGroup and DeleteConnectionGroup (the ids/endpoints above are per-run values).

## Steps to Reproduce
Cause 1: CreatePublicKey, then CreateKeyGroup listing that key, then DeletePublicKey(Id=<key id>, IfMatch=<etag>) -> expect HTTP 409 PublicKeyInUse; observe HTTP 501 NotImplemented.
Cause 2: CreatePublicKey + CreateKeyGroup, read the group's ETag, then fire two UpdateKeyGroup calls concurrently with that same If-Match -> expect exactly one HTTP 200 and one InvalidIfMatchVersion/PreconditionFailed; observe both HTTP 501 NotImplemented.
Cause 3: CreatePublicKey, CreateFieldLevelEncryptionProfile over it, CreateFieldLevelEncryptionConfig referencing the profile, then DeleteFieldLevelEncryptionProfile(Id=<profile>, IfMatch=<etag>) -> expect HTTP 409 FieldLevelEncryptionProfileInUse; observe HTTP 501 NotImplemented.
Cause 4: CreateDistribution with a minimal config whose DefaultCacheBehavior.TrustedKeyGroups = {Enabled: true, Quantity: 1, Items: ["<any well-formed key-group id not in this account>"]} -> expect HTTP 400 TrustedKeyGroupDoesNotExist and no distribution; observe HTTP 201 with the dangling id echoed in ActiveTrustedKeyGroups. Then repeat with {Enabled: true, Quantity: 2, Items: [<same id>]} (expect InconsistentQuantities naming both counts), {Enabled: true, Quantity: 0, Items: []} and {Enabled: false, Quantity: 1, Items: [<same id>]} (expect the two distinct InvalidArgument messages).
Cause 5: CreateDistributionWithTags with an Aliases entry and a ViewerCertificate naming a certificate that is not a valid us-east-1 certificate for that alias -> expect HTTP 400 InvalidViewerCertificate and nothing created; observe HTTP 201 and a distribution id.
Cause 6: GetAnycastIpList(Id="<non-empty id containing characters the aip_ format disallows>") -> expect HTTP 400 InvalidArgument; observe HTTP 404 EntityNotFound. Compare with GetAnycastIpList(Id="aip_" + 22 alphanumerics) which must be EntityNotFound.
Cause 7: GetResourcePolicy(ResourceArn="this-is-not-an-arn") -> expect InvalidArgument with "ARNs must start with 'arn:'"; observe InvalidArgument with "The specified resource type isn't valid. Specify a VPC origin and try again." (which is the correct message only for a well-formed ARN of an unsupported resource type, e.g. a KeyValueStore ARN).
Cause 8: CreateKeyGroup twice with the same Name -> the second must be HTTP 409 KeyGroupAlreadyExists with "Another key group with the same name already exists within the aws account."; observe the model's documentation string instead. GetPublicKey(Id=<a key-group id>) -> must be HTTP 404 NoSuchPublicKey with "The specified CloudFront public key does not exist."; observe "The specified public key doesn't exist."
Cause 9: CreateConnectionGroup(Name=..., Enabled=true), take its RoutingEndpoint, confirm GetConnectionGroupByRoutingEndpoint resolves it, then UpdateConnectionGroup(Enabled=false) + DeleteConnectionGroup(IfMatch=<etag>) and repeat the lookup -> expect HTTP 404 EntityNotFound with "The specified connection group does not exist."; observe both mutations answer 501 NotImplemented and the lookup still answers HTTP 200 with the group.
Finally, re-replay every check id listed in trace_ids.
