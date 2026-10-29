Issue-Source: trace-sweep-divergence
Issue-Trace: cfval-origin-oai-and-oac-both-set
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cfval-origin-oai-and-oac-both-set cloudfront-distribution-lifecycle-with-tags cloudfront-distribution-trusted-key-groups-active cloudfront-create-distribution-with-tags-lifecycle cloudfront-invalidation-lifecycle cloudfront-monitoring-subscription-workflow cloudfront-web-acl-association-workflow cloudfront-distribution-create-invalidate-monitor-webacl cloudfront-distribution-webacl-association cloudfront-distribution-webacl-monitoring-lifecycle cloudfront-web-acl-association-error-path cloudfront-distribution-tenant-webacl-association
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront|CreateDistribution|2xx->2xx|$.Distribution.DistributionConfig.Origins.Items.Origin.S3OriginConfig.OriginReadTimeout
Issue-Key: cloudfront|GetDistribution|2xx->2xx|$.Distribution.DistributionConfig.Origins.Items.Origin.S3OriginConfig.OriginReadTimeout
Issue-Key: cloudfront|CreateDistributionWithTags|2xx->2xx|$.Distribution.DistributionConfig.Origins.Items.Origin.S3OriginConfig.OriginReadTimeout
Issue-Key: cloudfront|GetDistributionConfig|2xx->2xx|$.DistributionConfig.Origins.Items.Origin.S3OriginConfig.OriginReadTimeout
Issue-Key: cloudfront|AssociateDistributionWebACL|4xx->2xx|$.ErrorResponse
Issue-Key: cloudfront|AssociateDistributionWebACL|4xx->2xx|status
Issue-Key: cloudfront|GetDistributionConfig|2xx->2xx|$.DistributionConfig.WebACLId
Issue-Key: cloudfront|AssociateDistributionTenantWebACL|4xx->2xx|$.ErrorResponse
Issue-Key: cloudfront|AssociateDistributionTenantWebACL|4xx->2xx|status
Issue-Commit: e6d9764b7c2fffef8d3218e52b84d80407a24432

# CloudFront distribution config: S3 origin default omitted and web ACL association unvalidated

> _Note: this issue was identified against the code base at commit `e6d9764b7c2fffef8d3218e52b84d80407a24432`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDistribution

## Description
Eight causes on the distribution-config surface, sharing one fault: the distribution config the emulator stores and serializes is not the config AWS would produce - a server-supplied default is dropped on every S3 origin, and an invalid web-ACL association is accepted and then persisted into the config.

Causes 1-4 (S3OriginConfig.OriginReadTimeout).
AWS: for an S3 origin, AWS fills in `OriginReadTimeout` with its default of 30 and returns it inside every serialized S3OriginConfig (Smithy: S3OriginConfig.OriginReadTimeout - 'Specifies how long, in seconds, CloudFront waits for a response from the origin ... the minimum timeout is 1 second, the maximum is 120 seconds'; recorded responses always carry it even when the request omitted it).
Emulator: the member is absent from the serialized S3OriginConfig on every distribution read and write path.
Why: a client that round-trips GetDistributionConfig -> UpdateDistribution loses the origin read timeout, and SDK code that reads the field gets null where AWS always supplies a number.
Fix: the distribution serializer (S3OriginConfig) plus the create-time default in distribution state.
Cause 1 of 8: CreateDistribution - traces cfval-origin-oai-and-oac-both-set#task#4.
Cause 2 of 8: GetDistribution - traces cfval-origin-oai-and-oac-both-set#task#5 (also cloudfront-distribution-lifecycle-with-tags#task#3, cloudfront-distribution-trusted-key-groups-active#task#4).
Cause 3 of 8: CreateDistributionWithTags - traces cloudfront-create-distribution-with-tags-lifecycle#task#1 (also cloudfront-distribution-lifecycle-with-tags#task#1, cloudfront-invalidation-lifecycle#setup#1, cloudfront-monitoring-subscription-workflow#setup#1, cloudfront-web-acl-association-workflow#setup#1).
Cause 4 of 8: GetDistributionConfig - traces cloudfront-create-distribution-with-tags-lifecycle#task#2 (also cloudfront-distribution-lifecycle-with-tags#task#2 and #task#4, cloudfront-distribution-trusted-key-groups-active#task#5, cloudfront-monitoring-subscription-workflow#task#8, cloudfront-web-acl-association-workflow#task#2).

