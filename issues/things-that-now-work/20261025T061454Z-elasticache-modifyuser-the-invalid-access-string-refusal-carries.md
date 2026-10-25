Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elasticache.modify-user-invalid-access-string-rejected
Issue-Shape-Id: com.amazonaws.elasticache#AmazonElastiCacheV9
Issue-Key: elasticache.modify-user-invalid-access-string-rejected|run#11|AssertionError|assert message == "The access-string is invalid.", f"unexpected InvalidParameterValue message: {message!r}"
Issue-Commit: 19a1db04630b2d63bbe4711f31ea8801f0875fe0

# ElastiCache ModifyUser: the invalid-access-string refusal carries a paraphrased, request-echoing message instead of the service's fixed sentence (1 cause)

> _Note: this issue was identified against the code base at commit `19a1db04630b2d63bbe4711f31ea8801f0875fe0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyUser

## Description
One net-new root cause in the `elasticache` service, reached over the ordinary Query surface with no supporting EC2/VPC resources, so it is not blocked by another service's gap. The request IS refused and the error CODE and HTTP status are the modeled ones; what diverges is the refusal sentence the handler authors for a syntactically malformed Redis/Valkey ACL access string.

This cause is NOT covered by the open issues already on file. 20261024T182307Z-elasticache-refusal-fidelity-cacheparametergroupname-identifier-validation-missing-and.md enumerates six refusal-text causes (DeleteCacheParameterGroup identifier validation, ModifyCacheParameterGroup non-modifiable parameter, AddTagsToResource reserved prefix, tag-value charset, RemoveTagsFromResource TagNotFound, CreateUserGroup already-exists) and names none of the access-string rule. 20261025T054338Z-elasticache-modifyuser-the-nopasswordrequired-authenticationmode-conflict-refusal-does.md is also ModifyUser but covers a DIFFERENT rule and a different error class: the NoPasswordRequired + AuthenticationMode InvalidParameterCombination conflict, not AccessString syntax validation. 20261024T075915Z-elasticache-rbac-surface-deleteusergroup-and-modifyuser-answer-501.md covers ModifyUser answering 501 NotImplemented; here ModifyUser is implemented and does refuse with the correct modeled code, so that issue does not describe this divergence either. The resolved issue 20261023T155010Z-elasticache-modifyuser-stub.md (ModifyUser scaffold stub) is fixed and does not cover it.

Cause 1 of 1 — the ModifyUser AccessString syntax refusal does not emit AWS's fixed sentence.
AWS: Smithy (services/service-com-amazonaws-elasticache-amazonelasticachev9/smithy-model/com-amazonaws-elasticache-amazonelasticachev9.json) binds `ModifyUser` with input `ModifyUserMessage{UserId (required, @length min 1, @pattern ^[a-zA-Z][a-zA-Z0-9\-]*$), AccessString (@pattern \S), AppendAccessString (@pattern \S), Passwords, NoPasswordRequired, AuthenticationMode, Engine}` and output `User` at 200. Its modeled errors are `InvalidParameterValueException` (httpError 400, awsQueryError code `InvalidParameterValue`, documented "The value for a parameter is invalid."), `InvalidParameterCombinationException` (400, `InvalidParameterCombination`), `InvalidUserStateFault` (400, `InvalidUserState`), `ServiceLinkedRoleNotFoundFault` (400, `ServiceLinkedRoleNotFoundFault`) and `UserNotFoundFault` (404, `UserNotFound`). The model documents `AccessString` only as "Access permissions string used for this user" and puts no grammar on it beyond `@pattern \S`, so the ACL grammar itself is enforced server-side only. For the message: recorded behavior only — the recording shows AWS refusing a malformed access string with HTTP 400, Error.Code `InvalidParameterValue`, and a FIXED service sentence that names neither the offending token nor the member: "The access-string is invalid." That sentence is a constant for this condition, not built from the request, so it is the rule rather than one recording's values.
Emulator: the status and code are right (HTTP 400 `InvalidParameterValue`), but the message is a per-value paraphrase built by echoing the rejected token and the member name back at the caller: "Invalid value not-a-real-acl-token for AccessString: it is not a Valkey or Redis OSS ACL rule." Generally, for any submitted access string containing a token that is not an ACL rule, the emulator reports the first offending token and the member it arrived on where AWS reports only its fixed sentence.
Why: a client that branches on or displays AWS's refusal text for an illegal ACL string does not match, and tooling cannot distinguish "AWS rejected my access string" from some other `InvalidParameterValue` by text. The emulator's form also commits to information AWS deliberately does not disclose (which token offended, and whether it was `AccessString` or `AppendAccessString`), so a caller written against the emulator learns a diagnosis that real AWS will never supply and that cannot be relied on. Because the check stops at this assertion, the divergence additionally masks the rest of the behavior it guards: that the refusal is not partially applied (the stored `AccessString` must still be the pre-call value), that a subsequent syntactically valid ModifyUser is accepted and converges, and that after DeleteUser both DescribeUsers and ModifyUser answer `UserNotFound` with "User <id> not found." None of those assertions is reached while the message diverges, so their fidelity is currently unverified.
Fix: the shared access-string validation in `crate::state` — the helper `crate::state::access_string_rejection(member, value)`, which is the single statement of this rule that both `CreateUser` and `ModifyUser` go through (`modify_user.rs` calls it for both `AccessString` and `AppendAccessString`, and again on the appended RESULT) — must render the service's fixed sentence for the malformed-ACL case instead of a per-value "Invalid value <token> for <member>: ..." paraphrase. Changing it there fixes CreateUser, ModifyUser's `AccessString` path and its `AppendAccessString` path at once; the handler keeps calling it and keeps classifying the refusal as `InvalidParameterValueException`, which is already correct.
Traces: elasticache.modify-user-invalid-access-string-rejected.

## Evidence
Cause 1 — elasticache.modify-user-invalid-access-string-rejected:
  setup (succeeds): elasticache CreateUser(UserId=<id>, UserName=<id>, Engine="redis", AccessString="off -@all", NoPasswordRequired=true, Tags=[...]) -> 200, then polled to Status "active" via DescribeUsers.
  call: elasticache ModifyUser(UserId=<id>, AccessString="on ~* &*&*&* not-a-real-acl-token %%%garbage%%%")
  expected: HTTP 400, Error.Code `InvalidParameterValue` (modeled `InvalidParameterValueException`, awsQueryError code `InvalidParameterValue`, httpError 400), message exactly 'The access-string is invalid.'
  actual:   HTTP 400, Error.Code `InvalidParameterValue` (status and code CORRECT), message 'Invalid value not-a-real-acl-token for AccessString: it is not a Valkey or Redis OSS ACL rule.'
  replay assertion (check line 42): "AssertionError: unexpected InvalidParameterValue message: 'Invalid value not-a-real-acl-token for AccessString: it is not a Valkey or Redis OSS ACL rule.'" raised by `assert message == "The access-string is invalid.", f"unexpected InvalidParameterValue message: {message!r}"`.
  verdict: mismatch (the emulator answered; it answered with the wrong refusal text).

  Source of the diverging text: services/service-com-amazonaws-elasticache-amazonelasticachev9/src/ops/modify_user.rs lines 215-229 loop over `("AccessString", input.access_string)` and `("AppendAccessString", input.append_access_string)` and lift whatever `crate::state::access_string_rejection(member, value)` returns into `InvalidParameterValueException { message }`; lines 368-377 do the same for the appended RESULT. The handler's classification is right, so the paraphrase originates in `crate::state::access_string_rejection`, which composes "Invalid value {value} for {member}: ..." rather than the service's fixed sentence.

  Model citations used: `smithy_query(query="operation", operation="ModifyUser")` -> errors include `{shape: InvalidParameterValueException, errorKind: client, httpError: 400, awsQueryError: {code: "InvalidParameterValue", httpResponseCode: 400}}`; `smithy_query(query="doc", shape="ModifyUserMessage", member="AccessString")` -> "<p>Access permissions string used for this user.</p>" (no grammar, so the ACL rule set and its refusal text are server-side only — recorded behavior).

  Assertions the divergence masks (never reached, so currently unverified): DescribeUsers must still report AccessString == "off -@all" after the refusal (no partial application); ModifyUser(AccessString="on ~* +@all") must then be accepted and converge to that value with Status "active"; after DeleteUser, DescribeUsers and ModifyUser on the dead id must both answer `UserNotFound` with 'User <id> not found.'

## Steps to Reproduce
Point an elasticache client at the emulator (Query surface; no VPC/EC2/subnet-group resources are needed for any step).

1. aws elasticache create-user --user-id <id> --user-name <id> --engine redis --access-string 'off -@all' --no-password-required
2. aws elasticache describe-users --user-id <id>   # poll until Status == "active"
3. aws elasticache modify-user --user-id <id> --access-string 'on ~* &*&*&* not-a-real-acl-token %%%garbage%%%'
   -> observe HTTP 400 InvalidParameterValue with message 'Invalid value not-a-real-acl-token for AccessString: it is not a Valkey or Redis OSS ACL rule.'
   -> AWS answers HTTP 400 InvalidParameterValue with the fixed sentence 'The access-string is invalid.'
4. Same divergence on the sibling member: aws elasticache modify-user --user-id <id> --append-access-string 'not-a-real-acl-token'
   -> the emulator again echoes the token and names AppendAccessString; AWS emits the same fixed sentence.
5. Once the text matches, the remainder of the check runs and should be confirmed:
   a. aws elasticache describe-users --user-id <id>  -> AccessString must still be 'off -@all' (refusal not partially applied)
   b. aws elasticache modify-user --user-id <id> --access-string 'on ~* +@all'  -> 200, converges to that value at Status active
   c. aws elasticache delete-user --user-id <id>; then describe-users and modify-user on <id> -> both UserNotFound, message 'User <id> not found.'

Replay: elasticache.modify-user-invalid-access-string-rejected
