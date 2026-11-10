Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront.deep-cloudfront-1-cf-edge-fn-lambda-edge-unqualified-arn-rejected cloudfront.distribution-cache-behavior-order-preserved cloudfront.function-association-requires-published-stage-rejected cloudfront.multi-tenant-distribution-with-direct-only-fields-rejected cloudfront.response-headers-policy-csp-too-long-fails cloudfront.distribution-tenant-round-trips-unvalidated-domain-and-default-connection-group cloudfront.get-managed-certificate-details-round-trip cloudfront.tenant-only-distribution-round-trips-tenant-config-and-omits-price-class cloudfront.cfcache-min-ttl-floors-origin-no-cache-directive cloudfront.e2e-distribution-invalidation-cache-refresh cloudfront.e2e-distribution-webacl-association-blocks-request cloudfront.e2e-invalidation-clears-cached-object cloudfront.edge-requests-deliver-requests-metric-with-global-dimension cloudfront.deploy-cloudfront-1-cache-policy-invalid-ttl-order-update-rejected-rolls-back cloudfront.deploy-cloudfront-1-cache-policy-out-of-band-ttl-drift-detected-then-updated-in-place cloudfront.associate-distribution-web-acl-nonexistent-distribution-fails cloudfront.function-live-stage-before-publish-rejected cloudfront.op-contract-create-cloudfront-origin-access-identity cloudfront.op-contract-create-continuous-deployment-policy cloudfront.create-invalidation-rejects-a-reused-caller-reference-with-different-paths
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront.deep-cloudfront-1-cf-edge-fn-lambda-edge-unqualified-arn-rejected|_error#18|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Key: cloudfront.distribution-cache-behavior-order-preserved|_error#27|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded and returned {result!r}")
Issue-Key: cloudfront.function-association-requires-published-stage-rejected|_error#13|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded with {result!r}")
Issue-Key: cloudfront.multi-tenant-distribution-with-direct-only-fields-rejected|_error#11|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: cloudfront.response-headers-policy-csp-too-long-fails|_error#4|AssertionError|raise AssertionError(
Issue-Key: cloudfront.distribution-tenant-round-trips-unvalidated-domain-and-default-connection-group|run#11|AssertionError|assert [(d.get("Domain"), d.get("Status")) for d in domains] == [(domain, "inactive")], (
Issue-Key: cloudfront.get-managed-certificate-details-round-trip|_expect_error#8|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Key: cloudfront.tenant-only-distribution-round-trips-tenant-config-and-omits-price-class|run#17|AssertionError|assert tls == "TLSv1.2_2021", f"MinimumProtocolVersion reads {tls!r}, declared 'TLSv1.2_2021'"
Issue-Key: cloudfront.cfcache-min-ttl-floors-origin-no-cache-directive|_poll_until#30|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront.e2e-distribution-invalidation-cache-refresh|_poll_fetch#22|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront.e2e-distribution-webacl-association-blocks-request|_poll_http_get#21|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront.e2e-invalidation-clears-cached-object|_poll_http_get#15|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront.edge-requests-deliver-requests-metric-with-global-dimension|_poll_until#27|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront|GetCachePolicyConfig|NoSuchCachePolicy
Issue-Key: cloudfront.associate-distribution-web-acl-nonexistent-distribution-fails|run#8|AssertionError|assert msg == not_found_msg, f"expected message {not_found_msg!r}, got {msg!r}"
Issue-Key: cloudfront.function-live-stage-before-publish-rejected|run#8|AssertionError|assert message == missing, f"describing LIVE before publish said {message!r}, expected {missing!r}"
Issue-Key: cloudfront.op-contract-create-cloudfront-origin-access-identity|run#17|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: cloudfront.op-contract-create-continuous-deployment-policy|run#20|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: cloudfront.create-invalidation-rejects-a-reused-caller-reference-with-different-paths|run#8|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Commit: 0de7718ade8f368be0e47aca52d0dd18a585e082

