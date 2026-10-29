Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront.create_cache_policy_duplicate_name_rejected cloudfront.op-contract-create-cache-policy cloudfront.origin-request-policy-duplicate-name-fails cloudfront.op-contract-createOriginRequestPolicy cloudfront.op-contract-createResponseHeadersPolicy cloudfront.key-group-duplicate-name-fails cloudfront.function-duplicate-name-conflict cloudfront.op-contract-createFunction cloudfront.op-contract-createKeyValueStore cloudfront.oac-duplicate-name-fails cloudfront.op-contract-createPublicKey cloudfront.op-contract-create-connection-function cloudfront.op-contract-create-connection-group cloudfront.associate-distribution-tenant-web-acl-limit-exceeded-fails cloudfront.associate-distribution-web-acl-entity-limit-exceeded-rejected cloudfront.op-contract-associate-distribution-web-acl cloudfront.distribution-web-acl-disassociate-no-association-rejected cloudfront.distribution-tenant-delete-stale-etag-rejected cloudfront.delete-distribution-tenant-stale-etag-rejected cloudfront.delete-distribution-tenant-stale-version-rejected cloudfront.cache-policy-update-stale-etag-fails cloudfront.update-origin-request-policy-concurrent-modification-detected cloudfront.delete-function-stale-etag-rejected cloudfront.get-key-group-cross-resource-id-rejected cloudfront.op-contract-get-key-group cloudfront.op-contract-get-public-key cloudfront.op-contract-get-public-key-config cloudfront.op-contract-create-field-level-encryption-profile cloudfront.op-contract-get-invalidation-for-distribution-tenant cloudfront.op-contract-get-invalidation-for-distribution-tenant-not-found-rejected cloudfront.list-distributions-by-key-group-invalid-argument-rejected cloudfront.connection-group-delete-enabled-rejected cloudfront.delete-distribution-tenant-in-use-connection-group-fails cloudfront.oac-in-use-delete-rejected cloudfront.oai-in-use-delete-fails cloudfront.key-value-store-delete-in-use-by-function-fails cloudfront.cache-policy-invalid-ttl-order-rejected cloudfront.public-key-immutable-field-fails cloudfront.op-contract-listCachePolicies cloudfront.update-anycast-ip-list-invalid-argument-rejected cloudfront.create-invalidation-for-tenant-batch-too-large-rejected cloudfront.e2e-distribution-tenant-invalidation-scoped-to-tenant cloudfront.invalidation-for-tenant-lifecycle cloudfront.op-contract-get-distribution-tenant-by-domain cloudfront.list-domain-conflicts-invalid-argument-rejected
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront.create_cache_policy_duplicate_name_rejected|run#10|AssertionError|assert msg == dup_msg, f"expected message {dup_msg!r}, got {msg!r}"
Issue-Key: cloudfront.op-contract-create-cache-policy|run#16|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: cloudfront.origin-request-policy-duplicate-name-fails|run#12|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: cloudfront.op-contract-createOriginRequestPolicy|run#17|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: cloudfront.op-contract-createResponseHeadersPolicy|run#18|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: cloudfront.key-group-duplicate-name-fails|run#14|AssertionError|assert message == "Another key group with the same name already exists within the aws account.", (
Issue-Key: cloudfront.function-duplicate-name-conflict|run#9|AssertionError|assert error.get("Message") == "", f"expected an empty message, got {error.get('Message')!r}"
Issue-Key: cloudfront.op-contract-createFunction|run#20|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: cloudfront.op-contract-createKeyValueStore|run#18|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: cloudfront.oac-duplicate-name-fails|run#12|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: cloudfront.op-contract-createPublicKey|run#19|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: cloudfront.op-contract-create-connection-function|run#20|AssertionError|assert err_message == expected, f"expected message {expected!r}, got {err_message!r}"
Issue-Key: cloudfront.op-contract-create-connection-group|run#23|AssertionError|assert err_message == expected, f"expected message {expected!r}, got {err_message!r}"
Issue-Key: cloudfront.associate-distribution-tenant-web-acl-limit-exceeded-fails|run#25|AssertionError|assert msg == precondition_msg, (
Issue-Key: cloudfront.associate-distribution-web-acl-entity-limit-exceeded-rejected|run#14|AssertionError|assert msg == precondition_msg, (
Issue-Key: cloudfront.op-contract-associate-distribution-web-acl|run#16|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: cloudfront.distribution-web-acl-disassociate-no-association-rejected|run#15|AssertionError|assert message == _PRECONDITION_FAILED, (
Issue-Key: cloudfront.distribution-tenant-delete-stale-etag-rejected|run#12|AssertionError|assert message == _PRECONDITION_FAILED, f"expected message {_PRECONDITION_FAILED!r}, got {message!r}"
Issue-Key: cloudfront.delete-distribution-tenant-stale-etag-rejected|run#16|AssertionError|assert message == "Your request is missing requirements for this operation. Update your request and try again.", f"unexpected PreconditionFailed message: {messa
Issue-Key: cloudfront.delete-distribution-tenant-stale-version-rejected|run#17|AssertionError|assert message == "Your request is missing requirements for this operation. Update your request and try again.", f"unexpected PreconditionFailed message: {messa
Issue-Key: cloudfront.cache-policy-update-stale-etag-fails|run#11|AssertionError|assert msg == expected_msg, f"expected message {expected_msg!r}, got {msg!r}"
Issue-Key: cloudfront.update-origin-request-policy-concurrent-modification-detected|run#15|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: cloudfront.delete-function-stale-etag-rejected|run#8|AssertionError|assert code == "InvalidIfMatchVersion", f"expected InvalidIfMatchVersion, got {code}"
Issue-Key: cloudfront.get-key-group-cross-resource-id-rejected|run#16|AssertionError|assert message == "Id format is invalid", (
Issue-Key: cloudfront.op-contract-get-key-group|run#17|AssertionError|assert message == "Id format is invalid", (
Issue-Key: cloudfront.op-contract-get-public-key|run#15|AssertionError|assert message == "The specified CloudFront public key does not exist.", (
Issue-Key: cloudfront.op-contract-get-public-key-config|run#14|AssertionError|assert message == "The specified CloudFront public key does not exist.", (
Issue-Key: cloudfront.op-contract-create-field-level-encryption-profile|run#28|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: cloudfront.op-contract-get-invalidation-for-distribution-tenant|run#4|AssertionError|assert message == "DistributionTenantId format is invalid", (
Issue-Key: cloudfront.op-contract-get-invalidation-for-distribution-tenant-not-found-rejected|run#8|AssertionError|assert message == "DistributionTenantId format is invalid", (
Issue-Key: cloudfront.list-distributions-by-key-group-invalid-argument-rejected|run#9|AssertionError|assert message == "The specified key group does not exist.", f"unexpected message {message!r}"
Issue-Key: cloudfront.connection-group-delete-enabled-rejected|run#9|AssertionError|assert msg == not_disabled_msg, f"expected message {not_disabled_msg!r}, got {msg!r}"
Issue-Key: cloudfront.delete-distribution-tenant-in-use-connection-group-fails|run#14|AssertionError|assert message == "You must disable the distribution tenant before you can delete it.", f"unexpected ResourceNotDisabled message: {message!r}"
Issue-Key: cloudfront.oac-in-use-delete-rejected|run#29|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: cloudfront.oai-in-use-delete-fails|run#17|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: cloudfront.key-value-store-delete-in-use-by-function-fails|run#14|AssertionError|assert message == expected_in_use, f"expected {expected_in_use!r}, got {message!r}"
Issue-Key: cloudfront.cache-policy-invalid-ttl-order-rejected|run#5|AssertionError|assert msg == ttl_rule, (
Issue-Key: cloudfront.public-key-immutable-field-fails|run#21|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: cloudfront.op-contract-listCachePolicies|run#10|AssertionError|assert msg == "1 validation error detected: Value 'not-a-real-policy-type' at 'type' failed to satisfy constraint: Member must satisfy enum value set: [custom,
Issue-Key: cloudfront.update-anycast-ip-list-invalid-argument-rejected|run#10|AssertionError|assert code == "InvalidArgument", f"expected InvalidArgument, got {code!r} ({message!r})"
Issue-Key: cloudfront.create-invalidation-for-tenant-batch-too-large-rejected|run#9|AssertionError|assert code == "413", f"expected 413 (BatchTooLarge), got {code!r}"
Issue-Key: cloudfront.e2e-distribution-tenant-invalidation-scoped-to-tenant|run#21|AssertionError|assert code == "EntityNotFound", (
Issue-Key: cloudfront.invalidation-for-tenant-lifecycle|run#16|AssertionError|assert code == "EntityNotFound", f"expected EntityNotFound, got {code}"
Issue-Key: cloudfront.op-contract-get-distribution-tenant-by-domain|run#12|AssertionError|assert cert_message == (
Issue-Key: cloudfront.list-domain-conflicts-invalid-argument-rejected|run#8|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# cloudfront: 10 refusal-fidelity divergences — duplicate-name, precondition, not-found, in-use, TTL-order, enum, BatchTooLarge and tenant-invalidation refusals all carry emulator-authored wording or the wrong error code

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCachePolicy

## Description
This issue bundles 10 net-new root causes in the cloudfront crate. All ten are refusals the emulator DOES raise, with either the wrong error code or a message that is not how AWS words that error class. Related open issues 20261023T140640Z-cloudfront-9-fidelity-divergences-19-control-plane-operations.md, 20261024T182555Z-cloudfront-ten-fidelity-divergences-distribution-policy-tenant-validation.md and 20261027T001538Z-cloudfront-nine-control-plane-divergences-deletepublickey-updatekeygroup-deletefieldlevelencryptionprofile.md cover neighbouring checks but none of the replay keys below.

Cause 1 of 10: every duplicate-name / duplicate-caller-reference conflict is worded by the emulator, not by AWS.
AWS: the conflict errors are modelled per resource (Smithy: CachePolicyAlreadyExists, OriginRequestPolicyAlreadyExists, ResponseHeadersPolicyAlreadyExists, KeyGroupAlreadyExists 409, FunctionAlreadyExists, EntityAlreadyExists, OriginAccessControlAlreadyExists, PublicKeyAlreadyExists, CloudFrontOriginAccessIdentityAlreadyExists). Each carries AWS's fixed sentence and nothing from the request: "Another cache policy with the same name already exists within the aws account.", "Another origin request policy with the same name already exists within the aws account.", "Another response headers policy with the same name already exists within the aws account.", "An origin access control with the same name already exists.", "The caller reference that you are using to create a CloudFront public key is associated with another public key in your account.", "A connection function with the same name already exists in this AWS account.", "A connection group with the same name already exists in this AWS account."; CreateFunction's and CreateKeyValueStore's conflicts carry an EMPTY message.
Emulator: each handler composes its own sentence and interpolates the requested name and/or the colliding resource id, e.g. "A cache policy with the name <Name> already exists in this account. You must provide a unique name. To modify an existing cache policy, use UpdateCachePolicy.", "An origin access control with the specified parameters already exists: <Id>", "The specified public key already exists: <Id>", "A key value store with the name <Name> already exists in this account", and for CreateFunction a long "...To update an existing function, use UpdateFunction." sentence where AWS sends no message at all.
Why: a client that branches on the message (or logs/asserts it) sees prose AWS never sends, and the leaked resource id discloses state the request did not name.
Fix: the conflict branch of each create handler's validation in the cloudfront crate; the sentence must be fixed text per error shape, with no request or state values interpolated.
Traces: cloudfront.create_cache_policy_duplicate_name_rejected, .op-contract-create-cache-policy, .origin-request-policy-duplicate-name-fails, .op-contract-createOriginRequestPolicy, .op-contract-createResponseHeadersPolicy, .key-group-duplicate-name-fails, .function-duplicate-name-conflict, .op-contract-createFunction, .op-contract-createKeyValueStore, .oac-duplicate-name-fails, .op-contract-createPublicKey, .op-contract-create-connection-function, .op-contract-create-connection-group.

