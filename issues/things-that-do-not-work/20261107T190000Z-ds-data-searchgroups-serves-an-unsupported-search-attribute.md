Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ds.ds-data-group-inventory-and-search-answer-for-the-builtin-groups
Issue-Key: ds.ds-data-group-inventory-and-search-answer-for-the-builtin-groups|_refusal#38|AssertionError|raise AssertionError(

# ds-data SearchGroups serves a search over an attribute the API does not support for search instead of refusing it with ValidationException / Reason INVALID_ATTRIBUTE_FOR_SEARCH

## Operation
SearchGroups

## Description
One cause, split out of 20261107T015500Z-ds-data-searchgroups-echoes-the-constant-realm-corp.md (the realm defect that blocked this leg of the same behavioral check and is now fixed). With the realm fixed, the check reaches its next leg and the emulator diverges there.

=== Cause 1 of 1: an unsupported `SearchAttributes` entry is searched instead of refused ===
Failing check: ds.ds-data-group-inventory-and-search-answer-for-the-builtin-groups

AWS: Smithy (com.amazonaws.directoryservicedata) SearchGroups -> POST /Groups/SearchGroups; `SearchAttributes` is `@required`, targets `LdapDisplayNameList` (len 1..25, member `LdapDisplayName` len 1..63 pattern `^[A-Za-z*][A-Za-z-*]*$`), and its own documentation says "One or more data attributes that are used to search for a group. For a list of supported attributes, see Directory Service Data Attributes" (https://docs.aws.amazon.com/directoryservice/latest/admin-guide/ad_data_attributes.html). `ValidationExceptionReason` declares `INVALID_ATTRIBUTE_FOR_SEARCH` precisely for this case, so a request naming an attribute that table does not list as searchable for the object kind is refused with `ValidationException` (HTTP 400) carrying that `Reason`; `SearchUsers` has the same member and the same reason. Recorded behavior: `SearchGroups(SearchAttributes=['Cn'])` against a live Managed Microsoft AD is REFUSED, while `SearchAttributes=['SAMAccountName']` is served.

Emulator: every attribute name is searched. `services/service-com-amazonaws-directoryservicedata-directoryservicedata/src/state.rs` (`object_matches_search`, and the `SAM_ACCOUNT_NAME_ATTRIBUTES` / `SID_ATTRIBUTES` / … alias tables above it) matches whatever the request names against the object's structural members and its `OtherAttributes`, and answers HTTP 200 with the groups that happen to match — so `Cn`, which the alias table treats as a synonym of `sAMAccountName`, returns the group, and an attribute no object holds returns an empty page. There is no searchable-attribute layer anywhere: `grep -r INVALID_ATTRIBUTE_FOR_SEARCH services/service-com-amazonaws-directoryservicedata-directoryservicedata/src` matches only the modelled value list in `src/ops/types/shared_types.rs`.

Replay: "AssertionError: SearchGroups over 'Cn', an attribute Active Directory does not index for search SUCCEEDED where real AWS refuses it".

What the fixer needs that this repository does not carry: the SET of attributes the API supports for SEARCH, per object kind (user vs group). It is published only in the admin-guide attribute table the member's own documentation links (ad_data_attributes.html) — the Smithy model carries the attribute NAME constraints but no catalogue, and the recording shows one data point (`Cn` refused, `SAMAccountName` served). Writing the catalogue from that single data point would fit the recorded instance rather than the rule, and refusing every name outside a guessed set would also refuse the `OtherAttributes` searches this crate's own suites exercise (`Description`, `Info`, `Department`, …) with no evidence. So the fix is: take the supported-attribute table from that documentation as the source, state it once in the crate (one list per object kind, each entry citing that table), refuse a request naming anything outside it with `ValidationException` + `Reason` `INVALID_ATTRIBUTE_FOR_SEARCH` ahead of any object lookup, and reconcile the alias tables in `src/state.rs` with it (today `cn` is treated as a search name for `sAMAccountName`). The same layer serves `SearchUsers`.

## Evidence
expected (AWS): dsdata.search_groups(DirectoryId='d-xxxxxxxxxx', SearchString=<group>, SearchAttributes=['Cn']) -> HTTP 400 ValidationException, Reason INVALID_ATTRIBUTE_FOR_SEARCH, Message naming the attribute the request may not search on.
  actual (emulator): HTTP 200 with the matching group in `Groups` (the response the check reported carried Content-Length 233), because `Cn` is matched as a synonym of `sAMAccountName`.
  code: services/service-com-amazonaws-directoryservicedata-directoryservicedata/src/state.rs — `SAM_ACCOUNT_NAME_ATTRIBUTES = &["samaccountname", "cn"]` and `object_matches_search`; services/service-com-amazonaws-directoryservicedata-directoryservicedata/src/ops/search_groups.rs calls `group.matches_search(&input.search_attributes, &input.search_string)` with no attribute admissibility check; the same for src/ops/search_users.rs.

Smithy consulted (services/service-com-amazonaws-directoryservicedata-directoryservicedata/smithy-model/com-amazonaws-directoryservicedata-directoryservicedata.json):
  SearchGroups: POST /Groups/SearchGroups, 200, @readonly; input {DirectoryId(req), SearchString(req, len 1..64, @sensitive), SearchAttributes(req, LdapDisplayNameList len 1..25), Realm, NextToken, MaxResults}; errors AccessDeniedException(403), DirectoryUnavailableException(400), InternalServerException(500), ThrottlingException(429), ValidationException(400).
  ValidationExceptionReason values include INVALID_ATTRIBUTE_FOR_SEARCH, INVALID_ATTRIBUTE_FOR_GROUP, INVALID_ATTRIBUTE_FOR_USER, INVALID_ATTRIBUTE_FOR_MODIFY, INVALID_ATTRIBUTE_NAME — a family of per-context attribute allow-lists the service maintains.

## Steps to Reproduce
1. Boot the emulator; create botocore clients for 'ds', 'ec2' and 'ds-data' against it.
2. ds.create_microsoft_ad(Name='<unique>.example.com', ...); poll until Stage == 'Active'; ds.enable_directory_data_access(DirectoryId=<id>); poll until DataAccessStatus == 'Enabled'.
3. dsdata.create_group(DirectoryId=<id>, SAMAccountName='s2rGrp<unique>').
4. dsdata.search_groups(DirectoryId=<id>, SearchString='s2rGrp<unique>', SearchAttributes=['SAMAccountName']) -> HTTP 200 with the group (correct today).
5. dsdata.search_groups(DirectoryId=<id>, SearchString='s2rGrp<unique>', SearchAttributes=['Cn']) -> the emulator answers HTTP 200 with the group; AWS refuses the call with ValidationException / Reason INVALID_ATTRIBUTE_FOR_SEARCH.
6. Repeat step 5 against dsdata.search_users with an attribute the table does not list for a user, then re-replay ds.ds-data-group-inventory-and-search-answer-for-the-builtin-groups.
