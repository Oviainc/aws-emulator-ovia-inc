Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elasticache.create-serverless-cache-usage-limit-out-of-range-rejected
Issue-Shape-Id: com.amazonaws.elasticache#AmazonElastiCacheV9
Issue-Key: elasticache.create-serverless-cache-usage-limit-out-of-range-rejected|run#4|AssertionError|assert code == "InvalidParameterValue", f"expected InvalidParameterValue, got {code}"
Issue-Commit: f4e5251e6df88363d8e23ace2ff5bff3be31b423

# CreateServerlessCache with an out-of-range CacheUsageLimits bound answers NotImplemented instead of the modeled 400 InvalidParameterValue

> _Note: this issue was identified against the code base at commit `f4e5251e6df88363d8e23ace2ff5bff3be31b423`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateServerlessCache

## Description
One cause, grounded in the single failing check named below. It is NOT covered by the open issue 20261026T234107Z-elasticache-serverless-usage-limit-refusal-still-answers-notimplemented.md: that issue carries only the replay key elasticache.serverless-cache-usage-limits-inverted-rejected and states only the inverted-bounds rule (Minimum > Maximum, message "Invalid values for <Member>. Minimum must not be greater than Maximum."); this check exercises a different rule (a single bound outside the documented 1-5000 range) with a different required message, and the report did not mark it OWNED by that issue. The EC2 scaffold issues this suite reports (CreateInternetGateway, CreateRouteTable) likewise do not carry this check's key or this requirement.