Cause 2 of 10: ONE generic sentence is used for every If-Match precondition refusal, and PreconditionFailed is raised where AWS raises InvalidIfMatchVersion.
AWS: Smithy models both PreconditionFailed (412) and InvalidIfMatchVersion (400) on the ETag-taking operations (e.g. AssociateDistributionTenantWebACL, DisassociateDistributionWebACL, UpdateKeyGroup, DeleteKeyGroup, DeletePublicKey, DeleteOriginAccessControl all list both). The wording is per operation family and is fixed text with nothing from the request: the policy/function/tenant update family uses "The request failed because it didn't meet the preconditions for this operation.", while the web-ACL association/disassociation family uses "Your request is missing requirements for this operation. Update your request and try again." DeleteFunction with a stale ETag is classified InvalidIfMatchVersion, not PreconditionFailed.
Emulator: one shared sentence, "The request failed because it didn't meet the preconditions in one or more request-header fields.", is served for all of them, and DeleteFunction's stale-ETag refusal is classified PreconditionFailed.
Why: a client cannot tell a stale-ETag concurrency loss from a malformed/absent If-Match, and retry logic keyed on InvalidIfMatchVersion never fires.
Fix: the shared precondition helper in the cloudfront crate's state/error module plus the per-operation classification in the ETag-taking handlers.
Traces: cloudfront.associate-distribution-tenant-web-acl-limit-exceeded-fails, .associate-distribution-web-acl-entity-limit-exceeded-rejected, .op-contract-associate-distribution-web-acl, .distribution-web-acl-disassociate-no-association-rejected, .distribution-tenant-delete-stale-etag-rejected, .delete-distribution-tenant-stale-etag-rejected, .delete-distribution-tenant-stale-version-rejected, .cache-policy-update-stale-etag-fails, .update-origin-request-policy-concurrent-modification-detected, .delete-function-stale-etag-rejected.

