// SPDX-License-Identifier: CC0-1.0

//! `ListReplicationSets`, end to end over the `restJson1` wire.
//!
//! The rule under test: the operation reports, in one stable order and paged by the
//! service's `maxResults` / `nextToken` contract, the ARN of EVERY replication set the
//! account's store holds — an empty list (a success, not an error) when it holds none —
//! and, being `@readonly`, leaves the store exactly as it found it.
//!
//! Each suite is a table spanning what that rule generalises over: how many sets the
//! store holds, their creation order, their status, their Regions, and the page size the
//! request asks for. The recorded case the issue quoted ("an account with no replication
//! set") is ONE row.
//!
//! The store is seeded directly through the shared engine because the only operation
//! that creates a replication set, `CreateReplicationSet`, refuses every call as
//! deprecated (Amazon Web Services stopped accepting new Incident Manager customers on
//! November 7, 2025), so no request will ever add one. Seeding the very `crate::state`
//! map the handler reads is what makes these assertions about persisted state rather
//! than about a hardcoded answer.

use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use service_com_amazonaws_ssmincidents_ssmincidents::ops::types::{
    RegionInfo, REGION_STATUS_VALUES, REPLICATION_SET_STATUS_VALUES,
};
use service_com_amazonaws_ssmincidents_ssmincidents::state::{
    replication_set_arn, service_state, ReplicationSetEntry, MAX_REPLICATION_SET_ARNS_PER_PAGE,
};

use super::incident_lifecycle::{Emulator, Reply};

/// The canonical placeholder account id every ARN the emulator mints carries.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The principal ARN the harness's access key id maps to; the member it fills
/// (`ReplicationSet$createdBy`) targets `Arn`.
fn principal() -> String {
    format!("arn:aws:iam::{ACCOUNT}:user/AKIAIOSFODNN7EXAMPLE")
}

/// One replication set to persist before listing.
#[derive(Debug, Clone)]
struct SeedCase {
    /// The id segment of the set's ARN.
    id: &'static str,
    /// `createdTime`, as whole seconds since the epoch: the primary list-order key.
    created_secs: i64,
    /// The Regions the set replicates to (`ReplicationSet$regionMap`'s keys).
    regions: &'static [&'static str],
    /// A `ReplicationSetStatus` value — source: `REPLICATION_SET_STATUS_VALUES` in
    /// `ops/types/shared_types.rs`, i.e. the model's enum.
    status: &'static str,
    deletion_protected: bool,
}

/// Rows spanning the replication sets a store can hold: a one-Region set and a
/// many-Region one, deletion protection on and off, several `ReplicationSetStatus`
/// values (including `CREATING` and `FAILED`, which the issue never cited — a set's ARN
/// is listed whatever its status, which is what `GetReplicationSet` reports), ids
/// ordering differently by ARN than by creation time, and two sets created at the SAME
/// instant so the ARN tiebreak is exercised.
fn seed_cases() -> Vec<SeedCase> {
    vec![
        SeedCase {
            id: "11111111-1111-1111-1111-111111111111",
            created_secs: 1_700_000_000,
            regions: &["us-east-1"],
            status: REPLICATION_SET_STATUS_VALUES[0], // ACTIVE
            deletion_protected: true,
        },
        SeedCase {
            id: "aaaaaaaa-0000-0000-0000-000000000000",
            created_secs: 1_700_000_001,
            regions: &["us-east-1", "eu-west-1", "ap-southeast-2"],
            status: REPLICATION_SET_STATUS_VALUES[1], // CREATING
            deletion_protected: false,
        },
        SeedCase {
            id: "00000000-ffff-ffff-ffff-ffffffffffff",
            created_secs: 1_700_000_002,
            regions: &["eu-central-1", "us-west-2"],
            status: REPLICATION_SET_STATUS_VALUES[3], // FAILED
            deletion_protected: false,
        },
        SeedCase {
            id: "bbbbbbbb-1111-1111-1111-111111111111",
            created_secs: 1_700_000_003,
            regions: &["us-west-1"],
            status: REPLICATION_SET_STATUS_VALUES[4], // UPDATING
            deletion_protected: true,
        },
        // Same instant as the row above: the order must still be total, broken by ARN.
        SeedCase {
            id: "cccccccc-2222-2222-2222-222222222222",
            created_secs: 1_700_000_003,
            regions: &["ca-central-1", "sa-east-1"],
            status: REPLICATION_SET_STATUS_VALUES[0], // ACTIVE
            deletion_protected: false,
        },
    ]
}

