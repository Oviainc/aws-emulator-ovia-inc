Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront.cache-behavior-first-listed-match-wins-over-more-specific-pattern cloudfront.cf-cache-key-querystring-allowlist-separates-cache-entries cloudfront.cf-compression-serves-gzip-only-when-accept-encoding-present cloudfront.cf-continuous-deployment-single-header-routes-to-staging-distribution cloudfront.cf-response-headers-policy-adds-and-overrides-headers-at-edge cloudfront.cf-unsigned-request-to-trusted-key-group-distribution-rejected cloudfront.cf-wildcard-invalidation-clears-only-matching-objects cloudfront.cfcache-conditional-get-304-only-for-the-matching-etag cloudfront.cfcache-cookie-allowlist-separates-cache-entries cloudfront.cfcache-range-beyond-object-rejected-and-partial-served-from-cache cloudfront.deep-cloudfront-1-cf-edge-fn-throw-yields-503-rejected cloudfront.deep-cloudfront-1-cf-edge-fn-viewer-request-rewrite cloudfront.deep-cloudfront-1-cf-edge-fn-viewer-response-header cloudfront.deploy-cloudfront-distribution-serves-origin-content cloudfront.deploy-distribution-cfn-serves-live-traffic cloudfront.e2e-invalidation-clears-cached-content cloudfront.e2e-invalidation-refreshes-stale-cached-object cloudfront.e2e-webacl-association-blocks-then-unblocks-requests cloudfront.https-only-and-disallowed-method-rejected-at-edge cloudfront.origin-group-fails-over-to-secondary-origin-at-the-edge cloudfront.origin-request-policy-header-does-not-enter-the-cache-key cloudfront.server-side-origin-access-control-signs-requests cloudfront.server-side-security-headers-policy-enforced-on-response cloudfront.web-acl-association-enforced-at-edge cloudfront.associate-distribution-tenant-web-acl-not-found-rejected cloudfront.disassociate-distribution-web-acl-no-association-invalid-argument-rejected cloudfront.disassociate-web-acl-invalid-argument-rejected cloudfront.web-acl-association-invalid-webacl-rejected cloudfront.op-contract-create-distribution cloudfront.op-contract-createOriginAccessControl cloudfront.get-distribution-tenant-by-domain-malformed-domain-invalid-argument-rejected cloudfront.op-contract-deleteKeyGroup cloudfront.op-contract-deletePublicKey cloudfront.op-contract-deleteResponseHeadersPolicy cloudfront.update-key-group-already-exists-rejected cloudfront.update-key-group-duplicate-name-fails cloudfront.update-key-group-duplicate-name-rejected cloudfront.update-domain-association-not-found-rejected cloudfront.list-distributions-by-owned-resource-not-found-rejected cloudfront.deploy-cache-policy-enforced-via-cloudformation cloudfront.op-contract-get-field-level-encryption-config cloudfront.update-distribution-tenant-cname-conflict-rejected cloudfront.update-distribution-tenant-duplicate-customization-fails cloudfront.list-functions-unsupported-operation-rejected cloudfront.list-connection-functions-unsupported-operation-rejected
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront.cache-behavior-first-listed-match-wins-over-more-specific-pattern|_poll_until#28|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront.cf-cache-key-querystring-allowlist-separates-cache-entries|_poll_until#24|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront.cf-compression-serves-gzip-only-when-accept-encoding-present|_poll_until#24|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront.cf-continuous-deployment-single-header-routes-to-staging-distribution|_poll_until#29|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront.cf-response-headers-policy-adds-and-overrides-headers-at-edge|_poll_until#22|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront.cf-unsigned-request-to-trusted-key-group-distribution-rejected|_poll_until#27|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront.cf-wildcard-invalidation-clears-only-matching-objects|_poll_until#19|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront.cfcache-conditional-get-304-only-for-the-matching-etag|_poll_until#23|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront.cfcache-cookie-allowlist-separates-cache-entries|_poll_until#27|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront.cfcache-range-beyond-object-rejected-and-partial-served-from-cache|_poll_until#22|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront.deep-cloudfront-1-cf-edge-fn-throw-yields-503-rejected|_poll_until#9|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront.deep-cloudfront-1-cf-edge-fn-viewer-request-rewrite|_poll_until#10|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront.deep-cloudfront-1-cf-edge-fn-viewer-response-header|_poll_until#9|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront.deploy-cloudfront-distribution-serves-origin-content|_fetch_with_retries#10|Timeout|time.sleep(15)
Issue-Key: cloudfront.deploy-distribution-cfn-serves-live-traffic|_poll_http_get#8|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront.e2e-invalidation-clears-cached-content|_fetch_with_retries#9|Timeout|time.sleep(10)
Issue-Key: cloudfront.e2e-invalidation-refreshes-stale-cached-object|_poll_http_get#18|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront.e2e-webacl-association-blocks-then-unblocks-requests|_poll_http_get#19|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront.https-only-and-disallowed-method-rejected-at-edge|_poll_until#25|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront.origin-group-fails-over-to-secondary-origin-at-the-edge|_poll_until#28|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront.origin-request-policy-header-does-not-enter-the-cache-key|_poll_until#29|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront.server-side-origin-access-control-signs-requests|_fetch#21|Timeout|time.sleep(10)
Issue-Key: cloudfront.server-side-security-headers-policy-enforced-on-response|_fetch#5|Timeout|time.sleep(10)
Issue-Key: cloudfront.web-acl-association-enforced-at-edge|_poll_until_status#5|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront.associate-distribution-tenant-web-acl-not-found-rejected|_error_code_and_message#8|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: cloudfront.disassociate-distribution-web-acl-no-association-invalid-argument-rejected|_error#5|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: cloudfront.disassociate-web-acl-invalid-argument-rejected|_error#11|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded and returned {result!r}")
Issue-Key: cloudfront.web-acl-association-invalid-webacl-rejected|_error_code_and_message#12|AssertionError|raise AssertionError("expected the call to be rejected, but it succeeded")
Issue-Key: cloudfront.op-contract-create-distribution|run#23|AssertionError|raise AssertionError(
Issue-Key: cloudfront.op-contract-createOriginAccessControl|_error#14|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: cloudfront.get-distribution-tenant-by-domain-malformed-domain-invalid-argument-rejected|_error#17|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: cloudfront.op-contract-deleteKeyGroup|run#13|AssertionError|assert bad_code == "InvalidIfMatchVersion", f"expected InvalidIfMatchVersion, got {bad_code}"
Issue-Key: cloudfront.op-contract-deletePublicKey|run#13|AssertionError|assert code == "PublicKeyInUse", f"expected PublicKeyInUse, got {code}"
Issue-Key: cloudfront.op-contract-deleteResponseHeadersPolicy|run#12|AssertionError|assert code == "IllegalDelete", f"expected IllegalDelete deleting a managed policy, got {code}"
Issue-Key: cloudfront.update-key-group-already-exists-rejected|run#8|AssertionError|assert code == "KeyGroupAlreadyExists", f"expected KeyGroupAlreadyExists, got {code!r}"
Issue-Key: cloudfront.update-key-group-duplicate-name-fails|run#15|AssertionError|assert code == "KeyGroupAlreadyExists", f"expected KeyGroupAlreadyExists, got {code}"
Issue-Key: cloudfront.update-key-group-duplicate-name-rejected|run#10|AssertionError|assert code == "KeyGroupAlreadyExists", f"expected KeyGroupAlreadyExists, got {code!r}"
Issue-Key: cloudfront.update-domain-association-not-found-rejected|run#7|AssertionError|assert code == "EntityNotFound", f"expected EntityNotFound, got {code!r}"
Issue-Key: cloudfront.list-distributions-by-owned-resource-not-found-rejected|run#9|AssertionError|assert code == "InvalidArgument", f"expected InvalidArgument, got {code!r}"
Issue-Key: cloudfront|GetDistributionConfig|NoSuchDistribution
Issue-Key: cloudfront.op-contract-get-field-level-encryption-config|run#27|AssertionError|assert fwd_qa is True, f"expected ForwardWhenQueryArgProfileIsUnknown True, got {fwd_qa!r}"
Issue-Key: cloudfront|UpdateDistributionTenant|CNAMEAlreadyExists
Issue-Key: cloudfront|UpdateDistributionTenant|InvalidArgument
Issue-Key: cloudfront.list-functions-unsupported-operation-rejected|run#8|AssertionError|assert "Items" in function_list, f"ListFunctions response missing 'Items': {function_list!r}"
Issue-Key: cloudfront.list-connection-functions-unsupported-operation-rejected|run#3|AssertionError|assert code == "ServiceUnavailable", f"expected ServiceUnavailable, got {code}"
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# cloudfront: 10 more divergences — the edge data plane serves nothing (24 checks), web-ACL association and origin references are unvalidated, six control-plane ops answer 501, and CFN-declared CloudFront resources are invisible to CloudFront

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDistribution

## Description
This issue bundles 10 net-new root causes in the cloudfront crate: behaviour (not just wording) that diverges. Open issues 20261027T001538Z-cloudfront-the-edge-data-plane-serves-nothing-so.md, 20261024T182555Z-cloudfront-ten-fidelity-divergences-distribution-policy-tenant-validation.md, 20261023T140640Z-cloudfront-s3-a-cfn-deployed-distribution-does-not.md and the UpdateKeyGroup/DeleteKeyGroup/DeletePublicKey/DeleteResponseHeadersPolicy scaffold-stub issues describe adjacent surface, but none of them carries the replay keys below.

Cause 1 of 10: the distribution's edge domain answers nothing, so no edge behaviour can be observed.
AWS: once GetDistribution reports Status=Deployed, an HTTPS request to the returned DomainName is served by the edge: 200 with the origin's body and the cache/response-headers/compression/range semantics applied, 304 for a matching If-None-Match, 403 for an unsigned request to a TrustedKeyGroups behavior or a blocking web ACL, 416/206 for ranges, 503 when a CloudFront function throws. Smithy models DomainName on Distribution but not the edge protocol, so: recorded behavior only — the recordings show an HTTP response within seconds of Deployed.
Emulator: no response ever arrives on the edge domain; all 24 checks below run to `Check timed out after 60.0s`. Cache-key composition, compression, continuous-deployment header routing, response-headers policies, wildcard invalidation, conditional GET, cookie/query-string allowlists, range handling, CloudFront-function execution, origin-group failover, OAC request signing, web-ACL enforcement and viewer-protocol/method enforcement are therefore all unverifiable.
Why: every data-plane guarantee a CloudFront client depends on is absent; a client cannot distinguish a denied request from a dead edge.
Fix: the CloudFront edge data-plane handler and the host-based router entry that maps d<id>.cloudfront.net to a stored distribution.
Traces: cloudfront.cache-behavior-first-listed-match-wins-over-more-specific-pattern, .cf-cache-key-querystring-allowlist-separates-cache-entries, .cf-compression-serves-gzip-only-when-accept-encoding-present, .cf-continuous-deployment-single-header-routes-to-staging-distribution, .cf-response-headers-policy-adds-and-overrides-headers-at-edge, .cf-unsigned-request-to-trusted-key-group-distribution-rejected, .cf-wildcard-invalidation-clears-only-matching-objects, .cfcache-conditional-get-304-only-for-the-matching-etag, .cfcache-cookie-allowlist-separates-cache-entries, .cfcache-range-beyond-object-rejected-and-partial-served-from-cache, .deep-cloudfront-1-cf-edge-fn-throw-yields-503-rejected, .deep-cloudfront-1-cf-edge-fn-viewer-request-rewrite, .deep-cloudfront-1-cf-edge-fn-viewer-response-header, .deploy-cloudfront-distribution-serves-origin-content, .deploy-distribution-cfn-serves-live-traffic, .e2e-invalidation-clears-cached-content, .e2e-invalidation-refreshes-stale-cached-object, .e2e-webacl-association-blocks-then-unblocks-requests, .https-only-and-disallowed-method-rejected-at-edge, .origin-group-fails-over-to-secondary-origin-at-the-edge, .origin-request-policy-header-does-not-enter-the-cache-key, .server-side-origin-access-control-signs-requests, .server-side-security-headers-policy-enforced-on-response, .web-acl-association-enforced-at-edge.

