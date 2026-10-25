Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront.managed-cache-policy-delete-rejected cloudfront.managed-policy-catalog-lookup cloudfront.update-oai-callerreference-immutable-rejected cloudfront.list-distributions-by-realtime-log-config-index-consistency
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront.managed-cache-policy-delete-rejected|run#17|AssertionError|assert code == "IllegalDelete", f"expected IllegalDelete deleting a managed policy, got {code}"
Issue-Key: cloudfront.managed-policy-catalog-lookup|run#23|AssertionError|assert not missing, (
Issue-Key: cloudfront.update-oai-callerreference-immutable-rejected|run#12|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: cloudfront|TagResource|NotImplemented
Issue-Commit: 35f0669fbed72946ad7349f4657cbc8fac9f7c2b

# cloudfront: 4 net-new fidelity divergences — managed-policy catalogs incomplete, DeleteCachePolicy 404s on a managed policy, OAI IllegalUpdate message invented, and Kinesis TagResource stub blocks the realtime-log index check

> _Note: this issue was identified against the code base at commit `35f0669fbed72946ad7349f4657cbc8fac9f7c2b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteCachePolicy

## Description
Four distinct root causes in the cloudfront emulator, each grounded in its own failing behavioral check. None of these four is marked ALREADY FILED or OWNED in the replay report.

=== Cause 1 of 4: DeleteCachePolicy does not know the AWS-managed cache policies, so deleting one answers 404 NoSuchCachePolicy instead of 400 IllegalDelete ===
AWS: Smithy DeleteCachePolicy (DELETE /2020-05-31/cache-policy/{Id}, success 204) models IllegalDelete (httpError 400, doc "Deletion is not allowed for this entity.") alongside NoSuchCachePolicy (httpError 404). An AWS-managed cache policy exists but is read-only reference data, so the delete is refused as an illegal KIND of delete, not as a missing resource. Recorded behavior for the message: "The request failed because delete is not allowed for this policy." (example text; the requirement is the 400/IllegalDelete classification).
Emulator: GetCachePolicy(Id=658327ea-f89d-4fab-a63d-7e88639e58f6) succeeds and ListCachePolicies(Type='managed') lists that id with Type='managed' — so the managed cache catalog IS present for reads — but DeleteCachePolicy on the same id answers 404 NoSuchCachePolicy. The handler evidently resolves ids only against the customer-created policy store.
Why: a client cannot distinguish "this policy kind is undeletable" from "this policy does not exist". Inventory/cleanup tooling that reaps policies by id treats the managed catalog entry as already-gone and loses the guard AWS gives it; a 404 also wrongly implies the managed id is not resolvable at all, contradicting the Get/List answers for the same id.
Fix: the DeleteCachePolicy handler and its id resolution (services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/delete_cache_policy.rs) must resolve against the same managed-policy catalog that GetCachePolicy/ListCachePolicies read, and classify a managed hit as IllegalDelete (400) before the not-found path.
Traces: cloudfront.managed-cache-policy-delete-rejected (assertion: "expected IllegalDelete deleting a managed policy, got NoSuchCachePolicy").

=== Cause 2 of 4: the AWS-managed origin-request-policy catalog is empty — ListOriginRequestPolicies(Type='managed') returns no Items ===
AWS: Smithy ListOriginRequestPolicies takes Type as an OriginRequestPolicyType enum of exactly {managed, custom}; OriginRequestPolicySummary.Type is required and documented as "either managed (created by Amazon Web Services) or custom (created in this Amazon Web Services account)". The managed partition is AWS reference data present in every account, and contains at least Managed-AllViewer and Managed-CORS-S3Origin (stable names referenced by customer CloudFormation templates and the console). Recorded behavior: the managed partition is non-empty in every account.
Emulator: ListOriginRequestPolicies(Type='managed'), drained across Marker/NextMarker, returns an empty Items list — "managed origin request policies missing: ['Managed-AllViewer', 'Managed-CORS-S3Origin'] (present: [])". The managed cache-policy catalog in the same check DID answer, so this is specifically the origin-request-policy catalog that is never seeded. (The check aborts here, so the managed response-headers-policy catalog asserted immediately afterwards is untested in this run and may be empty too — seed all three families.)
Why: every template or distribution that references a managed origin-request policy by its fixed id fails at CreateDistribution/UpdateDistribution time, and any client enumerating the managed partition to resolve a policy by name gets nothing. This is reference data about AWS, not customer state, so no sequence of client calls can repair it.
Fix: seed the AWS-managed origin-request-policy catalog (ids, names and bodies) in the crate's reference-data/state module and serve it from ListOriginRequestPolicies(Type='managed') plus GetOriginRequestPolicy/GetOriginRequestPolicyConfig, keeping the managed and custom partitions disjoint. Open issue 20261025T025628Z-cloudfront-lambda-cloudfront-listoriginrequestpolicies-type-managed-serves-an-empty-page.md describes the same empty page from a different check and does not carry this check's replay key; open issue 20261025T020309Z-cloudfront-disassociatedistributiontenantwebacl-answers-notimplemented-and-the-aws-managed.md covers the CACHE-policy catalog, which now answers for reads.
Traces: cloudfront.managed-policy-catalog-lookup.

=== Cause 3 of 4: UpdateCloudFrontOriginAccessIdentity's IllegalUpdate refusal carries an invented message that leaks the request's CallerReference ===
AWS: Smithy UpdateCloudFrontOriginAccessIdentity models IllegalUpdate (httpError 400, doc "The update contains modifications that are not allowed."); the model states no message text, so recorded behavior only: a CallerReference mutation is refused with the service's fixed sentence "You cannot update the value of CallerReference." — a constant string that does not embed any request value.
Emulator: the code is correct (IllegalUpdate, 400) but the message is "The CallerReference of an origin access identity cannot be changed; it is sim2real-oai-immutable-564e35df7903" — a hand-written sentence that additionally echoes the stored CallerReference back to the caller.
Why: the operation/status/code are right, so this is message fidelity only: clients and fixtures that match on the service's fixed refusal sentence (and recorded-trace comparisons) see a value-dependent string instead of a constant one. Same class as the already-fixed 20261024T210022Z-s3-getbuckettagging-s-nosuchtagset-refusal-carries-an-invented.md.
Fix: the IllegalUpdate construction in the UpdateCloudFrontOriginAccessIdentity handler (services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/update_cloud_front_origin_access_identity.rs) should emit the service's constant CallerReference sentence and not interpolate the resource's value. Note: the stub issue 20261024T182555Z-cloudfront-updatecloudfrontoriginaccessidentity-stub.md is already FIXED — the operation now runs and refuses correctly, so this is not a reopen of that refusal, only the message text.
Traces: cloudfront.update-oai-callerreference-immutable-rejected.

=== Cause 4 of 4: the realtime-log-config index check never reaches CloudFront — Kinesis TagResource is an unimplemented scaffold stub ===
AWS: the check builds the CloudFront realtime-log prerequisite chain (CreateStream -> wait ACTIVE -> TagResource on the stream ARN) before any CloudFront call. Kinesis TagResource answers 200 with an empty body on AWS.
Emulator: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the TagResource operation: "operation TagResource is scaffolded but not implemented in this emulator". The two preceding Kinesis calls (CreateStream, DescribeStreamSummary) succeeded, so this is TagResource specifically. The failing call is in the KINESIS crate, not the cloudfront one — recorded here because the failing check's id is cloudfront.list-distributions-by-realtime-log-config-index-consistency and that is the service token the report attributes it to. Note kinesis is NOT in the bootstrapped set reported by get_bootstrap_status (service-com-amazonaws-kinesis-kinesis-20131202 is pending), so a fixer should route this cause to the kinesis crate.
Why: every CloudFront realtime-log behavior that needs a Kinesis destination is unreachable — the ListDistributionsByRealtimeLogConfig index consistency (association appears, removal de-indexes) is never exercised at all, so the CloudFront side of it is completely unverified.
Fix: implement TagResource in the kinesis crate (services/service-com-amazonaws-kinesis-kinesis-20131202/src/ops/tag_resource.rs). This is a separate operation from the already-fixed 20261025T002357Z-kinesis-addtagstostream-stub.md.
Traces: cloudfront.list-distributions-by-realtime-log-config-index-consistency.

## Evidence
Cause 1 — cloudfront.managed-cache-policy-delete-rejected [mismatch]:
  expected: DeleteCachePolicy(Id=658327ea-f89d-4fab-a63d-7e88639e58f6) -> 400 IllegalDelete (example message "The request failed because delete is not allowed for this policy."), policy still present afterwards.
  actual:   AssertionError: expected IllegalDelete deleting a managed policy, got NoSuchCachePolicy (404).
  context:  the same check's earlier steps PASSED — DeleteCachePolicy on a customer policy worked, GetCachePolicy on the managed id returned Name=Managed-CachingOptimized, and ListCachePolicies(Type='managed') listed that id with Type='managed'.

Cause 2 — cloudfront.managed-policy-catalog-lookup [mismatch]:
  expected: ListOriginRequestPolicies(Type='managed') contains Managed-AllViewer and Managed-CORS-S3Origin, each with Type='managed'.
  actual:   AssertionError: managed origin request policies missing: ['Managed-AllViewer', 'Managed-CORS-S3Origin'] (present: []).
  context:  the managed CACHE-policy assertions in the same check passed first (catalog non-empty, Managed-CachingOptimized at its documented id, TTLs 1/86400/31536000), so only the origin-request family is proven empty here.

Cause 3 — cloudfront.update-oai-callerreference-immutable-rejected [mismatch]:
  expected: code IllegalUpdate, message 'You cannot update the value of CallerReference.'
  actual:   code IllegalUpdate (correct), message 'The CallerReference of an origin access identity cannot be changed; it is sim2real-oai-immutable-564e35df7903' — a value-interpolated sentence.

Cause 4 — cloudfront.list-distributions-by-realtime-log-config-index-consistency [error]:
  expected: TagResource on the Kinesis stream ARN -> 200, then the CloudFront realtime-log-config / distribution / index assertions run.
  actual:   botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the TagResource operation: operation TagResource is scaffolded but not implemented in this emulator. Nothing in CloudFront was exercised.

## Steps to Reproduce
Cause 1: cf.create_cache_policy(...) a customer policy; cf.delete_cache_policy on it (succeeds); then cf.get_cache_policy(Id='658327ea-f89d-4fab-a63d-7e88639e58f6') and cf.list_cache_policies(Type='managed') (both answer, id present as managed); then cf.delete_cache_policy(Id='658327ea-f89d-4fab-a63d-7e88639e58f6', IfMatch=<its ETag>) -> observe 404 NoSuchCachePolicy where AWS gives 400 IllegalDelete.

Cause 2: cf.list_origin_request_policies(Type='managed', MaxItems='100'), draining Marker/NextMarker -> observe an empty Items list; AWS returns at least Managed-AllViewer and Managed-CORS-S3Origin. Then repeat for cf.list_response_headers_policies(Type='managed') to confirm whether that family is seeded.

Cause 3: cf.create_cloud_front_origin_access_identity(CloudFrontOriginAccessIdentityConfig={'CallerReference': R, 'Comment': 'original'}); then cf.update_cloud_front_origin_access_identity(Id=<id>, IfMatch=<ETag>, CloudFrontOriginAccessIdentityConfig={'CallerReference': R + '-mutated', 'Comment': '...'}) -> IllegalUpdate is returned (correct) but with the invented, value-interpolated message.

Cause 4: kinesis.create_stream(StreamName=N, ShardCount=1); poll describe_stream_summary until ACTIVE; kinesis.tag_resource(ResourceARN=<StreamARN>, Tags={'k':'v'}) -> 501/NotImplemented. Replay cloudfront.list-distributions-by-realtime-log-config-index-consistency to reach the CloudFront assertions behind it.
