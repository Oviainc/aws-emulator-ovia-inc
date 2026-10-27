Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 58a1ce99e9431000682e2d92b8b10e21bce3c765

# The scope-qualified pagination-token codec is written out three times, once per listing, and the copies have already drifted

_Filed: 2026-10-26T23:12:56Z_
_Operation: ListTables | Class: maintainability_

> _Note: this issue was identified against the code base at commit `58a1ce99e9431000682e2d92b8b10e21bce3c765`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Three of this crate's four `@paginated` listings encode their resume point with the identical computation — hex-encode `"<scope>#<item>"` — and decode it with the identical three steps: hex-decode, `split_once('#')`, then accept only when the scope half matches and the item half is non-empty. Only the value put in the scope half differs (the keyspace name, `"types/" + keyspace name`, and `hex(resourceArn)`), so the rule is one computation with three parameters, written three times.

The copies have already drifted in a way that matters: `list_types.rs` added a `PAGE_TOKEN_SCOPE` prefix precisely because "`ListTables` pages the same keyspace by a name of the same shape, and resuming a type listing after a TABLE name would silently skip types", and `list_tables.rs` has no such scope marker of its own — the cross-listing safety of the pair rests on only one of the two sides carrying the prefix. A fourth copy of the same shape added later has nothing to inherit from.

A single helper in `crate::state` (beside `page_window`, which was already hoisted there for the same reason) taking the scope string and the item, with each listing keeping only its own scope expression, reproduces every token byte-for-byte and leaves every response and every existing test unchanged.

## Evidence

Copy 1, `services/service-com-amazonaws-keyspaces-keyspacesservice/src/ops/list_tables.rs:93-104`:
```
fn encode_page_token(keyspace_name: &str, table_name: &str) -> String {
    ::hex::encode(format!("{keyspace_name}#{table_name}"))
}
fn decode_page_token(token: &str, keyspace_name: &str) -> Option<String> {
    let decoded = String::from_utf8(::hex::decode(token).ok()?).ok()?;
    let (keyspace, table) = decoded.split_once('#')?;
    (keyspace == keyspace_name && !table.is_empty()).then(|| table.to_string())
}
```
Copy 2, `src/ops/list_types.rs:104-115` — the same computation with a scope prefix bolted on:
```
fn encode_page_token(keyspace_name: &str, type_name: &str) -> String {
    ::hex::encode(format!("{PAGE_TOKEN_SCOPE}{keyspace_name}#{type_name}"))
}
fn decode_page_token(token: &str, keyspace_name: &str) -> Option<String> {
    let decoded = String::from_utf8(::hex::decode(token).ok()?).ok()?;
    let (keyspace, type_name) = decoded.strip_prefix(PAGE_TOKEN_SCOPE)?.split_once('#')?;
    (keyspace == keyspace_name && !type_name.is_empty()).then(|| type_name.to_string())
}
```
with the drift stated in its own doc, `src/ops/list_types.rs:89-95`: "What a `ListTypes` resume point is prefixed with, so a token this listing issued is distinguishable from one another listing of the same keyspace issued: `ListTables` pages the same keyspace by a name of the same shape, and resuming a type listing after a TABLE name would silently skip types."

Copy 3, `src/ops/list_tags_for_resource.rs:94-105`:
```
fn encode_page_token(resource_arn: &str, tag_key: &str) -> String {
    ::hex::encode(format!("{}#{tag_key}", ::hex::encode(resource_arn)))
}
fn decode_page_token(token: &str, resource_arn: &str) -> Option<String> {
    let decoded = String::from_utf8(::hex::decode(token).ok()?).ok()?;
    let (arn, key) = decoded.split_once('#')?;
    (arn == ::hex::encode(resource_arn) && !key.is_empty()).then(|| key.to_string())
}
```
The precedent for hoisting, already done once for the window half of the same job, `src/state.rs:397-412`: "One page of a listing, and whether a further item follows it — the page window every `@paginated` operation of this service reports through" / `pub fn page_window<Item>(`.

Tests that cover the three codecs and must stay unchanged: `tests/all/pagination_window.rs`, `tests/all/list_types.rs`, `tests/all/list_keyspaces.rs`, and the tag-paging rows in `tests/all/resource_tagging.rs:787` ("Keys deliberately not in sorted order, so a page boundary cannot come out right by" accident).

## Steps to Reproduce

1. Read src/ops/list_tables.rs:93-104, src/ops/list_types.rs:104-115 and src/ops/list_tags_for_resource.rs:94-105 — the same hex(scope#item) encode and the same decode/split/verify triple, three times
2. Note the drift: only list_types.rs carries a PAGE_TOKEN_SCOPE marker, added for cross-listing safety against ListTables, which has none of its own
3. Compare with src/state.rs:397-422, where the other half of the same job (page_window) is already shared by all four listings