Cause 2 of 10: the web-ACL association/disassociation operations validate nothing — they accept and persist whatever is sent.
AWS: Smithy: AssociateDistributionWebACL / AssociateDistributionTenantWebACL model EntityNotFound (404) and InvalidArgument (400); DisassociateDistributionWebACL / DisassociateDistributionTenantWebACL model InvalidArgument (400) and EntityNotFound (404). A WebACLArn naming a web ACL that does not exist is refused; a syntactically invalid WebACLArn is refused InvalidArgument; a disassociate against a distribution with no association is refused InvalidArgument.
Emulator: all four succeed with HTTP 200 and a fresh ETag. The nonexistent-web-ACL associate returns the ARN it was given; the invalid-ARN associate echoes WebACLArn='not-a-valid-web-acl-arn'; the no-association disassociate returns 200.
Why: a client's association is silently accepted against a web ACL that cannot protect anything, and there is no way to detect the mistake — the read-back confirms it.
Fix: the validation in the four web-ACL association/disassociation handlers (resolve the ARN, require an existing association on disassociate).
Traces: cloudfront.associate-distribution-tenant-web-acl-not-found-rejected, .disassociate-distribution-web-acl-no-association-invalid-argument-rejected, .disassociate-web-acl-invalid-argument-rejected, .web-acl-association-invalid-webacl-rejected.

