Issue-Source: trace-sweep-divergence
Issue-Trace: elasticloadbalancing-account-limits-and-ssl-policies
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elasticloadbalancing-account-limits-and-ssl-policies elbv2-catalog-describe-ssl-policies
Issue-Shape-Id: com.amazonaws.elasticloadbalancingv2#ElasticLoadBalancing_v10
Issue-Key: elbv2|DescribeAccountLimits|2xx->4xx|$.DescribeAccountLimitsResponse
Issue-Key: elbv2|DescribeAccountLimits|2xx->4xx|status
Issue-Key: elbv2|DescribeSSLPolicies|2xx->4xx|$.DescribeSSLPoliciesResponse
Issue-Key: elbv2|DescribeSSLPolicies|2xx->4xx|status
Issue-Key: elbv2|DescribeSSLPolicies|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: elbv2|DescribeSSLPolicies|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Commit: c0919b0ae1fd753648687abf4b08014f282092d3

# elbv2 catalog reads refused 400 InvalidAction: the ELBv2 2015-12-01 API is unrouted

> _Note: this issue was identified against the code base at commit `c0919b0ae1fd753648687abf4b08014f282092d3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAccountLimits

## Description
3 causes, all produced by ONE mechanism: no request for API version 2015-12-01 ever reaches an ELBv2 handler. Elastic Load Balancing publishes one endpoint for two APIs; the shared signing name / endpoint prefix 'elasticloadbalancing' is claimed by the Classic ELB crate, whose front door (services/service-com-amazonaws-elasticloadbalancing-elasticloadbalancing-v7/src/service/api_version.rs, serves_requested_version / UnsupportedApiVersionOperation) serves only Version=2012-06-01 and answers every other envelope 400 InvalidAction with 'Could not find operation <Action> for version <Version>'. The ELBv2 model (com.amazonaws.elasticloadbalancingv2#ElasticLoadBalancing_v10, version 2015-12-01, awsQuery, 51 bound operations) is reachable only under its own route key 'elasticloadbalancingv2' (services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10/src/service.rs, ROUTE_KEY), which no AWS SDK or CLI client ever uses. Related OPEN issues that do NOT cover these fixture steps (none carries a wire replay key, so re-replaying them proves nothing about this corpus): 20261023T025808Z-elbv2-awsquery-action-table-at-api-version-2015.md (behavioral corpus, trace_ids are 'elbv2.*' check ids), 20261023T004835Z-acm-elbv2-elbv2-createloadbalancer-is-not-dispatchable-could-not-find.md (CreateLoadBalancer only), 20261022T220446Z-elbv2-elastic-load-balancing-v2-is-unreachable-every-request.md (describes a 500 AmbiguousService symptom, a different response).

Cause 1 of 3: DescribeAccountLimits.
AWS: Smithy DescribeAccountLimits -> output {Limits, NextMarker}, errors: NONE modeled, so any error response is a divergence; a well-formed Action=DescribeAccountLimits&Version=2015-12-01 POST is answered 200 with a <DescribeAccountLimitsResponse> carrying the per-account Limits list (target-groups, listeners-per-application-load-balancer, rules-per-application-load-balancer, trust-stores, ...).
Emulator: 400 with Error.Code 'InvalidAction' and no DescribeAccountLimitsResponse element at all.
Why: a client cannot read any ELBv2 account limit, and 'InvalidAction' tells an SDK the action does not exist, so retry and validation logic treats a well-formed request as a caller programming error rather than a server gap.
Fix: the awsQuery action/version dispatch for the shared elasticloadbalancing endpoint (Classic crate src/service/api_version.rs plus the ELBv2 crate's service claim), so a Version=2015-12-01 envelope reaches the ELBv2 operation; then the DescribeAccountLimits handler/serializer in the elasticloadbalancingv2 crate.
Traces: elasticloadbalancing-account-limits-and-ssl-policies#task#1.

Cause 2 of 3: DescribeSSLPolicies success path.
AWS: Smithy DescribeSSLPolicies -> output {SslPolicies, NextMarker}; a call with no Names filter is answered 200 with the predefined policy catalogue (each entry Name + Ciphers[Name,Priority] + SslProtocols + SupportedLoadBalancerTypes), e.g. ELBSecurityPolicy-2016-08.
Emulator: 400 Error.Code 'InvalidAction'; DescribeSSLPoliciesResponse missing.
Why: no client can discover which TLS policies exist, so an HTTPS listener cannot be configured against a real policy name.
Fix: same version dispatch, then the DescribeSSLPolicies handler and its seeded policy catalogue in the elasticloadbalancingv2 crate.
Traces: elasticloadbalancing-account-limits-and-ssl-policies#task#2 (also task#3, elbv2-catalog-describe-ssl-policies#task#1).

Cause 3 of 3: DescribeSSLPolicies rejection path for an unknown policy name.
AWS: Smithy DescribeSSLPolicies declares SSLPolicyNotFoundException, awsQueryError code 'SSLPolicyNotFound', HTTP 400; an unknown name in Names is refused with that code. The message rule here is a fixed service string, recorded as 'One or more SSL policies not found' (example).
Emulator: HTTP 400 but Error.Code 'InvalidAction' with message 'Could not find operation DescribeSSLPolicies for version 2015-12-01'.
Why: the status is right by accident while the code is not; a client branching on SSLPolicyNotFound cannot distinguish 'this policy name is wrong' from 'this API does not exist'.
Fix: same version dispatch, then DescribeSSLPolicies' name lookup and its modeled-error rendering.
Traces: elasticloadbalancing-account-limits-and-ssl-policies#task#4.

## Evidence
Fixture elasticloadbalancing-account-limits-and-ssl-policies (service token 'elbv2'):
- task#1 DescribeAccountLimits: expected 200 + $.DescribeAccountLimitsResponse.DescribeAccountLimitsResult.Limits.member[...] (target-groups, targets-per-application-load-balancer, listeners-per-application-load-balancer, rules-per-*, trust-stores, ...); actual 400, DescribeAccountLimitsResponse missing.
- task#2 DescribeSSLPolicies: expected 200 + $.DescribeSSLPoliciesResponse.DescribeSSLPoliciesResult.SslPolicies.member (Name 'ELBSecurityPolicy-2016-08', 18 Ciphers, SslProtocols TLSv1/TLSv1.1/TLSv1.2); actual 400, DescribeSSLPoliciesResponse missing. Same cluster: task#3 and elbv2-catalog-describe-ssl-policies#task#1.
- task#4 DescribeSSLPolicies (unknown name): expected 400 $.ErrorResponse.Error.Code 'SSLPolicyNotFound' (message example 'One or more SSL policies not found'); actual 400 Code 'InvalidAction', Message 'Could not find operation DescribeSSLPolicies for version 2015-12-01'.
All three actual bodies are the Classic front door's UnsupportedApiVersionOperation envelope (400 InvalidAction, 'Could not find operation <Action> for version 2015-12-01').

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. POST to the elasticloadbalancing endpoint a form body 'Action=DescribeAccountLimits&Version=2015-12-01' signed for service 'elasticloadbalancing' (what an AWS SDK elbv2 client sends). Observe 400 InvalidAction instead of 200 + Limits.
3. Repeat with 'Action=DescribeSSLPolicies&Version=2015-12-01' (no Names) and with 'Names.member.1=no-such-policy'; both answer InvalidAction instead of 200 + SslPolicies and 400 SSLPolicyNotFound.
4. Re-replay fixture elasticloadbalancing-account-limits-and-ssl-policies steps task#1, task#2, task#4.
