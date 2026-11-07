Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ds.ds-data-user-create-describe-search-roundtrip ds.ds-data-group-inventory-and-search-answer-for-the-builtin-groups
Issue-Key: ds.ds-data-user-create-describe-search-roundtrip|run#18|AssertionError|assert desc.get("UserPrincipalName") == f"{sam}@{fqdn.upper()}", (
Issue-Key: ds.ds-data-group-inventory-and-search-answer-for-the-builtin-groups|_refusal#38|AssertionError|raise AssertionError(
Issue-Commit: edd8d27229a8bc99e308a4b32d393f13d6f32cb7

# ds-data: 2 net-new root causes — UserPrincipalName is built against the realm in its stored case instead of the uppercased domain name, and Search{Users,Groups} accept an attribute AWS does not index for search ('Cn') instead of refusing it with ValidationException/INVALID_ATTRIBUTE_FOR_SEARCH

> _Note: this issue was identified against the code base at commit `edd8d27229a8bc99e308a4b32d393f13d6f32cb7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SearchGroups, DescribeUser

## Description
Two distinct, net-new root causes in the ds-data (Directory Service Data) object store, both observed against a live, Active, data-access-enabled Managed Microsoft AD directory the caller owns. They are independent: one is a derived-field composition defect in the user record, the other is a missing input-validation gate in the search family.

Relation to already-open issues: 20261023T134711Z-ds-data-user-objects-are-not-real-active.md bundles six ds-data user-object causes and its Cause 4 names the UPN; that issue's replay key is its Enabled=false assertion, which now passes (the roundtrip check reaches the UPN assertion 7 assertions later, and the directory-derived realm and DC= components are now correct, so the remaining divergence is the CASE of the domain part alone). The suite did not mark this check as owned by that issue, so Cause 1 below is filed on its own evidence and scoped narrowly to the case rule. 20261107T015500Z-ds-data-searchgroups-echoes-the-constant-realm-corp.md concerns the Realm SearchGroups ECHOES (now correct in this replay); Cause 2 below concerns which SearchAttributes values SearchGroups ACCEPTS, which is disjoint. 20261107T090943Z-ds-data-otherattributes-keys-are-echoed-with-the.md owns the OtherAttributes key-canonicalization cause and the duplicate-user check; neither cause below is that one.

Cause 1 of 2: UserPrincipalName is composed with the directory's domain name in its stored (lower) case rather than uppercased.
AWS: recorded behavior only. CreateUser sends no UPN — Smithy com.amazonaws.directoryservicedata#CreateUser input is {DirectoryId (@required, pattern ^d-[0-9a-f]{10}$), SAMAccountName (@required, UserName, length 1..20, pattern ^[\w\-.]+$), EmailAddress, GivenName, Surname, OtherAttributes, ClientToken} and has NO UserPrincipalName member, while DescribeUserResult$UserPrincipalName (target UserPrincipalName, length 1..256, @sensitive) is output-only, so the value is DERIVED by Active Directory. The recording shows the rule: the UPN is <sAMAccountName>@<the directory's domain name UPPERCASED>, i.e. the sAMAccountName is preserved in the case it was created with and the domain part is upper-cased. Both halves are values from the request and from the ds control-plane directory record (DirectoryDescription$Name); no literal from any one run is part of the requirement.
Emulator: DescribeUser answered HTTP 200 with UserPrincipalName = '<sam>@<fqdn as stored, lower case>' — in the replay, 'sim2realUser1@sim2real-behav-cf2ccb0ff64c.example.com' where AWS yields 'sim2realUser1@SIM2REAL-BEHAV-CF2CCB0FF64C.EXAMPLE.COM'. The domain part is simply the stored realm string, never case-normalized. The same wrong value is served by the SearchUsers projection, which widens its result with the stored field when the search names userPrincipalName.
Why: a UPN is a logon name. A client that authenticates, binds or looks a principal up by the UPN it read back from DescribeUser/SearchUsers presents a name the real directory does not hold for that object, and code that compares a UPN it constructed itself (sam@FQDN.upper()) against the one the API returns finds no match and treats the account as missing or mismatched.
Fix: the UPN derivation in the ds-data state's Directory record — uppercase the directory's domain name when composing the principal name, leaving the sAMAccountName as stored; the CreateUser handler that stores the derived value and the SearchUsers/DescribeUser projections then follow without further change. File and line citations are under Traces.
Traces: ds.ds-data-user-create-describe-search-roundtrip (assertion at check line 324: desc.get('UserPrincipalName') == f"{sam}@{fqdn.upper()}").

Cause 2 of 2: SearchGroups (and SearchUsers, same rule and same code path) accept a SearchAttributes value that is a valid LDAP display name but is NOT searchable, answering 200 with matches instead of refusing the request.
AWS: SearchGroups declares ValidationException (Smithy: @error client, httpError 400) and that exception carries a modelled Reason member of enum ValidationExceptionReason, whose values include INVALID_ATTRIBUTE_FOR_SEARCH and INVALID_ATTRIBUTE_NAME. Per the AWS Directory Service Data attribute table, 'Cn' is a valid User+Group attribute but is Searchable: No, so naming it in SearchAttributes is refused with HTTP 400, Code ValidationException, Reason INVALID_ATTRIBUTE_FOR_SEARCH and the message "Attribute '<name>' cannot be searched." — the fixed text is "Attribute '" / "' cannot be searched." and the only variable part is the offending attribute name taken verbatim from the request's SearchAttributes (recorded behavior for the message wording; the status, code and Reason enum value are modelled). A name the table has no entry for at all (e.g. 'whenCreated') draws Reason INVALID_ATTRIBUTE_NAME instead, so the two are distinguished by the Reason, not by the code.
Emulator: the request succeeded — HTTP 200 with a Groups list containing the created group, DirectoryId and Realm echoed. The handler builds its page directly from the request's SearchAttributes with no searchable-attribute allowlist anywhere ahead of it, and its structural-attribute table additionally treats 'cn' as a synonym of sAMAccountName, so an unsearchable attribute both passes validation and matches. The same handler shape and the same table back SearchUsers, so the unsearchable-attribute family is unguarded for users too.
Why: a client's negative path inverts. Code that probes which attributes a directory will search, or that surfaces the service's refusal to the user, sees a successful search against the emulator and a 400 ValidationException against AWS; worse, the emulator silently returns MATCHES for an attribute AWS never searches, so a caller concludes an object matches a predicate the real directory would never have evaluated. A caller that branches on Reason to tell an unsearchable attribute from an unknown one gets no Reason at all.
Fix: input validation in the search family — a searchable-attribute allowlist checked in SearchGroups' and SearchUsers' handlers (or in the shared search entry in the ds-data state) before any record is walked, refusing with ValidationException/400, Reason INVALID_ATTRIBUTE_FOR_SEARCH for a known-but-unsearchable LdapDisplayName and INVALID_ATTRIBUTE_NAME for a name the schema does not hold, with the message templated on the offending name. The 'cn' alias in the structural-attribute table must stop making an unsearchable attribute matchable. File and line citations are under Traces.
Traces: ds.ds-data-group-inventory-and-search-answer-for-the-builtin-groups (step 6 of the check: SearchGroups(DirectoryId=<id>, SearchString=<group sam>, SearchAttributes=['Cn']) SUCCEEDED where AWS refuses; the check's positive control — the same wiring searching SAMAccountName — passes in step 4, so the search path itself works and only the gate is missing).

Component citations (services/service-com-amazonaws-directoryservicedata-directoryservicedata/): src/state.rs:539-541 `Directory::user_principal_name` formats `{sam_account_name}@{self.realm}` with no case normalization [Cause 1]; src/state.rs:251-252 projects that stored value into a searched user; src/ops/search_groups.rs:190-272 and src/ops/search_users.rs:189-274 walk records straight from input.search_attributes with no attribute allowlist (the only ValidationException either raises is invalid_next_token) [Cause 2]; src/state.rs:147 `const SAM_ACCOUNT_NAME_ATTRIBUTES: &[&str] = &["samaccountname", "cn"]` makes 'cn' match [Cause 2]; src/state.rs:95-97 shows INVALID_NEXT_TOKEN is the only ValidationExceptionReason the crate constructs.

## Evidence
Failing checks (behavioral suite simparity-behavioral-aws-v1, both listed under service heading '### ds'):

1) [mismatch] ds.ds-data-user-create-describe-search-roundtrip — Cause 1
   check_ds_ds_data_user_create_describe_search_roundtrip.py line 324:
     assert desc.get("UserPrincipalName") == f"{sam}@{fqdn.upper()}"
   AssertionError: the UPN is DERIVED by Active Directory -- CreateUser never sent one -- as the sAMAccountName against the UPPERCASED domain name, expected sim2realUser1@'SIM2REAL-BEHAV-CF2CCB0FF64C.EXAMPLE.COM', got 'sim2realUser1@sim2real-behav-cf2ccb0ff64c.example.com'
   Expected (AWS), general form: UserPrincipalName == '<SAMAccountName as created>@<directory domain name UPPERCASED>'.
   Actual (emulator): '<SAMAccountName as created>@<directory domain name in its stored lower case>'.
   Note the realm itself is now derived from the directory (the domain part is the fqdn the directory was created with, no longer a constant), so CASE is the whole remaining divergence of this field.

2) [mismatch] ds.ds-data-group-inventory-and-search-answer-for-the-builtin-groups — Cause 2
   check_ds_ds_data_group_inventory_and_search_answer_for_the_builtin_groups.py line 319 -> _refusal() line 51:
     AssertionError: SearchGroups over 'Cn', an attribute Active Directory does not index for search SUCCEEDED where real AWS refuses it; the response was
       {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'DirectoryId': 'd-003ba5fe5a',
        'Groups': [{'GroupScope': 'Global', 'GroupType': 'Security', 'SAMAccountName': 's2rGrpInv859c53eb7a4d', 'SID': 'S-1-5-21-...-1000'}],
        'Realm': 'sim2real-behav-46ee5c0ef936.example.com'}
   Expected (AWS): HTTP 400, Error.Code 'ValidationException', top-level Reason 'INVALID_ATTRIBUTE_FOR_SEARCH' (restJson1 puts the modelled Reason member beside Message, not inside Error), Message "Attribute 'Cn' cannot be searched." — fixed text "Attribute '" + "' cannot be searched.", variable part = the attribute name from the request's SearchAttributes.
   Actual (emulator): HTTP 200 with a non-empty Groups list; no error, no Reason.
   The check's own positive control (step 4: SearchGroups with SearchAttributes=['SAMAccountName'], equality not prefix, realm and DirectoryId echoed) passes in this same replay, so the search path and its echoes are correct and only the unsearchable-attribute gate is absent.

Smithy consulted (services/service-com-amazonaws-directoryservicedata-directoryservicedata/smithy-model/com-amazonaws-directoryservicedata-directoryservicedata.json):
  CreateUser: input {DirectoryId(@required, httpQuery, ^d-[0-9a-f]{10}$), SAMAccountName(@required, UserName, 1..20, ^[\w\-.]+$), EmailAddress, GivenName, Surname, OtherAttributes, ClientToken(@idempotencyToken)} — NO UserPrincipalName member; output {DirectoryId, SID, SAMAccountName}; POST /Users/CreateUser, 200.
  DescribeUser: output includes UserPrincipalName (1..256, @sensitive) and DistinguishedName (@sensitive), both output-only (derived); POST /Users/DescribeUser, 200.
  SearchGroups: input {DirectoryId(@required), SearchString(@required, 1..64, @sensitive), SearchAttributes(@required, LdapDisplayNameList, 1..25), Realm, NextToken, MaxResults(1..250)}; errors AccessDeniedException(403), DirectoryUnavailableException(400), InternalServerException(500), ThrottlingException(429), ValidationException(400); POST /Groups/SearchGroups, 200.
  ValidationException: @error client, httpError 400, members {Message, Reason -> ValidationExceptionReason}.
  ValidationExceptionReason values: INVALID_REALM, INVALID_DIRECTORY_TYPE, INVALID_SECONDARY_REGION, INVALID_NEXT_TOKEN, INVALID_ATTRIBUTE_VALUE, INVALID_ATTRIBUTE_NAME, INVALID_ATTRIBUTE_FOR_USER, INVALID_ATTRIBUTE_FOR_GROUP, INVALID_ATTRIBUTE_FOR_SEARCH, INVALID_ATTRIBUTE_FOR_MODIFY, DUPLICATE_ATTRIBUTE, MISSING_ATTRIBUTE, ATTRIBUTE_EXISTS, LDAP_SIZE_LIMIT_EXCEEDED, LDAP_UNSUPPORTED_OPERATION.

Emulator code confirming each cause (services/service-com-amazonaws-directoryservicedata-directoryservicedata/):
  src/state.rs:538-541   `Directory::user_principal_name` -> format!("{sam_account_name}@{}", self.realm)        [Cause 1]
  src/state.rs:251-252   searched-user projection returns the stored user_principal_name verbatim              [Cause 1]
  src/ops/search_groups.rs:259  `group.matches_search(&input.search_attributes, &input.search_string)` is the only use of SearchAttributes; no allowlist check precedes it   [Cause 2]
  src/ops/search_users.rs:258   the same, for users                                                            [Cause 2]
  src/state.rs:147       SAM_ACCOUNT_NAME_ATTRIBUTES = ["samaccountname", "cn"] — 'Cn' folds onto sAMAccountName and therefore MATCHES   [Cause 2]
  src/state.rs:95-97     INVALID_NEXT_TOKEN is the only ValidationExceptionReason constant in the crate        [Cause 2]

## Steps to Reproduce
Common setup (both causes, one directory):
1. Boot the emulator; create botocore clients for 'ds', 'ec2', 'sts' and 'ds-data' against it.
2. Build a VPC with two subnets in distinct AZs, then ds.create_microsoft_ad(Name='<unique>.example.com', ShortName='<NETBIOS>', Password=..., Edition='Standard', VpcSettings=...) and poll ds.describe_directories until Stage == 'Active'. Keep the Name you passed — every expectation below is derived from it, nothing is a literal from a prior run.
3. ds.enable_directory_data_access(DirectoryId=<id>); poll ds.describe_directory_data_access until DataAccessStatus == 'Enabled'.

Cause 1 (UserPrincipalName case):
4. dsdata.create_user(DirectoryId=<id>, SAMAccountName='sim2realUser1', GivenName='Sim', Surname='Real', EmailAddress='sim2realUser1@<fqdn>') — send no UPN; the API has no member for one.
5. dsdata.describe_user(DirectoryId=<id>, SAMAccountName='sim2realUser1') and compare UserPrincipalName to 'sim2realUser1@' + <fqdn>.upper(). The emulator returns the domain part in its stored lower case; AWS returns it uppercased with the sAMAccountName's own case preserved.
6. dsdata.search_users(DirectoryId=<id>, SearchString='sim2realUser1', SearchAttributes=['SAMAccountName','UserPrincipalName']) and confirm the same wrong-case value is carried into the search projection.

Cause 2 (unsearchable SearchAttributes accepted):
7. dsdata.create_group(DirectoryId=<id>, SAMAccountName='<grp>').
8. Positive control: dsdata.search_groups(DirectoryId=<id>, SearchString='<grp>', SearchAttributes=['SAMAccountName']) returns exactly that group — this must keep working.
9. dsdata.search_groups(DirectoryId=<id>, SearchString='<grp>', SearchAttributes=['Cn']). The emulator answers HTTP 200 with the group in Groups. AWS refuses: HTTP 400, Code 'ValidationException', Reason 'INVALID_ATTRIBUTE_FOR_SEARCH', Message "Attribute 'Cn' cannot be searched."
10. Contrast the unknown-name case: dsdata.search_groups(..., SearchAttributes=['whenCreated']) must draw Reason 'INVALID_ATTRIBUTE_NAME', so the two refusals are distinguished by Reason and not by code.
11. Repeat step 9 against dsdata.search_users with SearchAttributes=['Cn'] — the same unguarded code path.
12. Confirm a refused search mutates nothing: dsdata.list_groups(DirectoryId=<id>) must hold exactly the inventory it held before.
13. Re-replay both checks: ds.ds-data-user-create-describe-search-roundtrip must get past its line-324 UPN assertion and through the DescribeUser / SearchUsers / ListUsers projections and post-delete refusals, and ds.ds-data-group-inventory-and-search-answer-for-the-builtin-groups must reach and pass its step-6 refusal assertions on Code, Reason and Message.
