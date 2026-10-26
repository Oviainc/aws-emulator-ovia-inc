Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elasticache.remove-default-user-from-group-rejected
Issue-Shape-Id: com.amazonaws.elasticache#AmazonElastiCacheV9
Issue-Key: elasticache.remove-default-user-from-group-rejected|run#12|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Commit: 69d9d2e12a458c911688cbf85bce3839498ad470

# ElastiCache ModifyUserGroup: the DefaultUserRequired refusal carries a paraphrased message instead of the service's sentence (1 cause)

> _Note: this issue was identified against the code base at commit `69d9d2e12a458c911688cbf85bce3839498ad470`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyUserGroup

## Description
Cause 1 of 1: ModifyUserGroup refuses the removal of the mandatory `default` user with the right error code but the wrong message text.

AWS: ModifyUserGroup models `DefaultUserRequired` (Smithy: ModifyUserGroup errors include DefaultUserRequired, errorKind client, httpError 400, awsQueryError code `DefaultUserRequired`/400; shape documentation "You must add default user to a user group."). For a call that would leave the group without a member whose user name is `default`, the recorded AWS response is HTTP 400, Error.Code `DefaultUserRequired`, Error.Message exactly `Redis user group needs to contain a user with the user name default.` — that whole sentence is FIXED text for this error class: it contains no request-supplied value, no group id, no user id and no timestamp, so there is no variable part to fill in from the request or from emulator state. The model does not prescribe the sentence, so the wording above is recorded behavior only; the status and code are modeled.

Emulator: the same call is refused with the right status and code but the message `A user group must contain the default user.` — a paraphrase built in the handler (`format!("A user group must contain the {} user.", crate::state::DEFAULT_USER_ID)`), interpolating the stored default-user id into an invented sentence. Replay: `AssertionError: expected message 'Redis user group needs to contain a user with the user name default.', got 'A user group must contain the default user.'`. The rest of the check's contract is met: the code is `DefaultUserRequired`, the membership is left unchanged (`default` and the non-default member both still present, group back to `active`), and the positive control — removing the non-default member — succeeds.

Why: a client that keys off the refusal text — error-message assertions in tests, operator runbooks, log scrapers and remediation code that distinguish "the default user may not be removed" from other `DefaultUserRequired` situations — sees a sentence AWS never emits. Message text is part of the refusal contract, so anything matching on it breaks against the emulator while the code-level check passes, which makes the divergence silent until the string comparison runs.

Fix: the refusal path of the ModifyUserGroup handler — the `DefaultUserRequired` branch that guards removal of the default member must emit the service's fixed sentence verbatim rather than formatting one from `crate::state::DEFAULT_USER_ID`. If other handlers raise `DefaultUserRequired` (for example a CreateUserGroup whose `UserIds` omits `default`), they must share the same message source so one wording is not fixed in isolation. This is the same family of defect as open issues 20261025T054338Z-elasticache-modifyuser-the-nopasswordrequired-authenticationmode-conflict-refusal-does.md and 20261025T061454Z-elasticache-modifyuser-the-invalid-access-string-refusal-carries.md (paraphrased refusal messages on ModifyUser), but neither covers ModifyUserGroup or `DefaultUserRequired`.

Traces: failing check `elasticache.remove-default-user-from-group-rejected` (verdict: mismatch). Emulator source of the diverging message: services/service-com-amazonaws-elasticache-amazonelasticachev9/src/ops/modify_user_group.rs lines 384-394.

## Evidence
Check: elasticache.remove-default-user-from-group-rejected [mismatch]
Call: ModifyUserGroup(UserGroupId=<group with members ['default', <user>]>, UserIdsToRemove=['default'])

Expected (real AWS, recorded):
  HTTP 400
  Error.Code    = DefaultUserRequired
  Error.Message = "Redis user group needs to contain a user with the user name default."   <- fixed text, no variable parts

Actual (emulator):
  HTTP 400
  Error.Code    = DefaultUserRequired            (correct)
  Error.Message = "A user group must contain the default user."   <- paraphrase, 'default' interpolated from crate::state::DEFAULT_USER_ID

Replay assertion:
  AssertionError: expected message 'Redis user group needs to contain a user with the user name default.', got 'A user group must contain the default user.'

Correct in the emulator (not part of this defect): code, 400 status, no-effect semantics (membership unchanged, group settles back to 'active'), and the positive control removal of the non-default member.

Model citation: Smithy ModifyUserGroup errors -> DefaultUserRequired {errorKind: client, httpError: 400, awsQueryError: {code: DefaultUserRequired, httpResponseCode: 400}}; DefaultUserRequired documentation: "You must add default user to a user group."

## Steps to Reproduce
1. elasticache.CreateUser(UserId=<u>, UserName=<u>, Engine='redis', AccessString='on ~* +@all', NoPasswordRequired=true)
2. elasticache.CreateUserGroup(UserGroupId=<g>, Engine='redis', UserIds=['default', <u>]) and wait until DescribeUserGroups reports Status='active'
3. elasticache.ModifyUserGroup(UserGroupId=<g>, UserIdsToRemove=['default'])
4. Inspect the ClientError: Error.Code is DefaultUserRequired (correct) but Error.Message is 'A user group must contain the default user.' instead of AWS's 'Redis user group needs to contain a user with the user name default.'
5. Re-replay the behavioral check `elasticache.remove-default-user-from-group-rejected` to confirm the fix.
