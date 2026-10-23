Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ds.ds-data-user-create-describe-search-roundtrip
Issue-Key: ds.ds-data-user-create-describe-search-roundtrip|run#17|AssertionError|assert desc.get("Enabled") is False, (
Issue-Commit: a59989a9f63c403b66485a4e93f54ab6df22c2a6

# ds-data user objects are not real Active Directory objects: Enabled defaults true, and Realm/DN/UPN/attribute names are hardcoded rather than derived from the directory (6 causes)

> _Note: this issue was identified against the code base at commit `a59989a9f63c403b66485a4e93f54ab6df22c2a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeUser

## Description
The ds-data (Directory Service Data) object store answers DescribeUser from a record whose directory-derived fields are constants, not properties of the `ds` directory the object lives in. Six distinct root causes, all observable in the single DescribeUser response the failing check recorded. The check asserts them in sequence and aborts at the first (Cause 1), so Causes 2-6 are evidenced from that same recorded response body and will each surface in turn as the earlier ones are fixed — all six must be closed before re-replaying.

Cause 1 of 6: a ds-data-created user comes back Enabled=true.
AWS: CreateUser (Smithy com.amazonaws.directoryservicedata#CreateUser) takes NO password member — its input is DirectoryId, SAMAccountName, EmailAddress, GivenName, Surname, OtherAttributes, ClientToken only. Active Directory will not activate an account with no password, so DescribeUser reports Enabled=false (DescribeUserResult$Enabled, target Boolean) for an account created this way; DisableUser/EnableUser are what move the flag afterwards. Recorded behavior only for the false default (the model documents the member but not the default).
Emulator: DescribeUser answered HTTP 200 with 'Enabled': True.
Why: a client that keys on Enabled to decide whether an account still needs a password set / needs enabling takes the wrong branch, and provisioning flows that assert the password-less account is inactive silently pass against the emulator and fail against AWS. UserSummary$Enabled carries the same wrong value into ListUsers/SearchUsers.
Fix: CreateUser's handler — the created User record's `enabled` field, in services/service-com-amazonaws-directoryservicedata-directoryservicedata/src/ops/create_user.rs (see Traces for the line), and the EnableUser/DisableUser transitions that must remain the only way to change it.
Traces: ds.ds-data-user-create-describe-search-roundtrip. Component: src/ops/create_user.rs:247-250 sets `enabled: true` with a comment reasoning from DisableUser's doc; src/state.rs:107-108 holds the field and src/state.rs:118-126 projects it into UserSummary.

Cause 2 of 6: Realm and the base DN are a build-time constant, not the directory's own domain name.
AWS: a directory's Kerberos realm / AD domain name is chosen by the `ds` control plane at CreateMicrosoftAD (Name=<fqdn>), and ds-data reports THAT domain. Smithy: DescribeUserResult$Realm (target Realm, length 1..255, pattern ^([a-zA-Z0-9]+[\\.-])+([a-zA-Z0-9])+[.]?$) — the member exists precisely so the data plane can state the domain it resolved the object in, and DescribeUserRequest$Realm is matched against it. Recorded behavior: for a directory created with Name=<fqdn>, DescribeUser returns Realm=<fqdn> and a DistinguishedName ending in that fqdn's DC= components.
Emulator: for a directory whose ds Name was `<unique>.example.com` (visible in the same response's EmailAddress, `sim2realUser1@sim2real-behav-1105d345e209.example.com`, which is echoed from the request), DescribeUser answered Realm 'corp.example.com' and DistinguishedName 'CN=sim2realUser1,CN=Users,DC=corp,DC=example,DC=com'. Every directory gets the same realm regardless of the domain name it was created with.
Why: the realm and the base DN are the two values a client uses to build an LDAP bind/search or a Kerberos principal against the directory it just created. Both are wrong for every directory, so nothing derived from them resolves; a request that correctly passes Realm=<its own fqdn> is also refused as naming a foreign domain by the realm matcher.
Fix: the ds-data state must take the directory's domain name from the `ds` control-plane directory record (DirectoryDescription$Name) when it materializes a Directory, instead of a module constant; the realm matcher and DN builder then follow. Cross-plane state sharing between the ds and ds-data crates is the real work here.
Traces: ds.ds-data-user-create-describe-search-roundtrip. Component: src/state.rs:28-34 `pub const DEFAULT_REALM: &str = "corp.example.com"`, src/state.rs:201-210 `Directory::new` assigning it, src/state.rs:220-228 `domain_components`, src/state.rs:522-534 `realm_matches`/`resolved_realm`.

Cause 3 of 6: DistinguishedName places the user in the domain's default CN=Users container instead of the delegated OU.
AWS: recorded behavior only — a Managed Microsoft AD does not give the customer write access to the domain's CN=Users container; a ds-data-created user lands under the delegated OU named after the directory's NetBIOS ShortName, i.e. 'CN=<SAMAccountName>,OU=Users,OU=<ShortName>,<DC= components of the domain name>'. The DN is derived by the directory from its own naming context; CreateUser sends no DN. Smithy: DescribeUserResult$DistinguishedName (target DistinguishedName, length 1..256, @sensitive).
Emulator: 'CN=sim2realUser1,CN=Users,DC=corp,DC=example,DC=com' — the ',OU=Users,OU=<ShortName>,' segment is absent entirely, and the container is CN=Users.
Why: the DN is what a client hands to LDAP tooling, to a GPO scope or to an ACL. A DN naming a container the object is not in (and which in a Managed AD the caller cannot write) fails against the real directory, and code that parses the OU out of the DN to learn which directory delegated the object gets nothing.
Fix: the DN builder in the ds-data state must compose the delegated-OU path from the directory's NetBIOS ShortName (also a `ds` control-plane value, same cross-plane dependency as Cause 2).
Traces: ds.ds-data-user-create-describe-search-roundtrip. Component: src/state.rs:230-237 `Directory::distinguished_name` formats `CN={name},CN=Users,{domain_components}`; called from src/ops/create_user.rs:240.

Cause 4 of 6: UserPrincipalName is built against the constant realm in its stored case, not the uppercased domain name.
AWS: recorded behavior only — CreateUser never sends a UPN; Active Directory derives it as <sAMAccountName>@<DOMAIN NAME UPPERCASED>. Smithy: DescribeUserResult$UserPrincipalName (target UserPrincipalName, length 1..256, @sensitive) is output-only and has no input counterpart on CreateUser, confirming it is derived.
Emulator: 'sim2realUser1@corp.example.com' — wrong domain (Cause 2) and lower case, so even once the realm is correct the UPN still diverges by case.
Why: a UPN is a logon name. A client that authenticates or looks the principal up by the UPN it read back from DescribeUser presents a name the directory does not hold.
Fix: the UPN derivation in the ds-data state — uppercase the directory's domain name when composing it.
Traces: ds.ds-data-user-create-describe-search-roundtrip. Component: src/state.rs:239-242 `Directory::user_principal_name` formats `{sam}@{self.realm}` verbatim; called from src/ops/create_user.rs:241.

Cause 5 of 6: OtherAttributes are returned under the request's spelling instead of the canonical AD schema display name.
AWS: recorded behavior only — the service resolves a requested attribute name case-insensitively against the AD schema and answers with the SCHEMA's spelling: an attribute written by CreateUser as 'description' comes back from DescribeUser keyed 'Description'. This is a directory-side normalization, not an echo. Smithy supports the case-insensitivity half: CreateUserRequest$OtherAttributes documents "Attribute names are case insensitive" (Attributes map, length 1..25).
Emulator: DescribeUser answered 'OtherAttributes': {'description': {'S': 'sim2real-behavioral-roundtrip'}} — the key is exactly the one the request supplied, so any casing a client writes is the casing it reads back.
Why: a client that indexes the returned map by the canonical LDAP display name finds nothing, and two writers using different casing produce two different observable keys for one attribute.
Fix: the attribute store/projection in the ds-data state must canonicalize an attribute's LdapDisplayName against a schema name table on store and project under the canonical name.
Traces: ds.ds-data-user-create-describe-search-roundtrip. Component: src/state.rs:450-455 `stored_attributes` collects the request map unchanged; src/state.rs:134-147 `User::projected_attributes` matches case-insensitively but emits `name.clone()` — the stored (request) spelling — and src/state.rs:109-111 documents this as intentional.

Cause 6 of 6: the ResourceNotFoundException message for a user missing inside a reachable directory does not follow the recorded template.
AWS: recorded behavior only — DescribeUser/UpdateUser on a deleted user answer ResourceNotFoundException, HTTP 404 (Smithy: ResourceNotFoundException @error client, httpError 404, declared on both operations), with a message built from the request as "User not found in directory <DirectoryId>." — capitalized object kind, no object name, trailing period. The status and code are the requirement; the message is quoted here because its rule is known (it is composed from the request's DirectoryId and is asserted by exact equality in the blessed check).
Emulator: constructs "user <SAMAccountName> was not found in directory <DirectoryId>" — different template, and it discloses the object name AWS withholds.
Why: the 404/ResourceNotFoundException contract itself is correct, so this is the narrowest cause in the bundle; it matters because the check asserts the template exactly and because the emulator's wording leaks the queried account name.
Fix: the not-found payload builders in the ds-data state.
Traces: ds.ds-data-user-create-describe-search-roundtrip (reached only after Causes 1-5 are fixed). Component: src/state.rs:536-561 `user_not_found` / `group_not_found` / `member_not_found`.

Relation to already-open ds/ds-data issues: 20261022T085720Z-ds-ds-data-cross-plane-contract-broken-ds.md and 20261022T085720Z-ds-data-no-directory-authorization-gate-absent-directoryid.md both concern the AUTHORIZATION GATE in front of an ABSENT DirectoryId (AccessDeniedException/403/IAM_AUTH vs ResourceNotFoundException). This issue is disjoint: every call here is against a live, Active, data-access-enabled directory the caller owns, and the divergence is in the CONTENT of the user object the data plane serves. 20261022T175927Z-ds-directory-creation-skips-password-and-vpc-validation.md concerns ds control-plane input validation and error envelopes, not ds-data object fidelity.

## Evidence
Failing check: ds.ds-data-user-create-describe-search-roundtrip [mismatch].

Assertion that aborted the check (check_ds_ds_data_user_create_describe_search_roundtrip.py line 331):
  assert desc.get("Enabled") is False, "a ds-data CreateUser sets no password, so Active Directory must leave the account INACTIVE -- Enabled must be False"

Actual DescribeUser response recorded from the emulator (HTTP 200), for a user created moments earlier by ds-data CreateUser(DirectoryId=d-003ba5fe5a, SAMAccountName='sim2realUser1', GivenName='Sim', Surname='Real', EmailAddress='sim2realUser1@sim2real-behav-1105d345e209.example.com', OtherAttributes={'description': {'S': 'sim2real-behavioral-roundtrip'}}) in a directory created by ds CreateMicrosoftAD(Name='sim2real-behav-1105d345e209.example.com', ShortName='SIM2REAL'):
  {'DirectoryId': 'd-003ba5fe5a',
   'DistinguishedName': 'CN=sim2realUser1,CN=Users,DC=corp,DC=example,DC=com',
   'EmailAddress': 'sim2realUser1@sim2real-behav-1105d345e209.example.com',
   'Enabled': True,
   'GivenName': 'Sim',
   'OtherAttributes': {'description': {'S': 'sim2real-behavioral-roundtrip'}},
   'Realm': 'corp.example.com',
   'SAMAccountName': 'sim2realUser1',
   'SID': 'S-1-5-21-2886073900-3733782474-697738302-1000',
   'Surname': 'Real',
   'UserPrincipalName': 'sim2realUser1@corp.example.com'}

Expected (AWS), per field, for that same create:
  Enabled                 false                                      (emulator: True)                                    [Cause 1]
  Realm                   'sim2real-behav-<id>.example.com'          (emulator: 'corp.example.com')                      [Cause 2]
  DistinguishedName       'CN=sim2realUser1,OU=Users,OU=SIM2REAL,DC=sim2real-behav-<id>,DC=example,DC=com'
                                                                     (emulator: 'CN=sim2realUser1,CN=Users,DC=corp,DC=example,DC=com')  [Causes 2+3]
  UserPrincipalName       'sim2realUser1@SIM2REAL-BEHAV-<ID>.EXAMPLE.COM'
                                                                     (emulator: 'sim2realUser1@corp.example.com')        [Causes 2+4]
  OtherAttributes key     'Description'                              (emulator: 'description')                           [Cause 5]
  SID                     'S-1-5-21-<domain>-<rid>'                  (emulator: matches the required shape — not a defect)

Not a divergence (recorded as correct): the DescribeUser/UpdateUser-on-deleted-user refusal is ResourceNotFoundException at HTTP 404, the modeled httpError; only the message template diverges [Cause 6]: emulator 'user sim2realUser1 was not found in directory d-003ba5fe5a' vs 'User not found in directory d-003ba5fe5a.'

Code confirming each cause (services/service-com-amazonaws-directoryservicedata-directoryservicedata/):
  src/ops/create_user.rs:247-250  `enabled: true`                                                   [Cause 1]
  src/state.rs:34                 `pub const DEFAULT_REALM: &str = "corp.example.com"`              [Cause 2]
  src/state.rs:201-210            `Directory::new` assigns DEFAULT_REALM for every DirectoryId       [Cause 2]
  src/state.rs:230-237            DN = `CN={name},CN=Users,{domain_components}` — no delegated OU    [Cause 3]
  src/state.rs:239-242            UPN = `{sam}@{self.realm}` — not uppercased                        [Cause 4]
  src/state.rs:450-455, 134-147   attributes stored and projected under the request's spelling       [Cause 5]
  src/state.rs:536-561            not-found message builders                                         [Cause 6]

Smithy consulted (services/service-com-amazonaws-directoryservicedata-directoryservicedata/smithy-model/com-amazonaws-directoryservicedata-directoryservicedata.json):
  CreateUser input = {DirectoryId(req, ^d-[0-9a-f]{10}$), SAMAccountName(req), EmailAddress, GivenName, Surname, OtherAttributes, ClientToken} — NO password member; output = {DirectoryId, SID, SAMAccountName}; POST /Users/CreateUser, 200.
  DescribeUser output = {DirectoryId, Realm, SID, SAMAccountName, DistinguishedName(@sensitive), UserPrincipalName(@sensitive), EmailAddress, GivenName, Surname, Enabled, OtherAttributes}; UserPrincipalName and DistinguishedName are output-only, i.e. derived; errors include ResourceNotFoundException httpError 404 and AccessDeniedException httpError 403; POST /Users/DescribeUser, 200.
  CreateUserRequest$OtherAttributes doc: "Attribute names are case insensitive."

## Steps to Reproduce
1. Boot the emulator and create botocore clients for 'ds', 'ec2' and 'ds-data' against it.
2. Build a VPC with two subnets in distinct AZs, then ds.create_microsoft_ad(Name='sim2real-behav-<unique>.example.com', ShortName='SIM2REAL', Password=..., Edition='Standard', VpcSettings=...) and poll ds.describe_directories until Stage == 'Active'. Note the Name you passed — every expectation below is derived from it.
3. ds.enable_directory_data_access(DirectoryId=<id>) and poll ds.describe_directory_data_access until DataAccessStatus == 'Enabled'.
4. dsdata.create_user(DirectoryId=<id>, SAMAccountName='sim2realUser1', GivenName='Sim', Surname='Real', EmailAddress='sim2realUser1@<fqdn>', OtherAttributes={'description': {'S': 'sim2real-behavioral-roundtrip'}}).
5. dsdata.describe_user(DirectoryId=<id>, SAMAccountName='sim2realUser1', OtherAttributes=['description']) and inspect the response:
   - Cause 1: Enabled is True; AWS returns False (CreateUser set no password).
   - Cause 2: Realm is 'corp.example.com', not the <fqdn> the directory was created with; create a SECOND directory with a different Name and confirm it reports the SAME realm, which is the constant.
   - Cause 3: DistinguishedName is 'CN=sim2realUser1,CN=Users,DC=corp,DC=example,DC=com'; AWS returns '...,OU=Users,OU=SIM2REAL,<DC= components of <fqdn>>'.
   - Cause 4: UserPrincipalName is 'sim2realUser1@corp.example.com'; AWS returns 'sim2realUser1@' + <fqdn>.upper().
   - Cause 5: OtherAttributes is keyed 'description' (the request's spelling); AWS keys it 'Description'.
6. dsdata.delete_user(DirectoryId=<id>, SAMAccountName='sim2realUser1'), then dsdata.describe_user(...) again: confirm ResourceNotFoundException at HTTP 404 (correct) but compare Message to 'User not found in directory <id>.' (Cause 6). Repeat with dsdata.update_user(DirectoryId=<id>, SAMAccountName='sim2realUser1', GivenName='x').
7. Re-replay check ds.ds-data-user-create-describe-search-roundtrip; it must get past line 331 and all the way through the three projections (DescribeUser / SearchUsers / ListUsers) and the post-delete refusals.
