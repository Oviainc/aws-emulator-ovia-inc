Issue-Source: trace-sweep-divergence
Issue-Trace: elasticache-serverless-cache-iam-user-endpoint-discovery
Issue-Step: task#11
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elasticache-serverless-cache-iam-user-endpoint-discovery
Issue-Shape-Id: com.amazonaws.elasticache#AmazonElastiCacheV9
Issue-Key: elasticache|DescribeServerlessCaches|2xx->4xx|$
Issue-Key: elasticache|DescribeServerlessCaches|2xx->4xx|$.DescribeServerlessCachesResponse
Issue-Key: elasticache|DescribeServerlessCaches|2xx->4xx|$.DescribeServerlessCachesResponse.DescribeServerlessCachesResult.ServerlessCaches.member.Status
Issue-Key: elasticache|DescribeServerlessCaches|2xx->4xx|status
Issue-Commit: b11785bf9bf8af57a00ad7e4d814b1c5f1d66117

# ElastiCache DescribeServerlessCaches never converges: the serverless cache poll answers 404 ServerlessCacheNotFoundFault for the whole window

> _Note: this issue was identified against the code base at commit `b11785bf9bf8af57a00ad7e4d814b1c5f1d66117`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeServerlessCaches

## Description
1 net-new wire cause on the elasticache serverless-cache read path. The replay marked this step neither owned by an open issue nor blocked by an unimplemented-operation stub. The related open issue 20261109T204539Z-elasticache-user-user-group-and-serverless-cache-lifecycle.md covers the RBAC/serverless steps setup#6, task#8, task#9, task#10, task#13, teardown#15, teardown#17 and teardown#20 of this same fixture, and 20261104T091523Z-elasticache-createserverlesscache-refuses-every-real-subnet-id-the.md covers a CreateServerlessCache subnet refusal, but neither covers the DescribeServerlessCaches availability poll and neither carries this step's replay key, so it must be re-replayed on its own before this issue is closed.

Cause 1 of 1: DescribeServerlessCaches refuses a cache the same session created and never reaches an available status. AWS: Smithy - DescribeServerlessCaches takes ServerlessCacheName/MaxResults/NextToken and returns ServerlessCaches (list of ServerlessCache) plus NextToken; its only modeled refusals are InvalidParameterCombination (400), InvalidParameterValue (400) and ServerlessCacheNotFoundFault (awsQueryError code ServerlessCacheNotFoundFault, HTTP 404, documented as "The serverless cache was not found or does not exist."), so the 404 is correct only for a name that names no cache. The ServerlessCache shape documents Status with allowed values CREATING, AVAILABLE, DELETING, CREATE-FAILED and MODIFYING, and carries ServerlessCacheName, Engine, MajorEngineVersion, FullEngineVersion, Description, CacheUsageLimits, StorageEncryptionType, SecurityGroupIds, Endpoint, ReaderEndpoint, ARN, UserGroupId, SubnetIds, SnapshotRetentionLimit, DailySnapshotTime, NetworkType, ConnectionType and CreateTime. Recorded behavior: a client polls DescribeServerlessCaches for the cache created earlier in the same session and AWS answers HTTP 200 with that one cache settled at Status 'available', ConnectionType 'vpc', NetworkType 'ipv4', StorageEncryptionType 'sse-elasticache', the requested Engine/MajorEngineVersion/UserGroupId/SubnetIds, the serverlesscache ARN, and both Endpoint (port 6379) and ReaderEndpoint (port 6380) populated. Emulator: HTTP 404 with Error.Type 'Sender', Error.Code 'ServerlessCacheNotFoundFault' and message 'Serverless cache with name <serverless-cache-name> not found.' for the entire poll window - the replay reports 'status: expected 200, got 404' and '$: poll capped at 180s', with the whole $.DescribeServerlessCachesResponse body missing and $.DescribeServerlessCachesResponse.DescribeServerlessCachesResult.ServerlessCaches.member.Status never reaching 'available'. In that message the fixed text is 'Serverless cache with name ' and ' not found.' and the only variable part is the ServerlessCacheName taken from the request; the defect is that this refusal is raised at all for a cache the session just created, not its wording. Why: a client that creates a serverless cache and then waits for it to become available - the normal way to learn its Endpoint/ReaderEndpoint address before connecting - blocks until its own timeout and then aborts, and because no cache is ever readable the endpoint-discovery and IAM-user connection path downstream of it can never run. Fix: the DescribeServerlessCaches handler's name lookup in the elasticache crate plus the serverless-cache state it reads - a cache created by CreateServerlessCache must be registered in that state and must transition CREATING -> AVAILABLE so a poll converges, instead of the lookup missing and raising ServerlessCacheNotFoundFault. Traces: elasticache-serverless-cache-iam-user-endpoint-discovery#task#11.

