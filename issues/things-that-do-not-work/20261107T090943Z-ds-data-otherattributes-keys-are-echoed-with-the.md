Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ds.ds-data-duplicate-sam-account-name-rejected-and-original-untouched
Issue-Key: ds.ds-data-duplicate-sam-account-name-rejected-and-original-untouched|run#19|AssertionError|assert (after.get("OtherAttributes") or {}).get("Description", {}).get("S") == marker, (
Issue-Commit: 2d393b0b08d4e0faded23ebb784005e509785021

# ds-data: OtherAttributes keys are echoed with the caller's spelling instead of the canonical AD schema name, so DescribeUser answers 'description' where AWS answers 'Description'

> _Note: this issue was identified against the code base at commit `2d393b0b08d4e0faded23ebb784005e509785021`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeUser

## Description
One net-new root cause, grounded in the failing behavioral check the suite lists under service `ds`.

Cause 1 of 1: the Directory Service Data plane stores a custom `OtherAttributes` key with the exact spelling the write used and reports it back verbatim, instead of canonicalizing the case-insensitive attribute name to its Active Directory schema name. A user written with `OtherAttributes={"description": {"S": ...}}` and read back with `OtherAttributes=["description"]` answers under the key `description`, where AWS answers under the key `Description`.
Failing check: ds.ds-data-duplicate-sam-account-name-rejected-and-original-untouched (verdict: mismatch; AssertionError at the assertion on `(after.get("OtherAttributes") or {}).get("Description", {}).get("S")`, with the emulator's DescribeUser answer carrying `'OtherAttributes': {'description': {'S': 'sim2real-first-write'}}`).

AWS: an `OtherAttributes` key is an AD SCHEMA name, not a caller-chosen label. Smithy (com.amazonaws.directoryservicedata): `Attributes` is a map @length(min 1, max 25) whose key targets `LdapDisplayName` (@length 1..63, pattern `^[A-Za-z*][A-Za-z-*]*$`), and `AttributeValue`'s own documentation states "Each attribute value is described as a name-value pair. The name is the AD schema name, and the value is the data itself." `DescribeUser$OtherAttributes` is documented as "One or more attribute names to be returned for the user" and the name is matched case-insensitively, as AD matches an attribute name. Recorded behavior (the model does not spell the individual schema names out): the service resolves the case-insensitive name the request used to the schema's own display name and answers under THAT spelling — a write and a read that both say `description` are answered with the key `Description`. The rule is the requirement: the key in the response is the AD schema name of the attribute, whatever casing the request used to name it; the specific recorded string `sim2real-first-write` is the attribute VALUE from that run and is not part of the template.

Emulator: the written spelling is the stored key and the reported key. services/service-com-amazonaws-directoryservicedata-directoryservicedata/src/state.rs:404-423 (`project_attributes`) is documented as reporting attributes "under the names they are stored as": it folds the REQUESTED names only to decide membership (`wanted.contains(&fold(name))`) and then emits `(name.clone(), value.clone())` — the stored key, unchanged — and the write path (`crate::state::stored_attributes`, called from src/ops/create_user.rs:249) keeps the request's spelling as that key. So `DescribeUser` answered HTTP 200 with `OtherAttributes` keyed `description` rather than `Description`. The rest of that check's legs matched: the duplicate `CreateUser` was correctly refused 400 `ConflictException` with the message "User already exists in directory <directory-id>." and the original user's GivenName/Surname/EmailAddress were untouched — only the attribute KEY diverges, so the regression is purely in how the attribute name is canonicalized.

Why: a client reads a custom attribute by its AD schema name, because that is the name AWS documents and the name every other AD tool uses — `resp["OtherAttributes"]["Description"]["S"]`. Against the emulator that lookup raises KeyError / returns None even though the attribute is present and holds the right value, so an attribute that round-trips on AWS reads as absent. Worse, the key depends on whichever casing the WRITER happened to use, so two writers that both set the same AD attribute produce two different response keys for one object, and nothing a reader can key on is stable. The same projection serves DescribeGroup, SearchUsers and SearchGroups (`Group::to_searched_group` / `projected_attributes` at src/state.rs:329-362 both route through it), so every read of a custom attribute in this service carries the defect.

Fix: the attribute-name canonicalization in the Directory Service Data state layer — the write path that chooses the stored key (`stored_attributes`) and/or the single projection rule every read goes through (`project_attributes`), so one AD attribute has one reported name regardless of the casing a request used to write or to ask for it. It belongs in that one shared place rather than per-handler, because DescribeUser, DescribeGroup, SearchUsers and SearchGroups all project through it.

Relation to open issues: nothing open covers attribute-name canonicalization. 20261107T015500Z-ds-data-searchgroups-echoes-the-constant-realm-corp.md concerns the reported Realm, and 20261104T054355Z / 20261023T134711Z / 20261107T042250Z concern the data-access gate and user-object fidelity; none carries this check's replay key and fixing any of them will not get this check re-replayed.

Traces: ds.ds-data-duplicate-sam-account-name-rejected-and-original-untouched; emulator code cited above: services/service-com-amazonaws-directoryservicedata-directoryservicedata/src/state.rs:404-423, src/ops/create_user.rs:249, src/ops/describe_user.rs:213-226.

## Evidence
Expected (AWS), on an Active Managed Microsoft AD with DataAccessStatus 'Enabled':
  dsdata.create_user(DirectoryId=<id>, SAMAccountName=<name>, GivenName='First', Surname='Original', EmailAddress='<name>@<fqdn>', OtherAttributes={'description': {'S': <marker>}}) -> 200
  dsdata.describe_user(DirectoryId=<id>, SAMAccountName=<name>, OtherAttributes=['description']) -> 200 with
      OtherAttributes == {'Description': {'S': <marker>}}
  i.e. the response key is the attribute's AD schema name ('Description'), NOT the spelling the request used ('description'); the value is whatever was written.

Actual (emulator):
  dsdata.describe_user(DirectoryId='d-003ba5fe5a', SAMAccountName='sim2realOrig', OtherAttributes=['description']) -> 200 with
      {'DirectoryId': 'd-003ba5fe5a', 'DistinguishedName': 'CN=sim2realOrig,CN=Users,DC=corp,DC=example,DC=com', 'EmailAddress': 'sim2realOrig@sim2real-behav-6ce843e29fc9.example.com', 'Enabled': True, 'GivenName': 'First', 'OtherAttributes': {'description': {'S': 'sim2real-first-write'}}, 'Realm': 'corp.example.com', 'SAMAccountName': 'sim2realOrig', 'SID': 'S-1-5-21-2886073900-3733782474-697738302-1000', 'Surname': 'Original', 'UserPrincipalName': 'sim2realOrig@corp.example.com'}
  The OtherAttributes key is 'description' (the caller's spelling) where AWS reports 'Description'. (The directory id, fqdn, SID and marker above are that run's values and are not part of the requirement.)

Check failure as the suite reported it:
  [mismatch] ds.ds-data-duplicate-sam-account-name-rejected-and-original-untouched
  AssertionError: the refused duplicate overwrote the original's custom attribute: {... 'OtherAttributes': {'description': {'S': 'sim2real-first-write'}} ...}
  (raised by `assert (after.get("OtherAttributes") or {}).get("Description", {}).get("S") == marker`; the check's own comment states the rule — "'Description', not 'description': the service answers with the canonical LDAP schema spelling of the case-insensitive name that was written". The value WAS preserved, so the refused duplicate did not in fact overwrite anything — the only divergence is the key.)

Smithy consulted (services/service-com-amazonaws-directoryservicedata-directoryservicedata/smithy-model/...json):
  Attributes: map, @length(min 1, max 25), key -> LdapDisplayName (@length 1..63, pattern ^[A-Za-z*][A-Za-z-*]*$), value -> AttributeValue
  LdapDisplayName: string, @length 1..63, pattern ^[A-Za-z*][A-Za-z-*]*$
  AttributeValue (union S|N|BOOL|SS) doc: "Each attribute value is described as a name-value pair. The name is the AD schema name, and the value is the data itself."
  DescribeUser$OtherAttributes (LdapDisplayNameList, @length 1..25): "One or more attribute names to be returned for the user."

Emulator source:
  services/service-com-amazonaws-directoryservicedata-directoryservicedata/src/state.rs:404-423 — `project_attributes`, documented "reported under the names they are stored as", emits `(name.clone(), value.clone())` for each stored key whose fold matches a requested name.
  src/ops/create_user.rs:249 — `other_attributes: crate::state::stored_attributes(input.other_attributes)`, which keys the record by the request's own spelling.
  src/ops/describe_user.rs:225 — `other_attributes: user.projected_attributes(input.other_attributes.as_deref())`.

## Steps to Reproduce
1. Boot the emulator; build botocore clients for 'ds', 'ec2' and 'ds-data' against it.
2. ds.create_microsoft_ad(Name='sim2real-behav-<unique>.example.com', ShortName='SIM2REAL', Password='Sim2Real#Pass99', Edition='Standard', VpcSettings={VpcId: <shared vpc>, SubnetIds: [<two subnets in distinct AZs>]}); poll ds.describe_directories until Stage == 'Active'.
3. ds.enable_directory_data_access(DirectoryId=<id>); poll ds.describe_directory_data_access until DataAccessStatus == 'Enabled'.
4. dsdata.create_user(DirectoryId=<id>, SAMAccountName='sim2realOrig', GivenName='First', Surname='Original', EmailAddress='sim2realOrig@<fqdn>', OtherAttributes={'description': {'S': 'sim2real-first-write'}}) -> 200.
5. dsdata.describe_user(DirectoryId=<id>, SAMAccountName='sim2realOrig', OtherAttributes=['description']).
   Emulator: OtherAttributes == {'description': {'S': 'sim2real-first-write'}}.
   AWS: OtherAttributes == {'Description': {'S': 'sim2real-first-write'}} — the key is the AD schema name, not the request's spelling.
6. Repeat step 5 with OtherAttributes=['DESCRIPTION'] (names are case-insensitive): the attribute must still be returned, and still under the canonical key 'Description'.
7. Write the same attribute on a second user using a different casing, e.g. dsdata.create_user(..., SAMAccountName='sim2realOther', OtherAttributes={'Description': {'S': 'x'}}), then DescribeUser both users: AWS reports the SAME key for both; the emulator reports whichever casing each write used.
8. Re-replay ds.ds-data-duplicate-sam-account-name-rejected-and-original-untouched.