/// Persist `cases` in the very store the handler reads, through the shared engine the
/// router handed the service.
fn seed(emulator: &Emulator, cases: &[SeedCase]) {
    let state = service_state(&*emulator.engine).expect("the ssm-incidents state store");
    let mut store = state.lock();
    for case in cases {
        let created = DateTime::from_secs(case.created_secs);
        let region_map: std::collections::HashMap<String, RegionInfo> = case
            .regions
            .iter()
            .enumerate()
            .map(|(index, region)| {
                (
                    (*region).to_string(),
                    RegionInfo {
                        // Only the first Region carries a customer key, so both the
                        // present and the absent `sseKmsKeyId` shape is stored.
                        sse_kms_key_id: (index == 0).then(|| {
                            format!("arn:aws:kms:{region}:{ACCOUNT}:key/{}", case.id)
                        }),
                        // Source: `REGION_STATUS_VALUES` in `ops/types/shared_types.rs`.
                        status: REGION_STATUS_VALUES[index % REGION_STATUS_VALUES.len()]
                            .to_string(),
                        status_message: None,
                        status_update_date_time: created,
                    },
                )
            })
            .collect();
        store.replication_sets.insert(
            replication_set_arn(case.id),
            ReplicationSetEntry {
                arn: replication_set_arn(case.id),
                region_map,
                status: case.status.to_string(),
                deletion_protected: case.deletion_protected,
                created_time: created,
                created_by: principal(),
                last_modified_time: created,
                last_modified_by: principal(),
            },
        );
    }
}

/// The ARNs `cases` must be listed in: oldest `createdTime` first, ties broken by ARN.
fn expected_order(cases: &[SeedCase]) -> Vec<String> {
    let mut rows: Vec<(i64, String)> = cases
        .iter()
        .map(|case| (case.created_secs, replication_set_arn(case.id)))
        .collect();
    rows.sort();
    rows.into_iter().map(|(_, arn)| arn).collect()
}

/// The `replicationSetArns` a reply carries, or `None` when the member is absent.
fn listed(reply: &Reply) -> Option<Vec<String>> {
    let items = reply.body.get("replicationSetArns")?.as_array()?.clone();
    Some(
        items
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .unwrap_or_else(|| panic!("replicationSetArns holds a non-string: {value:?}"))
                    .to_string()
            })
            .collect(),
    )
}

