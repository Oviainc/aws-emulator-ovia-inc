// SPDX-License-Identifier: CC0-1.0

//! `ListDatastores` continuation tokens are positions in ONE region's collection.
//!
//! A data store is a regional resource, so the data stores of two regions are two
//! collections. The token this listing mints therefore names the region it is a position
//! in: replayed against any other region it is a token this service did not issue for
//! that collection, and is refused with `ValidationException` rather than honoured — a
//! resume in the wrong collection would otherwise answer a page that skips or repeats
//! records.
//!
//! Every case drives real HTTP through the production front door
//! (`MultiServiceRouter::dispatch`), signing each request for a chosen region through the
//! harness's `Emulator::call_in` — the SigV4 credential scope is where the service reads
//! the region from. The suites are TABLES: the axes are the region a token was minted in,
//! the page boundary it was minted at, the region it is replayed against, and the page
//! size a walk uses.

use serde_json::Value;

use service_com_amazonaws_medicalimaging_ahigatewayservice::state::{
    DATASTORE_ACTIVATION_DELAY, INVALID_NEXT_TOKEN_MESSAGE,
};

use crate::datastore_lifecycle::{Answer, Emulator, SETTLE_MULTIPLE};

/// The regions these cases address, with how many data stores each one holds. Real AWS
/// region codes, every one inside the `[a-z0-9-]+` alphabet the service's ARN pattern
/// admits for a region (`smithy_query(query="shape",shape="DatastoreArn")`). The issue
/// cited the `us-east-1`/`eu-west-1` pair; the rest are rows it did not name, including
/// one region nothing is created in.
///
/// The stores are created region by region IN THIS ORDER, so an earlier region's stores
/// are older than a later one's: a token replayed across the pair therefore names a
/// position before the whole of one collection in one direction and past the whole of it
/// in the other — the two ways a resume in a foreign collection goes wrong.
const REGIONS: [(&str, usize); 4] = [
    ("us-east-1", 3),
    ("eu-west-1", 2),
    ("ap-northeast-1", 3),
    ("ca-central-1", 0),
];

/// `GET /datastore` in `region` with the query string `query`
/// (`smithy_query(query="operation",operation="ListDatastores")`).
async fn list_in(emulator: &Emulator, region: &str, query: &str) -> Answer {
    let uri = if query.is_empty() {
        "/datastore".to_string()
    } else {
        format!("/datastore?{query}")
    };
    emulator.call_in(region, "GET", &uri, None).await
}

