// SPDX-License-Identifier: CC0-1.0

//! `ListFieldLevelEncryptionConfigs` over a store a delete has changed, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes.
//!
//! The rule under test is the one this listing states: "list all field-level encryption
//! configurations that have been created in CloudFront for this account" — a configuration is
//! reported because it is STORED at the instant of the call, so the listing follows the store
//! rather than the history of creates. Hence, once a delete has removed a configuration's
//! record:
//!
//! * the listing reports exactly the configurations that remain, in the one order every page
//!   indexes into, with `Quantity` stating each page's own length and no `Items` element at all
//!   for an account left holding none;
//! * walking the pages at ANY admissible page size enumerates those survivors exactly once,
//!   `NextMarker` being present exactly when configurations remain;
//! * a `Marker` naming a configuration that has since been deleted is a resume POSITION, so
//!   the page it opens holds the survivors that sort after it and never the deleted record.
//!
//! The rows differ along what that rule generalises over: how many configurations the account
//! stores, and which of them the delete removes — none, the first in order, one in the middle,
//! the last, several at once, and every one of them.
//!
//! The wire harness is [`crate::distribution_lifecycle`]'s, the create and the reader are
//! [`crate::field_level_encryption_config_lifecycle`]'s, the delete is the delete suite's, the
//! listing call and its page scanner are the listing suite's, and the profile every mapping
//! points at is stored through the profile suite's own create path — reused rather than copied.

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::MAX_ITEMS;

use crate::distribution_lifecycle::{emulator, is_version_token};
use crate::field_level_encryption_config_delete::delete_config;
use crate::field_level_encryption_config_lifecycle::{
    create_config, get_config, is_config_id, simple,
};
use crate::field_level_encryption_config_list::{list_configs, page_ids};
use crate::field_level_encryption_profile_lifecycle::store_profile;

/// One case: how many configurations the account stores, and which of them — by position in
/// the one id order the listing reports — the delete removes.
struct Row {
    case: &'static str,
    stored: usize,
    deleted: &'static [usize],
}

/// The cases: the account's size, and the positions the delete empties in it.
const ROWS: &[Row] = &[
    Row {
        case: "nothing deleted",
        stored: 3,
        deleted: &[],
    },
    Row {
        case: "the first configuration in order deleted",
        stored: 4,
        deleted: &[0],
    },
    Row {
        case: "a configuration in the middle deleted",
        stored: 5,
        deleted: &[2],
    },
    Row {
        case: "the last configuration in order deleted",
        stored: 4,
        deleted: &[3],
    },
    Row {
        case: "several configurations deleted",
        stored: 6,
        deleted: &[0, 2, 5],
    },
    Row {
        case: "every configuration deleted",
        stored: 2,
        deleted: &[0, 1],
    },
];

/// Create `count` configurations, each under its own caller reference, and report their minted
/// ids beside the version token each delete below is gated on, in id order — the order the
/// listing reports them in.
async fn store_configs(
    router: &MultiServiceRouter,
    profile: &str,
    count: usize,
    case: &str,
) -> Vec<(String, String)> {
    let mut stored: Vec<(String, String)> = Vec::new();
    for index in 0..count {
        // "A unique number that ensures the request can't be replayed": one reference per
        // configuration, so no create is read as a repeat of another.
        let reference = format!("fle-config-list-after-delete-{index}");
        let config = simple(case, &reference, profile);
        let answer = create_config(router, &config.xml()).await;
        assert_eq!(
            answer.status, 201,
            "case {case:?}: expected HTTP 201 from CreateFieldLevelEncryptionConfig for \
             {reference}, got {} with {}",
            answer.status, answer.raw
        );
        let id = answer.root().text_at("Id").unwrap_or_default().to_string();
        assert!(
            is_config_id(&id),
            "case {case:?}: expected a minted configuration id for {reference}, got {id:?} in {}",
            answer.raw
        );
        let etag = answer.header("etag").to_string();
        assert!(
            is_version_token(&etag),
            "case {case:?}: expected a minted version token for {reference}, got {etag:?}"
        );
        stored.push((id, etag));
    }
    stored.sort_by(|left, right| left.0.cmp(&right.0));
    stored
}

