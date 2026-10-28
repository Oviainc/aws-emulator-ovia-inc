Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 293c15726352cf981422bfe1d7cee5f6a5389f12

# The "an empty NextToken is no token, an undecodable one is a 400 worded by page_token_report" rule is written out twelve times, once per paged handler

_Filed: 2026-10-28T18:53:37Z_
_Operation: ListAssets | Class: maintainability_

> _Note: this issue was identified against the code base at commit `293c15726352cf981422bfe1d7cee5f6a5389f12`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every paged operation in this crate opens with the same three-part rule, copied verbatim into its handler:

1. an empty `NextToken` names no resume point (`.filter(|token| !token.is_empty())`),
2. a non-empty one is decoded with `crate::state::decode_page_token(<that list's PAGE stamp>, token)`,
3. `None` from that decode is refused as a 400 `ValidationException` whose body is `crate::state::page_token_report(<this operation's name>)`.

`grep -n decode_page_token src/ops` finds TWELVE copies of that block: `list_assets.rs:196`, `list_asset_instances.rs:197`, `list_blocking_instances_for_capacity_task.rs:199`, `list_capacity_tasks.rs:184`, `list_catalog_items.rs:163`, `list_orderable_instance_types.rs:165`, `list_orders.rs:178`, `list_outposts.rs:161`, `list_quotes.rs:155`, `list_sites.rs:159`, `get_outpost_instance_types.rs:200` and `get_outpost_supported_instance_types.rs:223`. Only the PAGE stamp, the operation's error-enum variant and the operation name vary; the rule itself is identical.

That is the one remaining copy-per-handler rule in a crate that otherwise states each shared paging/filtering/update rule exactly once in `state.rs` precisely so the copies cannot drift (`page`, `page_size`, `decode_page_token`, `filter_accepts`, `identifier_filter_accepts`, `apply` all carry a "shared … so they cannot drift apart" doc). The twelve copies have already drifted in two observable ways: half of them name the operation with the `Self::NAME` constant and half with a re-typed string literal, and `list_quotes.rs` builds the refusal through a different error channel entirely from the other eleven.

The defect is the duplication, not any response: the varying parts can be passed in, so a single `state` helper that answers the decoded boundary or the report string leaves every body, status and existing test byte-for-byte unchanged (each handler keeps wrapping the report in its own modelled error variant, including `list_quotes.rs`, whose channel is the subject of its own separate open issue).

## Evidence

Copy 1 — `src/ops/list_assets.rs:196-207` (operation named by a re-typed literal):

        let after = match input.next_token.as_deref().filter(|token| !token.is_empty()) {
            Some(token) => Some(
                crate::state::decode_page_token(crate::state::ASSET_PAGE, token).ok_or_else(
                    || {
                        ListAssetsErrorModeled::ValidationException(ValidationException {
                            message: Some(crate::state::page_token_report("ListAssets")),
                        })
                    },
                )?,
            ),
            None => None,
        };

Copy 2 — `src/ops/list_asset_instances.rs:197-207` (same rule, operation named by `Self::NAME`):

        let after = match input.next_token.as_deref().filter(|token| !token.is_empty()) {
            Some(token) => Some(
                crate::state::decode_page_token(crate::state::ASSET_INSTANCE_PAGE, token)
                    .ok_or_else(|| {
                        ListAssetInstancesErrorModeled::ValidationException(ValidationException {
                            message: Some(crate::state::page_token_report(Self::NAME)),
                        })
                    })?,
            ),
            None => None,
        };

Copy 3, the drift — `src/ops/list_quotes.rs:155-166` states the same rule through a different error channel:

        let after = match input.next_token.as_deref().filter(|token| !token.is_empty()) {
            Some(token) => Some(
                crate::state::decode_page_token(crate::state::QUOTE_PAGE, token).ok_or_else(
                    || {
                        crate::scaffolded::Scaffolded::validation(crate::state::page_token_report(
                            Self::NAME,
                        ))
                    },
                )?,
            ),
            None => None,
        };

The crate's own convention this block is the exception to — `src/state.rs:5888-5893` on the helper these same handlers all call one line later:

    /// Cut one page out of an already-filtered sequence of records, and issue the token
    /// that resumes after it.
    ///
    /// Shared by both list operations so they cannot drift apart: the records are
    /// ordered by creation sequence, one more than the page would fill is what proves a
    /// further page exists, and the token names the last sequence this page returned.

and `src/state.rs:5877-5878` on the sibling selection rule:

    /// Shared by the list operations that take an identifier filter so they cannot drift
    /// apart on that rule.

and `src/state.rs:5049-5051`:

    /// it omits leaves the stored one exactly as it was. Shared by the three update
    /// operations so they cannot drift apart on that rule.

The twelve call sites, from `grep -n decode_page_token services/service-com-amazonaws-outposts-outpostsolafservice/src/ops`:

    list_sites.rs:161, list_quotes.rs:157, list_outposts.rs:163, list_orders.rs:180,
    list_orderable_instance_types.rs:167, list_catalog_items.rs:165,
    list_capacity_tasks.rs:186, list_blocking_instances_for_capacity_task.rs:201,
    list_assets.rs:198, list_asset_instances.rs:199,
    get_outpost_supported_instance_types.rs:225, get_outpost_instance_types.rs:202

Test coverage that must stay unchanged (each pins the 400 for a replayed and for an unissued token): `tests/all/outpost_assets.rs:733-746` ("a ListOutposts token replayed on ListAssets" -> 400 ValidationException, and "a token this service never issued at all" -> 400), `tests/all/asset_instances.rs:583` ("A token this service issued for ANOTHER list is not a boundary of this one."), `tests/all/blocking_instances.rs:753` (same comment), plus the paging suites `capacity_task_listing.rs`, `order_listing.rs`, `quote_listing.rs`, `catalog_items.rs`, `orderable_instance_types.rs`, `supported_instance_types.rs` and `configured_instance_types.rs`.

## Steps to Reproduce

1. grep -n decode_page_token services/service-com-amazonaws-outposts-outpostsolafservice/src/ops — twelve hits, one per paged handler.
2. Read `src/ops/list_assets.rs:196-207` and `src/ops/list_asset_instances.rs:197-207`: the same empty-token filter, the same decode, the same `page_token_report` refusal, differing only in the PAGE stamp, the error variant and how the operation is named (literal vs `Self::NAME`).
3. Read `src/ops/list_quotes.rs:155-166`: the twelfth copy has already drifted onto a different error channel (`crate::scaffolded::Scaffolded::validation`) from the other eleven.
4. Compare with `src/state.rs:5888-5893`, `src/state.rs:5877-5878` and `src/state.rs:5049-5051`, where every other rule more than one handler of this crate applies is stated once with an explicit "so they cannot drift apart" note.
