Issue-Source: trace-sweep-divergence
Issue-Trace: elasticache-serverless-cache-iam-user-endpoint-discovery
Issue-Step: setup#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elasticache-serverless-cache-iam-user-endpoint-discovery
Issue-Shape-Id: com.amazonaws.elasticache#AmazonElastiCacheV9
Issue-Key: elasticache|CreateUser|2xx->4xx|status
Issue-Key: elasticache|CreateUserGroup|2xx->4xx|$.CreateUserGroupResponse
Issue-Key: elasticache|CreateUserGroup|2xx->4xx|status
Issue-Key: elasticache|DescribeUserGroups|2xx->4xx|$
Issue-Key: elasticache|DescribeUserGroups|2xx->4xx|$.DescribeUserGroupsResponse
Issue-Key: elasticache|DescribeUserGroups|2xx->4xx|$.DescribeUserGroupsResponse.DescribeUserGroupsResult.UserGroups.member.Status
Issue-Key: elasticache|DescribeUserGroups|2xx->4xx|status
Issue-Key: elasticache|CreateServerlessCache|2xx->4xx|$.CreateServerlessCacheResponse
Issue-Key: elasticache|CreateServerlessCache|2xx->4xx|status
Issue-Key: elasticache|DeleteServerlessCache|2xx->4xx|status
Issue-Key: elasticache|DeleteUserGroup|2xx->4xx|status
Issue-Key: elasticache|DeleteUser|2xx->4xx|status
Issue-Commit: 622e0f603a4886f3f4d7e4980155df7d3d956f5b

# ElastiCache user, user-group and serverless-cache lifecycle collapses: a valid CreateUser is refused and every dependent step answers 404