/// The `nextToken` a reply carries, if any.
fn next_token(reply: &Reply) -> Option<String> {
    reply
        .body
        .get("nextToken")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// A deterministic fingerprint of every stored replication set, so a read can be shown
/// to leave the store untouched.
fn store_fingerprint(emulator: &Emulator) -> Vec<String> {
    let state = service_state(&*emulator.engine).expect("the ssm-incidents state store");
    let store = state.lock();
    let mut rows: Vec<String> = store
        .replication_sets
        .values()
        .map(|entry| {
            let mut regions: Vec<String> = entry
                .region_map
                .iter()
                .map(|(name, info)| {
                    format!(
                        "{name}={}/{:?}/{}",
                        info.status,
                        info.sse_kms_key_id,
                        info.status_update_date_time.secs()
                    )
                })
                .collect();
            regions.sort();
            format!(
                "{}|{}|{}|{}|{}|{}|{}|{}",
                entry.arn,
                entry.status,
                entry.deletion_protected,
                entry.created_time.secs(),
                entry.created_by,
                entry.last_modified_time.secs(),
                entry.last_modified_by,
                regions.join(","),
            )
        })
        .collect();
    rows.sort();
    rows
}

/// Every stored replication set's ARN is reported, for every store size: none, one and
/// several. The "none" row is the recorded case — an empty list is a SUCCESS.
#[tokio::test]
async fn listing_reports_the_arn_of_every_stored_replication_set() {
    let all = seed_cases();
    // Rows: an empty store, one set, and every larger prefix of the seed table.
    for count in [0usize, 1, 2, 3, 4, 5] {
        let emulator = Emulator::new();
        let cases = &all[..count];
        seed(&emulator, cases);

        let reply = emulator.post("/listReplicationSets", json!({})).await;
        assert_eq!(
            reply.status, 200,
            "case {count} set(s): expected 200, got {} body {:?}",
            reply.status, reply.body
        );
        let arns = listed(&reply).unwrap_or_else(|| {
            panic!(
                "case {count} set(s): replicationSetArns is required, got {:?}",
                reply.body
            )
        });
        assert_eq!(
            arns,
            expected_order(cases),
            "case {count} set(s): unexpected ARNs, got {:?}",
            reply.body
        );
        assert_eq!(
            next_token(&reply),
            None,
            "case {count} set(s): a single page holding everything carries no nextToken, got {:?}",
            reply.body
        );
    }
}

/// An account that was never onboarded to Incident Manager lists an EMPTY set and
/// succeeds: HTTP 200 with `replicationSetArns: []` and no token — never an error, and
/// never the scaffold's 501.
#[tokio::test]
async fn an_account_with_no_replication_set_lists_an_empty_set() {
    let emulator = Emulator::new();

    // Rows: the bare request, each published bound of `maxResults`, and a token-free
    // repeat — an empty store answers the same way for all of them.
    for request in [
        json!({}),
        json!({ "maxResults": 1 }),
        json!({ "maxResults": MAX_REPLICATION_SET_ARNS_PER_PAGE }),
    ] {
        let reply = emulator
            .post("/listReplicationSets", request.clone())
            .await;
        assert_eq!(
            reply.status, 200,
            "case {request:?}: expected 200, got {} body {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "",
            "case {request:?}: expected a success body, got {:?}",
            reply.body
        );
        assert_eq!(
            listed(&reply),
            Some(Vec::new()),
            "case {request:?}: expected an empty replicationSetArns, got {:?}",
            reply.body
        );
        assert_eq!(
            next_token(&reply),
            None,
            "case {request:?}: an empty list carries no nextToken, got {:?}",
            reply.body
        );
    }
}

/// Paging: each page holds at most `maxResults` ARNs, the pages partition the stored
/// sets exactly once in list order, and the last page carries no token.
#[tokio::test]
async fn paging_visits_every_stored_set_exactly_once_and_terminates() {
    let cases = seed_cases();
    let expected = expected_order(&cases);

    // Rows: page sizes below, equal to and above the stored count, each published bound
    // of `maxResults` (the model's range is 1..100) and "no maxResults at all".
    for page_size in [Some(1), Some(2), Some(3), Some(5), Some(7), Some(100), None] {
        let emulator = Emulator::new();
        seed(&emulator, &cases);

        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({});
            if let Some(size) = page_size {
                request["maxResults"] = json!(size);
            }
            if let Some(ref value) = token {
                request["nextToken"] = json!(value);
            }
            let reply = emulator
                .post("/listReplicationSets", request.clone())
                .await;
            assert_eq!(
                reply.status, 200,
                "case {page_size:?}: expected 200, got {} body {:?}",
                reply.status, reply.body
            );
            let page = listed(&reply).unwrap_or_else(|| {
                panic!(
                    "case {page_size:?}: replicationSetArns is required, got {:?}",
                    reply.body
                )
            });
            if let Some(size) = page_size {
                assert!(
                    page.len() <= size as usize,
                    "case {page_size:?}: a page exceeded maxResults, got {} items",
                    page.len()
                );
            }
            seen.extend(page);
            pages += 1;
            assert!(
                pages <= 20,
                "case {page_size:?}: paging did not terminate after {pages} pages"
            );
            token = next_token(&reply);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, expected,
            "case {page_size:?}: paging must visit every stored set exactly once, in list order"
        );
    }
}

/// A `nextToken` resumes strictly after the set the previous page ended on, named by
/// that set's own position — so a set removed from an already-returned page does not
/// make the next page skip or repeat another.
#[tokio::test]
async fn paging_survives_a_set_removed_between_two_pages() {
    let cases = seed_cases();
    let expected = expected_order(&cases);

    // Rows: drop the first and then the last ARN of the already-returned page.
    for victim_index in [0usize, 1] {
        let emulator = Emulator::new();
        seed(&emulator, &cases);

        let first = emulator
            .post("/listReplicationSets", json!({ "maxResults": 2 }))
            .await;
        let page_one = listed(&first).unwrap_or_default();
        assert_eq!(
            page_one.len(),
            2,
            "case {victim_index}: expected a 2-item first page, got {:?}",
            first.body
        );
        let token = next_token(&first)
            .unwrap_or_else(|| panic!("case {victim_index}: expected a nextToken"));

        let victim = page_one[victim_index].clone();
        {
            let state = service_state(&*emulator.engine).expect("the ssm-incidents state store");
            let mut store = state.lock();
            store.replication_sets.remove(&victim);
        }

        let mut rest: Vec<String> = Vec::new();
        let mut token = Some(token);
        while let Some(current) = token {
            let reply = emulator
                .post(
                    "/listReplicationSets",
                    json!({ "maxResults": 2, "nextToken": current }),
                )
                .await;
            assert_eq!(
                reply.status, 200,
                "case {victim_index}: expected 200 resuming, got {} body {:?}",
                reply.status, reply.body
            );
            rest.extend(listed(&reply).unwrap_or_default());
            token = next_token(&reply);
        }

        for shown in &page_one {
            assert!(
                !rest.contains(shown),
                "case {victim_index}: {shown} was returned twice across pages"
            );
        }
        let tail: Vec<String> = expected[2..].to_vec();
        assert_eq!(
            rest, tail,
            "case {victim_index}: resuming must return exactly the sets after the first page"
        );
    }
}

