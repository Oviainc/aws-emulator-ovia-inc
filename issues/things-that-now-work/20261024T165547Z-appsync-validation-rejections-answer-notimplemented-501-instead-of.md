Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync.api-cache-ttl-out-of-range-rejected appsync.create-data-source-missing-iam-role-rejected
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync.api-cache-ttl-out-of-range-rejected|run#5|AssertionError|assert code == "BadRequestException", f"got {code!r}"
Issue-Key: appsync.create-data-source-missing-iam-role-rejected|run#9|AssertionError|assert code == "BadRequestException", f"expected BadRequestException, got {code}"
Issue-Commit: 2a22026c0f737b6187455fb636429eae77d015a7

# AppSync validation rejections answer NotImplemented (501) instead of BadRequestException (400): CreateApiCache out-of-range ttl and CreateDataSource missing serviceRoleArn

> _Note: this issue was identified against the code base at commit `2a22026c0f737b6187455fb636429eae77d015a7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApiCache

## Description
Two AppSync control-plane requests that real AWS REFUSES with a modeled 400 BadRequestException are instead answered by the emulator with `NotImplemented` (HTTP 501, "operation <X> is scaffolded but not implemented in this emulator"), so clients cannot distinguish a rejected request from an absent service. Both handlers are still scaffold stubs. Related open issues 20261024T032443Z-appsync-createapicache-stub.md and 20261024T032443Z-appsync-createdatasource-stub.md ask for those two operations to be implemented and carry OTHER checks' replay keys, but neither records the two REJECTION contracts below nor the two check ids in this issue, which is what this issue adds; close this issue only when both rejection paths below behave as described.

Cause 1 of 2: CreateApiCache does not enforce the documented ttl window.
AWS: Smithy `CreateApiCacheRequest.ttl` is documented "TTL in seconds for cache entries. Valid values are 1\u20133,600 seconds"; CreateApiCache models `BadRequestException` with httpError 400 (POST /v1/apis/{apiId}/ApiCaches). A ttl outside 1..=3600 (recorded: ttl=3601, apiCachingBehavior=FULL_REQUEST_CACHING, type=SMALL) is refused with 400 `BadRequestException`; recorded message, as an example of the rule "the refusal names the 1\u20133600 second window": "TTL value cannot be smaller than 1 second and larger than 3600 seconds." The refusal must leave NO cache behind, so a following GetApiCache returns 404 `NotFoundException` (recorded message example: "Unable to get the cache as it doesn't exist, please create the cache first.").
Emulator: the CreateApiCache call returns error code `NotImplemented` instead of `BadRequestException` \u2014 the check asserts the error code and gets 'NotImplemented'.
Why: a client that provisions a cache and branches on BadRequestException to correct an operator-supplied ttl sees an unmodeled server-side 501 instead, so it retries or aborts rather than fixing the input; and because nothing is persisted or validated, the "rejected create leaves no ApiCache" invariant is untestable.
Fix: the CreateApiCache handler and its input validation in the appsync service crate (ops/create_api_cache.rs), plus the ApiCache state it writes, and GetApiCache's not-found path.
Traces: appsync.api-cache-ttl-out-of-range-rejected.

Cause 2 of 2: CreateDataSource does not reject a role-requiring data source type with no serviceRoleArn.
AWS: Smithy CreateDataSource (POST /v1/apis/{apiId}/datasources) takes `type` (required, DataSourceType) and an OPTIONAL `serviceRoleArn`, and models `BadRequestException` httpError 400. For a type that AppSync must assume a role to invoke (recorded: type=AWS_LAMBDA with lambdaConfig and no serviceRoleArn) the create is refused with 400 `BadRequestException` rather than creating a data source AppSync could never invoke; recorded message example: "Service role arn not specified." The rejection is effect-free: ListDataSources does not contain the name and GetDataSource for it returns 404 `NotFoundException` (recorded message example: "Data source <name> not found"). The identical configuration WITH a valid serviceRoleArn succeeds.
Emulator: the CreateDataSource call returns error code `NotImplemented` instead of `BadRequestException`.
Why: a client wiring up a Lambda data source gets no validation signal at create time; on real AWS the misconfiguration is caught synchronously with a 400, while here the call fails as a server error, so IaC/CLI flows mis-classify an input error as an outage and the "no partial resource" guarantee is unverifiable.
Fix: the CreateDataSource handler and its per-type required-field validation in the appsync service crate (ops/create_data_source.rs), the DataSource state it writes, and GetDataSource/ListDataSources not-found behavior.
Traces: appsync.create-data-source-missing-iam-role-rejected.

## Evidence
Cause 1 \u2014 appsync.api-cache-ttl-out-of-range-rejected (verdict: mismatch):
  expected: error code "BadRequestException" (HTTP 400) from create_api_cache(apiId=<api>, ttl=3601, apiCachingBehavior="FULL_REQUEST_CACHING", type="SMALL"), then get_api_cache -> "NotFoundException"
  actual:   AssertionError: got 'NotImplemented'
            (assert code == "BadRequestException", f"got {code!r}" at check_appsync_api_cache_ttl_out_of_range_rejected.py:42)
  Smithy: CreateApiCacheRequest.ttl doc "Valid values are 1\u20133,600 seconds"; CreateApiCache errors include BadRequestException (400) and NotFoundException (404).

Cause 2 \u2014 appsync.create-data-source-missing-iam-role-rejected (verdict: mismatch):
  expected: error code "BadRequestException" (HTTP 400) from create_data_source(apiId=<api>, name=<ds>, type="AWS_LAMBDA", lambdaConfig={...}) with serviceRoleArn omitted, then the name absent from list_data_sources and get_data_source -> "NotFoundException"
  actual:   AssertionError: expected BadRequestException, got NotImplemented
            (check_appsync_create_data_source_missing_iam_role_rejected.py:45)
  Smithy: CreateDataSource errors include BadRequestException (400), NotFoundException (404); serviceRoleArn is an optional member, so the per-type requirement is enforced by the handler.

Both operations are still scaffold stubs in services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice/src/ops/ (create_api_cache.rs, create_data_source.rs), which is why every request to them \u2014 valid or invalid \u2014 comes back NotImplemented.

## Steps to Reproduce
1. Boot the emulator and point an AppSync client at it.
2. Cause 1: create_graphql_api(name=<unique>, authenticationType="API_KEY"); then create_api_cache(apiId=<apiId>, ttl=3601, apiCachingBehavior="FULL_REQUEST_CACHING", type="SMALL"). Observe the error code is NotImplemented (501) where AWS answers BadRequestException (400) naming the 1\u20133600s window. Then get_api_cache(apiId=<apiId>) and observe it does not answer NotFoundException (404).
3. Cause 2: create_graphql_api(name=<unique>, authenticationType="API_KEY"); then create_data_source(apiId=<apiId>, name="bad_ds", type="AWS_LAMBDA", lambdaConfig={"lambdaFunctionArn": "arn:aws:lambda:us-east-1:123456789012:function:does-not-exist"}) with no serviceRoleArn. Observe NotImplemented (501) where AWS answers BadRequestException (400); then list_data_sources / get_data_source(name="bad_ds") and observe the no-leftover-resource invariant cannot be confirmed.
4. Replay appsync.api-cache-ttl-out-of-range-rejected and appsync.create-data-source-missing-iam-role-rejected.