/// The `datastoreId` of every summary an answer carries, in the order it reported them.
fn listed_ids(answer: &Answer) -> Vec<String> {
    answer
        .field(&["datastoreSummaries"])
        .as_array()
        .map(|summaries| {
            summaries
                .iter()
                .map(|summary| {
                    summary["datastoreId"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Create every region's data stores and wait out the activation the clock owes them.
async fn seeded_emulator() -> Emulator {
    let emulator = Emulator::new();
    for (region, stores) in REGIONS {
        for index in 0..stores {
            let created = emulator
                .call_in(
                    region,
                    "POST",
                    "/datastore",
                    Some(serde_json::json!({
                        "datastoreName": format!("tokens.{region}.{index}"),
                        "clientToken": format!("tokens-{region}-{index}"),
                    })),
                )
                .await;
            assert_eq!(
                created.status, 200,
                "seeding store {index} in {region:?}: expected HTTP 200, got {created:?}"
            );
        }
    }
    tokio::time::sleep(SETTLE_MULTIPLE * DATASTORE_ACTIVATION_DELAY).await;
    emulator
}

/// Every data store `region` reports in one page — the order this listing walks, which
/// each paged walk below is compared against.
async fn whole_collection_in(emulator: &Emulator, region: &str) -> Vec<String> {
    let answer = list_in(emulator, region, "maxResults=50").await;
    assert_eq!(
        answer.status, 200,
        "case {region:?}: expected HTTP 200 from an unpaged ListDatastores, got {answer:?}"
    );
    assert!(
        answer.field(&["nextToken"]).is_null(),
        "case {region:?}: expected no nextToken on a page that exhausts the collection, got {answer:?}"
    );
    listed_ids(&answer)
}

/// The token `region`'s listing mints after serving its first `boundary` records.
async fn token_after(emulator: &Emulator, region: &str, boundary: usize) -> String {
    let answer = list_in(emulator, region, &format!("maxResults={boundary}")).await;
    assert_eq!(
        answer.status, 200,
        "case {region:?} boundary {boundary}: expected HTTP 200, got {answer:?}"
    );
    answer
        .field(&["nextToken"])
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {region:?} boundary {boundary}: expected a nextToken, got {answer:?}")
        })
        .to_string()
}

/// One row of the cross-region replay table.
#[derive(Debug)]
struct ReplayCase {
    minted_in: &'static str,
    boundary: usize,
    replayed_in: &'static str,
    replay_holds: usize,
}

/// RULE: a listing's continuation token is a position in the one collection it was minted
/// against — for data stores, the collection of the region the minting request was
/// addressed to — so a token presented to any other region is one this service did not
/// issue for that collection and is refused with `ValidationException` (HTTP 400) and the
/// service's fixed invalid-token sentence, never honoured as a resume point.
///
/// Rows span the axes the rule generalises over: which region minted the token, which
/// page boundary it was minted at, and which region it is replayed against — including a
/// region whose stores are all OLDER than the token's record (where honouring it would
/// answer an empty page), one whose stores are all NEWER (where honouring it would
/// re-serve records), and one holding no stores at all. The control for every token is the
/// same token replayed in its OWN region, where it must still resume the walk exactly.
#[tokio::test]
async fn a_list_datastores_token_is_a_position_only_in_the_region_that_minted_it() {
    let emulator = seeded_emulator().await;

    let mut rows: Vec<ReplayCase> = Vec::new();
    for (minted_in, stores) in REGIONS {
        // A token exists at every boundary that leaves a further page.
        for boundary in 1..stores {
            for (replayed_in, replay_holds) in REGIONS {
                if replayed_in != minted_in {
                    rows.push(ReplayCase {
                        minted_in,
                        boundary,
                        replayed_in,
                        replay_holds,
                    });
                }
            }
        }
    }
    assert!(
        !rows.is_empty(),
        "expected the seeded regions to mint at least one token to replay"
    );

    for row in &rows {
        let token = token_after(&emulator, row.minted_in, row.boundary).await;

        let refused = list_in(
            &emulator,
            row.replayed_in,
            &format!("maxResults=1&nextToken={token}"),
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {row:?}: expected HTTP 400 for a token minted in another region, got {refused:?}"
        );
        assert_eq!(
            refused.error_code, "ValidationException",
            "case {row:?}: expected ValidationException, got {refused:?}"
        );
        assert_eq!(
            refused.string(&["message"]),
            INVALID_NEXT_TOKEN_MESSAGE,
            "case {row:?}: expected the service's invalid-token sentence, got {refused:?}"
        );
        assert!(
            refused.field(&["datastoreSummaries"]).is_null(),
            "case {row:?}: expected no page beside the refusal, got {refused:?}"
        );

        // The replayed-against region still reports its own stores, untouched by the
        // refused call: the token named a position nowhere in this collection, which is
        // not the same as this collection being empty.
        let held = whole_collection_in(&emulator, row.replayed_in).await;
        assert_eq!(
            held.len(),
            row.replay_holds,
            "case {row:?}: expected {} store(s) still reported in {:?}, got {held:?}",
            row.replay_holds,
            row.replayed_in
        );

        // The control: the same token in its own region resumes the walk at the record
        // after the boundary.
        let baseline = whole_collection_in(&emulator, row.minted_in).await;
        let resumed = list_in(
            &emulator,
            row.minted_in,
            &format!("maxResults=1&nextToken={token}"),
        )
        .await;
        assert_eq!(
            resumed.status, 200,
            "case {row:?}: expected the token to be honoured in its own region, got {resumed:?}"
        );
        assert_eq!(
            listed_ids(&resumed),
            vec![baseline[row.boundary].clone()],
            "case {row:?}: expected the resume to serve the record after the boundary, got {resumed:?}"
        );
    }
}

/// One row of the paged-walk table.
#[derive(Debug)]
struct WalkCase {
    region: &'static str,
    page_size: usize,
}

/// RULE: paging a listing to the end with the tokens it mints visits every record of that
/// collection exactly once, in the listing's own order, whatever page size the caller
/// asks for — a resume neither skips a record nor repeats one.
///
/// Rows span the page-size axis (one record at a time, a size that divides the collection
/// evenly, one that does not, and one larger than the whole collection) across every
/// seeded region, including the empty one.
#[tokio::test]
async fn paging_a_region_to_the_end_visits_its_data_stores_exactly_once() {
    let emulator = seeded_emulator().await;

    let rows: Vec<WalkCase> = REGIONS
        .into_iter()
        .flat_map(|(region, _)| {
            [1usize, 2, 3, 50]
                .into_iter()
                .map(move |page_size| WalkCase { region, page_size })
        })
        .collect();

    for row in &rows {
        let expected = whole_collection_in(&emulator, row.region).await;

        let mut walked: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut query = format!("maxResults={}", row.page_size);
            if let Some(token) = &next_token {
                query.push_str(&format!("&nextToken={token}"));
            }
            let answer = list_in(&emulator, row.region, &query).await;
            assert_eq!(
                answer.status, 200,
                "case {row:?} page {pages}: expected HTTP 200, got {answer:?}"
            );
            let page = listed_ids(&answer);
            assert!(
                page.len() <= row.page_size,
                "case {row:?} page {pages}: expected at most {} record(s), got {page:?}",
                row.page_size
            );
            walked.extend(page);
            pages += 1;
            match answer.field(&["nextToken"]) {
                Value::String(token) => next_token = Some(token.clone()),
                _ => break,
            }
            assert!(
                pages <= expected.len(),
                "case {row:?}: paged further than the collection can hold, got {walked:?}"
            );
        }

        assert_eq!(
            walked, expected,
            "case {row:?}: expected the walk to visit this region's stores exactly once in order, got {walked:?}"
        );
    }
}
