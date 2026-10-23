Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: memorydb.user-password-rules-rejected memorydb.user-password-count-and-auth-mode-transitions memorydb.resource-name-rules-rejected memorydb.resource-names-fold-to-lowercase memorydb.default-user-and-open-access-acl-immutable memorydb.parameter-value-rejections-leave-group-unchanged
Issue-Shape-Id: com.amazonaws.memorydb#AmazonMemoryDB
Issue-Key: memorydb.user-password-rules-rejected|run#4|AssertionError|assert msg == want_msg, f"{suffix}: message was {msg!r}"
Issue-Key: memorydb.user-password-count-and-auth-mode-transitions|run#15|AssertionError|assert msg == "Maximum number of passwords allowed in this service is 2.", f"message was {msg!r}"
Issue-Key: memorydb.resource-name-rules-rejected|run#13|AssertionError|assert msg == want, f"user {label}: message was {msg!r}"
Issue-Key: memorydb.resource-names-fold-to-lowercase|run#12|AssertionError|assert msg == f"User {lower} already exists.", f"message was {msg!r}"
Issue-Key: memorydb.default-user-and-open-access-acl-immutable|run#8|AssertionError|assert msg == want_msg, f"refusal {i}: message was {msg!r}"
Issue-Key: memorydb.parameter-value-rejections-leave-group-unchanged|run#15|AssertionError|assert msg == want_msg, f"{label}: message was {msg!r}"
Issue-Commit: 7f205ee96657d4151f96c4aa7296372f34590a63

# memorydb: six refusal paths answer the right error code with the wrong message template (names/passwords/default user/parameter values/already-exists)

> _Note: this issue was identified against the code base at commit `7f205ee96657d4151f96c4aa7296372f34590a63`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateUser

## Description
Six independent root causes in the memorydb emulator's refusal messages, found by replaying the behavioral suite (memorydb scored 4/20). In every one of them the emulator already raises the RIGHT modelled error with the right HTTP status; what diverges is the message template, and each of these templates is request-derived (it names the member, the constraint and/or the resource the request named), so the template is the requirement — the recorded strings below are the form, with the names/values coming from the request. Each cause must be closed before this issue is done; each one is also the FIRST assertion its check reaches, so the rest of each check's behavior is still ungraded behind it.

