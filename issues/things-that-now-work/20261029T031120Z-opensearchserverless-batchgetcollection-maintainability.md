Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: c088b2b7fac1291fdb05737f13bee382d9ee2825

# The ids/names mutual-exclusion refusal is written out twice, once per batch reader

_Filed: 2026-10-29T03:11:20Z_
_Operation: BatchGetCollection | Class: maintainability_

> _Note: this issue was identified against the code base at commit `c088b2b7fac1291fdb05737f13bee382d9ee2825`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The rule that a batch read must name `ids` or `names` but not both — the branch AND both refusal sentences — is implemented verbatim in two handlers of this crate: `src/ops/batch_get_collection.rs` and `src/ops/batch_get_collection_group.rs`. It is one rule read off one pair of modelled member docs (`ids`/`names` on both request shapes carry the same "You can't provide names and IDs in the same request" sentence), so the two copies can drift: a change to the branch (for example adding a third selector, or wording the "neither" case differently) applied to one reader leaves the other answering a different refusal for the same shape of request.

This crate already states where a shared helper belongs — `src/state.rs` hosts "a helper more than one operation reaches" — and it already keeps exactly this kind of one-rule-one-place discipline elsewhere (`crate::state::first_repeated_tag_key` for the tag-key uniqueness rule across four handlers, `crate::state::policy_document_from_text` for the policy-parse rule across the policy handlers, `crate::state::statuses_phrase` for the deletable-status sentence across two deletes). The selector rule is the one that was not factored out.

The fix is a pure extraction: one helper in `src/state.rs` returning the same `Option<&str>`/message, called from both handlers. Every response byte and every existing test stays unchanged.

## Evidence

Copy 1 — services/service-com-amazonaws-opensearchserverless-opensearchserverless/src/ops/batch_get_collection.rs:153-160:
```
        // From `ids`' and `names`' own documentation: "You can't provide names and IDs in
        // the same request" — and a request that names neither selects no collection at
        // all, so neither combination is servable.
        let refusal = match (input.ids.as_deref(), input.names.as_deref()) {
            (Some(_), Some(_)) => Some("ids and names cannot be provided in the same request"),
            (None, None) => Some("one of ids or names must be provided"),
            _ => None,
        };
```

Copy 2 — services/service-com-amazonaws-opensearchserverless-opensearchserverless/src/ops/batch_get_collection_group.rs:156-163 (same branch, same two sentences):
```
        // From `ids`' and `names`' own documentation: "You can't provide names and IDs in
        // the same request" — and a request that names neither selects no collection
        // group at all, so neither combination is servable.
        let refusal = match (input.ids.as_deref(), input.names.as_deref()) {
            (Some(_), Some(_)) => Some("ids and names cannot be provided in the same request"),
            (None, None) => Some("one of ids or names must be provided"),
            _ => None,
        };
```

The crate's own stated home for a shared helper — services/service-com-amazonaws-opensearchserverless-opensearchserverless/src/state.rs:17-18:
```
//! `src/ops/mod.rs` is generated, so a helper more than one operation reaches lives here
//! rather than in a second module under `src/ops/`.
```

Sibling rules that ARE factored into that one place — src/state.rs:2028 `pub fn first_repeated_tag_key(` ("the rule is enforced by the handler that takes the member", called from create_collection.rs:228, create_collection_group.rs:228, tag_resource.rs:155, untag_resource.rs:150) and src/state.rs:2999 `pub fn policy_document_from_text(` ("Both halves of the rule ... in one place").

Test coverage that pins both copies' observable answers (so an extraction must leave them green):
- tests/all/collection_lifecycle.rs:511-525 — `for (case, request) in [("both", ...), ("neither", json!({}))]` against `BatchGetCollection`, asserting `(status, code) == (CLIENT_ERROR, "ValidationException")`.
- tests/all/collection_group_lifecycle.rs:389-403 — the same two rows against `BatchGetCollectionGroup`: `"case BatchGetCollectionGroup({case}): expected 400 ValidationException"`.

## Steps to Reproduce

1. Open services/service-com-amazonaws-opensearchserverless-opensearchserverless/src/ops/batch_get_collection.rs and read lines 153-168: the (ids, names) match arms and the two refusal strings.
2. Open src/ops/batch_get_collection_group.rs and read lines 156-171: the identical match arms and the identical two refusal strings.
3. Confirm no shared helper exists: `grep -rn "cannot be provided in the same request" services/service-com-amazonaws-opensearchserverless-opensearchserverless/src` returns the two handler copies and nothing in src/state.rs.
4. Confirm the crate factors comparable rules into state.rs instead: `grep -rn "first_repeated_tag_key\|policy_document_from_text" services/service-com-amazonaws-opensearchserverless-opensearchserverless/src` shows one definition in state.rs with several handler call sites.
5. Change one copy's wording or branch and observe that the other reader keeps the old answer, while tests/all/collection_lifecycle.rs:511 and tests/all/collection_group_lifecycle.rs:389 each only exercise one copy.