## Evidence
All quotes are from the wire replay of corpus simparity-wire-aws-v1 (verdict: mismatch). Resource names, ids, request ids and timestamps in the recorded payload are per-run values that fill AWS's templates; the HTTP status, the error code and the template's fixed text are the requirement.

Cause 1 - fixture elasticache-serverless-cache-iam-user-endpoint-discovery, step task#11, operation DescribeServerlessCaches; expected status 200, actual status 404:
  status: expected 200, got 404
  $: poll capped at 180s (expected {'DescribeServerlessCachesResponse': {'DescribeServerlessCachesResult': {'ServerlessCaches': {'member': {'MajorEngineVersion': '9', 'FullEngineVersion': '9.0', 'Engine': 'valkey', 'Description': '', 'SnapshotRetentionLimit': '0', 'ConnectionType': 'vpc', 'UserGroupId': '<user-group-id>', 'Endpoint': {'Address': '<shaped-not-asserted>', 'Port': '6379'}, 'ReaderEndpoint': {'Address': '<shaped-not-asserted>', 'Port': '6380'}, 'SecurityGroupIds': {'SecurityGroupId': '<shaped-not-asserted>'}, 'ARN': 'arn:aws:elasticache:<region>:<account-id>:serverlesscache:<serverless-cache-name>', 'NetworkType': 'ipv4', 'SubnetIds': {'SubnetId': ['<subnet-id>', '<subnet-id>']}, 'ServerlessCacheName': '<serverless-cache-name>', 'StorageEncryptionType': 'sse-elasticache', 'CreateTime': '<shaped-not-asserted>', 'Status': 'available'}}}, 'ResponseMetadata': {'RequestId': '<shaped-not-asserted>'}}}, got {'ErrorResponse': {'Error': {'Type': 'Sender', 'Code': 'ServerlessCacheNotFoundFault', 'Message': 'Serverless cache with name <serverless-cache-name> not found.'}, 'RequestId': '<request-id>'}})
  $.DescribeServerlessCachesResponse: missing
  $.DescribeServerlessCachesResponse.DescribeServerlessCachesResult.ServerlessCaches.member.Status: never reached 'available'
  divergence_key: service=elasticache|DescribeServerlessCaches|status=200->404|keys=$,$.DescribeServerlessCachesResponse,$.DescribeServerlessCachesResponse.DescribeServerlessCachesResult.ServerlessCaches.member.Status,status

Smithy cited: DescribeServerlessCaches (input ServerlessCacheName/MaxResults/NextToken; output ServerlessCaches + NextToken; errors InvalidParameterCombination 400, InvalidParameterValue 400, ServerlessCacheNotFoundFault 404 - "The serverless cache was not found or does not exist."); ServerlessCache.Status allowed values CREATING, AVAILABLE, DELETING, CREATE-FAILED, MODIFYING; ServerlessCache members Endpoint/ReaderEndpoint/ARN/UserGroupId/SubnetIds/StorageEncryptionType/NetworkType/ConnectionType; CreateServerlessCache output ServerlessCache.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (git_clone, then replay_wire_corpus).
2. Create an RBAC user group and then call elasticache CreateServerlessCache with a ServerlessCacheName, Engine 'valkey', MajorEngineVersion '9', that UserGroupId and valid SubnetIds/SecurityGroupIds.
3. Poll DescribeServerlessCaches with ServerlessCacheName set to that name, waiting for ServerlessCaches.member.Status to become 'available'.
4. Observe HTTP 404 with Error.Code 'ServerlessCacheNotFoundFault' and message 'Serverless cache with name <name> not found.' on every attempt for the full 180s window, where AWS answers HTTP 200 with the single cache at Status 'available' and both Endpoint (port 6379) and ReaderEndpoint (port 6380) populated.
5. Replay fixture elasticache-serverless-cache-iam-user-endpoint-discovery step task#11 to confirm.
