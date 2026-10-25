Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__wafv2.blocked-and-counted-requests-carry-the-webacl-and-rule-dimensions
Issue-Key: cloudwatch__wafv2|AssociateWebACL|NotImplemented
Issue-Commit: 2e744e838767d761bb8115d027d1acb9d75669f4

# WAFv2 CloudWatch metric dimensions cannot be exercised: AssociateWebACL answers NotImplemented

> _Note: this issue was identified against the code base at commit `2e744e838767d761bb8115d027d1acb9d75669f4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateWebACL

## Description
Cause 1 of 1: a web ACL cannot be put in front of a resource, so WAF's CloudWatch metrics are never produced.
AWS: AssociateWebACL binds a REGIONAL web ACL to a supported resource and returns an empty success body (Smithy com.amazonaws.wafv2 AssociateWebACL -> AssociateWebACLResponse with no members; its modeled errors are WAFInvalidParameterException / WAFNonexistentItemException / WAFUnavailableEntityException, all client/400). With the association live, blocked and counted requests are reported to CloudWatch in the AWS/WAFV2 namespace with the WebACL and Rule dimensions, so GetMetricStatistics/ListMetrics can select a single rule's counts.
Emulator: the call fails with 'An error occurred (NotImplemented) when calling the AssociateWebACL operation: operation AssociateWebACL is scaffolded but not implemented in this emulator', so the check never reaches the metric assertions and nothing about the WebACL/Rule dimensions is observable.
Why: a client that drives WAF dashboards or alarms off AWS/WAFV2 metrics has nothing to read, and the operation itself returns an error code AWS never returns for this request.
Fix: implement the wafv2 AssociateWebACL handler (services/service-com-amazonaws-wafv2-awswaf-20190729/src/ops/associate_web_acl.rs) and the AWS/WAFV2 metric emission (WebACL/Rule/Region dimensions) that WAF rule evaluation must feed into the cloudwatch metric store.
Traces: cloudwatch__wafv2.blocked-and-counted-requests-carry-the-webacl-and-rule-dimensions.
Note: this report also files a wafv2-scoped stub issue naming AssociateWebACL; that issue covers the operation stub only — the cross-service half of this cause (AWS/WAFV2 metrics with the WebACL and Rule dimensions) is not covered by it and remains unverified until this check passes.

## Evidence
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the AssociateWebACL operation: operation AssociateWebACL is scaffolded but not implemented in this emulator. Expected: a 200 empty AssociateWebACLResponse, followed by AWS/WAFV2 metrics (BlockedRequests/CountedRequests) carrying the WebACL and Rule dimensions.

## Steps to Reproduce
1. CreateWebACL (REGIONAL) with one Block rule and one Count rule, SampledRequestsEnabled/CloudWatchMetricsEnabled true.
2. Call wafv2 AssociateWebACL for a supported protected resource — it answers NotImplemented.
3. Drive matching and non-matching requests and read cloudwatch metrics in AWS/WAFV2 filtered on the WebACL and Rule dimensions.
4. Or replay cloudwatch__wafv2.blocked-and-counted-requests-carry-the-webacl-and-rule-dimensions.