Cause 5 of 8: AssociateDistributionWebACL accepts a web ACL that does not exist.
AWS: 400 `InvalidArgument` with the fixed message `The specified web ACL isn't valid. Specify a web ACL that was previously validated and try again.`
Emulator: 200 OK and the association is made.
Why: the distribution ends up referencing a non-existent WAF web ACL, so a client can never discover the bad reference.
Fix: AssociateDistributionWebACL validation (resolve the web ACL before associating).
Traces: cloudfront-distribution-create-invalidate-monitor-webacl#task#11 (also cloudfront-distribution-webacl-association#task#3, cloudfront-distribution-webacl-monitoring-lifecycle#task#8, cloudfront-web-acl-association-error-path#task#2, cloudfront-web-acl-association-workflow#task#3).

Cause 6 of 8: GetDistributionConfig then reports the rejected WebACLId.
AWS: because the association was refused, `DistributionConfig.WebACLId` stays empty.
Emulator: WebACLId holds the ARN that should have been rejected.
Why: downstream reads see a web ACL the account does not have; this is the persisted consequence of cause 5 and must be re-checked after it is fixed.
Fix: distribution state (do not write WebACLId when the association is refused).
Traces: cloudfront-distribution-create-invalidate-monitor-webacl#task#12 (also cloudfront-distribution-webacl-monitoring-lifecycle#task#11, cloudfront-web-acl-association-error-path#task#3).

Cause 7 of 8: AssociateDistributionTenantWebACL accepts a web ACL that does not exist.
AWS: 400 `InvalidArgument`, same fixed message as cause 5.
Emulator: 200 OK.
Why/Fix: as cause 5, in AssociateDistributionTenantWebACL validation.
Traces: cloudfront-distribution-tenant-webacl-association#task#3.

Cause 8 of 8: GetDistributionConfig reports both defects together - the omitted OriginReadTimeout and a WebACLId holding a plainly malformed value that was never validated.
AWS: WebACLId empty (the association was refused) and S3OriginConfig.OriginReadTimeout present.
Emulator: WebACLId is the raw non-ARN string from the request and OriginReadTimeout is absent.
Why: shows the association path accepts a value that is not even ARN-shaped.
Fix: AssociateDistributionWebACL validation + distribution serializer.
Traces: cloudfront-web-acl-association-workflow#task#5 (also cloudfront-web-acl-association-workflow#teardown#6).

Related open issues that do NOT cover these causes: 20261022T025800Z-cloudfront-createdistribution-accepts-configurations-aws-rejects.md, 20261029T175823Z-cloudfront-10-more-divergences-the-edge-data-plane.md.

## Evidence
Causes 1-4: '$.Distribution.DistributionConfig.Origins.Items.Origin.S3OriginConfig.OriginReadTimeout: missing (expected 30, got None)' on CreateDistribution and GetDistribution; the same path under $.DistributionConfig... on CreateDistributionWithTags and GetDistributionConfig. Status matches (201/200) in all four.

Cause 5: AssociateDistributionWebACL expected 400 with ErrorResponse {Type: Sender, Code: InvalidArgument, Message: "The specified web ACL isn't valid. Specify a web ACL that was previously validated and try again."}, got 200 and no ErrorResponse.
Cause 6: GetDistributionConfig '$.DistributionConfig.WebACLId: value_mismatch (expected '', got 'arn:aws:wafv2:us-east-1:<account>:global/webacl/nonexistent-probe-acl/...')'.
Cause 7: AssociateDistributionTenantWebACL expected 400 InvalidArgument with the same message, got 200.
Cause 8: GetDistributionConfig '$.DistributionConfig.WebACLId: value_mismatch (expected '', got 'not-a-valid-web-acl-arn')' together with the missing OriginReadTimeout.

The account id and the probe ACL name in cause 6 are per-run values; the requirement is that WebACLId is empty and that the association is refused with AWS's fixed InvalidArgument sentence.

## Steps to Reproduce
1. Replay the wire corpus against the emulator.
2. Causes 1-4: CreateDistribution / CreateDistributionWithTags with an S3 origin, then GetDistribution and GetDistributionConfig; S3OriginConfig has no OriginReadTimeout member.
3. Causes 5-8: call AssociateDistributionWebACL (and AssociateDistributionTenantWebACL) with a web-ACL ARN that does not exist, and again with a plainly malformed string; both are accepted 200, and GetDistributionConfig afterwards reports the value in WebACLId.
4. Representative step: fixture cfval-origin-oai-and-oac-both-set, step task#4 (CreateDistribution).