Cause 3 of 10: CreateDistribution accepts a cache behavior whose TargetOriginId matches no declared Origin.
AWS: Smithy: CreateDistribution models NoSuchOrigin with httpError 404 for exactly this case — a DefaultCacheBehavior/CacheBehavior TargetOriginId that is not the Id of any member of Origins.
Emulator: 201 Created; the distribution is stored with a dangling TargetOriginId.
Why: a client's distribution is created in a state the service can never route, and the mistake surfaces only as unexplained edge failures instead of at create time.
Fix: CreateDistribution's (and UpdateDistribution's) DistributionConfig validation — cross-check every TargetOriginId against Origins/OriginGroups before storing.
Traces: cloudfront.op-contract-create-distribution.

Cause 4 of 10: CreateOriginAccessControl accepts a second OAC with a name already in use.
AWS: a duplicate OAC Name is refused with the modelled already-exists conflict; the create does not return a second Id.
Emulator: 201 Created with a new Id and the duplicate Name stored.
Why: two OACs with one name make the signing configuration ambiguous and un-addressable by name.
Fix: the uniqueness check in CreateOriginAccessControl's validation.
Traces: cloudfront.op-contract-createOriginAccessControl.

Cause 5 of 10: GetDistributionTenantByDomain accepts a malformed domain and answers with an unrelated tenant.
AWS: a Domain that is not a syntactically valid domain name is refused InvalidArgument (400); it never resolves to a tenant.
Emulator: 200 with a full DistributionTenant payload for a tenant whose Domains do not contain the requested value.
Why: a lookup by a typo'd domain silently returns some other tenant, so a caller acts on the wrong resource.
Fix: GetDistributionTenantByDomain's domain-member validation and the lookup's exact-match condition.
Traces: cloudfront.get-distribution-tenant-by-domain-malformed-domain-invalid-argument-rejected.

Cause 6 of 10: six control-plane operations answer 501 NotImplemented, so their modelled refusals and effects never happen.
AWS: Smithy models these fully — UpdateKeyGroup -> KeyGroupAlreadyExists (409), InvalidIfMatchVersion (400), NoSuchResource (404), PreconditionFailed (412); DeleteKeyGroup -> InvalidIfMatchVersion (400), ResourceInUse (409), PreconditionFailed (412); DeletePublicKey -> PublicKeyInUse (409), NoSuchPublicKey (404); DeleteResponseHeadersPolicy -> IllegalDelete for a managed policy; UpdateDomainAssociation -> EntityNotFound (404); ListDistributionsByOwnedResource -> InvalidArgument (400).
Emulator: every call answers 501 with "operation <Name> is scaffolded but not implemented in this emulator", so the checks see NotImplemented where they expect KeyGroupAlreadyExists, InvalidIfMatchVersion, PublicKeyInUse, IllegalDelete, EntityNotFound or InvalidArgument.
Why: key-group rotation, public-key retirement, managed-policy protection, domain re-association and owned-resource listing are all unusable, and the error code a client branches on never arrives.
Fix: implement the handlers services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/update_key_group.rs, delete_key_group.rs, delete_public_key.rs, delete_response_headers_policy.rs, plus the UpdateDomainAssociation and ListDistributionsByOwnedResource handlers in the same directory. (Scaffold-stub issues exist for four of those operations from other checks; these six replay keys are not on them.)
Traces: cloudfront.op-contract-deleteKeyGroup, .op-contract-deletePublicKey, .op-contract-deleteResponseHeadersPolicy, .update-key-group-already-exists-rejected, .update-key-group-duplicate-name-fails, .update-key-group-duplicate-name-rejected, .update-domain-association-not-found-rejected, .list-distributions-by-owned-resource-not-found-rejected.

