Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: memorydb.acl-membership-reflected-on-users memorydb.default-user-and-open-access-acl-immutable memorydb.redis-engine-rejects-a-valkey-only-engine-version memorydb.describe-users-pages-cover-every-user memorydb.parameter-update-and-reset-roundtrip memorydb.resource-name-rules-rejected memorydb.resource-names-fold-to-lowercase memorydb.user-password-count-and-auth-mode-transitions memorydb.user-password-punctuation-accepted
Issue-Shape-Id: com.amazonaws.memorydb#AmazonMemoryDB
Issue-Key: memorydb.acl-membership-reflected-on-users|run#13|KeyError|assert acl["MinimumEngineVersion"] == "6.2", f"password-only ACL MEV {acl['MinimumEngineVersion']!r}"
Issue-Key: memorydb|DescribeACLs|ACLNotFoundFault
Issue-Key: memorydb.redis-engine-rejects-a-valkey-only-engine-version|run#11|AssertionError|assert got == want, f"Engine=redis EngineVersion={_VALKEY_ONLY_VERSION!r} raised {got!r}, expected {want!r}"
Issue-Key: memorydb.describe-users-pages-cover-every-user|run#5|AssertionError|assert code == "InvalidParameterValueException", f"{op} MaxResults={bad} gave {code}: {msg!r}"
Issue-Key: memorydb.parameter-update-and-reset-roundtrip|run#6|KeyError|assert created["Description"] == " ", f"an omitted description reads back as {created['Description']!r}"
Issue-Key: memorydb.resource-name-rules-rejected|_error#13|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Key: memorydb.resource-names-fold-to-lowercase|run#8|AssertionError|assert user["Name"] == lower, f"user stored as {user['Name']!r}, expected {lower!r}"
Issue-Key: memorydb.user-password-count-and-auth-mode-transitions|run#7|AssertionError|assert created["AccessString"] == "on ~* resetchannels +@all", (
Issue-Key: memorydb|CreateUser|InvalidParameterValueException
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# MemoryDB users/ACLs/parameter groups: missing MinimumEngineVersion, no built-in open-access ACL, no name rules or case folding, no AccessString normalisation, wrong MaxResults error class, and over-strict password charset (8 causes)

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateUser, CreateACL, DescribeUsers, DescribeACLs, CreateParameterGroup

## Description
Eight independent root causes in the MemoryDB emulator's user/ACL/parameter-group surface, all grounded in failing behavioral checks. Each must be closed before this issue is submitted.

Cause 1 of 8: User$MinimumEngineVersion and ACL$MinimumEngineVersion are never reported.
AWS: Smithy (com.amazonaws.memorydb) models User$MinimumEngineVersion ('The minimum engine version supported for the user') and ACL$MinimumEngineVersion ('The minimum engine version supported for the ACL') as members of every User/ACL AWS returns. Recorded behavior: a password-authenticated user and a password-only ACL report '6.2', and an ACL that has an IAM-authenticated member (and the IAM user itself) report '7.0'; removing the IAM member restores '6.2'.
Emulator: the member is absent entirely from CreateUser/DescribeUsers/CreateACL/DescribeACLs bodies, so a client sees KeyError: 'MinimumEngineVersion'. The projection hard-codes it away (services/service-com-amazonaws-memorydb-amazonmemorydb/src/state.rs, StoredUser::to_api minimum_engine_version: None and StoredAcl::to_api minimum_engine_version: None).
Why: a client that reads the minimum engine version to decide whether an ACL/user can be attached to a given cluster engine version gets a missing key instead of a value, and cannot tell a password ACL (6.2) from an IAM one (7.0).
Fix: derive the value in the ACL/User projection in src/state.rs (from each user's authentication type, and for an ACL from its members' types) and populate it in the describe/create handlers for users and ACLs.
Traces: memorydb.acl-membership-reflected-on-users.