# cloudfront: ten fidelity divergences — distribution/policy/tenant validation gaps, no edge data plane, CFN-created cache policy missing, and ids appended to rejection messages

> _Note: this issue was identified against the code base at commit `0de7718ade8f368be0e47aca52d0dd18a585e082`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDistribution

## Description
This issue bundles TEN net-new root causes in the cloudfront service. A fixer must close all ten before submitting. Related open issues that do NOT cover these causes but belong to the same families: 20261022T025800Z-cloudfront-createdistribution-accepts-configurations-aws-rejects.md (other configurations), 20261023T140640Z-cloudfront-lambda-createdistribution-accepts-a-lambda-edge-association-pointing-at.md (the CREATE path of cause 1), 20261023T140640Z-cloudfront-s3-a-cfn-deployed-distribution-does-not.md (the DISTRIBUTION analogue of cause 9). None of the checks below is marked ALREADY FILED or OWNED in the replay report.

Cause 1 of 10: UpdateDistribution accepts a Lambda@Edge association with an unqualified function ARN.
AWS: Smithy: UpdateDistribution (PUT /2020-05-31/distribution/{Id}/config) models InvalidLambdaFunctionAssociation (400); a LambdaFunctionARN must name a published version, never an unqualified ARN or $LATEST. Recorded behavior: 400 refusal, config unchanged.
Emulator: 200 with the association stored (replay shows Status InProgress and the config echoed back).
Why: a client ships a distribution whose edge function association production refuses; the emulator certified it.
Fix: UpdateDistribution's validation of DefaultCacheBehavior/CacheBehaviors LambdaFunctionAssociations (src/ops/update_distribution.rs).
Traces: cloudfront.deep-cloudfront-1-cf-edge-fn-lambda-edge-unqualified-arn-rejected.

Cause 2 of 10: UpdateDistribution accepts a cache behavior that omits members CreateDistribution filled in.
AWS: Smithy: UpdateDistribution models InvalidArgument (400). Recorded behavior: the SAME CacheBehavior struct CreateDistribution accepts is REJECTED by UpdateDistribution with 400 InvalidArgument, because update requires the optional flags create defaulted to be present in the submitted config.
Emulator: 200, the partial behavior list is accepted and stored.
Why: update-path trajectories that are over-permissive offline fail with 400 in production; the asymmetry AWS enforces is erased.
Fix: UpdateDistribution's config validation (src/ops/update_distribution.rs).
Traces: cloudfront.distribution-cache-behavior-order-preserved.

Cause 3 of 10: CreateDistribution accepts a FunctionAssociation naming a function that has no LIVE stage.
AWS: Smithy: CreateDistribution (POST /2020-05-31/distribution, 201) models InvalidFunctionAssociation (400) and InvalidArgument (400). Recorded behavior: associating a CloudFront Function that has never been published is refused 400 and no distribution is created.
Emulator: 201 Created with the association stored.
Why: a distribution is created referencing code that does not exist at the edge; the publish step the real API forces is skippable.
Fix: CreateDistribution validation against the function store's LIVE stage (src/ops/create_distribution.rs).
Traces: cloudfront.function-association-requires-published-stage-rejected.

Cause 4 of 10: CreateDistribution accepts direct-only fields on a tenant-only (multi-tenant) distribution.
AWS: Smithy: CreateDistribution models InvalidArgument (400); ConnectionMode=tenant-only forbids legacy ForwardedValues and IsIPV6Enabled. Recorded behavior: 400 InvalidArgument, message "You can't specify the ForwardedValues field for a multi-tenant distribution." and "You can't specify the IsIPV6Enabled field for a multi-tenant distribution." respectively; no distribution is created either time, while the unbroken tenant-only config is accepted.
Emulator: both creates succeed, so there is no error code at all.
Why: the connection-mode rule is invisible; a multi-tenant estate is built on a config AWS will not take.
Fix: CreateDistribution's connection-mode validation (src/ops/create_distribution.rs).
Traces: cloudfront.multi-tenant-distribution-with-direct-only-fields-rejected.

