Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ds-data.absent-directory-is-denied-rather-than-reported-missing ds-data.client-token-is-validated-on-every-mutating-operation-rejected ds-data.pagination-token-is-not-parsed-before-authorization-rejects ds-data.sam-account-name-shape-differs-by-operation-family ds-data.attribute-value-union-rejects-every-member-but-string ds-data.sensitive-member-values-are-withheld-when-rejected ds-data.realm-members-are-validated-independently-and-never-resolved
Issue-Shape-Id: com.amazonaws.directoryservicedata#DirectoryServiceData
Issue-Key: ds-data.absent-directory-is-denied-rather-than-reported-missing|_refusal#7|AssertionError|raise AssertionError(
Issue-Key: ds-data.client-token-is-validated-on-every-mutating-operation-rejected|_refusal#7|AssertionError|raise AssertionError(f"the call was expected to be refused but SUCCEEDED with {result!r}")
Issue-Key: ds-data.pagination-token-is-not-parsed-before-authorization-rejects|_refusal#6|AssertionError|raise AssertionError(f"the call was expected to be refused but SUCCEEDED with {result!r}")
Issue-Key: ds-data.sam-account-name-shape-differs-by-operation-family|run#8|AssertionError|assert at_20[0] == "AccessDeniedException", (
Issue-Key: ds-data.attribute-value-union-rejects-every-member-but-string|_refusal#7|AssertionError|raise AssertionError(f"the call was expected to be refused but SUCCEEDED with {result!r}")
Issue-Key: ds-data.sensitive-member-values-are-withheld-when-rejected|run#5|AssertionError|assert message == (
Issue-Key: ds-data.realm-members-are-validated-independently-and-never-resolved|run#7|AssertionError|assert message == (
Issue-Commit: 5e32f6bad4f368147eb90818a03953ca666b028c

# ds-data: no directory-authorization gate (absent DirectoryId served/404'd instead of AccessDenied 403 IAM_AUTH), non-string AttributeValue members accepted, @sensitive values echoed in validation messages, multi-member clause order wrong (4 causes)

