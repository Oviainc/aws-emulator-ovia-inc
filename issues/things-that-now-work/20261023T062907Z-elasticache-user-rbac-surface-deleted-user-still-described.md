Issue-Source: trace-sweep-divergence
Issue-Trace: elasticache-user-and-user-group-access-control
Issue-Step: task#10
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elasticache-user-and-user-group-access-control elasticache-user-and-user-group-rbac
Issue-Shape-Id: com.amazonaws.elasticache#AmazonElastiCacheV9
Issue-Key: elasticache|DescribeUsers|4xx->2xx|$
Issue-Key: elasticache|DescribeUsers|4xx->2xx|$.ErrorResponse
Issue-Key: elasticache|DescribeUsers|4xx->2xx|status
Issue-Key: elasticache|CreateUser|2xx->2xx|$.CreateUserResponse.CreateUserResult.AccessString
Issue-Key: elasticache|DescribeUsers|2xx->2xx|$.DescribeUsersResponse.DescribeUsersResult.Users.member.AccessString
Issue-Key: elasticache|CreateUser|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: elasticache|DescribeUsers|4xx->4xx|$.ErrorResponse.RequestId
Issue-Commit: c0919b0ae1fd753648687abf4b08014f282092d3

# ElastiCache user RBAC surface: deleted user still described, AccessString not normalized, error RequestId missing