Cause 5 of 10: CreateResponseHeadersPolicy accepts an over-length Content-Security-Policy.
AWS: Smithy: CreateResponseHeadersPolicy (POST /2020-05-31/response-headers-policy, 201) models TooLongCSPInResponseHeadersPolicy (400): "The length of the Content-Security-Policy header value in the response headers policy exceeds the maximum."
Emulator: 201 Created with the oversized CSP stored verbatim.
Why: a policy that production rejects is accepted, and the quota that governs it is unobservable.
Fix: CreateResponseHeadersPolicy validation of SecurityHeadersConfig.ContentSecurityPolicy length (src/ops/create_response_headers_policy.rs).
Traces: cloudfront.response-headers-policy-csp-too-long-fails.

Cause 6 of 10: distribution-tenant domain validation is not modeled — domains read "active" at once and a pending managed certificate never blocks the create.
AWS: Smithy: CreateDistributionTenant (POST /2020-05-31/distribution-tenant, 201) models InvalidArgument (400) and InvalidAssociation (409), and DistributionTenant.Domains carries a per-domain Status. Recorded behavior: a domain whose ManagedCertificateRequest is still pending validation reads Status "inactive", and a tenant create that names such a domain in the position the check exercises is refused.
Emulator: the create returns 201 and the domain reads {'Domain': ..., 'Status': 'active'} immediately.
Why: a caller believes a tenant domain is serving when its certificate was never validated; nothing ever transitions, so validation-gated workflows cannot be written against the emulator.
Fix: the distribution-tenant/managed-certificate slice of state plus CreateDistributionTenant validation (src/state.rs, src/ops/create_distribution_tenant.rs).
Traces: cloudfront.distribution-tenant-round-trips-unvalidated-domain-and-default-connection-group, cloudfront.get-managed-certificate-details-round-trip.

Cause 7 of 10: ViewerCertificate.MinimumProtocolVersion is not round-tripped.
AWS: Smithy: DistributionConfig.ViewerCertificate.MinimumProtocolVersion is an enum the service stores and echoes; CreateDistribution models InvalidMinimumProtocolVersion (400) for an unacceptable one. Recorded behavior: a config declaring TLSv1.2_2021 reads back TLSv1.2_2021.
Emulator: the stored config reads MinimumProtocolVersion 'TLSv1' after TLSv1.2_2021 was declared — the submitted value is dropped and a default substituted.
Why: a caller auditing the TLS floor of its distributions sees a weaker policy than it configured, and a conformance check on the field fails against the emulator.
Fix: the ViewerCertificate branch of the distribution config store/serializer (src/state.rs, src/ops/create_distribution.rs).
Traces: cloudfront.tenant-only-distribution-round-trips-tenant-config-and-omits-price-class.

Cause 8 of 10: distributions serve NOTHING at their edge domain — every data-plane check hangs to the harness cap.
AWS: recorded behavior only (no model surface for the edge): once GetDistribution reports Status Deployed, a GET of https://<DomainName>/<key> answers from the origin and then from cache, carrying X-Cache: Miss/Hit from cloudfront; MinTTL floors an origin no-cache directive; an invalidation makes the next fetch re-read the origin; an associated WAF web ACL answers 403 for a matching path; and AWS/CloudFront Requests datapoints appear under {DistributionId, Region=Global} for those requests.
Emulator: the distribution's DomainName answers nothing at all. Each check polls its own long budget (240-1700 s) for a first 200 and is killed by the suite's 60 s cap; the harness marks this DATAPLANE_UNIMPLEMENTED rather than an environment fault, because the hostname is the target's own. Note PROPAGATION_DELAY in src/state.rs is 500 ms, so Status does reach Deployed — the hang is the data plane, not the status transition.
Why: no caching, invalidation, WAF-at-the-edge or edge-metric behavior can be exercised; every e2e CDN workflow is unreachable.
Fix: a data-plane listener for distribution domain names that resolves the behavior/origin for the request path and applies the cache policy (new data-plane component alongside the cloudfront service crate).
Traces: cloudfront.cfcache-min-ttl-floors-origin-no-cache-directive, cloudfront.e2e-distribution-invalidation-cache-refresh, cloudfront.e2e-distribution-webacl-association-blocks-request, cloudfront.e2e-invalidation-clears-cached-object, cloudfront.edge-requests-deliver-requests-metric-with-global-dimension.

