Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 099b155a5bca3b1c2e4dd48b7f9cfde84b7dc96c

# A nextToken this operation never issued is accepted and silently truncates the listing instead of being refused

_Filed: 2026-10-28T19:57:49Z_
_Operation: ListInvestigationGroups | Class: pagination_

> _Note: this issue was identified against the code base at commit `099b155a5bca3b1c2e4dd48b7f9cfde84b7dc96c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`decode_page_token` is documented as refusing any token the operation did not issue, but it only checks the token's SHAPE: hex-decodable to 16 ASCII-alphanumeric bytes (`crate::state::is_group_id`). Any 32-character hex string whose decoded bytes satisfy that shape — a group id no group ever had — therefore passes, and the handler hands it straight to `groups_in_region_from(&region, start.as_deref())`, whose own doc states `start_id` need not be a stored id and that the walk simply begins at the first id that is not before it.

The consequence is a wrong 200 rather than the documented 400: a forged token whose decoded id sorts above every stored id (e.g. hex of `zzzzzzzzzzzzzzzz` = `7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a`) makes the walk yield nothing, so the response is `{"investigationGroups": []}` with NO `nextToken`. A caller is told the Region holds no investigation group while the store holds one, and the absent `nextToken` means the response is indistinguishable from a complete, final page — the listing silently hides persisted state instead of refusing the token. Nothing in the handler ties the decoded id back to the store (or to the Region), which is the only thing that could make the documented rule true. This is a separate defect from the already-open (ListInvestigationGroups, error-response) issue, which concerns the SHAPE of the error returned for the tokens that ARE refused; this one is about tokens that are never refused at all.

## Evidence

The rule the crate states for a token — `services/service-com-amazonaws-aiops-aiops/src/ops/list_investigation_groups.rs:121-127`:
```
/// What a `nextToken` has to be, for the refusal of one this operation did not issue.
...
const NEXT_TOKEN_REQUIREMENT: &str =
    "Member must be a pagination token returned by a previous ListInvestigationGroups call";
```
and `src/ops/list_investigation_groups.rs:139-140`:
```
/// The group id a page token names, or a 400 refusal when the token is not one this
/// operation issued.
```
What the code actually checks — `src/ops/list_investigation_groups.rs:148-159`:
```
fn decode_page_token(token: &str) -> Result<String, ListInvestigationGroupsError> {
    hex::decode(token)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .filter(|id| crate::state::is_group_id(id))
        .ok_or_else(|| { ... })
}
```
`is_group_id` is a pure shape test — `src/state.rs:1090-1092`:
```
pub fn is_group_id(text: &str) -> bool {
    text.len() == GROUP_ID_LEN && text.bytes().all(|b| b.is_ascii_alphanumeric())
}
```
and the reader it is handed to accepts an id no group has — `src/state.rs:301-302`:
```
/// for a listing alike. `start_id` need not be a stored id: the walk simply
/// begins at the first id that is not before it.
```
A token is minted only from a STORED group's id — `src/ops/list_investigation_groups.rs:230-232`:
```
            if investigation_groups.len() == page_size {
                next_token = Some(encode_page_token(&group.id));
```
Test coverage: `tests/all/investigation_groups.rs:623-639` pins exactly this rule — `// A token this operation never issued is refused, not silently ignored.` — but probes only `nextToken=not-a-token`, which fails the hex decode, so the shape-valid forged token is untested.

## Steps to Reproduce

1. POST /investigationGroups signed for us-west-2 with {"name":"g","roleArn":"arn:aws:iam::000000000000:role/AIOpsAssistantRole"} — 201, the Region now holds one group
2. GET /investigationGroups signed for us-west-2 — 200, the listing reports that group and carries no nextToken
3. GET /investigationGroups?nextToken=7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a (hex of the never-issued id "zzzzzzzzzzzzzzzz") signed for us-west-2
4. Observe 200 with {"investigationGroups": []} and no nextToken — the existing group is hidden and the page looks final
5. Compare with src/ops/list_investigation_groups.rs:121-127 and :139-140, which state a token this operation did not issue is refused with 400
