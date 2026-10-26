Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elasticache.serverless-cache-usage-limits-inverted-rejected
Issue-Shape-Id: com.amazonaws.elasticache#AmazonElastiCacheV9
Issue-Key: elasticache.serverless-cache-usage-limits-inverted-rejected|_assert_error#3|AssertionError|assert _fault_base(code) == _fault_base(expected), (
Issue-Commit: 56c908e9384776713583471da50d0f78dde9fb40

# ElastiCache serverless-cache usage limits: ModifyServerlessCache is still a 501 scaffold stub, and the inverted-limits create refusal is answered NotImplemented (2 causes)

> _Note: this issue was identified against the code base at commit `56c908e9384776713583471da50d0f78dde9fb40`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyServerlessCache

## Description
Two distinct causes keep the serverless-cache CacheUsageLimits contract unimplementable end to end. Both are grounded in the single surviving failing check elasticache.serverless-cache-usage-limits-inverted-rejected (every other failing elasticache check in this run was marked ALREADY FILED or OWNED by an open issue and was skipped).

Cause 1 of 2 — REOPENS 20261022T193451Z-elasticache-createserverlesscache-stub.md: its fix left the call refused.
AWS: CreateServerlessCache models InvalidParameterValueException (HTTP 400, awsQueryError code InvalidParameterValue) — Smithy: CreateServerlessCache errors include InvalidParameterValueException {httpError 400, awsQueryError InvalidParameterValue}; CacheUsageLimits.DataStorage{Minimum,Maximum,Unit} and ECPUPerSecond{Minimum,Maximum} are plain IntegerOptional with NO @range trait, so a semantically contradictory Minimum > Maximum reaches the service and AWS refuses it with that modeled 400 (recorded behavior for the message, e.g. "Invalid values for DataStorage. Minimum must not be greater than Maximum.").
Emulator: the check's very first CreateServerlessCache (DataStorage Minimum=50 > Maximum=10) is answered with error code NotImplemented — the scaffold's HTTP 501 refusal ("operation ... is scaffolded but not implemented in this emulator") — instead of the modeled 400 InvalidParameterValueException. The check asserts `expected InvalidParameterValueException, got NotImplemented`. Note for the fixer: the CreateServerlessCache handler in this checkout is NOT a stub and already carries the inversion rule, so the 501 is raised somewhere else on that call's path — the check builds the call's SubnetIds from the harness network fixture (`ctx.network().subnet_ids[:3]`) inside the same expression, and this suite reports two EC2 scaffold stubs (CreateInternetGateway, blocking 39 checks, and AuthorizeSecurityGroupIngress, blocking 12) that a network fixture would plausibly hit; those two already have their own filed issues (20261022T202249Z-ec2-createinternetgateway-stub.md and the auto-filed AuthorizeSecurityGroupIngress issue). Determine which operation emits the 501 on this path before writing code: if it is one of those EC2 stubs, close this cause by pointing at them; if it is the ElastiCache wire path, the refusal must become the modeled 400.
Why: a client that sends a contradictory usage-limit envelope gets a 501 instead of a 400 client error. 501 is not retried, not modeled, and not classified as a client fault by any SDK's error handling, so a caller cannot distinguish "your request is invalid" from "this service is unavailable", and no negative-path test against the emulator can validate its own input handling.
Fix: the CreateServerlessCache request path in services/service-com-amazonaws-elasticache-amazonelasticachev9/src/ops/create_serverless_cache.rs (its validation already delegates to crate::state::validate_cache_usage_limits), plus whichever scaffolded operation on this check's path still answers 501.
Traces: elasticache.serverless-cache-usage-limits-inverted-rejected (first _assert_error, "CreateServerlessCache with DataStorage Minimum=50 > Maximum=10").

