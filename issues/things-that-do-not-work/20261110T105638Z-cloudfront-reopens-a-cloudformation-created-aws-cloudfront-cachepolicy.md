Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront.deploy-cache-policy-cfn-enforced-on-distribution cloudfront.deploy-cache-policy-cfn-ttl-enforced cloudfront.deploy-cloudfront-1-cache-policy-invalid-ttl-order-update-rejected-rolls-back cloudfront.deploy-cloudfront-1-cache-policy-out-of-band-ttl-drift-detected-then-updated-in-place
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront|GetCachePolicyConfig|NoSuchCachePolicy
Issue-Commit: c52a7255ce2600c21f38f62abb43a8c83b37855a

# cloudfront: REOPENS — a CloudFormation-created AWS::CloudFront::CachePolicy still does not exist in CloudFront, so GetCachePolicyConfig answers 404 NoSuchCachePolicy right after CREATE_COMPLETE

> _Note: this issue was identified against the code base at commit `c52a7255ce2600c21f38f62abb43a8c83b37855a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCachePolicyConfig

## Description
REOPENS 20261024T182555Z-cloudfront-ten-fidelity-divergences-distribution-policy-tenant-validation.md: its fix left the call refused. That issue's Cause 9 carried exactly this replay key (Issue-Key: cloudfront|GetCachePolicyConfig|NoSuchCachePolicy) and the two deploy-cloudfront-1-cache-policy-* check ids below; after the fix the same call is still refused, and two further checks now fail the same way.

Cause 1 of 1: a cache policy provisioned by CloudFormation is not reachable through the CloudFront control plane.
AWS: Smithy (cloudfront 2020-05-31): GetCachePolicyConfig is GET /2020-05-31/cache-policy/{Id}/config and answers 200 with the stored CachePolicyConfig; it models NoSuchCachePolicy (httpError 404) ONLY for an id that names no cache policy. A stack containing an AWS::CloudFront::CachePolicy that has reached CREATE_COMPLETE has a physical resource id that IS a CloudFront cache-policy Id, so GetCachePolicyConfig on it answers 200 with the TTLs the template declared, and the policy is usable as CacheBehavior.CachePolicyId. Recorded behavior: after CREATE_COMPLETE, GetCachePolicyConfig(Id=<physical id of the CachePolicy resource>) returns the deployed MinTTL/DefaultTTL/MaxTTL and ParametersInCacheKeyAndForwardedToOrigin; a stack UPDATE that pushes DefaultTTL above MaxTTL leaves the resource UPDATE_FAILED and rolls the stack back with the policy keeping its id and original TTLs.
Emulator: GetCachePolicyConfig on that physical id is refused 404 NoSuchCachePolicy with message "The specified cache policy does not exist." (that status, that code and that message are the requirement for a genuinely unknown id; here the id is one CloudFormation itself reported). So the CFN-declared cache policy was either never created in the CloudFront state, or the physical id CFN reports names nothing in it. Every check that deploys a cache policy by template and then reads it back dies on the first read.
Why: every infrastructure-as-code trajectory that declares cache policies and then reads, enforces or drifts them is impossible — the caller gets a 404 for a resource CloudFormation just told it exists. The TTL-order refusal/rollback contract and the "policy enforced on a distribution" contract are both unreachable.
Fix: the CloudFormation resource handler for AWS::CloudFront::CachePolicy and the physical id it reports — the created policy must be registered in the cloudfront service's cache-policy state under exactly that id (the CFN-to-cloudfront resource bridge, plus the cloudfront cache-policy state slice).
Traces: cloudfront.deploy-cache-policy-cfn-enforced-on-distribution, cloudfront.deploy-cache-policy-cfn-ttl-enforced, cloudfront.deploy-cloudfront-1-cache-policy-invalid-ttl-order-update-rejected-rolls-back, cloudfront.deploy-cloudfront-1-cache-policy-out-of-band-ttl-drift-detected-then-updated-in-place. Also related and still open for the DISTRIBUTION analogue: 20261029T175823Z-cloudfront-10-more-divergences-the-edge-data-plane.md (which owns cloudfront.deploy-cache-policy-enforced-via-cloudformation, failing with NoSuchDistribution, not NoSuchCachePolicy).

## Evidence
All four checks die on the first read-back of the stack's cache policy:

botocore.errorfactory.NoSuchCachePolicy: An error occurred (NoSuchCachePolicy) when calling the GetCachePolicyConfig operation: The specified cache policy does not exist.

Expected: 200 with the CachePolicyConfig the template declared (the TTLs and ParametersInCacheKeyAndForwardedToOrigin of the AWS::CloudFront::CachePolicy resource).
Actual: HTTP 404, code NoSuchCachePolicy, message "The specified cache policy does not exist." — the fixed text AWS uses for a truly absent policy, returned here for a physical id CloudFormation reported as CREATE_COMPLETE.
Message parts: the whole sentence is fixed text with no variable part; the divergence is that it is returned at all for an id that exists.

## Steps to Reproduce
1. Point cloudformation and cloudfront clients at the emulator (cloudfront control plane in us-east-1).
2. CreateStack with a template containing an AWS::CloudFront::CachePolicy resource (logical id Policy) declaring MinTTL/DefaultTTL/MaxTTL.
3. Wait for the stack to reach CREATE_COMPLETE.
4. DescribeStackResource(StackName, LogicalResourceId="Policy") to read PhysicalResourceId.
5. cloudfront.get_cache_policy_config(Id=<that physical id>) -> observe 404 NoSuchCachePolicy instead of 200 with the declared TTLs.
6. Or replay the four check ids in trace_ids.