> _Note: this issue was identified against the code base at commit `c0919b0ae1fd753648687abf4b08014f282092d3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeUsers

## Description
Five divergences on the elasticache user/user-group (RBAC) surface: a deleted user is never observed as gone, the stored AccessString is not normalized the way the service normalizes it, and the error envelope again omits RequestId. Each cause is separate and must be closed separately. (replay: state_reset_failures=0.)

Cause 1 of 5: after DeleteUser, DescribeUsers keeps returning the user — the poll for UserNotFound never converges.
AWS: Smithy: DescribeUsers models UserNotFoundFault (awsQueryError code `UserNotFound`, HTTP 404). Recorded behavior after the user is deleted is HTTP 404 with Error.Type `Sender`, Error.Code `UserNotFound`; recorded example message 'User <UserId> not found.'
Emulator: answers HTTP 200 with the full User body (Status 'active', its ARN, Authentication{Type password}) for the whole poll window — the replay capped at 60s after 7 attempts and never saw an error code.
Why: the delete is not reflected in state, so a client cannot tell a user was removed; teardown loops hang, the user id/name stays occupied, and an "is this credential revoked?" check answers 'active' for a deleted user — a security-relevant wrong answer.
Fix: the DeleteUser handler's state mutation plus the DescribeUsers lookup/not-found path in the user state store.
Traces: elasticache-user-and-user-group-access-control#task#10.

Cause 2 of 5: CreateUser does not normalize AccessString — the deny-all prefix is dropped.
AWS: Smithy: CreateUser requires AccessString (pattern \\S) and returns it in the User output. Recorded behavior: the service normalizes the submitted access string and echoes the expanded form, which carries the explicit deny-all term before the granted categories — recorded 'on ~sim2real:* -@all +@read +@write'.
Emulator: echoes 'on ~sim2real:* +@read +@write', i.e. the deny-all term is missing from the normalized value.
Why: the access string is the permission document. Without the leading -@all the expanded rule set is strictly more permissive than what AWS would have stored, so a client that reads back the AccessString to audit or to re-apply a user's permissions propagates a broader grant than the service's.
Fix: AccessString normalization in the CreateUser handler (shared with ModifyUser) before the user is stored and serialized.
Traces: elasticache-user-and-user-group-access-control#task#1.

Cause 3 of 5: DescribeUsers returns the same un-normalized AccessString.
AWS: recorded behavior — DescribeUsers reports the stored, normalized access string 'on ~sim2real:* -@all +@read +@write' for that user.
Emulator: reports 'on ~sim2real:* +@read +@write'.
Why: the read path confirms the wrong permission document, so the divergence of Cause 2 is observable on every subsequent audit, not just at create time.
Fix: follows the Cause 2 normalization (store the normalized form); the DescribeUsers serializer must report what the service would have stored.
Traces: elasticache-user-and-user-group-access-control#task#8.

Cause 4 of 5: CreateUser validation error omits RequestId from the awsQuery error envelope.
AWS: Smithy: CreateUser binds InvalidParameterValueException/UserAlreadyExistsFault/DuplicateUserNameFault (HTTP 400). The awsQuery fault shape is <ErrorResponse><Error>{Type,Code,Message}</Error><RequestId>...</RequestId></ErrorResponse>; the recording carries a RequestId.
Emulator: status 400 and Error.Code match, but $.ErrorResponse.RequestId is absent.
Why: the error envelope is not the protocol's shape, and SDK/clients lose the request id they surface for correlation.
Fix: the shared awsQuery error serializer for elasticache (same serializer as the parameter-group/snapshot issue).
Traces: elasticache-user-and-user-group-access-control#task#2.

Cause 5 of 5: DescribeUsers UserNotFound error omits RequestId.
AWS: Smithy: DescribeUsers models UserNotFoundFault (code `UserNotFound`, HTTP 404); recorded behavior is HTTP 404 with that code and a RequestId element.
Emulator: status 404 and code match; $.ErrorResponse.RequestId is absent.
Why: as Cause 4.
Fix: same shared awsQuery error serializer.
Traces: elasticache-user-and-user-group-rbac#task#2.

Note: open issue 20261022T193451Z-elasticache-describeusers-stub.md asks for DescribeUsers to be implemented; the operation now answers (these replays got 200/404 bodies, not NotImplemented) and that issue carries no wire replay key for these steps, so it does not cover the five causes above.

## Evidence
Replay of trace set simparity-wire-aws-v1, all clusters verdict=mismatch, stub_path empty, owned_by empty:
1. elasticache-user-and-user-group-access-control#task#10 DescribeUsers after DeleteUser — expected HTTP 404 {Error:{Type:Sender, Code:UserNotFound, Message:'User <id> not found.'}}; got HTTP 200 with DescribeUsersResult.Users.member{Status:'active', ARN: arn:aws:elasticache:...:user:<id>}; replay reports 'poll capped at 60s' and 'error code None not in [UserNotFound] after 7 attempt(s)'.
2. elasticache-user-and-user-group-access-control#task#1 CreateUser — $.CreateUserResponse.CreateUserResult.AccessString expected 'on ~sim2real:* -@all +@read +@write', got 'on ~sim2real:* +@read +@write' (HTTP 200 both sides).
3. elasticache-user-and-user-group-access-control#task#8 DescribeUsers — $.DescribeUsersResponse.DescribeUsersResult.Users.member.AccessString expected 'on ~sim2real:* -@all +@read +@write', got 'on ~sim2real:* +@read +@write'.
4. elasticache-user-and-user-group-access-control#task#2 CreateUser (invalid input) — 400 both sides; $.ErrorResponse.RequestId missing (expected present, got None).
5. elasticache-user-and-user-group-rbac#task#2 DescribeUsers (absent user) — 404 both sides; $.ErrorResponse.RequestId missing.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Cause 1: replay elasticache-user-and-user-group-access-control through step task#10 — CreateUser, DeleteUser, then poll DescribeUsers(UserId=<deleted id>) expecting HTTP 404 UserNotFound; observe HTTP 200 with Status 'active' until the poll caps.
3. Causes 2,3: steps task#1 and task#8 — CreateUser with AccessString 'on ~<keypattern> +@read +@write' and then DescribeUsers; compare the echoed AccessString against the service's normalized form that includes the deny-all term.
4. Causes 4,5: steps task#2 (CreateUser rejected with 400) and elasticache-user-and-user-group-rbac#task#2 (DescribeUsers for an absent user, 404); inspect the raw XML for a <RequestId> child of <ErrorResponse>.
5. Confirm the fix by re-replaying every step named in Traces.