Cause 3 of 10: entity-not-found refusals use emulator wording instead of AWS's fixed sentence.
AWS: Smithy: GetPublicKey/GetPublicKeyConfig -> NoSuchPublicKey (404), message "The specified CloudFront public key does not exist."; CreateFieldLevelEncryptionProfile's NoSuchPublicKey uses the same sentence; GetKeyGroup -> NoSuchResource (404) with message "Id format is invalid"; the distribution-tenant invalidation reads -> EntityNotFound (404) with AWS's own sentence; ListDistributionsByKeyGroup's refusal is not "The specified resource doesn't exist. Update your request and try again.". All are fixed text.
Emulator: "The specified public key doesn't exist.", "The specified key group does not exist.", "The specified distribution tenant doesn't exist.", "The specified resource doesn't exist. Update your request and try again." — contraction-styled paraphrases the service never sends.
Why: message-matching clients and fixtures break; the paraphrase is indistinguishable per-resource, so a client cannot tell which entity was missing.
Fix: the not-found branch of each read handler's validation; single-home the sentence per error shape.
Traces: cloudfront.get-key-group-cross-resource-id-rejected, .op-contract-get-key-group, .op-contract-get-public-key, .op-contract-get-public-key-config, .op-contract-create-field-level-encryption-profile, .op-contract-get-invalidation-for-distribution-tenant, .op-contract-get-invalidation-for-distribution-tenant-not-found-rejected, .list-distributions-by-key-group-invalid-argument-rejected.

Cause 4 of 10: resource-in-use and not-yet-disabled refusals are worded by the emulator and leak the blocking resource's id.
AWS: Smithy: DeleteOriginAccessControl -> OriginAccessControlInUse (409) "The CloudFront origin access control is still being used."; DeleteCloudFrontOriginAccessIdentity -> CloudFrontOriginAccessIdentityInUse (409) "The CloudFront origin access identity is still being used."; DeleteKeyValueStore -> "Cannot delete KeyValueStore <name> because it is associated with a function" (the store name is the ONLY variable part, taken from the request); DeleteConnectionGroup -> "You must disable the specified connection group before you can delete it."; DeleteDistributionTenant's ResourceNotDisabled carries AWS's own sentence.
Emulator: "Cannot delete the origin access control because it's in use by one or more distributions: <DistributionId>", "The Origin Access Identity specified is already in use: <Id>", "The entity cannot be deleted while it is in use.", "The specified CloudFront resource hasn't been disabled yet.", "The specified distribution tenant is not disabled. You must disable the distribution tenant before you can delete it." — all emulator prose, two of them echoing a state-decided id the request never named.
Why: the message no longer names which resource class blocked the delete (one generic "entity" sentence), and ids of unrelated distributions leak into the error.
Fix: the in-use/not-disabled branch of each delete handler's validation.
Traces: cloudfront.connection-group-delete-enabled-rejected, .delete-distribution-tenant-in-use-connection-group-fails, .oac-in-use-delete-rejected, .oai-in-use-delete-fails, .key-value-store-delete-in-use-by-function-fails.

Cause 5 of 10: the TTL-ordering refusal does not quote the ordering rule.
AWS: Smithy: CreateCachePolicy/CreateDistribution -> InvalidTTLOrder (400). Because InvalidArgument/InvalidTTLOrder is CloudFront's catch-all, the message is what attributes the refusal: "MinTTL, MaxTTL and DefaultTTL should follow order MinTTL less than or equal to DefaultTTL less than or equal to MaxTTL" — fixed text, no numbers from the request.
Emulator: InvalidArgument "An argument is invalid: MaxTTL must be greater than or equal to MinTTL, but the request submits MinTTL <MinTTL> and MaxTTL <MaxTTL>" — a rewritten rule that also interpolates the two submitted values.
Why: the client is told only about the Min/Max pair, so a DefaultTTL that breaks the same chain is not attributable from the message.
Fix: the TTL-chain check in the cache-policy validation.
Traces: cloudfront.cache-policy-invalid-ttl-order-rejected.

