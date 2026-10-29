Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 4fd05cf1a47f5cbbe05dba0186301153f554fae3

# The "DefaultLists namespace" page-token rule is written twice, once in each list listing, so the two copies can drift

_Filed: 2026-10-29T14:29:08Z_
_Operation: ListAppsLists | Class: maintainability_

> _Note: this issue was identified against the code base at commit `4fd05cf1a47f5cbbe05dba0186301153f554fae3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListAppsLists` and `ListProtocolsLists` each hand-roll the SAME page-token rule for the namespace their `DefaultLists` member selects: derive the namespace string from the boolean (`input.default_lists.to_string()`), decode a `NextToken` as a token scoped to that namespace and require the boundary to be a minted `ListId`, and mint the next token under the same namespace. The two copies are character-identical apart from the operation name inside the refusal message, and they agree on two things nothing single-homes: how a `DefaultLists` value is spelled as a page scope (`"true"` / `"false"`), and that a boundary of such a page must satisfy `is_minted_identifier`. Either copy can be changed without the other — a different namespace spelling on one side would silently make tokens of the two listings interchangeable again, which is exactly the property `encode_scoped_page_token` exists to prevent.

Everywhere else this crate single-homes exactly this kind of shared rule in `crate::state` — `page_size` for the page bound, `archive_superseded_list_version` for the list version archive, `resource_set_batch_basis` / `write_resource_set_resources` for the batch membership steps, `settle_pending_transition` for the clock-due transition, `refresh_tag_index` for the tag index — so the namespace-scoped token pair is the one shared listing rule with no owner in `state.rs`. An `encode_list_namespace_page_token(default_lists: bool, list_id: &str)` / `decode_list_namespace_page_token(default_lists: bool, token: &str)` pair beside `encode_scoped_page_token` would hold it once; both handlers would keep wording their own refusal, so every response and every existing test stays byte-identical.

## Evidence

Copy 1 — services/service-com-amazonaws-fms-awsfms-20180101/src/ops/list_apps_lists.rs:202-216:
```
        let namespace = input.default_lists.to_string();
        let start = match input.next_token.as_deref() {
            Some(token) => match crate::state::decode_scoped_page_token(&namespace, token)
                .filter(|list_id| crate::state::is_minted_identifier(list_id))
            {
                Some(start) => Some(start),
                None => {
                    return Err(crate::scaffolded::Scaffolded::validation(format!(
                        "NextToken is not a pagination token issued by ListAppsLists for \
                         DefaultLists {namespace}"
                    )));
                }
            },
            None => None,
        };
```
and its mint, list_apps_lists.rs:244-246:
```
        let next_token = remaining
            .next()
            .map(|(list_id, _)| crate::state::encode_scoped_page_token(&namespace, list_id));
```

Copy 2 — src/ops/list_protocols_lists.rs:198-212:
```
        let namespace = input.default_lists.to_string();
        let start = match input.next_token.as_deref() {
            Some(token) => match crate::state::decode_scoped_page_token(&namespace, token)
                .filter(|list_id| crate::state::is_minted_identifier(list_id))
            {
                Some(start) => Some(start),
                None => {
                    return Err(crate::scaffolded::Scaffolded::validation(format!(
                        "NextToken is not a pagination token issued by ListProtocolsLists for \
                         DefaultLists {namespace}"
                    )));
                }
            },
            None => None,
        };
```
and its mint, list_protocols_lists.rs:240-242:
```
        let next_token = remaining
            .next()
            .map(|(list_id, _)| crate::state::encode_scoped_page_token(&namespace, list_id));
```

The helper the two copies build on says the selecting value is what keeps the two namespaces apart — src/state.rs:3405-3416:
```
/// Encode a page boundary of a listing whose page set is SELECTED by a request
/// member — the accounts one policy applies to, the members of one resource set,
/// the lists of one namespace — as an opaque `PaginationToken` carrying the
/// selecting value beside the boundary.
...
/// Carrying the selecting value is what lets
/// [`decode_scoped_page_token`] tell a token minted for THIS page from one
/// minted for another
```

Test coverage that must stay unchanged: tests/all/narrowed_page_token_scoping.rs drives both listings through ONE shared fixture and ONE table — :126-141 holds the `ListAppsLists` and `ListProtocolsLists` rows, and :154 routes both to the same `list_namespace_fixture(page.operation)`, so the suite already treats the rule as one rule while the handlers keep two copies of it. The per-listing walks are pinned by tests/all/apps_lists_enumeration.rs and tests/all/protocols_lists_enumeration.rs, and the shared decode by tests/all/pagination_token_decoding.rs.

## Steps to Reproduce

1. Open services/service-com-amazonaws-fms-awsfms-20180101/src/ops/list_apps_lists.rs:202-216 and :244-246.
2. Open services/service-com-amazonaws-fms-awsfms-20180101/src/ops/list_protocols_lists.rs:198-212 and :240-242.
3. Observe the two blocks are the same rule — `input.default_lists.to_string()` as the page scope, `decode_scoped_page_token(&namespace, token).filter(is_minted_identifier)` on the way in, `encode_scoped_page_token(&namespace, list_id)` on the way out — differing only in the operation name inside the refusal message.
4. Grep `crate::state` for an owner of that pair (as `page_size`, `archive_superseded_list_version`, `resource_set_batch_basis` and `refresh_tag_index` own the crate's other shared rules): there is none, so changing the namespace spelling or the boundary shape check on one side leaves the other serving the old rule.
5. Hoist the pair into `crate::state` beside `encode_scoped_page_token` and call it from both handlers, leaving each handler's own refusal wording in place; every response and every existing test is unchanged.
