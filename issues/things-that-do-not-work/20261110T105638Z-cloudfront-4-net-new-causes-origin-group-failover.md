Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront.origin-group-fails-over-to-secondary-origin-at-the-edge cloudfront.web-acl-association-enforced-at-edge cloudfront.edge-requests-deliver-requests-metric-with-global-dimension cloudfront.distribution-cache-behavior-order-preserved
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront.origin-group-fails-over-to-secondary-origin-at-the-edge|_poll_until#30|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront.web-acl-association-enforced-at-edge|_poll_until_status#6|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront.edge-requests-deliver-requests-metric-with-global-dimension|_poll_until#49|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront.distribution-cache-behavior-order-preserved|_error#27|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded and returned {result!r}")
Issue-Commit: c52a7255ce2600c21f38f62abb43a8c83b37855a

# cloudfront: 4 net-new causes — origin-group failover, WAF web-ACL enforcement and AWS/CloudFront Requests metrics are all absent at the served edge, and UpdateDistribution accepts a CacheBehavior that omits SmoothStreaming

> _Note: this issue was identified against the code base at commit `c52a7255ce2600c21f38f62abb43a8c83b37855a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDistribution

## Description
FOUR net-new root causes in the cloudfront service; a fixer must close all four before submitting. The edge now answers (sibling failures carry the emulator's x-cache/x-amz-cf-id headers), so the open "edge domain answers nothing" issues do not cover these: 20261102T102137Z-cloudfront-a-deployed-distribution-s-edge-domain-answers.md and 20261107T192835Z-cloudfront-a-deployed-distribution-s-edge-domain-answers-787448.md are related but carry none of these replay keys. Cause 4 re-exposes Cause 2 of the resolved 20261024T182555Z-cloudfront-ten-fidelity-divergences-distribution-policy-tenant-validation.md, which is not a refusal here (the call succeeds), so it is described as the defect it is rather than a reopen.

Cause 1 of 4: an origin group does not fail over to its secondary origin at the edge.
AWS: Smithy: OriginGroup carries a required FailoverCriteria (the origin status codes that trigger failover) and required Members (a primary and a secondary origin). Recorded behavior: when a behavior targets an origin group and the primary origin answers one of the FailoverCriteria status codes (or is unreachable), CloudFront re-issues the fetch against the secondary member and the viewer is answered 200 with the SECONDARY origin's body.
Emulator: the viewer request never completes — the check exhausts its 180 s budget waiting for the secondary origin's content, so no failover is performed.
Why: a client cannot exercise or validate its high-availability origin design at all; a failover the distribution declares will be discovered only in production, and in the emulator the request simply hangs instead of either succeeding or failing.
Fix: the edge origin-fetch step — when the matched behavior's TargetOriginId names an OriginGroup, try the primary member and, on a FailoverCriteria status code or a connection failure, retry against the secondary before answering the viewer (cloudfront edge data plane, origin resolution).
Traces: cloudfront.origin-group-fails-over-to-secondary-origin-at-the-edge.

Cause 2 of 4: a WAF web ACL associated with a distribution is not enforced at the edge.
AWS: recorded behavior only for the edge path (the control-plane binding is DistributionConfig.WebACLId / AssociateDistributionWebACL): once a CLOUDFRONT-scope web ACL whose rules block a request pattern is associated, a matching viewer request is answered 403 by the edge and the origin is never reached, while a non-matching request is answered 200; disassociating restores 200 for both.
Emulator: the check exhausts its 180 s budget — the associated web ACL produces no observable effect at the edge, so neither the blocked 403 nor the post-disassociation 200 is ever seen.
Why: WAF protection a client believes is in force in front of its distribution is inert on the emulator, so a rule regression (or a rule that blocks legitimate traffic) is undetectable before production.
Fix: the edge request pipeline — evaluate the distribution's associated web ACL against the viewer request before the origin fetch and answer 403 on a block action (cloudfront edge data plane, plus the wafv2 web-ACL evaluation it must consult).
Traces: cloudfront.web-acl-association-enforced-at-edge.

Cause 3 of 4: edge requests produce no AWS/CloudFront Requests datapoints with the Region=Global dimension.
AWS: recorded behavior only: viewer requests served by a distribution are counted into the CloudWatch namespace AWS/CloudFront as metric Requests under the dimensions {DistributionId=<id>, Region=Global}, and GetMetricStatistics/GetMetricData over the request window reports a Sum at least equal to the number of requests driven.
Emulator: the check drives its requests against a Deployed distribution (with the managed CachingDisabled policy, so each request reaches the origin and is individually countable) and then polls CloudWatch to its 900 s cap without ever seeing a datapoint.
Why: no CloudFront observability workflow works — dashboards, alarms on request volume and capacity reporting all read an empty metric, so a client's monitoring configuration cannot be validated.
Fix: the edge data plane must emit CloudWatch metrics for every served request into AWS/CloudFront with the {DistributionId, Region=Global} dimension set (the edge metric-emission path into the cloudwatch service's metric store).
Traces: cloudfront.edge-requests-deliver-requests-metric-with-global-dimension.

Cause 4 of 4: UpdateDistribution accepts a CacheBehaviors item that omits the flags CreateDistribution defaults in.
AWS: Smithy: UpdateDistribution models InvalidArgument (httpError 400); CacheBehavior.SmoothStreaming is an optional boolean that CreateDistribution DEFAULTS in, while UpdateDistribution is a full replace that requires the submitted behavior to carry it. Recorded behavior: the SAME CacheBehavior struct CreateDistribution accepts is refused by UpdateDistribution with 400 InvalidArgument and the message "The parameter SmoothStreaming flag is missing." (fixed text; the leading parameter name identifies the omitted member), and the stored behavior list is untouched.
Emulator: the update SUCCEEDS with HTTP 200 and the partial behavior list is accepted and stored (the response echoes the three origins and the reordered behaviors).
Why: the create/update asymmetry AWS enforces is erased, so client code that builds an update body from hand-written behaviors passes offline and fails with 400 in production; worse, the permissive path persists a behavior list whose defaults the caller never chose.
Fix: UpdateDistribution's config-completeness validation for DefaultCacheBehavior and each CacheBehaviors item (the cloudfront UpdateDistribution handler's validation).
Traces: cloudfront.distribution-cache-behavior-order-preserved.

## Evidence
Cause 1: "Error: Check timed out after 180.0s" — the check polls the distribution's DomainName for the secondary origin's body after the primary is made to answer a FailoverCriteria status; expected 200 carrying the secondary origin's content.
Cause 2: "Error: Check timed out after 180.0s" — expected 403 from the edge for a request matching the associated CLOUDFRONT-scope web ACL's block rule, and 200 for a non-matching request.
Cause 3: "Error: Check timed out after 900.0s" — expected CloudWatch AWS/CloudFront metric Requests with dimensions {DistributionId=<id>, Region=Global} and a Sum >= 5 over the window in which 5 edge requests were driven; no datapoint ever appeared.
Cause 4: "AssertionError: expected the call to be rejected, but it succeeded and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'ETag': ..., 'Distribution': {... 'DistributionConfig': {... 'Origins': {'Quantity': 3, ...}}}}" — the asserted expectation is code == 'InvalidArgument' with message exactly "The parameter SmoothStreaming flag is missing." Expected: HTTP 400, code InvalidArgument, that message (all fixed text; 'SmoothStreaming' names the omitted member). Actual: HTTP 200 and the partial config stored. (Ids, ETags, hosts, ports and timestamps in the tail are per-run values, not requirements.)

## Steps to Reproduce
1. Point cloudfront and cloudwatch clients at the emulator (cloudfront control plane in us-east-1).
2. Cause 1: CreateDistribution with two origins and an OriginGroup over them (FailoverCriteria on 5xx/404), a behavior targeting the group; wait for Status Deployed; make the primary answer a FailoverCriteria code and GET through the distribution's DomainName -> no response carrying the secondary origin's body ever arrives.
3. Cause 2: create a CLOUDFRONT-scope wafv2 web ACL with a rule blocking a path, associate it with the distribution, wait for Deployed, GET the blocked path -> no 403 ever arrives; disassociate and GET again -> no 200 either.
4. Cause 3: create a distribution using the managed CachingDisabled cache policy (4135ea2d-6df8-44a3-9df3-4b5a84be39ad), wait for Deployed, drive 5 GETs at its DomainName, then GetMetricStatistics(Namespace='AWS/CloudFront', MetricName='Requests', Dimensions=[{DistributionId},{Region:Global}], Statistics=['Sum']) -> no datapoints.
5. Cause 4: CreateDistribution with 3 CacheBehaviors; GetDistributionConfig; rebuild CacheBehaviors.Items by hand WITHOUT SmoothStreaming; UpdateDistribution under the returned ETag -> 200 instead of 400 InvalidArgument "The parameter SmoothStreaming flag is missing."
Or replay the four check ids in trace_ids.
