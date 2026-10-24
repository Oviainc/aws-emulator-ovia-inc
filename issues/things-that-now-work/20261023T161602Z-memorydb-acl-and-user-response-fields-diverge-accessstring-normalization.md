Issue-Source: trace-sweep-divergence
Issue-Trace: memorydb-user-authentication-rules
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: memorydb-user-authentication-rules memorydb-user-lifecycle-and-credential-rules memorydb-default-user-and-open-access-guards
Issue-Shape-Id: com.amazonaws.memorydb#AmazonMemoryDB
Issue-Key: memorydb|CreateUser|2xx->2xx|$.User.AccessString
Issue-Key: memorydb|DescribeUsers|2xx->2xx|$.Users[*].AccessString
Issue-Key: memorydb|DeleteUser|2xx->2xx|$.User.AccessString
Issue-Key: memorydb|UpdateUser|2xx->2xx|$.User.AccessString
Issue-Key: memorydb|DescribeACLs|2xx->2xx|$.ACLs[*].UserNames[*]
Issue-Key: memorydb|DeleteACL|2xx->2xx|$.ACL.UserNames[*]
Issue-Commit: 205916506edbd7602d3640584e73a4ccd0a8f366

# ACL and user response fields diverge: AccessString normalization, default user

> _Note: this issue was identified against the code base at commit `205916506edbd7602d3640584e73a4ccd0a8f366`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateUser

## Description
Six causes in two closely-related response-shaping faults on successful (HTTP 200) calls: the user AccessString the emulator stores and echoes is not normalized the way AWS normalizes it, and newly created ACLs are reported with a 'default' member AWS does not include. All six causes must be closed before submitting.

Cause 1 of 6: CreateUser echoes the AccessString verbatim instead of normalized. AWS: recorded behavior only (Smithy: CreateUserRequest.AccessString / User.AccessString are plain Strings, doc 'Access permissions string used for this user.' — no normalization rule is modeled). The recording shows AWS returning 'on ~app:* resetchannels -@all +@read' for a request whose access string granted '+@read': AWS inserts the explicit '-@all' deny before the granted categories. Emulator: returns 'on ~app:* resetchannels +@read' — the '-@all' term is absent. Why: a client that compares the stored AccessString with the normalized form AWS returns (drift detection, idempotent reconciliation, IaC diffing) sees a permanent difference and re-applies the user forever.
Fix: the memorydb user state's access-string normalization, applied where the user is stored so every echo path agrees.
Traces: memorydb-user-authentication-rules#task#2.

Cause 2 of 6: DescribeUsers reports the same un-normalized AccessString. AWS: 'on ~app:* resetchannels -@all +@read'. Emulator: '-@all' missing. Fix: same normalization (read path inherits it once state is normalized). Traces: memorydb-user-authentication-rules#task#3.

Cause 3 of 6: DeleteUser's returned User carries the un-normalized AccessString. AWS: normalized form with '-@all'. Emulator: '-@all' missing. Fix: same. Traces: memorydb-user-authentication-rules#task#12.

Cause 4 of 6: UpdateUser's returned User carries the un-normalized AccessString for an updated access string ('on ~app::* resetchannels -@all +@read' expected). Emulator: '-@all' missing. Fix: the same normalization must run on update, not only on create. Traces: memorydb-user-lifecycle-and-credential-rules#task#7.

Cause 5 of 6: DescribeACLs reports a 'default' user in a newly created ACL's UserNames. AWS: recorded behavior — an ACL created without UserNames is returned with an empty UserNames list (Smithy: ACL.UserNames, doc 'The list of user names that belong to the ACL.'). Emulator: UserNames[0] = 'default'. Why: a client enumerating ACL membership sees a member AWS does not report, and any membership assertion or diff against AWS fails; removing it would also behave differently from AWS. Fix: the ACL state's creation path — do not seed the default user into UserNames.
Traces: memorydb-default-user-and-open-access-guards#task#8.

Cause 6 of 6: DeleteACL's returned ACL likewise carries UserNames[0] = 'default' where AWS returns an empty list. Fix: same ACL creation/serialization path. Traces: memorydb-default-user-and-open-access-guards#task#9.

## Evidence
Cause 1: HTTP 200 both sides; $.User.AccessString expected 'on ~app:* resetchannels -@all +@read', got 'on ~app:* resetchannels +@read' (memorydb-user-authentication-rules task#2). Cause 2: $.Users[0].AccessString same divergence (task#3; cluster also covers memorydb-user-lifecycle-and-credential-rules task#9). Cause 3: $.User.AccessString same divergence (task#12; also memorydb-user-lifecycle-and-credential-rules teardown#11). Cause 4: $.User.AccessString expected 'on ~app::* resetchannels -@all +@read', got 'on ~app::* resetchannels +@read' (memorydb-user-lifecycle-and-credential-rules task#7). Cause 5: $.ACLs[0].UserNames[0] extra (expected None, got 'default') (memorydb-default-user-and-open-access-guards task#8). Cause 6: $.ACL.UserNames[0] extra (expected None, got 'default') (task#9).

## Steps to Reproduce
Replay trace set simparity-wire-aws-v1. (1) CreateUser with an access string of the form 'on ~app:* resetchannels +@read', then DescribeUsers, UpdateUser and DeleteUser for that user, and compare the echoed AccessString with the recording: AWS returns it with an explicit '-@all' inserted before the granted categories, the emulator omits it. (2) CreateACL with no UserNames, then DescribeACLs and DeleteACL for it: AWS reports UserNames as empty, the emulator reports ['default']. Fixtures/steps: memorydb-user-authentication-rules#task#2, #task#3, #task#12, memorydb-user-lifecycle-and-credential-rules#task#7, memorydb-default-user-and-open-access-guards#task#8, #task#9.
