Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: memorydb.acl-membership-updates-rejected
Issue-Shape-Id: com.amazonaws.memorydb#AmazonMemoryDB
Issue-Key: memorydb.acl-membership-updates-rejected|run#10|AssertionError|assert msg == want_msg, f"UpdateACL {change!r}: message was {msg!r}"
Issue-Commit: e0f3b342c2995e24bb81f3397837b3776f1c553b

# MemoryDB ACL membership refusals use the wrong error code and non-AWS message wording (UpdateACL/CreateACL)

> _Note: this issue was identified against the code base at commit `e0f3b342c2995e24bb81f3397837b3776f1c553b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateACL

## Description
Behavioral check memorydb.acl-membership-updates-rejected drives the five documented UpdateACL membership refusals and the two CreateACL membership refusals, asserting each refusal's Error.Code and Error.Message and that the ACL is unchanged afterwards. The emulator refuses every one of them (the ACL state after each refusal is correct), but it words and classifies the refusals itself instead of the way MemoryDB does. 4 CAUSES, all of which must be closed before this issue is resolved; the check fails on Cause 1 first and stops there, so Causes 2-4 are the remaining refusals in the SAME check's refusal list, read from the handlers that build them (deterministic, no per-run value involved) and each of which will fail the same check in turn once the one before it is fixed.

Cause 1 of 4: UpdateACL re-adding a user the ACL already lists - message wording. OBSERVED in replay.
AWS: recorded behavior only for the text (Smithy: UpdateACL declares InvalidParameterCombinationException, HTTP 400, "The specified parameter combination is not valid"). AWS answers HTTP 400, code InvalidParameterCombinationException, with the message "<userName> is already a member of <aclName>." The fixed text is " is already a member of " plus the trailing full stop; the two variable parts are the user name taken from UserNamesToAdd and the ACL name taken from ACLName. AWS prefixes NEITHER value with a noun.
Emulator: HTTP 400, code InvalidParameterCombinationException (correct), message "User <userName> is already a member of ACL <aclName>." - it prepends "User " to the user name and "ACL " to the ACL name, so the fixed text does not match AWS's.
Why: the message is a requirement of the error class. A client that matches MemoryDB's refusal text (or presents it to an operator) does not recognise the emulator's, so an idempotent re-add is reported differently from production.
Fix: the UpdateACL handler's already-a-member refusal construction.
Traces: memorydb.acl-membership-updates-rejected; emulator wording at services/service-com-amazonaws-memorydb-amazonmemorydb/src/ops/update_acl.rs:323-334.

Cause 2 of 4: UpdateACL removing a user that is not a member - wrong error class and HTTP status.
AWS: Smithy: UpdateACL declares both InvalidParameterValueException (HTTP 400) and UserNotFoundFault (HTTP 404). A UserNamesToRemove entry that names an existing user which simply is not in this ACL is a bad parameter VALUE, not an absent user: AWS answers HTTP 400 with code InvalidParameterValueException and the message "User <userName> is not a member of ACL <aclName>." (fixed text "User " ... " is not a member of ACL " ... "."; the user name comes from UserNamesToRemove and the ACL name from ACLName).
Emulator: raises UserNotFoundFault for the same input - HTTP 404 and code UserNotFoundFault - while carrying the right message text. The status and the code are both wrong.
Why: a client branching on the error code treats a membership mistake as a non-existent user and goes off to create or look up the user instead of correcting the membership list; SDK retry/catch blocks keyed on UserNotFoundFault vs InvalidParameterValueException take the wrong branch, and the 404 vs 400 split misleads anything that classifies by status.
Fix: the UpdateACL handler's non-member removal path - map it onto the modelled InvalidParameterValueException instead of UserNotFoundFault (the UserNotFoundFault path must stay for a UserNamesToAdd entry naming a user that does not exist, which is correct today).
Traces: memorydb.acl-membership-updates-rejected; emulator classification at services/service-com-amazonaws-memorydb-amazonmemorydb/src/ops/update_acl.rs:299-307.

Cause 3 of 4: UpdateACL with neither membership member supplied - message wording.
AWS: recorded behavior only for the text (Smithy: UserNamesToAdd and UserNamesToRemove are both optional, len 1.., and the operation declares InvalidParameterCombinationException, HTTP 400). A call carrying ACLName and neither list is answered HTTP 400, code InvalidParameterCombinationException, message "Please specify the list of user names to be added or removed." - entirely fixed text, no variable part.
Emulator: HTTP 400, code InvalidParameterCombinationException (correct), message "At least one of UserNamesToAdd and UserNamesToRemove must be supplied." - emulator-invented wording that names the wire members instead of AWS's sentence.
Why: same class of defect as Cause 1 - message assertions and user-facing surfaces built against MemoryDB never match, and the text leaks the wire member names in a message AWS keeps member-agnostic.
Fix: the UpdateACL handler's empty-change refusal construction.
Traces: memorydb.acl-membership-updates-rejected; emulator wording at services/service-com-amazonaws-memorydb-amazonmemorydb/src/ops/update_acl.rs:212-227.

Cause 4 of 4: CreateACL with a user name listed twice in UserNames - wrong error class and non-AWS message.
AWS: Smithy: CreateACL declares both InvalidParameterValueException (HTTP 400) and DuplicateUserNameFault (HTTP 400, "A user with the specified name already exists"). For a UserNames list that repeats a name, AWS answers code InvalidParameterValueException with the message "Duplicate user name specified." - entirely fixed text: the repeated name is NOT echoed. The ACL is not created (a following DescribeACLs on it returns ACLNotFoundFault, "ACL <aclName> not found", which the emulator already gets right).
Emulator: raises DuplicateUserNameFault with the message "UserNames names user <userName> more than once." - wrong error code, emulator-invented sentence, and it echoes the request's user name that AWS omits.
Why: a client cannot tell a malformed request list from a real name collision, so error handling keyed on the code mis-routes; the echoed request value also makes the message non-deterministic for anything comparing against production text.
Fix: the CreateACL handler's duplicate-name refusal - map it onto the modelled InvalidParameterValueException with AWS's fixed sentence. The same duplicate-name wording is built in UpdateACL's member_set helper for a list that repeats a name, so the two write paths of one membership should be reconciled together.
Traces: memorydb.acl-membership-updates-rejected; emulator classification/wording at services/service-com-amazonaws-memorydb-amazonmemorydb/src/ops/create_acl.rs:206-217 and src/ops/update_acl.rs:145-157.

Not part of this issue: the open issue 20261029T174443Z-memorydb-error-messages-and-parameter-catalog-diverge-from.md covers the memorydb WIRE corpus (cluster/parameter-group/event messages and the parameter catalog). It names no ACL-membership operation and carries no replay key for this check, so it does not cover any cause above.

## Evidence
run_behavioral_suite (simparity-behavioral-aws-v1): 14/20 passed, memorydb 14/20, bootstrap_signal=false.

[mismatch] memorydb.acl-membership-updates-rejected
  AssertionError: UpdateACL {'UserNamesToAdd': ['<user>']}: message was 'User <user> is already a member of ACL <acl>.'
  The check asserted the code first and it passed (InvalidParameterCombinationException), then compared the message against AWS's '<user> is already a member of <acl>.' and failed. The names in the recorded run are per-run probe names (sim2real-mdb-<hex>-a / sim2real-mdb-<hex>-acl); only the template is the requirement.

Refusal table the check grades (expected code, expected message template):
  UpdateACL UserNamesToAdd=[existing member]  -> InvalidParameterCombinationException, '<user> is already a member of <acl>.'   | emulator: same code, 'User <user> is already a member of ACL <acl>.'                 (Cause 1, OBSERVED)
  UpdateACL UserNamesToRemove=[non-member]    -> InvalidParameterValueException (400), 'User <user> is not a member of ACL <acl>.' | emulator: UserNotFoundFault (404), same text          (Cause 2)
  UpdateACL UserNamesToAdd=['default']        -> InvalidParameterValueException, 'default user can't be added to another ACL.'  | emulator: MATCHES (state.rs default_user_membership_rejection)
  UpdateACL UserNamesToAdd=[unknown user]     -> UserNotFoundFault, 'User <user> not found.'                                   | emulator: MATCHES (state.rs not_found_message(User))
  UpdateACL with no lists                     -> InvalidParameterCombinationException, 'Please specify the list of user names to be added or removed.' | emulator: same code, 'At least one of UserNamesToAdd and UserNamesToRemove must be supplied.' (Cause 3)
  CreateACL UserNames=[u, u]                  -> InvalidParameterValueException, 'Duplicate user name specified.'              | emulator: DuplicateUserNameFault, 'UserNames names user <user> more than once.' (Cause 4)
  CreateACL UserNames=[unknown user]          -> UserNotFoundFault, 'User <user> not found.'                                   | emulator: MATCHES
  DescribeACLs on a refused ACL               -> ACLNotFoundFault, 'ACL <name> not found'                                      | emulator: MATCHES (state.rs not_found_message(Acl), no full stop)

Smithy evidence: UpdateACL errors = ACLNotFoundFault(404), DefaultUserRequired(400), DuplicateUserNameFault(400), InvalidACLStateFault(400), InvalidParameterCombinationException(400), InvalidParameterValueException(400), UserNotFoundFault(404). CreateACL errors = ACLAlreadyExistsFault, ACLQuotaExceededFault, DefaultUserRequired, DuplicateUserNameFault, InvalidParameterValueException, TagQuotaPerResourceExceeded, UserNotFoundFault. UserNameListInput is len 1.. . No member carries a trait constraining the message text, so the recorded wording is the requirement.

State after each refusal is correct in every case: DescribeACLs still reported ('active', [original member]) and the refused CreateACLs left no ACL behind - the defect is purely classification and wording.

## Steps to Reproduce
1. git_clone the repo and boot the emulator built from it.
2. Replay behavioral check memorydb.acl-membership-updates-rejected (suite simparity-behavioral-aws-v1). It reproduces deterministically.
3. Cause 1: create two users and an ACL holding only the first; wait for the ACL to become active; call UpdateACL(ACLName=<acl>, UserNamesToAdd=[<the member it already holds>]) and read Error.Code and Error.Message. Expect code InvalidParameterCombinationException and message '<user> is already a member of <acl>.'; the emulator returns 'User <user> is already a member of ACL <acl>.'
4. Cause 2: call UpdateACL(ACLName=<acl>, UserNamesToRemove=[<an existing user that is not in this ACL>]). Expect HTTP 400 / InvalidParameterValueException; the emulator returns HTTP 404 / UserNotFoundFault.
5. Cause 3: call UpdateACL(ACLName=<acl>) with neither UserNamesToAdd nor UserNamesToRemove. Expect message 'Please specify the list of user names to be added or removed.'; the emulator returns 'At least one of UserNamesToAdd and UserNamesToRemove must be supplied.'
6. Cause 4: call CreateACL(ACLName=<new>, UserNames=[<u>, <u>]) with the same name twice. Expect InvalidParameterValueException / 'Duplicate user name specified.'; the emulator returns DuplicateUserNameFault / 'UserNames names user <u> more than once.' Then DescribeACLs(ACLName=<new>) must still return ACLNotFoundFault (it does).
7. After each refusal, DescribeACLs(ACLName=<acl>) must report ('active', [the original membership]) - the emulator already satisfies this, so only the code/status/message need fixing.
8. A valid UpdateACL(UserNamesToAdd=[the second user]) afterwards must still land and settle the ACL back to active with both members.