Cause 6 of 10: the public-key immutability refusal is reworded.
AWS: UpdatePublicKey refuses a changed name or encoded material with the fixed sentence "You cannot modify encoded material and name of a public key once created."
Emulator: "You can't change the value of a public key."
Why: the message no longer says WHICH fields are immutable, so a client cannot tell that Comment is updatable while Name/EncodedKey are not.
Fix: UpdatePublicKey's validation in the cloudfront crate.
Traces: cloudfront.public-key-immutable-field-fails.

Cause 7 of 10: a bad enum value is refused as MalformedXML with a generic constraint report instead of the modelled InvalidArgument.
AWS: Smithy: ListCachePolicies takes Type (enum CachePolicyType: managed|custom) and models InvalidArgument (400); UpdateAnycastIpList models InvalidArgument (400) for a bad IpAddressType. AWS refuses an out-of-set enum on these REST-XML query/body members with InvalidArgument and CloudFront's own wording, not with MalformedXML.
Emulator: MalformedXML carrying the generic "1 validation error detected: Value '<value>' at '<member>' failed to satisfy constraint: Member must satisfy enum value set: [...]" text — a shape-validator message, not this service's.
Why: a client branching on InvalidArgument sees MalformedXML and treats a value error as a serialization bug.
Fix: the enum-member deserialization/validation path for the REST-XML query and body members in these handlers.
Traces: cloudfront.op-contract-listCachePolicies, cloudfront.update-anycast-ip-list-invalid-argument-rejected.

Cause 8 of 10: an over-size invalidation batch for a tenant is refused as TooManyInvalidationsInProgress instead of BatchTooLarge.
AWS: Smithy: CreateInvalidationForDistributionTenant models BatchTooLarge with httpError 413 AND TooManyInvalidationsInProgress with httpError 400 as two different errors. A batch whose path count exceeds the per-batch limit is BatchTooLarge/413; TooManyInvalidationsInProgress/400 is for concurrent in-flight invalidations.
Emulator: answers TooManyInvalidationsInProgress (400) for the over-size batch.
Why: a client gets a retry-later signal for a request that will never succeed as submitted, and the 413 status is lost.
Fix: the batch-size check in CreateInvalidationForDistributionTenant's validation, before the concurrency check.
Traces: cloudfront.create-invalidation-for-tenant-batch-too-large-rejected.

Cause 9 of 10: reading a distribution-tenant invalidation that does not belong to the tenant is refused NoSuchInvalidation instead of EntityNotFound.
AWS: the tenant-scoped invalidation operations use the tenant error family — EntityNotFound (404) — for an invalidation id that is not in the named tenant's scope; NoSuchInvalidation belongs to the distribution-scoped GetInvalidation.
Emulator: NoSuchInvalidation, for both the cross-tenant lookup and the tenant invalidation lifecycle read.
Why: a client handling the tenant API's documented error set never matches the code, so a cross-tenant read looks like a transport-level surprise.
Fix: the error classification in the distribution-tenant invalidation handlers.
Traces: cloudfront.e2e-distribution-tenant-invalidation-scoped-to-tenant, cloudfront.invalidation-for-tenant-lifecycle.

Cause 10 of 10: the tenant-by-domain and domain-conflicts refusals report an invalid domain where AWS reports the missing-certificate requirement.
AWS: GetDistributionTenantByDomain and ListDomainConflicts refuse with AWS's own fixed sentence; for ListDomainConflicts that sentence is "There must be at least one certificate in the distribution tenant or distribution, or there must be a managed certificate request".
Emulator: both answer "The request contains an invalid domain name."
Why: the client is pointed at the domain string instead of at the certificate requirement it actually has to satisfy, so the refusal is not actionable.
Fix: the validation order and message in the GetDistributionTenantByDomain / ListDomainConflicts handlers.
Traces: cloudfront.op-contract-get-distribution-tenant-by-domain, cloudfront.list-domain-conflicts-invalid-argument-rejected.