Cause 9 of 10: a CloudFormation-created AWS::CloudFront::CachePolicy does not exist in CloudFront.
AWS: Smithy: GetCachePolicyConfig answers 200 with the stored CachePolicyConfig and models NoSuchCachePolicy (404) only for an unknown Id. Recorded behavior: after a stack containing an AWS::CloudFront::CachePolicy reaches CREATE_COMPLETE, GetCachePolicyConfig on the resource's physical id returns the deployed TTLs; a stack UPDATE that pushes DefaultTTL above MaxTTL leaves the resource UPDATE_FAILED with HandlerErrorCode: InvalidRequest, rolls the stack back, and the policy keeps its id and original TTLs.
Emulator: GetCachePolicyConfig(Id=<stack physical id>) answers 404 NoSuchCachePolicy "The cache policy does not exist." immediately after CREATE_COMPLETE, so the CFN-declared policy was never created in CloudFront (or the physical id CFN reported names nothing).
Why: every IaC trajectory that deploys cache policies and then reads them back breaks, and the TTL-order refusal/rollback contract is unreachable.
Fix: the CloudFormation resource handler for AWS::CloudFront::CachePolicy and the physical-id it reports (CFN resource bridge into the cloudfront service).
Traces: cloudfront.deploy-cloudfront-1-cache-policy-invalid-ttl-order-update-rejected-rolls-back, cloudfront.deploy-cloudfront-1-cache-policy-out-of-band-ttl-drift-detected-then-updated-in-place.

Cause 10 of 10: rejection messages append request/resource identifiers instead of the fixed per-code sentence.
AWS: the message for these codes is a CONSTANT per error code, with nothing from the request in it. Smithy documents two of them exactly as recorded: NoSuchDistribution (404) "The specified distribution does not exist."; and the staging-distribution conflict is 409 StagingDistributionInUse, whose recorded wire text is "One or more staging distributions are associated with another continuous deployment policy in your account." For NoSuchFunctionExists (404) the recorded text is "The specified function does not exist." and for CloudFrontOriginAccessIdentityAlreadyExists (409) "The caller reference you are using to create the CloudFront origin access identity is associated with another identity."; the reused-caller-reference invalidation refusal records "Your request contains a caller reference that was used for a previous invalidation batch for the same distribution." (recorded behavior only for these three).
Emulator: the HTTP status and the error code match, but each message is a different sentence with an identifier appended — e.g. "The specified distribution does not exist: ENOTAREALDIST0", "The function does not exist: no function named <name> in the LIVE stage", "...is associated with another identity. Already exists: E7IYCGPKTANSF3", "A continuous deployment policy for this staging distribution already exists: 2c829abe-...", "The CallerReference <ref> was already used for invalidation <id> of this distribution with different paths".
Why: clients and golden-file tests that compare the service's documented per-code sentence see a mismatch, and the appended internal id leaks emulator-generated values into an error contract that carries none.
Fix: the per-code message constants used by these handlers (src/state.rs message constructors and the handlers that build these errors) — serve the per-code sentence with no request-derived suffix.
Traces: cloudfront.associate-distribution-web-acl-nonexistent-distribution-fails, cloudfront.function-live-stage-before-publish-rejected, cloudfront.op-contract-create-cloudfront-origin-access-identity, cloudfront.op-contract-create-continuous-deployment-policy, cloudfront.create-invalidation-rejects-a-reused-caller-reference-with-different-paths.