> _Note: this issue was identified against the code base at commit `5e32f6bad4f368147eb90818a03953ca666b028c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeUser

## Description
Four distinct root causes in the Directory Service Data (ds-data) emulator, all grounded in failing behavioral checks of this suite. All four are in one service, so they are bundled; a fixer must close ALL FOUR before submitting.

=== Cause 1 of 4: ds-data has no directory-authorization gate — a well-formed DirectoryId the account does not hold is served, or reported as not-found, instead of being refused with AccessDeniedException ===
Failing checks: ds-data.absent-directory-is-denied-rather-than-reported-missing, ds-data.client-token-is-validated-on-every-mutating-operation-rejected, ds-data.pagination-token-is-not-parsed-before-authorization-rejects, ds-data.sam-account-name-shape-differs-by-operation-family
AWS: every ds-data operation answers a syntactically valid DirectoryId (matching the modeled pattern ^d-[0-9a-f]{10}$) that the caller cannot reach with ONE envelope: AccessDeniedException, HTTP 403, with the modeled Reason member set to IAM_AUTH and a message that names the CALLER's principal and withholds the resource (recorded form: "User: <caller principal ARN> is not authorized to access this resource"). Smithy (com-amazonaws-directoryservicedata): AccessDeniedException @error client, httpError 403, member Reason -> AccessDeniedReason enum [IAM_AUTH, DIRECTORY_AUTH, DATA_DISABLED]; the AccessDeniedException doc states verbatim "It can also occur when the DirectoryId doesn't exist or the user, member, or group might be outside of your organizational unit (OU)". The suite's recordings (us-west-2, 2026-09-11) measured this refusal as uniform over all seventeen ds-data operations, including the eleven that also declare ResourceNotFoundException — so for an unreachable directory the modeled ResourceNotFoundException is never produced. The refusal happens BEFORE pagination-token parsing and BEFORE ClientToken semantics, so a fabricated NextToken or a valid ClientToken on an unreachable directory is still a 403 IAM_AUTH.
Emulator: there is no authorization layer at all; each handler resolves the DirectoryId straight out of local state. Reads on an unknown directory either (a) succeed with HTTP 200 and a fabricated directory — e.g. ListUsers(DirectoryId=<absent>) returned 200 {"DirectoryId": "d-4f8a1c07e2", "Realm": "corp.example.com", "Users": []} — or (b) return ResourceNotFoundException (DescribeUser on an absent directory produced "user <name> was not found in directory <id>"). Writes on an unknown directory SUCCEED and materialize state: CreateUser(DirectoryId=<absent>, SAMAccountName="sim2real-behav-u") returned HTTP 200 with a minted SID.
Why: a client gets the opposite of AWS's contract in both directions. Code paths that key on AccessDeniedException/Reason=IAM_AUTH to detect "this directory is not mine / data access is not available" never fire; code paths that treat 200 as proof the directory exists silently write users into a directory that does not exist, so a provisioning bug that AWS refuses with 403 is absorbed by the emulator and surfaces only in production. It also leaks the opposite information-hiding property: ds-data must not confirm or deny the directory's existence, and the emulator does both.
Fix: a shared directory-authorization gate in the ds-data crate, applied ahead of every operation handler and ahead of pagination-token/ClientToken handling — services/service-com-amazonaws-directoryservicedata-directoryservicedata/src/ops/*.rs currently each do their own `store.directories.get(&input.directory_id)` fallback (see Traces), so the gate belongs in that crate's shared state/middleware layer, with the handlers' not-found fallback reserved for objects INSIDE a reachable directory.

=== Cause 2 of 4: the non-string members of the AttributeValue union (N, BOOL, SS) are accepted where AWS refuses them with ValidationException / Reason INVALID_ATTRIBUTE_VALUE ===
Failing check: ds-data.attribute-value-union-rejects-every-member-but-string
AWS: OtherAttributes values are an AttributeValue union whose modeled members are S, N, BOOL and SS (Smithy: union AttributeValue, all four members @sensitive). Recorded behavior (us-west-2, 2026-09-11): a ds-data semantic attribute layer, which runs BEFORE authorization, refuses an entry sent as N, BOOL or SS with ValidationException (HTTP 400) and the modeled Reason INVALID_ATTRIBUTE_VALUE (ValidationExceptionReason enum includes INVALID_ATTRIBUTE_VALUE), message of the form "Invalid value for attribute: '<attributeName>'." — i.e. a different grammar from the Smithy-constraint layer, which says "<N> validation error(s) detected: ... failed to satisfy constraint: ..." and populates NO Reason. The same attribute sent as S passes this layer and is then stopped by authorization (403, IAM_AUTH), proving the layer ordering. The rejection is a property of the union MEMBER, not of the attribute's AD type.
Emulator: the non-string members are accepted. CreateUser(DirectoryId=<absent>, SAMAccountName="sim2real-behav-u", OtherAttributes={<attr>: {N|BOOL|SS: ...}}) returned HTTP 200 with a minted SID instead of any refusal — a model-driven implementation of all four union members.
Why: a client that writes a numeric or multi-valued attribute through OtherAttributes sees it accepted in the emulator and refused by AWS with ValidationException/INVALID_ATTRIBUTE_VALUE, so every caller of this extension point (the only way to read/write non-first-class AD attributes) is validated against the wrong contract. Code that branches on Reason INVALID_ATTRIBUTE_VALUE is dead in the emulator.
Fix: a semantic attribute-validation step in the ds-data crate's attribute handling, running before the authorization gate of Cause 1, for every operation that carries OtherAttributes/attribute values (create_user, update_user, create_group, update_group, add_group_member and siblings under services/service-com-amazonaws-directoryservicedata-directoryservicedata/src/ops/), emitting ValidationException with Reason INVALID_ATTRIBUTE_VALUE and its own message grammar rather than the constraint layer's.

=== Cause 3 of 4: @sensitive member values are echoed in constraint-validation messages instead of being withheld ===
Failing check: ds-data.sensitive-member-values-are-withheld-when-rejected
AWS: ds-data's constraint-validation refusal has one grammar with the value clause switched off for members the model marks @sensitive. Smithy: SearchUsersRequest.SearchString @sensitive, @length max 64; CreateUserRequest.GivenName / Surname @sensitive, @length max 64; CreateUserRequest.EmailAddress @sensitive, @length max 256; CreateUserRequest.SAMAccountName (UserName) is NOT sensitive, @length max 20. Recorded behavior: a non-sensitive member is reported as "Value '<value>' at '<member>' failed to satisfy constraint: ..." while a @sensitive member collapses to "Value at '<member>' failed to satisfy constraint: ..." — the value is removed, the member name and the violated constraint are still disclosed. The decision is per MEMBER inside one message (a request violating both echoes one value and withholds the other), and inside a structure the trait is honoured with an explicit marker ("AttributeValue(s=*** REDACTED ***)") while the non-sensitive map key is still echoed.
Emulator: the single validation template echoes every value. SearchUsers with a 65-character SearchString produced "1 validation error detected: Value 'sssss…(65 chars)' at 'searchString' failed to satisfy constraint: Member must have length less than or equal to 64" — the @sensitive value verbatim, where AWS emits "Value at 'searchString' …".
Why: the rule that produces this text is stated by the model (@sensitive) and the message is built from the request, so it is a requirement, not incidental wording. The emulator leaks exactly the values the trait exists to protect — user mail addresses, given names, surnames, search strings — into client logs and error reports, and any client assertion or redaction test built against AWS's "Value at '<member>'" form fails.
Fix: the shared constraint-validation/error-rendering path of the ds-data service (the centralized deserialize → validate walk that formats "N validation error(s) detected"), which must read the @sensitive trait per member and drop the value clause (and apply the nested "*** REDACTED ***" marker inside structures) rather than formatting every member identically.

=== Cause 4 of 4: multi-member constraint violations are reported in the wrong clause order ===
Failing check: ds-data.realm-members-are-validated-independently-and-never-resolved
AWS: when one request carries two malformed members, ds-data reports BOTH in one ValidationException with a fixed clause order that is neither request order nor first-fault-wins. Recorded behavior (us-west-2, 2026-09-11) for ListGroupMembers with both Realm and MemberRealm malformed: "2 validation errors detected: Value '<memberRealm>' at 'memberRealm' failed to satisfy constraint: Member must satisfy regular expression pattern: ([a-zA-Z0-9]+[\\.-])+([a-zA-Z0-9])+[.]?; Value '<realm>' at 'realm' failed to satisfy constraint: ..." — memberRealm FIRST, realm second. Smithy: both members target Realm with that pattern, so the clause text itself is model-derived; the ORDER is recorded behavior only.
Emulator: both violations are reported (count and per-clause text match) but in the opposite order — realm first, memberRealm second: "2 validation errors detected: Value 'not a realm!' at 'realm' failed to satisfy constraint: …; Value 'also not a realm!' at 'memberRealm' failed to satisfy constraint: …".
Why: the error code, status and violation count are correct, so the impact is narrow: clients that parse or snapshot the aggregated message (and this suite's recorded oracle) see the clauses transposed. Noted as the lowest-severity cause of this bundle; the divergence is in the deterministic ordering rule of the aggregated message, not in its wording.
Fix: the clause-ordering rule of the same shared constraint-validation renderer named in Cause 3, which must emit member clauses in the service's fixed order rather than in request/member-declaration order.

Note: the open/fixed ds-data issues in this repo do not cover any of the above — 20261022T000042Z-ds-data-validationexception-body-omits-message-for-every-member-constraint (fixed) only restored the Message member, and 20261022T000042Z-ds-data-request-layer-errors-answer-501-notimplemented-instead-of (fixed) covered the request layer envelope.

## Evidence
Cause 1 — expected (AWS, all 17 ops): AccessDeniedException / HTTP 403 / Reason "IAM_AUTH" / message "User: <caller ARN> is not authorized to access this resource".
  actual (emulator): ListUsers(DirectoryId=d-4f8a1c07e2) -> HTTP 200 {'DirectoryId': 'd-4f8a1c07e2', 'Realm': 'corp.example.com', 'Users': []} (check ds-data.absent-directory-is-denied-rather-than-reported-missing: "the call was expected to be refused but SUCCEEDED").
  actual: ListUsers/list op with absent DirectoryId d-1a05c3f7b8 -> HTTP 200, same fabricated envelope (ds-data.pagination-token-is-not-parsed-before-authorization-rejects).
  actual: CreateUser(DirectoryId=d-5e9207cba4, ClientToken='sim2real-behav-client-token-01') -> HTTP 200 {'SAMAccountName': 'sim2real-behav-u', 'SID': 'S-1-5-21-2066772624-1458028014-2881993578-1000'} (ds-data.client-token-is-validated-on-every-mutating-operation-rejected).
  actual: DescribeUser with a legal 20-char SAMAccountName on absent directory d-63be20d149 -> ('ResourceNotFoundException', 'user uuuuuuuuuuuuuuuuuuuu was not found in directory d-63be20d149', Reason None) where AWS gives AccessDeniedException (ds-data.sam-account-name-shape-differs-by-operation-family).
  code: services/service-com-amazonaws-directoryservicedata-directoryservicedata/src/ops/describe_user.rs:187-198 — "A read NEVER materialises a directory" then `store.directories.get(&input.directory_id)` -> ResourceNotFoundException; no AccessDeniedException path is reachable despite DescribeUserErrorModeled::AccessDeniedException existing at line 118.

Cause 2 — expected: CreateUser(..., OtherAttributes={<attr>: {'N'|'BOOL'|'SS': ...}}) -> ValidationException / HTTP 400 / Reason "INVALID_ATTRIBUTE_VALUE" / message "Invalid value for attribute: '<attr>'."
  actual: HTTP 200 {'DirectoryId': 'd-2b71e90c31', 'SAMAccountName': 'sim2real-behav-u', 'SID': 'S-1-5-21-1981470323-419191285-1959582543-1000'} — the union member was accepted and a user was created.

Cause 3 — expected: "1 validation error detected: Value at 'searchString' failed to satisfy constraint: Member must have length less than or equal to 64" (value withheld; member and bound disclosed).
  actual: "1 validation error detected: Value 'sssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssss' at 'searchString' failed to satisfy constraint: Member must have length less than or equal to 64" — the @sensitive value echoed.

Cause 4 — expected: "2 validation errors detected: Value '<memberRealm>' at 'memberRealm' failed to satisfy constraint: Member must satisfy regular expression pattern: ([a-zA-Z0-9]+[\\.-])+([a-zA-Z0-9])+[.]?; Value '<realm>' at 'realm' failed to satisfy constraint: <same pattern>".
  actual: "2 validation errors detected: Value 'not a realm!' at 'realm' failed to satisfy constraint: Member must satisfy regular expression pattern: ([a-zA-Z0-9]+[\\.-])+([a-zA-Z0-9])+[.]?; Value 'also not a realm!' at 'memberRealm' failed to satisfy constraint: <same pattern>" — clauses transposed.

## Steps to Reproduce
Boot the emulator and point a botocore 'ds-data' client at it with credentials for an account that holds NO directory with the id used below (any id matching ^d-[0-9a-f]{10}$ that DescribeDirectories does not list).

Cause 1:
1. dsd.list_users(DirectoryId='d-4f8a1c07e2') -> emulator answers HTTP 200 with an empty Users list; AWS answers AccessDeniedException 403 Reason IAM_AUTH.
2. dsd.create_user(DirectoryId='d-5e9207cba4', SAMAccountName='sim2real-behav-u', ClientToken='sim2real-behav-client-token-01') -> emulator answers HTTP 200 and mints a SID; AWS answers AccessDeniedException 403 Reason IAM_AUTH.
3. dsd.describe_user(DirectoryId='d-63be20d149', SAMAccountName='u'*20) -> emulator answers ResourceNotFoundException; AWS answers AccessDeniedException 403 Reason IAM_AUTH.
4. Repeat 1 with a fabricated NextToken: the emulator still answers 200; AWS still answers 403 IAM_AUTH (authorization precedes token parsing).

Cause 2:
5. dsd.create_user(DirectoryId='d-2b71e90c31', SAMAccountName='sim2real-behav-u', OtherAttributes={'codePage': {'N': 16}}) — likewise with {'BOOL': True} and {'SS': ['a','b']} -> emulator answers HTTP 200; AWS answers ValidationException 400, Reason INVALID_ATTRIBUTE_VALUE, message "Invalid value for attribute: 'codePage'."

Cause 3:
6. dsd.search_users(DirectoryId=<absent>, SearchString='s'*65, SearchAttributes=['Cn']) -> compare the Message: the emulator echoes the 65-char @sensitive value; AWS emits "Value at 'searchString' …".
7. dsd.create_user(DirectoryId=<absent>, SAMAccountName='s'*21, GivenName='g'*65) -> AWS echoes sAMAccountName's value and withholds givenName's in the SAME message.

Cause 4:
8. dsd.list_group_members(DirectoryId=<absent>, SAMAccountName='sim2real-behav-g', Realm='not a realm!', MemberRealm='also not a realm!') -> compare clause order: AWS reports memberRealm first, the emulator reports realm first.

Then re-replay the checks listed in trace_ids.
