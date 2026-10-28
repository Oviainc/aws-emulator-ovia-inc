Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 804429c0e5267423598a5b7dbc8d0525cc046b43

# Scope per-group pagination tokens by region, not by group name alone

_Filed: 2026-10-28T02:38:29Z_
_Operation: ListGroupResources | Class: pagination_

> _Note: this issue was identified against the code base at commit `804429c0e5267423598a5b7dbc8d0525cc046b43`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListGroupResources` builds the token "listing" name from the group NAME only (`format!("group-resources/{group_name}")`), while this crate's own identity rule is that a group is identified by the PAIR (region, name). A `NextToken` minted for the same-named group in a DIFFERENT region therefore decodes successfully and is honoured as a resume point, instead of being refused with the handler's own `BadRequestException` ("it was not issued by a previous ListGroupResources call for this group"). The result is a 200 carrying a silently truncated page (every member whose ARN sorts at or before the foreign resume key is dropped) where the handler intends an error — an error turned into a success.

The same construction is used by `ListGroupingStatuses` (`format!("grouping-statuses/{group_name}")`), so a fix should scope every per-group listing name with the region the token was issued in. Note the token encoding itself (`crate::state::encode_page_token`) carries no region either, so only the listing name can carry it.

## Evidence

The handler claims the token is scoped to one group:

  services/service-com-amazonaws-resourcegroups-ardi/src/ops/list_group_resources.rs:266-269
    "// The token is this service's own opaque encoding of the ARN the previous
     // page stopped at, scoped to THIS listing of THIS group; anything else
     // cannot name a resume point in it.
     let listing = listing_of(&key.name);"

  services/service-com-amazonaws-resourcegroups-ardi/src/ops/list_group_resources.rs:275-278 (the refusal it intends)
    "NextToken is not valid: it was not issued by a previous \
     ListGroupResources call for this group"

but the listing name carries only the name:

  services/service-com-amazonaws-resourcegroups-ardi/src/ops/list_group_resources.rs:321-325
    "/// The listing one group's pagination tokens belong to: a token issued here
     /// resumes THIS listing of THAT group, and no other.
     fn listing_of(group_name: &str) -> String {
         format!("group-resources/{group_name}")
     }"

while the crate's own state module says the name alone does not identify a group:

  services/service-com-amazonaws-resourcegroups-ardi/src/state.rs:63-67
    "A group name only has to be unique within one region of one account, so the name alone
     does not identify a group; the pair does."

and the token encoding carries no region:

  services/service-com-amazonaws-resourcegroups-ardi/src/state.rs:1770-1772
    "pub fn encode_page_token(listing: &str, last_key: &str) -> String {
         aws_smithy_types::base64::encode(format!("{PAGE_TOKEN_PREFIX}{listing}/{last_key}"))"

The sibling listing has the identical shape:

  services/service-com-amazonaws-resourcegroups-ardi/src/ops/list_grouping_statuses.rs:299-301
    "fn listing_of(group_name: &str) -> String { format!("grouping-statuses/{group_name}") }"

Test coverage shows the intended rule is already pinned — but only for two groups in ONE region:

  services/service-com-amazonaws-resourcegroups-ardi/tests/all/grouping_statuses.rs:528-531
    "\"a NextToken issued for another group's grouping statuses\".to_string(),
     json!({ \"Group\": \"refusals\", \"NextToken\": other_groups_token }),"
  services/service-com-amazonaws-resourcegroups-ardi/tests/all/resource_group_lifecycle.rs:1201
    "a token issued for another group's membership listing"

No test crosses regions with a token (tests/all/region_isolation.rs contains no `NextToken` case), so nothing justifies the divergence.

## Steps to Reproduce

1. In region eu-west-1: CreateGroup Name=foo with Configuration [{"Type":"AWS::EC2::HostManagement"}], then GroupResources Group=foo with two resource ARNs.
2. In region us-east-1: CreateGroup Name=foo with the same configuration, then GroupResources Group=foo with two other resource ARNs.
3. Call ListGroupResources in eu-west-1 with {"Group":"foo","MaxResults":1} and keep the returned NextToken.
4. Call ListGroupResources in us-east-1 with {"Group":"foo","MaxResults":1,"NextToken":<that token>}.
5. Expected (per the handler's own comment and refusal message): 400 BadRequestException, because the token was not issued by a previous ListGroupResources call for THIS group. Actual: 200, with the us-east-1 group's page resumed after the eu-west-1 group's resume ARN (members sorting at or before it are silently dropped).
6. Repeat with ListGroupingStatuses for the same pair of groups: same outcome.
