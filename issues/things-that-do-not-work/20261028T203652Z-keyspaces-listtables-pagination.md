Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 825b0e930e0bc7d005dad99ff95af4e6881702a4

# Page tokens are not Region-qualified, so a nextToken issued for a keyspace in one Region silently resumes the same-named keyspace in another

_Filed: 2026-10-28T20:36:52Z_
_Operation: ListTables | Class: pagination_

> _Note: this issue was identified against the code base at commit `825b0e930e0bc7d005dad99ff95af4e6881702a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate's pagination-token codec documents, as its reason for existing, that a token carrying a scope other than the listing's own is REFUSED rather than resumed, precisely so a foreign token cannot "resume at a point that would silently skip items". It also states, in the same module, that a keyspace (with the tables and types it holds) is a resource of ONE Region and that the same `keyspaceName` names a DIFFERENT resource in another Region.

The two rules are not joined up. `ListTables` qualifies its token with the bare `keyspaceName`, and `ListTypes` with `types/<keyspaceName>` — neither carries the Region the page came from. `ListKeyspaces` mints a token that is just the hex of a `keyspaceName`, with no scope half at all. So a `nextToken` a listing issued for keyspace `app` signed for `us-east-1` decodes successfully against a listing of keyspace `app` signed for `eu-west-1` — a different keyspace holding a different set of tables — and the handler resumes that other collection at `Bound::Excluded(<the other Region's table name>)`, returning a page that silently omits every table of the asking Region that sorts at or before that name. No `ValidationException` is raised, which is exactly the failure mode the codec's own doc says it prevents.

The sibling listing in the same crate shows what the correct scope looks like: `ListTagsForResource` qualifies its token with the hex of the full `resourceArn`, which is Region-qualified (`keyspace_arn`/`table_arn` embed the Region), so its tokens are already inseparable across Regions. `ListTables`, `ListTypes` and `ListKeyspaces` are the three that are not.

All three symptoms are one defect in one class, so they are consolidated here on `ListTables` (the clearest contrast with its ARN-scoped sibling).

## Evidence

The guarantee the code breaks — `services/service-com-amazonaws-keyspaces-keyspacesservice/src/state.rs:439-443` and `:448-455`:
```
/// The `scope` is also what keeps the listings apart. Two listings of this service page
/// collections whose item names have the same shape, so a token one issued must not resume
/// the other: a listing whose scope expression could collide with another's qualifies it
/// further (a prefix no other scope can carry), and [`decode_page_token`] then refuses a
/// foreign token rather than resuming at a point that would silently skip items.
...
/// The item a `nextToken` resumes after, or `None` when the token is not one
/// [`encode_page_token`] issued for `scope` — a token minted for a different scope, or by a
/// different listing, resumes nothing and is refused rather than silently ignored.
pub fn decode_page_token(token: &str, scope: &str) -> Option<String> {
    let decoded = String::from_utf8(::hex::decode(token).ok()?).ok()?;
    let (token_scope, item) = decoded.split_once(PAGE_TOKEN_SEPARATOR)?;
    (token_scope == scope && !item.is_empty()).then(|| item.to_string())
}
```

The other half of the rule, in the same module — `src/state.rs:819-823`:
```
    /// The keyspaces callers have created, partitioned by the Amazon Web Services Region
    /// of the request that created each one and keyed inside a Region by `keyspaceName` —
    /// "keyspace names must be unique within each Region" (`CreateKeyspace`), so a
    /// keyspace, with the tables and types it holds, is a resource of ONE Region and the
    /// same name names a different resource in another.
```

The break — `src/ops/list_tables.rs:87-97`:
```
/// The scope a `ListTables` resume point is qualified by, which is the keyspace whose
/// tables the page came from: a token this listing issued resumes only a listing of the
/// SAME keyspace, and [`crate::state::decode_page_token`] refuses one minted for another.
...
fn page_token_scope(keyspace_name: &str) -> &str {
    keyspace_name
}
```
and the handler resuming on it, `src/ops/list_tables.rs:181-228`:
```
            Some(token) => Some(crate::state::decode_page_token(
                token,
                page_token_scope(&input.keyspace_name),
            )
...
        let start = match resume_after.as_deref() {
            Some(after) => ::std::ops::Bound::Excluded(after),
            None => ::std::ops::Bound::Unbounded,
        };
        let (page, truncated) = crate::state::page_window(
            keyspace
                .tables
                .range::<str, _>((start, ::std::ops::Bound::Unbounded))
```
The same gap in `src/ops/list_types.rs:96-98`:
```
fn page_token_scope(keyspace_name: &str) -> String {
    format!("types/{keyspace_name}")
}
```
and in `src/ops/list_keyspaces.rs:87-101`, whose whole token is a bare keyspace name:
```
fn encode_page_token(keyspace_name: &str) -> String {
    ::hex::encode(keyspace_name)
}
...
fn decode_page_token(token: &str) -> Option<String> {
    let decoded = String::from_utf8(::hex::decode(token).ok()?).ok()?;
    is_keyspace_name(&decoded).then_some(decoded)
}
```

The sibling that DOES separate Regions, `src/ops/list_tags_for_resource.rs:88-95`:
```
/// The scope a `ListTagsForResource` resume point is qualified by: the resource the page's
/// tags are held on, hex-encoded so the scope half cannot contain the codec's separator
/// whatever the ARN carries, and so no other listing's scope collides with it.
fn page_token_scope(resource_arn: &str) -> String {
    ::hex::encode(resource_arn)
}
```
That ARN is Region-qualified — `src/state.rs:2028-2034`:
```
pub fn table_arn(region: &str, keyspace_name: &str, table_name: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("/keyspace/{keyspace_name}/table/{table_name}"),
    )
}
```

Test coverage: nothing pins the cross-Region case, so nothing constrains the fix. `tests/all/page_token_codec.rs:31-34` states the exact harm it is guarding against but varies only the keyspace NAME, never the Region:
```
/// The items every collection of every keyspace below holds — the same names in all of
/// them, deliberately: a token honoured outside the scope it was issued for would then
/// resume at a plausible point and silently skip items instead of failing loudly.
const ITEMS: [&str; 4] = ["p_alpha", "p_bravo", "p_charlie", "p_delta"];
```
and `tests/all/page_token_codec.rs:185-186` records the scope as the bare name:
```
            // `ListTables` qualifies its tokens with the keyspace name alone.
            token_scope: keyspace.to_string(),
```
The same-name-in-two-Regions shape is explicitly supported and tested — `tests/all/regional_partitioning.rs:15-16`:
```
//! 1. The same `keyspaceName` is created once per Region, each time minting that Region's
//!    ARN, while a second create in the SAME Region is still the modelled conflict.
```
but `regional_partitioning.rs` never pages a listing across Regions.

## Steps to Reproduce

1. Start the Keyspaces emulator and call with a SigV4 credential scope per request, as tests/all/regional_partitioning.rs does
2. Signed for us-east-1: CreateKeyspace {"keyspaceName":"app"}, then CreateTable for tables "t_aaa", "t_bbb", "t_ccc" in it (minimal schema each)
3. Signed for eu-west-1: CreateKeyspace {"keyspaceName":"app"}, then CreateTable for tables "t_aaa", "t_bbb", "t_ccc" in it
4. Wait out the transient CREATING span so every table is ACTIVE
5. Signed for us-east-1: ListTables {"keyspaceName":"app","maxResults":2} — it returns t_aaa, t_bbb and a nextToken (hex of "app#t_bbb")
6. Signed for eu-west-1: ListTables {"keyspaceName":"app","nextToken":"<that token>"} — expected a 400 ValidationException ("nextToken is not a pagination token this service issued for keyspace 'app'"), since the token was minted for a DIFFERENT keyspace resource; actual: 200 with only {"tables":[t_ccc]}, silently omitting eu-west-1's t_aaa and t_bbb
7. Repeat with ListTypes (token scope "types/app") and with ListKeyspaces (token is the bare hex of a keyspaceName) — both accept the other Region's token the same way