## Evidence
Replay output, verbatim tails (one per cause, in order):
1. AssertionError: expected the call to be rejected, but it succeeded: {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'Distribution': {'Id': 'EJZ50Z92T9DP30', 'Status': 'InProgress', ...}} (cf.update_distribution with an unqualified Lambda@Edge ARN).
2. AssertionError: expected the call to be rejected, but it succeeded and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}} — assertion was `partial_code == "InvalidArgument"` for cf.update_distribution with a CacheBehaviors list copied from the stored config.
3. AssertionError: expected the call to be refused, but it succeeded with {'ResponseMetadata': {'HTTPStatusCode': 201, ...}, 'Location': '.../distribution/E0LC535G268HO1', ...}.
4. AssertionError: the call succeeded; real AWS rejects it, so there is no error code — expected code 'InvalidArgument', message "You can't specify the ForwardedValues field for a multi-tenant distribution."
5. AssertionError: expected TooLongCSPInResponseHeadersPolicy rejection, but the call succeeded and returned {'ResponseMetadata': {'HTTPStatusCode': 201, ...}, 'ResponseHeadersPolicy': {... 'ContentSecurityPolicy': "default-src 'self'; xxxxx..." }}.
6. AssertionError: an unvalidated domain should read inactive, got [{'Domain': 'demo-tenant....example.com', 'Status': 'active'}]; and AssertionError: expected the call to be rejected, but it succeeded: {'ResponseMetadata': {'HTTPStatusCode': 201, ...}, 'DistributionTenant': {'Domains': [{'Domain': 'sim2real-cf-cert-....sim2real-cftest.net', 'Status': 'active'}], 'Status': 'InProgress'}}.
7. AssertionError: MinimumProtocolVersion reads 'TLSv1', declared 'TLSv1.2_2021'.
8. Error: Check timed out after 60.0s for all five data-plane checks; the checks' own exhaustion path raises "DATAPLANE_UNIMPLEMENTED: never got a 200 from <https://d....cloudfront.net/...>". Distribution status itself is fine (src/state.rs PROPAGATION_DELAY = 500 ms).
9. botocore.errorfactory.NoSuchCachePolicy: An error occurred (NoSuchCachePolicy) when calling the GetCachePolicyConfig operation: The cache policy does not exist. — raised at `before = cf.get_cache_policy_config(Id=_physical_id(cfn, stack_name, "Policy"))` right after the stack reached CREATE_COMPLETE.
10. AssertionError: expected message 'The specified distribution does not exist.', got 'The specified distribution does not exist: ENOTAREALDIST0'; AssertionError: describing LIVE before publish said 'The function does not exist: no function named sim2real-cf-fn-... in the LIVE stage', expected 'The specified function does not exist.'; AssertionError: expected message 'The caller reference you are using to create the CloudFront origin access identity is associated with another identity.', got '...Already exists: E7IYCGPKTANSF3'; AssertionError: expected message 'One or more staging distributions are associated with another continuous deployment policy in your account.', got 'A continuous deployment policy for this staging distribution already exists: 2c829abe-...'; AssertionError: expected message 'Your request contains a caller reference that was used for a previous invalidation batch for the same distribution.', got 'The CallerReference ... was already used for invalidation IGZNI4PND7H2S3 of this distribution with different paths'. (Status and error code matched in all five; only the message diverges.)

## Steps to Reproduce
Validation causes (1-5): create a distribution/policy with the one broken field named in each cause and observe 200/201 instead of the modeled 400.
Cause 6: CreateDistributionTenant with a ManagedCertificateRequest for a domain that is not validated -> 201 and Domains[0].Status == 'active'.
Cause 7: CreateDistribution with ViewerCertificate.MinimumProtocolVersion='TLSv1.2_2021', then GetDistributionConfig -> 'TLSv1'.
Cause 8: create a distribution over an S3 origin, wait for Status Deployed, then GET https://<DomainName>/<key> -> nothing ever answers 200.
Cause 9: CreateStack with an AWS::CloudFront::CachePolicy resource, wait for CREATE_COMPLETE, DescribeStackResource('Policy') for the physical id, then GetCachePolicyConfig(Id=<that id>) -> 404 NoSuchCachePolicy.
Cause 10: trigger each of the five refusals and compare the Message to the per-code sentence.
Or replay the 20 check ids in trace_ids.
