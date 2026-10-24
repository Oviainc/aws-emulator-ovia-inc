Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elasticache.modify-user-conflicting-auth-params-rejected elasticache.tag-unicode-value-round-trip elasticache.cache-parameter-numeric-precision-round-trip
Issue-Shape-Id: com.amazonaws.elasticache#AmazonElastiCacheV9
Issue-Key: elasticache|CreateUser|InvalidParameterValue
Issue-Key: elasticache.tag-unicode-value-round-trip|run#12|AssertionError|raise AssertionError("AddTagsToResource with an emoji tag value should have been rejected")
Issue-Key: elasticache.cache-parameter-numeric-precision-round-trip|run#9|AssertionError|raise AssertionError(
Issue-Commit: 1f316ce023d643c000ad0b8fb7b5799232d706fd

# ElastiCache validation gaps: a 40-character UserId is refused, tag values accept characters AWS rejects, and the redis7 parameter catalog publishes no numeric ranges

> _Note: this issue was identified against the code base at commit `1f316ce023d643c000ad0b8fb7b5799232d706fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateUser

## Description
Three independent root causes in the elasticache emulator's validation and catalog surface, found by replaying the behavioral suite against this checkout. Each cause is separate and must be closed separately.

Cause 1 of 3: CreateUser refuses a UserId of exactly 40 characters.
AWS: Smithy: CreateUser.UserId -> UserId, @length(min: 1) and @pattern ^[a-zA-Z][a-zA-Z0-9\-]*$ ONLY — the model states no upper bound, so the bound is server-side. ElastiCache documents the id as 1-40 characters, i.e. 40 is ACCEPTED and 41 is refused; recorded behavior: the check creates a user whose id is exactly 40 characters (prefix 'sim2real-behav-ecu-conflict-' plus the harness's 12-character suffix) and CreateUser returns 200 with Authentication.Type 'password'.
Emulator: answers HTTP 400 InvalidParameterValue, message 'User id length must be less than 40.' (example), because services/service-com-amazonaws-elasticache-amazonelasticachev9/src/state.rs::user_id_rejection refuses `count >= USER_ID_LENGTH_LIMIT (40)` instead of `> 40`. The refusal is applied by every operation that NAMES a user, so the same off-by-one also makes DeleteUser/DescribeUsers unable to address a legally-named user.
Why: a client whose naming convention produces ids at the documented maximum cannot create an RBAC user at all, and the whole ModifyUser conflict/precedence behavior behind it is unreachable; the error also misattributes a legal request to the caller.
Fix: the shared id validator in the elasticache state module (user_id_rejection) that CreateUser's handler calls, so the bound is inclusive of 40.
Traces: elasticache.modify-user-conflicting-auth-params-rejected (fails on the very first CreateUser call, before it can exercise the ModifyUser leg it is about).

Cause 2 of 3: AddTagsToResource accepts a tag value outside ElastiCache's permitted character set.
AWS: Smithy: Tag$Value carries no @length and no @pattern, so botocore validates nothing and the rule is server-side. Recorded behavior: AddTagsToResource with a tag value containing an emoji is REJECTED with HTTP 400, error code InvalidParameterValue, and the service's charset message (example: 'Tag values may only contain unicode letters, digits, whitespace, or one of these symbols: _ . : / = + \\ - @"'); the tag is not applied. The requirement is the 400/InvalidParameterValue refusal of a value outside {unicode letters, digits, whitespace, _ . : / = + \ - @}, not the message text.
Emulator: returns HTTP 200 and stores the tag — state.rs::tag_value_rejection bounds only the length (MAX_TAG_VALUE_LEN 256) and applies no character class, so the emulator is over-permissive on every tagging entry point (create Tags, AddTagsToResource).
Why: a client that relies on the service to reject malformed tag values stores values on the emulator that real AWS refuses, so code validated here fails in production; cost-allocation and policy tooling then sees tag values that could never exist.
Fix: the tag-value validator in the elasticache state module (tag_value_rejection, shared by tag_pairs and the AddTagsToResource handler).
Traces: elasticache.tag-unicode-value-round-trip (the CJK/Cyrillic/accented values round-trip correctly; only the emoji-rejection leg fails).

Cause 3 of 3: DescribeCacheParameters on a redis7 family group publishes no numeric AllowedValues, so no modifiable integer parameter exposes its range.
AWS: Smithy: DescribeCacheParameters -> CacheParameterGroupDetails.Parameters[] with ParameterName/ParameterValue/DataType/AllowedValues/IsModifiable/MinimumEngineVersion/ChangeType, paginated on Marker. Recorded behavior (same catalog the already-filed wire issue 20261023T062907Z-elasticache-catalog-describes-serve-synthesized-data-instead-of.md, Cause 2, records for DescribeEngineDefaultParameters): the family's published catalog returns AllowedValues populated for its integer parameters, including closed ranges of the form '<lo>-<hi>' spanning thousands of values, for modifiable parameters.
Emulator: every integer row of the published parameter catalog carries an EMPTY AllowedValues (the ParameterRow table in state.rs leaves the allowed-values column "" for maxmemory-samples, slowlog-max-len, tcp-keepalive, min-replicas-*, zset-max-ziplist-*, ... and the only non-empty integer form is 'timeout' => '0,20-'), so a client scanning DescribeCacheParameters for a modifiable integer parameter with a numeric range finds none and cannot discover what it may set.
Why: a client that reads the parameter catalog to decide which parameter to tune, and to what, has no range to validate against; parameter-tuning flows driven by discovery cannot run at all.
Fix: the published engine-default parameter catalog table behind DescribeCacheParameters / DescribeEngineDefaultParameters in the elasticache state module (same table as Cause 2 of 20261023T062907Z-elasticache-catalog-describes-serve-synthesized-data-instead-of.md — that issue is a wire finding about DescribeEngineDefaultParameters/DescribeCacheEngineVersions and does not carry this check; closing both together is expected). Note the write side, ModifyCacheParameterGroup, is separately filed as a scaffold stub (20261023T155010Z-elasticache-modifycacheparametergroup-stub.md), so this check needs both.
Traces: elasticache.cache-parameter-numeric-precision-round-trip.

## Evidence
Behavioral suite replay against this checkout (simparity-behavioral-ga-prod-v1), 13/99 passed.

Cause 1 — [error] elasticache.modify-user-conflicting-auth-params-rejected:
  botocore.errorfactory.InvalidParameterValueException: An error occurred (InvalidParameterValue) when calling the CreateUser operation: User id length must be less than 40.
  The call under test: create_user(UserId=<40 chars>, UserName=<same>, Engine='redis', AccessString='on ~* +@all', Passwords=[...]) — expected HTTP 200 with Authentication{Type: 'password', PasswordCount: 1}.
  Code: state.rs::user_id_rejection -> `(user_id.chars().count() >= USER_ID_LENGTH_LIMIT)` with `USER_ID_LENGTH_LIMIT = 40`.

Cause 2 — [mismatch] elasticache.tag-unicode-value-round-trip:
  AssertionError: AddTagsToResource with an emoji tag value should have been rejected
  The call: add_tags_to_resource(ResourceName=<parametergroup ARN>, Tags=[{'Key': 'emoji-tag', 'Value': 'party-<emoji>-time'}]) — expected HTTP 400 / InvalidParameterValue; got HTTP 200 and the tag persisted.
  Code: state.rs::tag_value_rejection checks only `chars().count() > MAX_TAG_VALUE_LEN`.

Cause 3 — [mismatch] elasticache.cache-parameter-numeric-precision-round-trip:
  AssertionError: no modifiable integer parameter with a wide numeric range was found in the redis7 CacheParameterGroupDetails; cannot exercise a numeric precision round-trip
  The check creates a custom redis7 cache parameter group, calls DescribeCacheParameters and scans for IsModifiable=true, DataType='integer', AllowedValues matching ^(\d+)-(\d+)$ with span >= 1000; zero rows qualify.
  Code: the ParameterRow catalog in state.rs supplies "" as AllowedValues for its integer rows.

## Steps to Reproduce
1. Boot the emulator from this checkout and point an elasticache client at it.
2. Cause 1: CreateUser with a UserId of exactly 40 characters matching ^[a-zA-Z][a-zA-Z0-9-]*$ (e.g. 'sim2real-behav-ecu-conflict-' + 12 chars), UserName the same, Engine='redis', AccessString='on ~* +@all', Passwords=['SimToRealConflictPass-One-2026']. Expect 200; the emulator returns 400 InvalidParameterValue. Re-run with 41 characters: that one must still be refused.
3. Cause 2: CreateCacheParameterGroup (family redis7), take its ARN, then AddTagsToResource with Tags=[{Key:'emoji-tag', Value:'party-\U0001F389-time'}]. Expect 400 InvalidParameterValue and no tag applied; the emulator returns 200 and ListTagsForResource shows the tag.
4. Cause 3: CreateCacheParameterGroup (family redis7) then DescribeCacheParameters on it and look for a modifiable integer parameter whose AllowedValues is a closed '<lo>-<hi>' range; the emulator publishes none.
5. Confirm the fix by re-replaying the checks named in Traces: elasticache.modify-user-conflicting-auth-params-rejected, elasticache.tag-unicode-value-round-trip, elasticache.cache-parameter-numeric-precision-round-trip.