Cause 7 of 10: a CloudFormation stack that declares CloudFront resources reaches CREATE_COMPLETE but the resources do not exist in CloudFront.
AWS: a stack whose template declares AWS::CloudFront::CachePolicy and AWS::CloudFront::Distribution creates them in CloudFront; GetDistributionConfig on the stack's physical id returns the deployed config, and the distribution references the created cache policy.
Emulator: the stack settles, and GetDistributionConfig on the stack's distribution id is refused 404 NoSuchDistribution "The specified distribution does not exist." The CFN provisioning path never registers the resource with the cloudfront service's state.
Why: infrastructure-as-code users get a green stack over nothing; nothing they declared is addressable through the CloudFront API.
Fix: the CloudFormation resource providers for the AWS::CloudFront::* types — they must write through to the cloudfront crate's state rather than only recording a stack resource.
Traces: cloudfront.deploy-cache-policy-enforced-via-cloudformation.

Cause 8 of 10: GetFieldLevelEncryptionConfig does not report the configured ForwardWhenQueryArgProfileIsUnknown.
AWS: QueryArgProfileConfig.ForwardWhenQueryArgProfileIsUnknown is a boolean set on create/update and echoed verbatim by the read; a config created with it true reads back true.
Emulator: the read reports false for a config created with it true — the flag is dropped between the write and the read.
Why: a caller round-tripping the config (the GetConfig -> edit -> Update pattern CloudFront requires) silently flips the fail-open/fail-closed behaviour of field-level encryption.
Fix: the field-level-encryption config serializer/state in the cloudfront crate — persist and emit QueryArgProfileConfig.ForwardWhenQueryArgProfileIsUnknown.
Traces: cloudfront.op-contract-get-field-level-encryption-config.

Cause 9 of 10: UpdateDistributionTenant refuses an update that restates the tenant's own domains.
AWS: an update that carries the domains the tenant already holds is not a CNAME conflict — CNAMEAlreadyExists (409) is for a CNAME held by a DIFFERENT resource — and restating one domain once is not "named more than once". Both calls proceed.
Emulator: one update is refused 409 CNAMEAlreadyExists "One or more of the CNAMEs you provided are already associated with a different resource." against the tenant's own domain, and another is refused 400 InvalidArgument "An argument is invalid: the domain <domain> is named more than once" — the exception escapes the check during setup, before the behaviour under test is reached.
Why: a tenant's domains and customizations become un-updatable: the only way to change a customization is to restate the domains, which the emulator rejects.
Fix: UpdateDistributionTenant's domain-conflict and duplicate-domain checks must exclude the tenant being updated and de-duplicate before counting.
Traces: cloudfront.update-distribution-tenant-cname-conflict-rejected, cloudfront.update-distribution-tenant-duplicate-customization-fails.

Cause 10 of 10: ListFunctions omits Items from its FunctionList, and ListConnectionFunctions refuses with the wrong code.
AWS: Smithy: ListFunctions -> ListFunctionsResult.FunctionList, and models InvalidArgument (400) and UnsupportedOperation (400). The FunctionList carries Items alongside MaxItems/Quantity. ListConnectionFunctions answers the unsupported-operation condition with ServiceUnavailable.
Emulator: ListFunctions returns `{'MaxItems': 5, 'Quantity': 0}` with no Items member at all; ListConnectionFunctions answers InvalidArgument where ServiceUnavailable is expected.
Why: a client that reads FunctionList['Items'] raises KeyError instead of seeing an empty list, and the unsupported-operation condition is misreported as a caller error, so a retry/backoff path never engages.
Fix: the ListFunctions response serializer (always emit Items) and the ListConnectionFunctions refusal classification.
Traces: cloudfront.list-functions-unsupported-operation-rejected, cloudfront.list-connection-functions-unsupported-operation-rejected.