/// A `nextToken` this operation did not issue is refused with the modelled
/// `ValidationException` (400) — never silently treated as "start from the beginning",
/// and never a 500.
#[tokio::test]
async fn a_next_token_this_operation_did_not_issue_is_refused() {
    let emulator = Emulator::new();
    seed(&emulator, &seed_cases());

    for (case, token) in [
        ("not a token", "not-a-token"),
        ("zero", "0"),
        ("a large offset", "999999999"),
        ("hex that is not a token", "6162"),
        ("an empty token", ""),
        // A token another list operation of this service issues is not one of ours.
        ("another list's token", &hex::encode("ssmi1|1700000000|0|arn")),
    ] {
        let reply = emulator
            .post("/listReplicationSets", json!({ "nextToken": token }))
            .await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected 400 for nextToken {token:?}, got {} body {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            reply.body
        );
    }
}

/// `maxResults` outside the modelled `range 1..100` is refused with 400; each published
/// bound is accepted.
#[tokio::test]
async fn max_results_outside_the_modelled_range_is_refused() {
    let emulator = Emulator::new();
    seed(&emulator, &seed_cases());

    for max_results in [0, MAX_REPLICATION_SET_ARNS_PER_PAGE + 1, -1] {
        let reply = emulator
            .post("/listReplicationSets", json!({ "maxResults": max_results }))
            .await;
        assert_eq!(
            reply.status, 400,
            "case \"maxResults {max_results}\": expected 400, got {} body {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "ValidationException",
            "case \"maxResults {max_results}\": expected ValidationException, got {:?}",
            reply.body
        );
    }

    for max_results in [1, MAX_REPLICATION_SET_ARNS_PER_PAGE] {
        let reply = emulator
            .post("/listReplicationSets", json!({ "maxResults": max_results }))
            .await;
        assert_eq!(
            reply.status, 200,
            "case \"maxResults {max_results}\": a value inside the range must be accepted, \
             got {} body {:?}",
            reply.status, reply.body
        );
    }
}

/// The operation is `@readonly`: listing — once, repeatedly, or all the way through a
/// paging walk — changes nothing a later caller sees.
#[tokio::test]
async fn listing_changes_nothing_a_later_caller_sees() {
    let cases = seed_cases();
    let emulator = Emulator::new();
    seed(&emulator, &cases);
    let before = store_fingerprint(&emulator);

    let first = emulator.post("/listReplicationSets", json!({})).await;
    let first_arns = listed(&first).expect("replicationSetArns is required");

    // A full paging walk, then the same unpaged call again.
    let mut token: Option<String> = None;
    loop {
        let mut request = json!({ "maxResults": 1 });
        if let Some(ref value) = token {
            request["nextToken"] = json!(value);
        }
        let reply = emulator.post("/listReplicationSets", request).await;
        assert_eq!(
            reply.status, 200,
            "paging walk: expected 200, got {} body {:?}",
            reply.status, reply.body
        );
        token = next_token(&reply);
        if token.is_none() {
            break;
        }
    }

    let again = emulator.post("/listReplicationSets", json!({})).await;
    assert_eq!(
        listed(&again).expect("replicationSetArns is required"),
        first_arns,
        "a repeated list must answer the same ARNs, got {:?}",
        again.body
    );
    assert_eq!(
        store_fingerprint(&emulator),
        before,
        "listing must not change the stored replication sets"
    );
    assert_eq!(
        first_arns,
        expected_order(&cases),
        "the listed ARNs must be exactly the stored ones, got {:?}",
        first.body
    );
}