/// Walk the pages at `size` and report every id served, in page order, asserting what each
/// page states about itself: the requested size echoed, `Quantity` stating the page's own
/// length, and `NextMarker` present exactly when the walk has not reached `total` yet and
/// naming the id the page ended at.
async fn walk_pages(
    router: &MultiServiceRouter,
    size: usize,
    total: usize,
    case: &str,
) -> Vec<String> {
    let mut walked: Vec<String> = Vec::new();
    let mut marker: Option<String> = None;
    loop {
        let query = match &marker {
            Some(value) => format!("?MaxItems={size}&Marker={value}"),
            None => format!("?MaxItems={size}"),
        };
        let page = list_configs(router, &query).await;
        assert_eq!(
            page.status, 200,
            "case {case:?}, page size {size}: expected HTTP 200 for {query}, got {} with {}",
            page.status, page.raw
        );
        assert_eq!(
            page.root().text_at("MaxItems"),
            Some(size.to_string().as_str()),
            "case {case:?}, page size {size}: expected the requested MaxItems echoed, got {}",
            page.raw
        );
        let ids = page_ids(&page);
        // The page served is the requested size bounded above by the server's own page
        // ceiling, and never longer than what remains.
        let ceiling = usize::try_from(MAX_ITEMS).expect("the ceiling fits a usize");
        assert!(
            ids.len() <= size.min(ceiling),
            "case {case:?}, page size {size}: a page served {} items, got {}",
            ids.len(),
            page.raw
        );
        assert_eq!(
            page.root().text_at("Quantity"),
            Some(ids.len().to_string().as_str()),
            "case {case:?}, page size {size}: expected Quantity to state the page's length, got {}",
            page.raw
        );
        walked.extend(ids.iter().cloned());
        let remaining = walked.len() < total;
        let next = page.root().text_at("NextMarker").map(str::to_string);
        assert_eq!(
            next.is_some(),
            remaining,
            "case {case:?}, page size {size}: expected NextMarker present {remaining} after {} of \
             {total} items, got {}",
            walked.len(),
            page.raw
        );
        match next {
            Some(value) => {
                assert_eq!(
                    Some(value.as_str()),
                    ids.last().map(String::as_str),
                    "case {case:?}, page size {size}: expected NextMarker to name the id the page \
                     ended at, got {}",
                    page.raw
                );
                marker = Some(value);
            }
            None => break,
        }
    }
    walked
}

/// A configuration is listed because it is STORED: once a delete has removed one, the listing
/// reports exactly the configurations that remain — at every admissible page size, and from a
/// resume point a deleted configuration once held.
#[tokio::test]
async fn the_listing_reports_what_the_store_holds_after_a_delete() {
    for row in ROWS {
        // One account per case: a fresh store, so each case's listing answers for exactly the
        // configurations that case created.
        let router = emulator();
        let profile = store_profile(&router, "fle-config-list-after-delete-profile").await;
        let stored = store_configs(&router, &profile, row.stored, row.case).await;

        let mut deleted_ids: Vec<String> = Vec::new();
        for position in row.deleted {
            let (id, etag) = &stored[*position];
            let answer = delete_config(&router, id, Some(etag)).await;
            assert_eq!(
                answer.status, 204,
                "case {:?}: expected HTTP 204 from DeleteFieldLevelEncryptionConfig for {id}, got \
                 {} with {}",
                row.case, answer.status, answer.raw
            );
            // Round trip: the record the listing answered from is gone, so what the listing
            // stops reporting below is the delete's doing.
            let read = get_config(&router, id).await;
            assert_eq!(
                read.status, 404,
                "case {:?}: expected HTTP 404 from GetFieldLevelEncryption after the delete of \
                 {id}, got {} with {}",
                row.case, read.status, read.raw
            );
            deleted_ids.push(id.clone());
        }

        let survivors: Vec<String> = stored
            .iter()
            .map(|(id, _)| id.clone())
            .filter(|id| !deleted_ids.contains(id))
            .collect();

        // The whole (sub-ceiling) collection the account is left holding.
        let listed = list_configs(&router, "").await;
        assert_eq!(
            listed.status, 200,
            "case {:?}: expected HTTP 200 from ListFieldLevelEncryptionConfigs, got {} with {}",
            row.case, listed.status, listed.raw
        );
        assert_eq!(
            page_ids(&listed),
            survivors,
            "case {:?}: expected exactly the configurations the store still holds, in order, got \
             {}",
            row.case,
            listed.raw
        );
        assert_eq!(
            listed.root().text_at("Quantity"),
            Some(survivors.len().to_string().as_str()),
            "case {:?}: expected Quantity {}, got {}",
            row.case,
            survivors.len(),
            listed.raw
        );
        assert_eq!(
            listed.root().at("Items").is_none(),
            survivors.is_empty(),
            "case {:?}: an account left holding no configuration carries no Items element at all, \
             got {}",
            row.case,
            listed.raw
        );
        assert!(
            listed.root().text_at("NextMarker").is_none(),
            "case {:?}: nothing remains beyond the whole collection, got {}",
            row.case,
            listed.raw
        );

        // Walking the pages at any admissible page size enumerates the survivors exactly once,
        // in that same order: sizes below, at and past what the account is left holding, and one
        // past the server's own page ceiling.
        let ceiling = usize::try_from(MAX_ITEMS).expect("the ceiling fits a usize");
        for size in (1..=survivors.len() + 1).chain([ceiling + 1]) {
            let walked = walk_pages(&router, size, survivors.len(), row.case).await;
            assert_eq!(
                walked, survivors,
                "case {:?}, page size {size}: walking the pages must enumerate every stored \
                 configuration exactly once, in order",
                row.case
            );
        }

        // A marker naming a configuration that has since been deleted is a resume POSITION: the
        // page it opens holds the survivors sorting after it, and never the deleted record.
        for deleted in &deleted_ids {
            let page = list_configs(&router, &format!("?Marker={deleted}")).await;
            assert_eq!(
                page.status, 200,
                "case {:?}: expected HTTP 200 for a marker naming the deleted {deleted}, got {} \
                 with {}",
                row.case, page.status, page.raw
            );
            let expected: Vec<String> = survivors
                .iter()
                .filter(|id| id.as_str() > deleted.as_str())
                .cloned()
                .collect();
            assert_eq!(
                page_ids(&page),
                expected,
                "case {:?}: expected the survivors after {deleted}, got {}",
                row.case,
                page.raw
            );
        }
    }
}
