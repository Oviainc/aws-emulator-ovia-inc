Issue-Source: trace-sweep-divergence
Issue-Trace: elasticache-error-user-group-missing-default
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elasticache-error-user-group-missing-default
Issue-Shape-Id: com.amazonaws.elasticache#AmazonElastiCacheV9
Issue-Key: elasticache|CreateUserGroup|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: elasticache|CreateUserGroup|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Commit: 09ea2d6b5347c41b77d17c7337847f8c3ab13f71

# ElastiCache CreateUserGroup misses the DefaultUserRequired refusal and rejects a valid UserGroupId on length

> _Note: this issue was identified against the code base at commit `09ea2d6b5347c41b77d17c7337847f8c3ab13f71`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateUserGroup

## Description
One root cause on the IMPLEMENTED ElastiCache user-group creation path: CreateUserGroup never reaches its modeled DefaultUserRequired refusal, because an invented UserGroupId length gate fires first and refuses a request AWS accepts past that point. The emulator answers HTTP 400 as AWS does, but with a different awsQuery Error/Code and a message AWS never emits for this request class.

Cause 1 of 1: a UserIds list that contains no user named 'default' must be refused with DefaultUserRequired; the emulator instead refuses the request with InvalidParameterValue and a UserGroupId-length message.
AWS: Smithy (services/service-com-amazonaws-elasticache-amazonelasticachev9/smithy-model/com-amazonaws-elasticache-amazonelasticachev9.json, service AmazonElastiCacheV9, API version 2015-02-02, protocol awsQuery): CreateUserGroup input requires UserGroupId (String) and Engine (EngineType, pattern ^[a-zA-Z]*$) and takes UserIds (UserIdListInput, @length min 1). CreateUserGroup models the error DefaultUserRequired -> awsQueryError {code 'DefaultUserRequired', httpResponseCode 400}, documented as 'You must add default user to a user group.' CreateUserGroupMessage.UserGroupId is documented only as 'The ID of the user group. This value is stored as a lowercase string.' and carries NO modeled @length constraint, so an ID AWS accepts must not be refused on length. The recorded AWS response for a request whose UserIds name no 'default' user is HTTP 400 with Error/Code 'DefaultUserRequired' and Error/Message 'Redis user group needs to contain a user with the user name default.' That whole sentence is fixed text for this error class: AWS interpolates nothing from the request or from state into it (no user group id, no user id, no limit).
Emulator: HTTP 400 with Error/Code 'InvalidParameterValue' and Error/Message 'User group id length must be less than 40.' — a different modeled error (InvalidParameterValueException -> awsQuery code 'InvalidParameterValue') carrying a length-rule sentence whose fixed text ('User group id length must be less than ') and variable part (the 40 limit) describe a constraint the model does not state, applied to a UserGroupId AWS accepted for validation purposes. The DefaultUserRequired check appears not to be implemented at all: the request is refused before the UserIds membership rule is ever evaluated.
Why: a client that branches on the error code cannot handle this refusal. botocore builds client.exceptions.DefaultUserRequired from the code 'DefaultUserRequired', so `except client.exceptions.DefaultUserRequired:` (the natural way to detect 'add a default user and retry') never fires, and the request instead surfaces as InvalidParameterValueException — which a caller reasonably treats as a malformed UserGroupId and 'fixes' by renaming the group, producing the same refusal forever. The displayed message also tells the user the wrong thing: it blames the group id's length instead of the missing default user, and it asserts a 40-character ceiling that is not part of the API contract, so a conforming request can be rejected outright.
Fix: the CreateUserGroup handler's input validation in the elasticache crate — implement the DefaultUserRequired rule (refuse when the resolved UserIds contain no user whose UserName is 'default', with AWS's fixed message text) and remove/recalibrate the unmodeled UserGroupId length gate so it does not pre-empt it for ids AWS accepts.
Traces: elasticache-error-user-group-missing-default#task#1 (cluster of 1).

Relation to open issues: 20261102T083323Z-elasticache-refusals-carry-the-wrong-error-code-and.md covers seven OTHER ElastiCache refusal paths (DescribeCacheClusters/DescribeReplicationGroups identifier text, DescribeCacheSubnetGroups code and name echo, CreateCacheSubnetGroup required-member, CreateReplicationGroup AuthToken) and 20261105T143748Z-elasticache-teardown-refusals-diverge-from-aws-a-valid.md covers teardown refusals; neither owns this fixture step (the replay report lists owned_by as empty for it), so this cause is net-new.

## Evidence
Replay of wire trace set simparity-wire-aws-v1 against this checkout (26 fixtures replayed, 227 steps, 153 passed, 74 mismatched, score 67.4%, emulator_up true).

Fixture elasticache-error-user-group-missing-default, step task#1, operation CreateUserGroup, verdict mismatch, cluster of 1 (affected: elasticache-error-user-group-missing-default#task#1).
HTTP status: expected 400, actual 400 (the status is correct).
$.ErrorResponse.Error.Code: expected 'DefaultUserRequired', got 'InvalidParameterValue'.
$.ErrorResponse.Error.Message: expected 'Redis user group needs to contain a user with the user name default.', got 'User group id length must be less than 40.'

Model evidence (services/service-com-amazonaws-elasticache-amazonelasticachev9/smithy-model/com-amazonaws-elasticache-amazonelasticachev9.json): CreateUserGroup errors include DefaultUserRequired (errorKind client, httpError 400, awsQueryError code 'DefaultUserRequired', httpResponseCode 400) with documentation 'You must add default user to a user group.', alongside InvalidParameterValueException (awsQueryError code 'InvalidParameterValue', 400), UserNotFoundFault (code 'UserNotFound', 404), DuplicateUserNameFault, UserGroupAlreadyExistsFault and UserGroupQuotaExceededFault. CreateUserGroupMessage members: UserGroupId (String, required, no length/pattern constraint), Engine (required, pattern ^[a-zA-Z]*$), UserIds (list, @length min 1), Tags. Nothing in the model imposes a 40-character ceiling on UserGroupId.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay wire trace set simparity-wire-aws-v1.
2. Call elasticache CreateUserGroup with a valid UserGroupId, Engine='redis' and a UserIds list that contains only non-default users (no user whose UserName is 'default').
3. Expect HTTP 400 with $.ErrorResponse.Error.Code 'DefaultUserRequired' and $.ErrorResponse.Error.Message 'Redis user group needs to contain a user with the user name default.'
4. Observe HTTP 400 with Error/Code 'InvalidParameterValue' and Error/Message 'User group id length must be less than 40.' — the unmodeled UserGroupId length gate refuses the request before the missing-default-user rule is evaluated.
5. Re-replay elasticache-error-user-group-missing-default#task#1 to confirm the fix.