## Evidence
Verbatim expected-vs-actual from the replay (one per cause):
1. CreateCachePolicy duplicate name — expected 'Another cache policy with the same name already exists within the aws account.' / got 'A cache policy with the name sim2real-dupcp-0c17b00c0372 already exists in this account. You must provide a unique name. To modify an existing cache policy, use UpdateCachePolicy.' (the name is a per-run value filling the emulator's template; the template itself is the defect). CreateFunction/CreateKeyValueStore: expected message '' / got a full sentence. CreateOriginAccessControl: expected 'An origin access control with the same name already exists.' / got 'An origin access control with the specified parameters already exists: E7IYCGPKTANSF3'.
2. AssociateDistributionTenantWebACL stale If-Match — code PreconditionFailed correct; expected 'Your request is missing requirements for this operation. Update your request and try again.' / got "The request failed because it didn't meet the preconditions in one or more request-header fields.". UpdateCachePolicy/UpdateOriginRequestPolicy: expected "The request failed because it didn't meet the preconditions for this operation." / same generic string got. DeleteFunction: expected code InvalidIfMatchVersion / got PreconditionFailed.
3. GetPublicKey(Id='PKBOGUS0000000000') — code NoSuchPublicKey correct; expected 'The specified CloudFront public key does not exist.' / got "The specified public key doesn't exist.". GetKeyGroup(unknown Id) — code NoSuchResource correct; expected 'Id format is invalid' / got 'The specified key group does not exist.'.
4. DeleteOriginAccessControl in use — expected 'The CloudFront origin access control is still being used.' / got "Cannot delete the origin access control because it's in use by one or more distributions: EVBB9JKPE6KWE0". DeleteKeyValueStore in use — expected 'Cannot delete KeyValueStore <name> because it is associated with a function' / got 'The entity cannot be deleted while it is in use.'. DeleteConnectionGroup enabled — expected 'You must disable the specified connection group before you can delete it.' / got "The specified CloudFront resource hasn't been disabled yet.".
5. CreateCachePolicy MinTTL=3600 MaxTTL=60 — expected 'MinTTL, MaxTTL and DefaultTTL should follow order MinTTL less than or equal to DefaultTTL less than or equal to MaxTTL' / got 'An argument is invalid: MaxTTL must be greater than or equal to MinTTL, but the request submits MinTTL 3600 and MaxTTL 60'.
6. UpdatePublicKey — expected 'You cannot modify encoded material and name of a public key once created.' / got "You can't change the value of a public key.".
7. ListCachePolicies(Type='not-a-real-policy-type') — got MalformedXML "1 validation error detected: Value 'not-a-real-policy-type' at 'type' failed to satisfy constraint: Member must satisfy enum value set: [managed, custom]"; UpdateAnycastIpList(IpAddressType='ipv7-bogus') — expected InvalidArgument / got MalformedXML with the same generic shape.
8. CreateInvalidationForDistributionTenant over-size batch — expected 413 BatchTooLarge / got TooManyInvalidationsInProgress (400).
9. GetInvalidationForDistributionTenant across tenants — expected EntityNotFound / got NoSuchInvalidation.
10. ListDomainConflicts — expected 'There must be at least one certificate in the distribution tenant or distribution, or there must be a managed certificate request' / got 'The request contains an invalid domain name.'

## Steps to Reproduce
1. Build and boot the emulator from this checkout, then replay the cloudfront behavioral checks listed in trace_ids.
2. Cause 1: CreateCachePolicy twice with the same Name; read Error.Message.
3. Cause 2: UpdateCachePolicy / AssociateDistributionTenantWebACL with a stale If-Match; read Error.Code and Error.Message. DeleteFunction with a stale If-Match; read Error.Code.
4. Cause 3: GetPublicKey(Id="PKBOGUS0000000000") and GetKeyGroup with an unknown id; read Error.Message.
5. Cause 4: create an OAC and reference it from a distribution, then DeleteOriginAccessControl; create a KeyValueStore associated with a function, then DeleteKeyValueStore; DeleteConnectionGroup while enabled.
6. Cause 5: CreateCachePolicy with MinTTL=3600, MaxTTL=60.
7. Cause 6: UpdatePublicKey changing Name or EncodedKey.
8. Cause 7: ListCachePolicies(Type="not-a-real-policy-type"); UpdateAnycastIpList(IpAddressType="ipv7-bogus").
9. Cause 8: CreateInvalidationForDistributionTenant with a path list over the per-batch limit.
10. Cause 9: create invalidations on two tenants of one distribution and GetInvalidationForDistributionTenant for tenant A's invalidation under tenant B.
11. Cause 10: ListDomainConflicts with no certificate on the tenant/distribution.