## Evidence
Cause 1: `Error: Check timed out after 60.0s` for all 24 listed checks; the preceding CreateDistribution/CreateCachePolicy/CreateFunction calls all returned 200/201 in the same runs, so only the edge fetch is missing.
Cause 2: AssociateDistributionTenantWebACL with a nonexistent web ACL — "expected a rejection, but the call SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200,...}, 'ETag': ..., 'Id': 'dt_...', 'WebACLArn': 'arn:aws:wafv2:us-east-1:...:global/webacl/sim2real-missing-.../00000000-0000-0000-0000-000000000000'}". DisassociateDistributionWebACL with WebACLArn='not-a-valid-web-acl-arn' — 200 and the invalid ARN echoed. Disassociate with no association — 200.
Expected: 404 EntityNotFound / 400 InvalidArgument respectively.
Cause 3: "expected create_distribution to reject a TargetOriginId with no matching Origin with NoSuchOrigin, but it SUCCEEDED and created 'EGZNI4PND7H2S3'" — expected 404 NoSuchOrigin.
Cause 4: "expected a ClientError, but the call SUCCEEDED and returned ... 'OriginAccessControl': {'Id': 'EJZ50Z92T9DP30', 'OriginAccessControlConfig': {'Name': '<duplicate name>', ...}}".
Cause 5: "expected the call to be rejected, but it SUCCEEDED and returned ... 'DistributionTenant': {... 'Domains': [{'Domain': 'sim2real-realdom....sim2real-cfbehav-test.com', 'Status': 'active'}] ...}" for a malformed requested domain — expected 400 InvalidArgument.
Cause 6: "expected InvalidIfMatchVersion, got NotImplemented" (DeleteKeyGroup), "expected PublicKeyInUse, got NotImplemented" (DeletePublicKey), "expected IllegalDelete deleting a managed policy, got NotImplemented" (DeleteResponseHeadersPolicy), "expected KeyGroupAlreadyExists, got 'NotImplemented'" (UpdateKeyGroup x3), "expected EntityNotFound, got 'NotImplemented'" (UpdateDomainAssociation), "expected InvalidArgument, got 'NotImplemented'" (ListDistributionsByOwnedResource). Message in each: "operation <Name> is scaffolded but not implemented in this emulator", HTTP 501.
Cause 7: botocore.errorfactory.NoSuchDistribution: An error occurred (NoSuchDistribution) when calling the GetDistributionConfig operation: The specified distribution does not exist. — raised on the physical id of a stack that had reached a settled state.
Cause 8: "expected ForwardWhenQueryArgProfileIsUnknown True, got False".
Cause 9: botocore.errorfactory.CNAMEAlreadyExists ... UpdateDistributionTenant: One or more of the CNAMEs you provided are already associated with a different resource. / botocore.errorfactory.InvalidArgument ... UpdateDistributionTenant: An argument is invalid: the domain <domain> is named more than once. Both were raised on calls the check expected to succeed.
Cause 10: "ListFunctions response missing 'Items': {'MaxItems': 5, 'Quantity': 0}"; "expected ServiceUnavailable, got InvalidArgument".

## Steps to Reproduce
1. Build and boot the emulator from this checkout, then replay the cloudfront behavioral checks listed in trace_ids.
2. Cause 1: CreateDistribution over any origin, poll until Status=Deployed, then HTTP GET https://<DomainName>/<path>. Observe that no response arrives.
3. Cause 2: AssociateDistributionTenantWebACL with a WebACLArn for a web ACL that was never created; DisassociateDistributionWebACL with WebACLArn="not-a-valid-web-acl-arn"; DisassociateDistributionWebACL on a distribution with no association. All three return 200.
4. Cause 3: CreateDistribution whose DefaultCacheBehavior.TargetOriginId names no Origins member. Observe 201.
5. Cause 4: CreateOriginAccessControl twice with the same Name. Observe 201 twice.
6. Cause 5: GetDistributionTenantByDomain(Domain="not a domain"). Observe 200 with some tenant.
7. Cause 6: call UpdateKeyGroup, DeleteKeyGroup, DeletePublicKey, DeleteResponseHeadersPolicy, UpdateDomainAssociation, ListDistributionsByOwnedResource. Observe 501 NotImplemented.
8. Cause 7: CreateStack with a template declaring AWS::CloudFront::CachePolicy and AWS::CloudFront::Distribution, wait for the stack to settle, then GetDistributionConfig on the stack's distribution physical id. Observe 404 NoSuchDistribution.
9. Cause 8: CreateFieldLevelEncryptionConfig with QueryArgProfileConfig.ForwardWhenQueryArgProfileIsUnknown=true, then GetFieldLevelEncryptionConfig. Observe false.
10. Cause 9: CreateDistributionTenant with one domain, then UpdateDistributionTenant restating that same domain while changing a customization. Observe 409 CNAMEAlreadyExists / 400 InvalidArgument.
11. Cause 10: ListFunctions on an account with no functions (observe no Items key); trigger ListConnectionFunctions' unsupported-operation condition (observe InvalidArgument).
