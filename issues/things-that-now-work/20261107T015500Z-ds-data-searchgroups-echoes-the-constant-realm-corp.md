Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ds.ds-data-group-inventory-and-search-answer-for-the-builtin-groups
Issue-Key: ds.ds-data-group-inventory-and-search-answer-for-the-builtin-groups|run#36|AssertionError|assert page.get("Realm") == fqdn, (
Issue-Commit: c48413f08eb6ef52de10538e8da3115aa7206fd4

# ds-data SearchGroups echoes the constant realm 'corp.example.com' instead of the searched directory's own domain name

> _Note: this issue was identified against the code base at commit `c48413f08eb6ef52de10538e8da3115aa7206fd4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SearchGroups

## Description
One net-new root cause, grounded in the failing behavioral check the suite lists under service `ds` (the check drives the ds control plane to build a live Managed Microsoft AD and then exercises the ds-data data plane against it).

=== Cause 1 of 1: SearchGroups' Realm is a build-time constant, not the domain name the ds control plane was given for the directory being searched ===
Failing check: ds.ds-data-group-inventory-and-search-answer-for-the-builtin-groups

AWS: Smithy (com.amazonaws.directoryservicedata) SearchGroups -> POST /Groups/SearchGroups, 200; output SearchGroupsResult = {DirectoryId (^d-[0-9a-f]{10}$), Realm (@length 1..255, pattern ^([a-zA-Z0-9]+[\\.-])+([a-zA-Z0-9])+[.]?$), Groups, NextToken}; errors AccessDeniedException(403), DirectoryUnavailableException(400), InternalServerException(500), ThrottlingException(429), ValidationException(400). Realm reports the Active Directory domain the search was resolved in — for a directory created through the ds control plane that is the directory's own fully qualified domain name, i.e. DirectoryDescription$Name as passed to CreateMicrosoftAD(Name=...). Recorded behavior: a SearchGroups response for a directory created with Name=<fqdn> carries Realm=<fqdn>, and the same page echoes DirectoryId=<the directory searched> (that part of the check passed). The rule is the derivation: Realm == the directory's own domain name, whatever that name is; it is never a fixed string.

Emulator: every SearchGroups page carries the ds-data crate's build-time constant realm, independent of the directory it searched. Replay: "AssertionError: expected SearchGroups to echo the directory's own realm, expected 'sim2real-behav-39a49819617a.example.com', got 'corp.example.com'" — the directory had been created minutes earlier with Name='sim2real-behav-39a49819617a.example.com' (the ds control plane holds that name; the check's earlier ListGroups/SID/well-known-RID legs all passed, so the object store itself is right and only the realm is wrong). Because the constant is assigned when the ds-data store materialises its own Directory record, it is wrong for EVERY directory except one literally named corp.example.com.

Why: Realm is what a client uses to build the LDAP/Kerberos principal (user@REALM, DN suffix) for the directory it just created, and it is also the value ds-data's own `Realm` / `MemberRealm` request members are matched against — so a caller that correctly passes its own fqdn to SearchGroups/ListGroups/ListGroupMembers is treated as naming a foreign domain and gets ResourceNotFoundException or an empty projection, while a caller that passes the emulator's constant is served. Code that stores or logs the realm it searched records a domain that does not exist in the account. It also blocks the rest of this check (the ValidationException / Reason INVALID_ATTRIBUTE_FOR_SEARCH leg for an unsearchable attribute, which is never reached).

Fix: the ds-data crate's realm source — services/service-com-amazonaws-directoryservicedata-directoryservicedata/src/state.rs (the DEFAULT_REALM constant assigned by `Directory::new`) and the `resolved_realm`/`realm_matches` helpers the Search*/List* handlers call (search_groups.rs, list_groups.rs, list_group_members.rs, …). The realm must be taken from the ds control plane's directory record (the cross-plane directory inventory the ds CreateMicrosoftAD handler already registers into) rather than from a module constant.

Relation to open issues: 20261104T054355Z-ds-data-serves-a-directory-whose-data-access.md (Cause 3) and 20261023T134711Z-ds-data-user-objects-are-not-real-active.md (Cause 2) describe the same constant-realm defect on the ListUsers and DescribeUser projections. NEITHER carries this check's replay key, and neither names the GROUP search projection, so fixing them as written will not get this check re-replayed; the fixer should treat this as the same seam and close it for the group/search projections too.

Traces: ds.ds-data-group-inventory-and-search-answer-for-the-builtin-groups (the SearchGroups Realm assertion; the preceding inventory, built-in well-known RID, shared-domain-SID and equality-vs-prefix legs of the same check all passed).

## Evidence
Cause 1 — expected (AWS): dsdata.search_groups(DirectoryId='d-xxxxxxxxxx', SearchString=<group just created>, SearchAttributes=['SAMAccountName']) -> HTTP 200 with Realm equal to the directory's own domain name, e.g. 'sim2real-behav-39a49819617a.example.com' (the Name the directory was created with), and DirectoryId equal to the directory searched.
  actual (emulator): HTTP 200 with DirectoryId correct but Realm 'corp.example.com'. Replay detail from ds.ds-data-group-inventory-and-search-answer-for-the-builtin-groups, line 308: "AssertionError: expected SearchGroups to echo the directory's own realm, expected 'sim2real-behav-39a49819617a.example.com', got 'corp.example.com'". The literal names/ids above are one run's values; the requirement is the derivation Realm == the directory's CreateMicrosoftAD `Name`.
  code: services/service-com-amazonaws-directoryservicedata-directoryservicedata/src/state.rs — `DEFAULT_REALM` is assigned by `Directory::new` for every DirectoryId; services/service-com-amazonaws-directoryservicedata-directoryservicedata/src/ops/list_group_members.rs:213 (`crate::state::resolved_realm(input.realm.as_deref(), &directory.realm)`) shows the same `directory.realm` field feeding every projection, and search_groups.rs reports it as SearchGroupsResult$Realm.

Smithy consulted (services/service-com-amazonaws-directoryservicedata-directoryservicedata/smithy-model/com-amazonaws-directoryservicedata-directoryservicedata.json):
  SearchGroups: POST /Groups/SearchGroups, 200, @readonly, @paginated(inputToken=NextToken, outputToken=NextToken, pageSize=MaxResults, items=Groups); input {DirectoryId(req, httpQuery, ^d-[0-9a-f]{10}$), SearchString(req, len 1..64, @sensitive), SearchAttributes(req, LdapDisplayNameList len 1..25), Realm(len 1..255, pattern ^([a-zA-Z0-9]+[\\.-])+([a-zA-Z0-9])+[.]?$), NextToken, MaxResults(1..250)}; output {DirectoryId, Realm(same constraints), Groups, NextToken}; errors AccessDeniedException(403), DirectoryUnavailableException(400), InternalServerException(500), ThrottlingException(429), ValidationException(400).
  ListGroups: same Realm member on ListGroupsResult, so the two projections must agree on one value.

## Steps to Reproduce
1. Boot the emulator; create botocore clients for 'ds', 'ec2', 'sts' and 'ds-data' against it.
2. Pick a VPC with two subnets in distinct AZs, then ds.create_microsoft_ad(Name='<unique>.example.com', ShortName='SIM2REAL', Password='Sim2Real#Pass99', Edition='Standard', VpcSettings={'VpcId': <vpc>, 'SubnetIds': [<two subnets>]}). Remember the Name passed — the expectation is derived from it. Poll ds.describe_directories(DirectoryIds=[<id>]) until Stage == 'Active'.
3. ds.enable_directory_data_access(DirectoryId=<id>); poll ds.describe_directory_data_access until DataAccessStatus == 'Enabled'.
4. dsdata.create_group(DirectoryId=<id>, SAMAccountName='s2rGrpInv<unique>').
5. dsdata.search_groups(DirectoryId=<id>, SearchString='s2rGrpInv<unique>', SearchAttributes=['SAMAccountName']) -> inspect the response's Realm: the emulator answers 'corp.example.com'; AWS answers the Name from step 2. DirectoryId is echoed correctly in both.
6. Create a SECOND directory with a different Name and repeat step 5 against it: the emulator reports the identical constant realm for both directories, proving the value is not derived from the directory.
7. Confirm the same constant on dsdata.list_groups / dsdata.list_group_members for the same directory, then re-replay ds.ds-data-group-inventory-and-search-answer-for-the-builtin-groups.
