Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: f08062a3928279f71031748dc453b175f9086c5b

# Per-Outpost page tokens carry a positional offset and no parent stamp, so another Outpost's token resumes this list and silently drops assets

_Filed: 2026-10-28T22:53:26Z_
_Operation: ListAssets | Class: pagination_

> _Note: this issue was identified against the code base at commit `f08062a3928279f71031748dc453b175f9086c5b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The four lists nested under an Outpost (`ListAssets`, `ListAssetInstances`, `ListBlockingInstancesForCapacityTask`, `GetOutpostInstanceTypes`) page by handing `crate::state::page` an item's POSITION in the parent's own vector, while `crate::state::page` / `encode_page_token` / `decode_page_token` are documented to carry a creation SEQUENCE and to stamp a token with "the list it belongs to". The stamp is the list KIND only (`"assets"`, `"assetInstances"`, …) and never the Outpost the list was cut from, and the number in it is an offset, not a sequence. Two defects follow, both of them silent omissions on a read:

1. A token minted while paging Outpost A's assets is honoured when replayed on Outpost B's `/outposts/{B}/assets` route — the stamp matches, so `decode_page_token` returns the offset and `page` drops every asset of B at or before that position. The caller gets a 200 with a short list and no indication anything was skipped, although this handler states, and the crate's own test suite pins, that a token this service did not issue for THIS list is a 400 `ValidationException` (a token stamped `outposts` or `blockingInstances` on the same route IS refused). A token issued for Outpost A's asset list was never issued for Outpost B's asset list.

2. Even on the SAME Outpost, the positional boundary is exactly the resume point `encode_page_token` documents as wrong: an Outpost's `assets` vector is REPLACED between two pages of a walk by `settle_outposts` (the install an order's fulfilment owes) and CLEARED by `UpdateOutpost` through `OutpostRecord::set_supported_hardware_type`. A second page taken after either event resumes at a position in a different, freshly minted hardware list and silently skips whatever now sits at the positions the token covers.

The fix belongs in the token the nested lists mint and decode (the parent must be part of what the stamp binds, and a boundary must identify the item, not its index); the other three handlers above decode the same way and share the defect.

## Evidence

The shared helper's documented contract — `src/state.rs:5834-5844`:

    /// Encode a page boundary as an opaque `Token`.
    ///
    /// The token carries the creation sequence of the LAST item the page returned, so
    /// the next page is everything created strictly after it — a resume point that
    /// stays correct when items are added or removed between pages, which a positional
    /// offset would not. It is STAMPED with the list it belongs to, so a token replayed
    /// against another list is refused rather than honoured as that list's boundary.
    pub fn encode_page_token(kind: &str, last_sequence: u64) -> String {
        format!("{last_sequence}##{kind}")
    }

and all the decode actually checks — `src/state.rs:5849-5855`:

    pub fn decode_page_token(kind: &str, token: &str) -> Option<u64> {
        let (sequence, stamp) = token.split_once("##")?;
        if stamp != kind {
            return None;
        }
        sequence.parse().ok()
    }

What `ListAssets` feeds it — `src/ops/list_assets.rs:221-234`:

    // The boundary this list pages on is an asset's POSITION in the Outpost's
    // installation order: an asset carries no creation sequence of its own, and
    // that order is the one stable order the whole list is reported in.
    let (assets, next_token) = crate::state::page(
        crate::state::ASSET_PAGE,
        record
            .assets
            .iter()
            .enumerate()
            .filter(|(_, asset)| matches_filters(asset, &input))
            .map(|(position, asset)| (position as u64, asset.clone())),
        after,
        size,
    );

while the same handler states the rule it does not keep — `src/ops/list_assets.rs:194-204`:

    // The resume point the caller's token names. A token this service did not
    // issue for THIS list is a bad request, not an empty page.
    ...
            crate::state::decode_page_token(crate::state::ASSET_PAGE, token).ok_or_else(

and the stamp the crate believes is enough — `src/state.rs:5749-5756`:

    /// The stamp a `ListAssets` page token carries.
    ///
    /// An asset is not a top-level resource with a creation sequence of its own: it is
    /// a unit of hardware installed on ONE Outpost, held in that Outpost's own
    /// installation order ([`OutpostRecord::assets`]), so the boundary such a token
    /// names is a position in THAT list and the route the token is replayed on names
    /// the Outpost it belongs to.
    pub const ASSET_PAGE: &str = "assets";

The route names the Outpost of the REQUEST, not of the token, and nothing compares the two.

The assets a token's positions point into are replaced between pages — `src/state.rs:3793-3801` (`settle_outposts`):

        record.outpost.life_cycle_status = Some(LIFE_CYCLE_STATUS_ACTIVE.to_string());
        record.active_at = None;
        if !hardware.is_empty() {
            record.assets = hardware;
        }

and `src/state.rs:555-566` (`set_supported_hardware_type`, called from `src/ops/update_outpost.rs:219-221`):

        self.assets.clear();
        self.active_at = Some(std::time::Instant::now() + OUTPOST_PROVISIONING);

Test coverage today: `tests/all/outpost_assets.rs:666-669` states the rule ("a `NextToken` this service did not issue for this list are 400 `ValidationException`") and `:733-746` exercises only a cross-LIST replay ("a ListOutposts token replayed on ListAssets", 400) and a never-issued token; `tests/all/asset_instances.rs:634-657` does the same for the sibling list. No test replays one Outpost's token on another Outpost's identical route, and no test pages a list across an install, so neither symptom is pinned by a case that states a reason for it.

## Steps to Reproduce

1. CreateSite, then CreateOutpost twice in the same Region with SupportedHardwareType=RACK — call them A and B.
2. For each of A and B, CreateOrder with one LineItem and wait for the order to be fulfilled, so `settle_outposts` installs each Outpost's hardware (several assets per Outpost).
3. GET /outposts/{A}/assets?MaxResults=1 — note the returned NextToken, which is `0##assets`.
4. GET /outposts/{B}/assets?NextToken=0%23%23assets — answered 200 with B's assets from position 1 on: B's first asset is silently missing, although the same route answers 400 ValidationException for `NextToken=0%23%23outposts`.
5. Second symptom: GET /outposts/{A}/assets?MaxResults=1 to get a token, then UpdateOutpost(A, SupportedHardwareType=SERVER) and wait for the new install to settle, then replay the token — the page resumes at an index of the newly installed SERVER hardware list and omits the unit now at position 0.