Cause 2 of 2 — ModifyServerlessCache answers 501 NotImplemented instead of its modeled response and errors.
AWS: Smithy: ModifyServerlessCache takes ServerlessCacheName (required) plus Description, CacheUsageLimits, RemoveUserGroup, UserGroupId, SecurityGroupIds, SnapshotRetentionLimit, DailySnapshotTime, Engine, MajorEngineVersion; it returns ServerlessCache and models InvalidParameterValueException (400 / InvalidParameterValue), InvalidParameterCombinationException (400), InvalidServerlessCacheStateFault (400), InvalidUserGroupStateFault (400), ServerlessCacheNotFoundFault (404 / ServerlessCacheNotFoundFault), ServiceLinkedRoleNotFoundFault (400), UserGroupNotFoundFault (404) and InvalidCredentialsException (408). A modify that inverts a CacheUsageLimits bound must be refused 400 InvalidParameterValueException and must leave the live cache's limits and status untouched; a legal widening must apply; a modify naming a deleted cache must be 404 ServerlessCacheNotFoundFault.
Emulator: services/service-com-amazonaws-elasticache-amazonelasticachev9/src/ops/modify_serverless_cache.rs still carries the `SCAFFOLDED, NOT IMPLEMENTED` marker and its handle() returns the scaffold error, so every ModifyServerlessCache answers HTTP 501 NotImplemented. The service's own state.rs doc text already records this ("ModifyServerlessCache and the serverless snapshot operations — still scaffolded 501s"). No open issue covers this operation (searched: no issue mentions ModifyServerlessCache), so it is net-new.
Why: the serverless cache's only mutable knobs are unreachable. A client cannot change usage limits, description, snapshot retention, user group or security groups at all, and the rejected-modify-must-not-partially-apply guarantee (a live cache must never end up with a floor above its ceiling) cannot be observed. Even once Cause 1 is fixed this check cannot pass, because its second half drives ModifyServerlessCache.
Fix: implement handle() in services/service-com-amazonaws-elasticache-amazonelasticachev9/src/ops/modify_serverless_cache.rs over the existing ServerlessCacheRecord store, reusing crate::state::validate_cache_usage_limits for the refusal, and delete the SCAFFOLDED marker line.
Traces: elasticache.serverless-cache-usage-limits-inverted-rejected (its ModifyServerlessCache steps: inverted-ECPU refusal, limits-unchanged assertion, legal widening, and modify-after-delete 404).

## Evidence
Failing check elasticache.serverless-cache-usage-limits-inverted-rejected [mismatch]:

  File "check_elasticache_serverless_cache_usage_limits_inverted_rejected.py", line 157, in run
    _assert_error(lambda: ec.create_serverless_cache(... CacheUsageLimits={"DataStorage": {"Minimum": 50, "Maximum": 10, "Unit": "GB"}, "ECPUPerSecond": {"Minimum": 1000, "Maximum": 5000}}, SubnetIds=ctx.network().subnet_ids[:3]), ...)
  AssertionError: CreateServerlessCache with DataStorage Minimum=50 > Maximum=10: expected InvalidParameterValueException, got NotImplemented

Cause 1 — expected: HTTP 400, error code InvalidParameterValue / InvalidParameterValueException (modeled on CreateServerlessCache); message example "Invalid values for DataStorage. Minimum must not be greater than Maximum." Actual: error code NotImplemented (scaffold HTTP 501). The same divergence is expected for ECPUPerSecond Minimum=9000 > Maximum=1000, which the check asserts next.

Cause 2 — expected (per Smithy): 200 with a ServerlessCache body for a legal ModifyServerlessCache, 400 InvalidParameterValue for an inverted CacheUsageLimits with the live cache's limits byte-identical and Status still "available" afterwards, and 404 ServerlessCacheNotFoundFault for a cache already deleted. Actual: HTTP 501 NotImplemented for every ModifyServerlessCache — services/.../src/ops/modify_serverless_cache.rs line 4: "//! SCAFFOLDED, NOT IMPLEMENTED: this operation answers HTTP 501 `NotImplemented` and asserts NOTHING about `ModifyServerlessCache`'s real behaviour."

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay elasticache.serverless-cache-usage-limits-inverted-rejected.
2. Cause 1: call CreateServerlessCache with Engine=valkey, a fresh ServerlessCacheName, SubnetIds from the harness network fixture, and CacheUsageLimits={"DataStorage":{"Minimum":50,"Maximum":10,"Unit":"GB"},"ECPUPerSecond":{"Minimum":1000,"Maximum":5000}}. Observe error code NotImplemented (501) instead of 400 InvalidParameterValue. Repeat with ECPUPerSecond={"Minimum":9000,"Maximum":1000}. Capture which operation the 501 is emitted for (ElastiCache CreateServerlessCache vs. an EC2 operation reached while building SubnetIds) before changing code.
3. Cause 2: create a serverless cache with valid limits (DataStorage 1-10 GB, ECPUPerSecond 1000-5000), poll DescribeServerlessCaches to Status=available, then call ModifyServerlessCache with CacheUsageLimits={"ECPUPerSecond":{"Minimum":9000,"Maximum":1000}}. Observe HTTP 501 NotImplemented instead of 400 InvalidParameterValue, and then that a legal widening (DataStorage max 20, ECPU max 9000) also answers 501.
4. Delete the cache and call ModifyServerlessCache on the deleted name: observe 501 instead of 404 ServerlessCacheNotFoundFault.