Cause 1 of 1: CreateServerlessCache with CacheUsageLimits={"DataStorage":{"Maximum":-5,"Unit":"GB"}} is answered with error code NotImplemented (the scaffold's HTTP 501) rather than the modeled HTTP 400 client refusal, so the out-of-range usage-limit contract is unobservable end to end and nothing after the first call in the check is reached.
AWS: Smithy: CreateServerlessCache models InvalidParameterValueException {errorKind client, httpError 400, awsQueryError code InvalidParameterValue, httpResponseCode 400} and ServerlessCacheNotFoundFault {client, 404}; CacheUsageLimits.DataStorage.Maximum is a plain IntegerOptional with NO @range trait, so botocore performs no client-side validation and the negative value reaches the service. Recorded behavior for the message: the fixed sentence "Invalid value for DataStorage limit. Must be between 1 and 5000. You can set the value to zero to remove the limit." — all fixed text; the variable part is the limit member named by the offending request element (DataStorage here, ECPUPerSecond for that limit), and the bounds quoted are that member's documented range. The refused create must leave nothing behind: a following DescribeServerlessCaches(ServerlessCacheName=<that name>) must answer 404 ServerlessCacheNotFoundFault with message "Serverless cache with name <ServerlessCacheName> not found." (fixed text around the requested name).
Emulator: the first CreateServerlessCache is answered with error code NotImplemented (HTTP 501), so the check dies on `assert code == "InvalidParameterValue", got NotImplemented` and the not-created assertion, the in-range positive control (DataStorage.Maximum=10 must succeed, reach Status=available and persist Maximum=10) and the persistence assertion are never exercised.
Why: 501 is not a modeled error for this operation, no SDK classifies it as a client fault, and it is not retryable in the way a 400 is final — so a caller cannot distinguish "your usage limit is out of range" from "this service is unavailable", and the entire serverless usage-limit negative path is untestable against the emulator. It also masks whether the emulator would have silently clamped the value to 0/None rather than refusing it, which is the over-permissive failure this check exists to catch.
Fix: identify which operation on this request sequence emits the scaffolded 501 before changing code — the ElastiCache CreateServerlessCache handler (services/service-com-amazonaws-elasticache-amazonelasticachev9/src/ops/create_serverless_cache.rs) is declared IMPLEMENTED and already routes usage limits through crate::state's usage-limit validation, so either that validation does not cover a single out-of-range bound (only inverted bounds) and the call falls through to the scaffold error channel, or the 501 comes from an EC2 operation reached while the check builds SubnetIds via ctx.network().subnet_ids[:3]. Whichever it is, the fix belongs in that operation's handler; the ElastiCache side then needs the range rule (1-5000, with 0 meaning "no limit") in CreateServerlessCache's validation, worded as above.
Traces: elasticache.create-serverless-cache-usage-limit-out-of-range-rejected (its first create_serverless_cache call).

## Evidence
Failing check elasticache.create-serverless-cache-usage-limit-out-of-range-rejected [mismatch], from this replay of suite simparity-behavioral-aws-v1 (39/152 passed):

  File "check_elasticache_create_serverless_cache_usage_limit_out_of_range_rejected.py", line 31, in run
    code, message = _error(lambda: ec.create_serverless_cache(
        ServerlessCacheName=<fresh name>, Engine="redis",
        CacheUsageLimits={"DataStorage": {"Maximum": -5, "Unit": "GB"}},
        SubnetIds=ctx.network().subnet_ids[:3]))
    assert code == "InvalidParameterValue", f"expected InvalidParameterValue, got {code}"
  AssertionError: expected InvalidParameterValue, got NotImplemented

Expected: HTTP 400, error code InvalidParameterValue (modeled InvalidParameterValueException, awsQueryError code InvalidParameterValue, httpError 400), message "Invalid value for DataStorage limit. Must be between 1 and 5000. You can set the value to zero to remove the limit." Then DescribeServerlessCaches(ServerlessCacheName=<that name>) must answer 404 ServerlessCacheNotFoundFault, message "Serverless cache with name <name> not found."; and the same call with DataStorage.Maximum=10 must succeed, reach Status=available and report CacheUsageLimits.DataStorage.Maximum=10.
Actual: error code NotImplemented (HTTP 501, the scaffold refusal), raised on the first CreateServerlessCache expression, which aborts the check before any of the above is reached.

Model evidence: smithy_query(operation="CreateServerlessCache", model_path="services/service-com-amazonaws-elasticache-amazonelasticachev9/smithy-model/com-amazonaws-elasticache-amazonelasticachev9.json") -> errors include InvalidParameterValueException {errorKind: client, httpError: 400, awsQueryError: {code: InvalidParameterValue, httpResponseCode: 400}} and ServerlessCacheNotFoundFault {client, 404}; input member CacheUsageLimits targets CacheUsageLimits, whose DataStorage.Maximum is IntegerOptional with no range constraint.

Related filed work (not covering this key): 20261026T234107Z-elasticache-serverless-usage-limit-refusal-still-answers-notimplemented.md (inverted Minimum>Maximum, check elasticache.serverless-cache-usage-limits-inverted-rejected) and this suite's ALREADY FILED entries for the unimplemented EC2 CreateInternetGateway / CreateRouteTable stubs.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay elasticache.create-serverless-cache-usage-limit-out-of-range-rejected.
2. Call CreateServerlessCache with Engine=redis, a fresh ServerlessCacheName, SubnetIds taken from the harness network fixture (the check uses ctx.network().subnet_ids[:3]) and CacheUsageLimits={"DataStorage": {"Maximum": -5, "Unit": "GB"}}. Observe error code NotImplemented (HTTP 501); expect HTTP 400 InvalidParameterValue with message "Invalid value for DataStorage limit. Must be between 1 and 5000. You can set the value to zero to remove the limit.".
3. Capture the wire exchange for step 2 and identify WHICH operation returned the 501 (ElastiCache CreateServerlessCache itself, or an EC2 operation reached while standing up the network fixture). Do this before changing code.
4. Repeat step 2 with SubnetIds that already resolve, to localize the defect: a NotImplemented here means the ElastiCache request path emits it.
5. After the refusal, call DescribeServerlessCaches(ServerlessCacheName=<the refused name>) and expect 404 ServerlessCacheNotFoundFault with message "Serverless cache with name <name> not found." — proving the refused create was not silently clamped and created.
6. Positive control: call the same operation with CacheUsageLimits={"DataStorage": {"Maximum": 10, "Unit": "GB"}}, poll DescribeServerlessCaches to Status=available and assert CacheUsageLimits.DataStorage.Maximum == 10, confirming the refusal is specific to the out-of-range value and not to CacheUsageLimits as such.