Cause 2 of 8: the account's built-in 'open-access' ACL is never materialised.
AWS: every MemoryDB account already has an 'open-access' ACL (whose only member is the built-in 'default' user) before any CreateACL call; DescribeACLs(ACLName='open-access') returns it, and CreateCluster(ACLName='open-access') is accepted. Recorded behavior: DescribeACLs(ACLName='open-access') returns the ACL; CreateACL(ACLName='open-access') is refused with ACLAlreadyExistsFault, UpdateACL/DeleteACL on it with InvalidParameterValueException, and the same recording shows the built-in 'default' user refusing UpdateUser/DeleteUser with InvalidParameterValueException.
Emulator: DescribeACLs(ACLName='open-access') answers ACLNotFoundFault (HTTP 404, message 'ACL open-access not found.'), and CreateCluster(ACLName='open-access') fails the same way. src/state.rs materialises only the built-in 'default' user (ensure_builtin_users) and no built-in ACL, so the account starts with zero ACLs.
Why: 'open-access' is the ACL every getting-started cluster is created with, so any client that provisions a cluster without first creating an ACL fails at CreateCluster; it also makes the ACL-not-found error mask whatever validation should have fired first (see the CreateCluster engine-version refusal in Cause 2's trace, which never got that far).
Fix: materialise the built-in 'open-access' ACL alongside the built-in default user in src/state.rs and enforce the built-in immutability refusals in the ACL/user update and delete handlers (the immutability refusals themselves were never reached in this run because the first DescribeACLs already failed).
Traces: memorydb.default-user-and-open-access-acl-immutable, memorydb.redis-engine-rejects-a-valkey-only-engine-version.

Cause 3 of 8: an out-of-range MaxResults is refused with an unmodeled error class.
AWS: DescribeUsers/DescribeACLs accept MaxResults in 1..50 and refuse anything outside it with InvalidParameterValueException (HTTP 400), which Smithy lists for this service (awsQueryError code InvalidParameterValue). Smithy declares NO ValidationException for DescribeUsers/DescribeACLs — their modeled error sets are {InvalidParameterCombinationException, UserNotFoundFault} and {ACLNotFoundFault, InvalidParameterCombinationException} plus the service-common set. Recorded message example: 'Invalid value for max results. Must be between 1 and 50'.
Emulator: DescribeUsers(MaxResults=0) answers error code ValidationException (message example 'MaxResults must be at least 1.'), an error code the model does not declare for the operation; MaxResults=51 is on the same path.
Why: a client that catches the modeled InvalidParameterValueException (or branches on the error code) does not catch the emulator's ValidationException, so paginating code that clamps page size sees an unhandled error class.
Fix: the MaxResults range validation for DescribeUsers/DescribeACLs (and the shared pagination validation they call) must raise the modeled InvalidParameterValueException instead of a generic ValidationException.
Traces: memorydb.describe-users-pages-cover-every-user.

Cause 4 of 8: CreateParameterGroup omits ParameterGroup$Description when the request omits it.
AWS: Smithy models CreateParameterGroup$Description as optional input and ParameterGroup$Description as an output member. Recorded behavior: a group created without a Description reads back with Description set to a single space (' '), not absent.
Emulator: the member is passed through unchanged (src/state.rs StoredParameterGroup::to_api, description: self.description.clone()), so an omitted description is absent from the response and a client sees KeyError: 'Description'.
Why: a client that reads ParameterGroup.Description after a create — e.g. to display or to diff a group against its family default — gets a missing key rather than the service's default value.
Fix: default the stored description to the service's default when the request omits it, in the CreateParameterGroup handler / the parameter-group state in src/state.rs, so the describe and create projections both carry it.
Traces: memorydb.parameter-update-and-reset-roundtrip.

Cause 5 of 8: user and ACL names are not validated for length or the service's tighter name rule.
AWS: the modeled pattern on UserName is ^[a-zA-Z][a-zA-Z0-9\-]*$ with no length cap, but the service enforces a tighter rule and refuses the request with InvalidParameterValueException (HTTP 400): names over 40 characters, names starting with a digit, names containing an underscore, and names containing two consecutive hyphens. Recorded message examples: 'User id length must be less than 40.' and 'User id must begin with a letter; must contain only ASCII letters, digits, and hyphens; and must not end with a hyphen or contain two consecutive hyphens.' (and the ACL-name equivalents). A 40-character name is accepted.
Emulator: CreateUser with a 41-character name returns HTTP 200 and persists the user (observed response User.Name 'sim2real-mdb-<id>-bbbbbbbbbbbbbbb', Status 'active'), so the refused names become listable resources; CreateACL is on the same path.
Why: a client relying on the service to reject an illegal name creates resources AWS would never have created, and names that cannot exist in real AWS (underscore, double hyphen, over-length) then flow into cluster creation and ACL membership.
Fix: add the name validation to the CreateUser / CreateACL handlers (shared with the other name-taking creates) so an illegal name is refused with the modeled InvalidParameterValueException before anything is persisted.
Traces: memorydb.resource-name-rules-rejected.

Cause 6 of 8: resource names are not folded to lowercase.
AWS: user, ACL and parameter-group names (and the parameter-group family) are stored lowercased; the mixed-case and lowercase spellings are the same resource, so the create echoes the lowercase name, a lowercase read finds it, and a later create with the lowercase spelling collides (UserAlreadyExistsFault / ACLAlreadyExistsFault / ParameterGroupAlreadyExistsFault, all HTTP 400 in Smithy). Recorded behavior only for the folding itself.
Emulator: CreateUser(UserName='...-MiXed') answers User.Name '...-MiXed' — the mixed-case spelling is stored verbatim, so the lowercase read misses it and the lowercase create does not collide. CreateACL and CreateParameterGroup (plus Family, e.g. 'MEMORYDB_VALKEY7') are on the same path.
Why: a client that writes a mixed-case name and later reads or deletes by the lowercase name AWS stored gets a not-found, and two spellings of one AWS resource become two distinct emulator resources.
Fix: normalise the name (and the parameter-group family) to lowercase on the way into state in the CreateUser / CreateACL / CreateParameterGroup handlers and in every lookup key used by the describe/update/delete handlers in src/state.rs.
Traces: memorydb.resource-names-fold-to-lowercase.

Cause 7 of 8: CreateUser does not normalise AccessString.
AWS: Smithy models CreateUser$AccessString (pattern \S) as required input and User$AccessString as output. Recorded behavior: the service normalises the access string before storing it — 'on ~* +@all' reads back as 'on ~* resetchannels +@all' — and a later AuthenticationMode change leaves that normalised string untouched.
Emulator: the request string is stored and echoed verbatim ('on ~* +@all'); src/state.rs keeps access_string as given.
Why: a client that compares the user's reported AccessString with what AWS stored (for drift detection or idempotent reconciliation) sees a permanent difference and will keep re-issuing UpdateUser.
Fix: normalise the access string in the CreateUser/UpdateUser handler path (shared with the state's user record in src/state.rs) so the stored and reported string is the normalised form.
Traces: memorydb.user-password-count-and-auth-mode-transitions.

Cause 8 of 8: CreateUser refuses passwords containing punctuation the service accepts.
AWS: the service accepts user passwords containing '@', '/', ',', '"', space and backslash — each such CreateUser succeeds and the user settles active with Authentication {Type: password, PasswordCount: 1}. Smithy puts no pattern on PasswordListInput members, so the only modeled constraints are the list's length and the documented 16..128 length. Hypothesis: the emulator encodes the Developer Guide's narrower prose character list, which the service does not enforce.
Emulator: CreateUser with a password containing '@' is refused with InvalidParameterValueException, message example "A user password must not contain any of the characters ',' '\"' '/' '@'." — the refusal comes from FORBIDDEN_PASSWORD_CHARS in services/service-com-amazonaws-memorydb-amazonmemorydb/src/state.rs, used by authentication_from_mode.
Why: a client with a generated or policy-mandated password containing any of those characters cannot create a user at all against the emulator, while the same call succeeds on AWS.
Fix: drop the character-class rejection from the shared password validation (authentication_from_mode in src/state.rs) used by the CreateUser and UpdateUser handlers, keeping the length and count rules.
Traces: memorydb.user-password-punctuation-accepted.

## Evidence
Suite: simparity-behavioral-aws-v1, memorydb 0/20 passed.

Cause 1 — memorydb.acl-membership-reflected-on-users:
  KeyError: 'MinimumEngineVersion' at `assert acl["MinimumEngineVersion"] == "6.2"` after DescribeACLs on a settled password-only ACL. Expected: '6.2' for a password-only ACL, '7.0' once an IAM user is a member. Actual: member absent from the ACL (and from User) body.

Cause 2 — memorydb.default-user-and-open-access-acl-immutable:
  botocore.errorfactory.ACLNotFoundFault: An error occurred (ACLNotFoundFault) when calling the DescribeACLs operation: ACL open-access not found.
  Expected: DescribeACLs(ACLName='open-access') returns the built-in ACL.
  memorydb.redis-engine-rejects-a-valkey-only-engine-version:
  CreateCluster(ClusterName=..., Engine='redis', EngineVersion='7.2', ACLName='open-access') raised ('ACLNotFoundFault', 'ACL open-access not found.'); expected ('InvalidParameterCombinationException', 'Invalid engine version 7.2 specified for the cluster'). The engine-version refusal itself is therefore unverified — the request never got past the ACL lookup.

Cause 3 — memorydb.describe-users-pages-cover-every-user:
  AssertionError: describe_users MaxResults=0 gave ValidationException: 'MaxResults must be at least 1.'
  Expected error code: InvalidParameterValueException (HTTP 400), message example 'Invalid value for max results. Must be between 1 and 50'. MaxResults=51 and the DescribeACLs equivalents are on the same path.

Cause 4 — memorydb.parameter-update-and-reset-roundtrip:
  KeyError: 'Description' at `assert created["Description"] == " "` on the CreateParameterGroup(Family='memorydb_valkey7') response with no Description in the request.

Cause 5 — memorydb.resource-name-rules-rejected:
  AssertionError: expected the call to be rejected, but it succeeded: {'User': {'Name': 'sim2real-mdb-fa70e00a5cb1-bbbbbbbbbbbbbbb' (41 chars), 'Status': 'active', 'AccessString': 'on ~* +@all', ...}, 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}
  Expected: InvalidParameterValueException, message example 'User id length must be less than 40.' (and the pattern message for a leading digit, an underscore, or a double hyphen; same for ACL names).

Cause 6 — memorydb.resource-names-fold-to-lowercase:
  AssertionError: user stored as 'sim2real-mdb-5f48e841672b-MiXed', expected 'sim2real-mdb-5f48e841672b-mixed'.

Cause 7 — memorydb.user-password-count-and-auth-mode-transitions:
  AssertionError: access string was not normalised: 'on ~* +@all' (expected 'on ~* resetchannels +@all' on the CreateUser response).

Cause 8 — memorydb.user-password-punctuation-accepted:
  botocore.errorfactory.InvalidParameterValueException: An error occurred (InvalidParameterValueException) when calling the CreateUser operation: A user password must not contain any of the characters ',' '\"' '/' '@'.
  Expected: HTTP 200 and an active one-password user for each of '@', '/', ',', '\"', ' ', '\\'.

## Steps to Reproduce
Boot the emulator and point a botocore memorydb client at it, then:

Cause 1: create_user(UserName=u, AccessString='on ~* +@all', AuthenticationMode={'Type':'password','Passwords':['Sim2realPassword-1']}); create_acl(ACLName=a, UserNames=[u]); poll describe_acls(ACLName=a) until Status=='active' and read ACLs[0]['MinimumEngineVersion'] (expect '6.2'); add an iam-type user with update_acl and re-read (expect '7.0').
Cause 2: describe_acls(ACLName='open-access') on a fresh emulator (expect the built-in ACL); create_cluster(ClusterName=c, NodeType='db.t4g.small', Engine='redis', EngineVersion='7.2', ACLName='open-access') (expect InvalidParameterCombinationException about the engine version, not ACLNotFoundFault).
Cause 3: describe_users(MaxResults=0) and describe_users(MaxResults=51); likewise describe_acls (expect error code InvalidParameterValueException).
Cause 4: create_parameter_group(ParameterGroupName=p, Family='memorydb_valkey7') with no Description and read ParameterGroup['Description'] (expect ' ').
Cause 5: create_user with a 41-character UserName, a UserName starting with a digit, one containing '_', and one containing '--' (each expect InvalidParameterValueException and no resource created); same four names via create_acl.
Cause 6: create_user(UserName='<base>-MiXed', ...) and read User['Name'] (expect the lowercase spelling), then describe_users(UserName='<base>-mixed') and a second create_user with the lowercase name (expect UserAlreadyExistsFault); repeat with create_acl and with create_parameter_group(ParameterGroupName='<base>-MiXed', Family='MEMORYDB_VALKEY7') (expect Family 'memorydb_valkey7').
Cause 7: create_user(UserName=u, AccessString='on ~* +@all', AuthenticationMode={'Type':'password','Passwords':['Sim2realPassword-1']}) and read User['AccessString'] (expect 'on ~* resetchannels +@all').
Cause 8: for ch in ['@','/',',','\"',' ','\\']: create_user(UserName=f'{base}-{i}', AccessString='on ~* +@all', AuthenticationMode={'Type':'password','Passwords':[f'Sim2realPassword{ch}x']}) (each expect HTTP 200 and PasswordCount 1).