=== Cause 1 of 6: CreateUser/UpdateUser password-length refusal ===
AWS: a password outside 16-128 characters is refused with HTTP 400 InvalidParameterValueException and the message 'Passwords length must be between 16-128 characters.' Smithy: AuthenticationMode$Passwords -> PasswordListInput (@length min 1, member String with NO length constraint), CreateUser errors include InvalidParameterValueException (HTTP 400) and InvalidParameterCombinationException (HTTP 400); the 16-128 bound and the text are recorded behavior only (AWS's RBAC password requirements).
Emulator: same code, message 'A user password must be between 16 and 128 characters long.'
Why: see 'Why' at the end — a client keyed on the message cannot tell the refusal apart.
Fix: the password validation shared by CreateUser/UpdateUser (services/service-com-amazonaws-memorydb-amazonmemorydb/src/ops/create_user.rs, update_user.rs and the password rules in src/state.rs).
Traces: memorydb.user-password-rules-rejected (case 'short', 15 chars; 'long', 129 chars). The same check also grades, still ungraded behind this: InvalidParameterCombinationException 'Password field is not allowed with authentication type: iam' for {Type: iam, Passwords: [...]}, InvalidParameterCombinationException 'A user with Authentication Mode: password, must have at least one password' for {Type: password} with no Passwords, and UserNotFoundFault with 'User <name> not found.' when the refused name is read back.

=== Cause 2 of 6: UpdateUser third-password refusal ===
AWS: a user may hold at most two passwords; a third is refused with HTTP 400 InvalidParameterValueException and 'Maximum number of passwords allowed in this service is 2.' Smithy: Authentication$PasswordCount doc 'The number of passwords belonging to the user. The maximum is two.'; UpdateUser errors include InvalidParameterValueException (HTTP 400). Message text recorded only; the bound (2) is the modelled/documented constraint the message names.
Emulator: same code, message 'A user can have at most 2 passwords.'
Fix: the password-count check in the UpdateUser handler (src/ops/update_user.rs, MAX_PASSWORDS_PER_USER in src/state.rs).
Traces: memorydb.user-password-count-and-auth-mode-transitions. Still ungraded behind it: that a refused third password leaves the settled user at PasswordCount 2 and status 'active', and that switching to {Type: iam} and back moves Authentication and MinimumEngineVersion between {'Type':'iam'}/7.0 and {'Type':'password','PasswordCount':1}/6.2.

=== Cause 3 of 6: user/ACL name length and pattern refusals ===
AWS: a user or ACL name longer than 40 characters is refused with HTTP 400 InvalidParameterValueException and 'User id length must be less than 40.' / 'ACL name length must be less than 40.'; a name that breaks the service's character rule is refused with 'User id must begin with a letter; must contain only ASCII letters, digits, and hyphens; and must not end with a hyphen or contain two consecutive hyphens.' (same sentence prefixed 'ACL name ' for an ACL). Smithy: CreateUser$UserName -> UserName with @length(min:1) and @pattern '^[a-zA-Z][a-zA-Z0-9\-]*$' (no maximum, and the pattern admits a double hyphen); CreateACL$ACLName -> String with no constraints at all. So both the 40-character cap and the double-hyphen rule, and the messages, are recorded behavior only.
Emulator: the length refusal carries the right code but the message 'UserName length must not exceed 40 characters.' — it names the wire member instead of 'User id' and states the bound differently.
Fix: name validation for users and ACLs (src/ops/create_user.rs, create_acl.rs and the shared name rules in src/state.rs).
Traces: memorydb.resource-name-rules-rejected (case 'too-long', 41 chars). Still ungraded behind it: the digit-leading, underscore and double-hyphen cases for both CreateUser and CreateACL, and that none of the refused names appears in DescribeUsers/DescribeACLs while a 40-character name is accepted.

=== Cause 4 of 6: already-exists refusals ===
AWS: re-creating an existing user is refused with HTTP 400 UserAlreadyExistsFault and 'User <name> already exists.' (the folded name the request named); an existing ACL with ACLAlreadyExistsFault and 'ACL <name> already exists. Please retry with another name.'; an existing parameter group with ParameterGroupAlreadyExistsFault and 'Parameter Group already exists.' Smithy: CreateUser errors include UserAlreadyExistsFault (HTTP 400) and CreateACL ACLAlreadyExistsFault (HTTP 400); message text recorded only, but it is built from the request's name.
Emulator: the right code with 'User with name <name> already exists.' — an extra 'with name' in the template.
Fix: the duplicate-name check in src/ops/create_user.rs, create_acl.rs, create_parameter_group.rs.
Traces: memorydb.resource-names-fold-to-lowercase (the lowercase re-create after a mixed-case create; the folding itself is correct — the stored name read back as the lowercase spelling). Still ungraded behind it: the ACL and parameter-group already-exists messages and the parameter-group family folding ('MEMORYDB_VALKEY7' -> 'memorydb_valkey7').

=== Cause 5 of 6: default user / open-access ACL immutability refusals ===
AWS: UpdateUser and DeleteUser on the built-in 'default' user are refused with HTTP 400 InvalidParameterValueException and 'default user can't be modified.' / 'default user can't be deleted.'; UpdateACL and DeleteACL on the built-in 'open-access' ACL with 'open-access ACL can't be modified.' / 'open-access ACL can't be deleted.'; CreateUser(UserName='default') with UserAlreadyExistsFault 'User default already exists.' and CreateACL(ACLName='open-access') with ACLAlreadyExistsFault 'ACL open-access already exists. Please retry with another name.' Smithy: UpdateUser/DeleteUser/UpdateACL/DeleteACL all declare InvalidParameterValueException (HTTP 400); the model does not describe the built-ins, so this is recorded behavior only.
Emulator: the right code with 'User default is a default user and cannot be modified.'
Fix: the built-in-resource guards in src/ops/update_user.rs, delete_user.rs, update_acl.rs, delete_acl.rs (built-ins seeded by state.rs ensure_builtins).
Traces: memorydb.default-user-and-open-access-acl-immutable (refusal 0). Still ungraded behind it: the other six refusals above and that the default user and open-access ACL read back byte-identical afterwards, plus that a new empty ACL created beside them settles active with empty UserNames/Clusters.

=== Cause 6 of 6: UpdateParameterGroup parameter-value refusals ===
AWS: a value outside a parameter's AllowedValues is refused with HTTP 400 InvalidParameterValueException and a message built from that parameter's own AllowedValues: 'invalid parameter value, allowed values are:<AllowedValues>' (and for a numeric parameter, 'value:<v> is outside of range:<AllowedValues>'). Smithy: UpdateParameterGroup$ParameterNameValues -> ParameterNameValueList (required), errors InvalidParameterValueException (HTTP 400), InvalidParameterCombinationException (HTTP 400), ParameterGroupNotFoundFault (HTTP 404); Parameter$AllowedValues doc 'The valid range of values for the parameter.' The texts are recorded only, but the rule is explicit: the message is assembled from the AllowedValues the service itself reported for that parameter via DescribeParameters.
Emulator: the right code with its own template, e.g. 'Invalid parameter value "bogus" for parameter maxmemory-policy: the allowed values are volatile-lru,allkeys-lru,volatile-lfu,allkeys-lfu,volatile-random,allkeys-random,volatile-ttl,noeviction.'
Fix: the parameter-value validation in src/ops/update_parameter_group.rs — build the message from the stored parameter's AllowedValues in AWS's two forms (enum/bool vs range), and use the recorded texts for the remaining classes the same check grades: 'invalid integer value: <v>', 'Could not find parameter with name: <name>', 'Duplicate parameters specified in the same call', InvalidParameterCombinationException 'Cannot modify more than 20 parameters in a single request', 'Cannot modify a default parameter group.', and ParameterGroupNotFoundFault 'ParameterGroup not found: <name>'.
Traces: memorydb.parameter-value-rejections-leave-group-unchanged (case 'enum'). Note this check later also reads/updates default.memorydb-valkey7, which is missing in the emulator — that part is already covered by open issue 20261022T212928Z-memorydb-missing-default-parameter-groups-describeusers-filter-names.md (cause 1); this cause is only about the refusal templates and is net-new.

Why (all six causes): the HTTP status and the error code are already right, so these do not break a client that branches on the code — but MemoryDB's refusals are the documented way to learn WHICH rule a request broke, and the emulator's wording does not carry the same facts (it names the wire member rather than the service's 'User id', drops the offending resource name, or re-words a constraint). A client, test suite or operator tool that asserts on, parses or surfaces the service's refusal text — the common pattern for validation errors, where the code alone is the same InvalidParameterValueException for every one of these rules — reads a different sentence on the emulator than on AWS, and a conformance test written against AWS fails on behavior the emulator otherwise gets right. Keeping the templates identical also keeps the facts the message is built FROM (the member, the bound, the resource, the AllowedValues) in the response.

## Evidence
Suite: simparity-behavioral-aws-v1, memorydb 4/20 (20%), 16 failed. Each line below is the first failing assertion of its check; the error code matched in every case (only the message differed).

Cause 1 — memorydb.user-password-rules-rejected [mismatch]:
  AssertionError: short: message was 'A user password must be between 16 and 128 characters long.'
  Expected (AWS): InvalidParameterValueException, 'Passwords length must be between 16-128 characters.' (CreateUser with a 15-character password; same for 129 characters)

Cause 2 — memorydb.user-password-count-and-auth-mode-transitions [mismatch]:
  AssertionError: message was 'A user can have at most 2 passwords.'
  Expected (AWS): InvalidParameterValueException, 'Maximum number of passwords allowed in this service is 2.' (UpdateUser with three passwords)

Cause 3 — memorydb.resource-name-rules-rejected [mismatch]:
  AssertionError: user too-long: message was 'UserName length must not exceed 40 characters.'
  Expected (AWS): InvalidParameterValueException, 'User id length must be less than 40.' (CreateUser with a 41-character name); for CreateACL, 'ACL name length must be less than 40.'

Cause 4 — memorydb.resource-names-fold-to-lowercase [mismatch]:
  AssertionError: message was 'User with name sim2real-mdb-efaf3eb2e126-mixed already exists.'
  Expected (AWS): UserAlreadyExistsFault, 'User sim2real-mdb-efaf3eb2e126-mixed already exists.' (i.e. 'User <name> already exists.')

Cause 5 — memorydb.default-user-and-open-access-acl-immutable [mismatch]:
  AssertionError: refusal 0: message was 'User default is a default user and cannot be modified.'
  Expected (AWS): InvalidParameterValueException, "default user can't be modified." (UpdateUser(UserName='default', AccessString='off -@all'))

Cause 6 — memorydb.parameter-value-rejections-leave-group-unchanged [mismatch]:
  AssertionError: enum: message was 'Invalid parameter value "bogus" for parameter maxmemory-policy: the allowed values are volatile-lru,allkeys-lru,volatile-lfu,allkeys-lfu,volatile-random,allkeys-random,volatile-ttl,noeviction.'
  Expected (AWS): InvalidParameterValueException, 'invalid parameter value, allowed values are:' + the AllowedValues string DescribeParameters reported for maxmemory-policy in that group.

Model references (services/service-com-amazonaws-memorydb-amazonmemorydb/smithy-model/com-amazonaws-memorydb-amazonmemorydb.json):
  CreateUser input: UserName (@length min 1, @pattern '^[a-zA-Z][a-zA-Z0-9\\-]*$'), AuthenticationMode (required), AccessString (@pattern '\\S'); errors UserAlreadyExistsFault/InvalidParameterValueException/InvalidParameterCombinationException/DuplicateUserNameFault/... all HTTP 400.
  AuthenticationMode$Passwords -> PasswordListInput @length(min:1), member String (no length bound) — the 16-128 rule is service-side.
  CreateACL$ACLName -> String (no length or pattern) — the 40-character and double-hyphen rules are service-side.
  UpdateUser errors: InvalidParameterValueException, InvalidParameterCombinationException, InvalidUserStateFault, UserNotFoundFault (HTTP 404).
  UpdateParameterGroup errors: InvalidParameterValueException, InvalidParameterCombinationException, InvalidParameterGroupStateFault, ParameterGroupNotFoundFault (404); Parameter$AllowedValues 'The valid range of values for the parameter.'

## Steps to Reproduce
Boot the emulator and use a botocore memorydb client. In each step the error code is already correct; compare Error.Message.

Cause 1: CreateUser(UserName=n, AccessString='on ~* +@all', AuthenticationMode={'Type':'password','Passwords':['A'*15]}).
  Expected message: 'Passwords length must be between 16-128 characters.'  Actual: 'A user password must be between 16 and 128 characters long.'

Cause 2: CreateUser with one valid password, wait active, UpdateUser(AuthenticationMode={'Type':'password','Passwords':[p1,p2]}), wait active, then UpdateUser with [p1,p2,p3].
  Expected: 'Maximum number of passwords allowed in this service is 2.'  Actual: 'A user can have at most 2 passwords.'

Cause 3: CreateUser with a 41-character name (and CreateACL with the same name).
  Expected: 'User id length must be less than 40.' / 'ACL name length must be less than 40.'  Actual: 'UserName length must not exceed 40 characters.'
  Also try names starting with a digit, containing '_' and containing '--': expect 'User id must begin with a letter; must contain only ASCII letters, digits, and hyphens; and must not end with a hyphen or contain two consecutive hyphens.'

Cause 4: CreateUser(UserName=base+'-MiXed', ...), then CreateUser(UserName=base+'-mixed', ...).
  Expected: UserAlreadyExistsFault 'User <lowercase-name> already exists.'  Actual: 'User with name <lowercase-name> already exists.'
  Same for CreateACL ('ACL <name> already exists. Please retry with another name.') and CreateParameterGroup ('Parameter Group already exists.').

Cause 5: UpdateUser(UserName='default', AccessString='off -@all'); DeleteUser(UserName='default'); UpdateACL(ACLName='open-access', UserNamesToRemove=['default']); DeleteACL(ACLName='open-access').
  Expected: "default user can't be modified." / "default user can't be deleted." / "open-access ACL can't be modified." / "open-access ACL can't be deleted."  Actual (first call): 'User default is a default user and cannot be modified.'

Cause 6: CreateParameterGroup(ParameterGroupName=g, Family='memorydb_valkey7'); DescribeParameters(g) to read AllowedValues for maxmemory-policy and timeout; then UpdateParameterGroup(g, [{'ParameterName':'maxmemory-policy','ParameterValue':'bogus'}]).
  Expected: 'invalid parameter value, allowed values are:<AllowedValues of maxmemory-policy>'  Actual: 'Invalid parameter value "bogus" for parameter maxmemory-policy: the allowed values are ...'
  Then UpdateParameterGroup(g, timeout='19') -> expect 'value:19 is outside of range:<AllowedValues of timeout>'; timeout='abc' -> 'invalid integer value: abc'; 'no-such-param' -> 'Could not find parameter with name: no-such-param'; timeout twice -> 'Duplicate parameters specified in the same call'; 21 parameters -> InvalidParameterCombinationException 'Cannot modify more than 20 parameters in a single request'.

Replay: memorydb.user-password-rules-rejected, memorydb.user-password-count-and-auth-mode-transitions, memorydb.resource-name-rules-rejected, memorydb.resource-names-fold-to-lowercase, memorydb.default-user-and-open-access-acl-immutable, memorydb.parameter-value-rejections-leave-group-unchanged.