> _Note: this issue was identified against the code base at commit `622e0f603a4886f3f4d7e4980155df7d3d956f5b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateUser

## Description
8 net-new wire causes on the elasticache RBAC / serverless-cache path. The replay marked none of these eight steps as owned by an open issue and none as blocked by an unimplemented-operation stub. Related open issues (20261108T030552Z-elasticache-createusergroup-misses-the-defaultuserrequired-refusal-and-rejects.md, 20261104T091523Z-elasticache-createserverlesscache-refuses-every-real-subnet-id-the.md, 20261109T151249Z-elasticache-serves-endpoints-it-does-not-listen-on.md) describe other refusals on these operations, but none of them covers a valid CreateUser being refused, none covers the user-group/serverless-cache operations answering 404 for resources the same session created, and none carries these fixtures' replay keys. Causes 2-8 are plainly downstream of cause 1, but each is a separate replayed divergence and each must be re-replayed before this issue is closed.

Cause 1 of 8: CreateUser refuses a request AWS accepts. AWS: Smithy - CreateUser input requires UserId (@length min 1, @pattern ^[a-zA-Z][a-zA-Z0-9\-]*$), UserName (@length min 1), Engine (@pattern ^[a-zA-Z]*$) and AccessString (@pattern \S), with optional Passwords (@length min 1), NoPasswordRequired and AuthenticationMode; output is the User shape (UserId, UserName, Status, Engine, MinimumEngineVersion, AccessString, UserGroupIds, Authentication, ARN). Its only modeled client refusals are DuplicateUserName, InvalidParameterCombination, InvalidParameterValue, ServiceLinkedRoleNotFoundFault, TagQuotaPerResourceExceeded, UserAlreadyExists and UserQuotaExceeded (all HTTP 400). Recorded behavior: for this request AWS answers HTTP 200 with the User body, so no modeled refusal applies - the request satisfies every input constraint above. Emulator: HTTP 400 ('status: expected 200, got 400'); no user is created. Why: a client that creates an ElastiCache RBAC user with a well-formed UserId/UserName/Engine/AccessString is told its input is invalid, so no RBAC user ever exists and every later step that references it fails. Fix: CreateUser's input validation in the elasticache operation handler - it must accept input that satisfies the modeled constraints and only raise the modeled 400s for the conditions they name. Traces: elasticache-serverless-cache-iam-user-endpoint-discovery#setup#6.

Cause 2 of 8: CreateUserGroup answers 404 for a user group AWS creates. AWS: Smithy - CreateUserGroup requires UserGroupId and Engine, takes UserIds (@length min 1), and returns the UserGroup shape (UserGroupId, Status, Engine, UserIds, MinimumEngineVersion, ReplicationGroups, ServerlessCaches, ARN); UserNotFoundFault is its only 404 (awsQueryError code UserNotFound). Recorded behavior: AWS answers HTTP 200 with Status 'creating', Engine 'valkey', UserIds listing both requested users and the usergroup ARN. Emulator: HTTP 404 - 'status: expected 200, got 404', '$.CreateUserGroupResponse: missing'. Why: a client cannot create a user group at all, so no RBAC authorization object can be attached to a cache. Fix: the CreateUserGroup handler and the user/user-group state in elasticache - a group whose members exist must be created and returned, not refused UserNotFound. Traces: elasticache-serverless-cache-iam-user-endpoint-discovery#task#8.

Cause 3 of 8: DescribeUserGroups never converges to Status 'active' and answers UserGroupNotFound instead. AWS: Smithy - DescribeUserGroups returns UserGroups; UserGroupNotFoundFault is awsQueryError code UserGroupNotFound, HTTP 404, raised only when the named group does not exist. Recorded behavior: polling the group created in the previous step returns HTTP 200 and the group settles at Status 'active'. Emulator: HTTP 404 with Error.Code 'UserGroupNotFound' and message 'User group <user-group-id> not found.' for the whole 180s poll window ('poll: not in [active] after 37 attempt(s)'); the fixed text there is 'User group ' and ' not found.' and the variable part is the UserGroupId from the request - the defect is that the refusal is raised at all for a group the same session created. Why: a client that waits for a user group to become active waits until its timeout and then aborts. Fix: the DescribeUserGroups handler's lookup plus the user-group state transition in elasticache. Traces: elasticache-serverless-cache-iam-user-endpoint-discovery#task#9.

Cause 4 of 8: CreateServerlessCache answers 404 when it names a user group. AWS: Smithy - CreateServerlessCache requires ServerlessCacheName and Engine and takes UserGroupId, SubnetIds, SecurityGroupIds, NetworkType and ConnectionType; it returns ServerlessCache, and UserGroupNotFoundFault (code UserGroupNotFound, HTTP 404) is raised only for a group that does not exist. Recorded behavior: AWS answers HTTP 200 with the ServerlessCache body (Status 'creating', Engine 'valkey', the requested UserGroupId, NetworkType 'ipv4', ConnectionType 'vpc', the requested SubnetIds and the serverlesscache ARN). Emulator: HTTP 404 - 'status: expected 200, got 404', '$.CreateServerlessCacheResponse: missing'. Why: a client cannot create an RBAC-protected serverless cache. Fix: CreateServerlessCache's user-group resolution in the elasticache handler and the serverless-cache state. Traces: elasticache-serverless-cache-iam-user-endpoint-discovery#task#10.

Cause 5 of 8: DescribeUserGroups does not report the serverless caches attached to a group. AWS: Smithy - the UserGroup output member ServerlessCaches targets UGServerlessCacheIdList. Recorded behavior: after the serverless cache is created, DescribeUserGroups answers HTTP 200 and the group carries ServerlessCaches listing that cache's name along with Status 'active'. Emulator: HTTP 404 - 'status: expected 200, got 404', '$.DescribeUserGroupsResponse: missing'. Why: a client cannot discover which caches a user group governs, so it cannot tell whether detaching or deleting the group is safe. Fix: the DescribeUserGroups handler and the back-reference from serverless-cache state to user-group state. Traces: elasticache-serverless-cache-iam-user-endpoint-discovery#task#13.

Cause 6 of 8: DeleteServerlessCache answers 404 for a cache that should exist. AWS: Smithy - DeleteServerlessCache returns the ServerlessCache being deleted; ServerlessCacheNotFoundFault is its 404. Recorded behavior: AWS answers HTTP 200 and moves the cache to a deleting state. Emulator: HTTP 404 - 'status: expected 200, got 404'. Why: teardown cannot delete the cache and a client's cleanup path fails, leaking the resource. Fix: the DeleteServerlessCache handler's lookup and the serverless-cache state in elasticache. Traces: elasticache-serverless-cache-iam-user-endpoint-discovery#teardown#15.

Cause 7 of 8: DeleteUserGroup answers 404 for a group that should exist. AWS: Smithy - DeleteUserGroup requires UserGroupId, returns the UserGroup shape, and raises UserGroupNotFoundFault (404) only for an unknown group. Recorded behavior: AWS answers HTTP 200 with the group in a deleting state. Emulator: HTTP 404 - 'status: expected 200, got 404'. Why: the user group cannot be deleted, so RBAC objects accumulate and cleanup never completes. Fix: the DeleteUserGroup handler's lookup and the user-group state. Traces: elasticache-serverless-cache-iam-user-endpoint-discovery#teardown#17.

Cause 8 of 8: DeleteUser answers 404 for a user that should exist. AWS: Smithy - DeleteUser requires UserId (@pattern ^[a-zA-Z][a-zA-Z0-9\-]*$), returns the User shape, and raises UserNotFoundFault (code UserNotFound, 404) only for an unknown user. Recorded behavior: AWS answers HTTP 200 with the user in a deleting state. Emulator: HTTP 404 - 'status: expected 200, got 404'. Why: a client cannot delete RBAC users, so its teardown fails and the user quota is consumed by resources it cannot remove. Fix: the DeleteUser handler's lookup and the user state in elasticache. Traces: elasticache-serverless-cache-iam-user-endpoint-discovery#teardown#20.

## Evidence
All quotes are from the wire replay of corpus simparity-wire-aws-v1 (verdict: mismatch on every step below). Resource names, ids and request ids in the recorded payloads are per-run values that fill AWS's templates; the statuses, error codes and the templates' fixed text are the requirement.

Cause 1 - fixture elasticache-serverless-cache-iam-user-endpoint-discovery, step setup#6, operation CreateUser; expected status 200, actual status 400:
  status: expected 200, got 400

Cause 2 - same fixture, step task#8, operation CreateUserGroup; expected 200, actual 404:
  status: expected 200, got 404
  $.CreateUserGroupResponse: missing (expected {'CreateUserGroupResult': {'UserGroupId': '<user-group-id>', 'MinimumEngineVersion': '7.2', 'ReplicationGroups': '', 'Engine': 'valkey', 'UserIds': {'member': ['<default-user-id>', '<iam-user-id>']}, 'ServerlessCaches': '', 'ARN': 'arn:aws:elasticache:<region>:<account-id>:usergroup:<user-group-id>', 'Status': 'creating'}}, got None)

Cause 3 - same fixture, step task#9, operation DescribeUserGroups; expected 200, actual 404:
  status: expected 200, got 404
  $.DescribeUserGroupsResponse.DescribeUserGroupsResult.UserGroups.member.Status: poll: not in ['active'] after 37 attempt(s) (expected 'active', got None)
  got {'ErrorResponse': {'Error': {'Type': 'Sender', 'Code': 'UserGroupNotFound', 'Message': 'User group <user-group-id> not found.'}}}

Cause 4 - same fixture, step task#10, operation CreateServerlessCache; expected 200, actual 404:
  status: expected 200, got 404
  $.CreateServerlessCacheResponse: missing (expected the ServerlessCache body with Status 'creating', Engine 'valkey', the requested UserGroupId/SubnetIds, NetworkType 'ipv4', ConnectionType 'vpc'), got None

Cause 5 - same fixture, step task#13, operation DescribeUserGroups; expected 200, actual 404:
  status: expected 200, got 404
  $.DescribeUserGroupsResponse: missing (expected the group with Status 'active' and ServerlessCaches {'member': '<serverless-cache-name>'}), got None

Cause 6 - same fixture, step teardown#15, operation DeleteServerlessCache; expected 200, actual 404:
  status: expected 200, got 404

Cause 7 - same fixture, step teardown#17, operation DeleteUserGroup; expected 200, actual 404:
  status: expected 200, got 404

Cause 8 - same fixture, step teardown#20, operation DeleteUser; expected 200, actual 404:
  status: expected 200, got 404

Smithy cited: CreateUser (required UserId/UserName/Engine/AccessString with the patterns above; errors DuplicateUserName, InvalidParameterCombination, InvalidParameterValue, ServiceLinkedRoleNotFoundFault, TagQuotaPerResourceExceeded, UserAlreadyExists, UserQuotaExceeded - all 400); CreateUserGroup (output UserGroup incl. ServerlessCaches/ReplicationGroups; UserNotFoundFault = UserNotFound, 404); CreateServerlessCache (UserGroupNotFoundFault = UserGroupNotFound, 404); DescribeUserGroups / DeleteUserGroup (UserGroupNotFoundFault, 404); DeleteUser (UserNotFoundFault, 404).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (git_clone, then replay_wire_corpus).
2. Cause 1: call elasticache CreateUser with a UserId matching ^[a-zA-Z][a-zA-Z0-9\-]*$, a UserName, Engine 'valkey', a non-blank AccessString and NoPasswordRequired/Passwords - observe HTTP 400 where AWS answers 200 with the User body. Replay fixture elasticache-serverless-cache-iam-user-endpoint-discovery step setup#6.
3. Cause 2: call CreateUserGroup with an Engine, a UserGroupId and UserIds naming existing users - observe HTTP 404 where AWS answers 200 with Status 'creating'. Step task#8.
4. Cause 3: poll DescribeUserGroups for that group - observe HTTP 404 Code 'UserGroupNotFound' for the whole window where AWS reaches Status 'active'. Step task#9.
5. Cause 4: call CreateServerlessCache naming that UserGroupId with valid SubnetIds - observe HTTP 404 where AWS answers 200 with the ServerlessCache body. Step task#10.
6. Cause 5: call DescribeUserGroups again after the cache exists - observe HTTP 404 where AWS answers 200 with ServerlessCaches listing the cache. Step task#13.
7. Causes 6-8: call DeleteServerlessCache, DeleteUserGroup and DeleteUser for those resources - observe HTTP 404 on each where AWS answers 200. Steps teardown#15, teardown#17, teardown#20.
