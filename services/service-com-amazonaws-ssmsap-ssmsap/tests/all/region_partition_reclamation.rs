// SPDX-License-Identifier: CC0-1.0

//! Region-partition reclamation, driven over the wire through the ONE place the store
//! performs it.
//!
//! RULE under test: the store keeps a Region partition exactly while that Region holds
//! something. A partition that holds nothing is dropped by the next request, whichever
//! operation and whichever regional endpoint that request is, and dropping it changes
//! nothing a caller can observe — an absent partition and an empty one read the same —
//! so only Regions that hold something count against
//! [`service_com_amazonaws_ssmsap_ssmsap::state::MAX_REGIONS`]. "Holds something" is
//! everything a partition holds, not its applications alone: a Region whose application
//! has been deregistered still holds the record of the calls already made in it, so it
//! is kept.
//!
//! The rows span what the rule generalises over: the operation that drives the
//! reclamation (a listing, a read, a refused grant, a refused start, a refused
//! deregistration, a listing of operations), the HTTP method and route that operation is
//! modelled with (`POST` with a JSON body, and the `GET /tags/{resourceArn}` route whose
//! every member is bound to the URI), the endpoint the request is signed for versus the
//! Regions whose partitions are reclaimed, and how many empty partitions the store holds
//! when the request arrives (none, a few, one past the ceiling).
//!
//! Every call goes through `MultiServiceRouter::dispatch` (the harness
//! `tests/all/application_lifecycle.rs` owns), so the restJson1 routing, the boundary
//! constraint walk, the handler and the response serialization all run exactly as they do
//! in the emulator binary. The ONE thing done through `crate::state` is PLANTING a bare
//! partition and counting the partitions held: no API of this service mints a partition
//! it stores nothing in, and no response reports how many partitions the store keeps.

use serde_json::{json, Value};

use crate::application_lifecycle::{rendered, settle, Emulator, Reply};
use crate::resource_permissions::register_in_region;
use crate::resource_tags::{application_arn, database_arn, registration, tags_path};
use service_com_amazonaws_ssmsap_ssmsap::ops::types::PERMISSION_ACTION_TYPE_VALUES;
use service_com_amazonaws_ssmsap_ssmsap::state::MAX_REGIONS;

/// An `ApplicationId` nothing is ever registered under, so every row that names it is
/// answered by a Region that holds no such application.
const MISSING_APP: &str = "reclaim-absent";

/// The error code a registration past the store's Region ceiling carries
/// (`smithy_query(query="operation", operation="RegisterApplication")`:
/// `ConflictException` `@httpError 409`).
const CONFLICT_EXCEPTION: &str = "ConflictException";

/// One request to drive the reclamation with: the route it addresses and the body it
/// carries, both built from the Region it is signed for so a row naming a resource by ARN
/// names one OF that Region.
struct Drive {
    label: &'static str,
    method: &'static str,
    path: fn(&str) -> String,
    /// `None` is no bytes at all, which is what a client sends for an operation whose
    /// every member is bound to the URI.
    body: fn(&str) -> Option<Value>,
}

/// The rows: one per operation shape that can be the next request to arrive.
fn drives() -> Vec<Drive> {
    vec![
        Drive {
            label: "a listing that reports nothing",
            method: "POST",
            path: |_| "/list-applications".to_string(),
            body: |_| Some(json!({})),
        },
        Drive {
            label: "a read of an application this Region does not hold",
            method: "POST",
            path: |_| "/get-application".to_string(),
            body: |_| Some(json!({ "ApplicationId": MISSING_APP })),
        },
        Drive {
            label: "a tag read of a resource this Region does not hold",
            method: "GET",
            path: |region| tags_path(&application_arn(region, MISSING_APP)),
            body: |_| None,
        },
        Drive {
            label: "a grant refused for a resource this Region does not hold",
            method: "POST",
            path: |_| "/put-resource-permission".to_string(),
            body: |region| {
                Some(json!({
                    "ResourceArn": database_arn(region, MISSING_APP, "HDB"),
                    "ActionType": PERMISSION_ACTION_TYPE_VALUES[0],
                    "SourceResourceArn": database_arn(region, "reclaim-source", "HDB"),
                }))
            },
        },
        Drive {
            label: "a start refused for an application this Region does not hold",
            method: "POST",
            path: |_| "/start-application".to_string(),
            body: |_| Some(json!({ "ApplicationId": MISSING_APP })),
        },
        Drive {
            label: "a deregistration refused for an application this Region does not hold",
            method: "POST",
            path: |_| "/deregister-application".to_string(),
            body: |_| Some(json!({ "ApplicationId": MISSING_APP })),
        },
        Drive {
            label: "a listing of the operations of an application this Region does not hold",
            method: "POST",
            path: |_| "/list-operations".to_string(),
            body: |_| Some(json!({ "ApplicationId": MISSING_APP })),
        },
    ]
}

/// Send `drive` at `region`'s endpoint.
async fn send(emulator: &Emulator, region: &str, drive: &Drive) -> Reply {
    emulator
        .send_request(drive.method, region, &(drive.path)(region), (drive.body)(region))
        .await
}

/// Place a BARE partition for each of `regions` — the one state no API of this service
/// leaves behind, since every write that reaches a partition stores something in it.
fn plant_empty_partitions(emulator: &Emulator, regions: &[String]) {
    emulator.with_state(|state| {
        for region in regions {
            state.region_entry(region);
        }
    });
}

/// How many Region partitions the store holds, which no response reports.
fn partitions(emulator: &Emulator) -> usize {
    let mut held = 0;
    emulator.with_state(|state| held = state.region_count());
    held
}

/// Whether the store holds a partition for `region`.
fn holds(emulator: &Emulator, region: &str) -> bool {
    let mut held = false;
    emulator.with_state(|state| held = state.holds_region(region));
    held
}

#[tokio::test]
async fn an_empty_partition_is_dropped_by_the_next_request_whatever_it_is() {
    for drive in drives() {
        let label = drive.label;
        let emulator = Emulator::new();
        let region = "us-reclaim-1".to_string();
        // Two more Regions, neither of them the endpoint any request below is signed for:
        // the reclamation is owed everywhere at the same instant, not only where the call
        // arrives.
        let elsewhere = ["eu-reclaim-2".to_string(), "ap-reclaim-3".to_string()];

        // What the call answers with no partition anywhere, and the fact that answering
        // it minted none.
        let baseline = rendered(&send(&emulator, &region, &drive).await);
        assert_eq!(
            partitions(&emulator),
            0,
            "case {label:?}: the call must mint no partition, got {} held",
            partitions(&emulator)
        );

        let planted: Vec<String> = std::iter::once(region.clone())
            .chain(elsewhere.iter().cloned())
            .collect();
        plant_empty_partitions(&emulator, &planted);
        assert_eq!(
            partitions(&emulator),
            planted.len(),
            "case {label:?}: expected the {} planted partitions to be held, got {}",
            planted.len(),
            partitions(&emulator)
        );

        // The same call again, now with an empty partition for its own Region and for two
        // others: an empty partition reads exactly as an absent one, so the answer is the
        // one it gave before.
        let after = rendered(&send(&emulator, &region, &drive).await);
        assert_eq!(
            after, baseline,
            "case {label:?}: an empty partition must change no response, expected \
             {baseline:?}, got {after:?}"
        );

        // And every one of them is gone — the partition of the Region the call was signed
        // for and the partitions of the Regions it was not.
        for region in &planted {
            assert!(
                !holds(&emulator, region),
                "case {label:?}: the empty partition of {region} must be reclaimed, and it \
                 is still held"
            );
        }
        assert_eq!(
            partitions(&emulator),
            0,
            "case {label:?}: expected no partition left, got {} held",
            partitions(&emulator)
        );
    }
}

#[tokio::test]
async fn only_regions_that_hold_something_count_against_the_partition_ceiling() {
    let emulator = Emulator::new();

    // One empty partition PAST the ceiling, so if any of them survived, the registrations
    // below could not all be accepted.
    let planted: Vec<String> = (0..=MAX_REGIONS)
        .map(|index| format!("us-empty-{index}"))
        .collect();
    plant_empty_partitions(&emulator, &planted);
    assert_eq!(
        partitions(&emulator),
        MAX_REGIONS + 1,
        "case \"planted past the ceiling\": expected {} partitions, got {}",
        MAX_REGIONS + 1,
        partitions(&emulator)
    );

    // ONE request — any operation, at any endpoint — reclaims all of them.
    let reply = emulator.send("/list-applications", json!({})).await;
    assert_eq!(
        reply.status, 200,
        "case \"one request past the ceiling\": expected HTTP 200, got {reply:?}"
    );
    assert_eq!(
        partitions(&emulator),
        0,
        "case \"one request past the ceiling\": expected every empty partition reclaimed, \
         got {} held",
        partitions(&emulator)
    );

    // So the whole ceiling is free: a registration in each of MAX_REGIONS Regions is
    // accepted, which it could not be had the empty partitions kept counting.
    let first = emulator
        .send_in_region(
            &planted[0],
            "/register-application",
            registration("reclaim-app", &[], Some(&["HDB"])),
        )
        .await;
    assert_eq!(
        first.status, 200,
        "case \"the first registration after the reclamation\": expected HTTP 200, got \
         {first:?}"
    );
    let first_operation = first
        .member("OperationId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("case \"the first registration after the reclamation\": expected an OperationId, got {first:?}")
        })
        .to_string();
    for region in &planted[1..MAX_REGIONS] {
        register_in_region(
            &emulator,
            &format!("a registration in {region}"),
            region,
            registration("reclaim-app", &[], Some(&["HDB"])),
        )
        .await;
    }
    assert_eq!(
        partitions(&emulator),
        MAX_REGIONS,
        "case \"MAX_REGIONS registrations\": expected {MAX_REGIONS} partitions, got {}",
        partitions(&emulator)
    );

    // A partition that holds something is NOT dropped, so the ceiling is now spent and
    // one more Region is refused with the modelled ConflictException.
    let over = &planted[MAX_REGIONS];
    let refused = emulator
        .send_in_region(
            over,
            "/register-application",
            registration("reclaim-app", &[], Some(&["HDB"])),
        )
        .await;
    assert_eq!(
        refused.status, 409,
        "case \"one Region past {MAX_REGIONS}\": expected HTTP 409, got {refused:?}"
    );
    assert_eq!(
        refused.code(),
        CONFLICT_EXCEPTION,
        "case \"one Region past {MAX_REGIONS}\": expected {CONFLICT_EXCEPTION}, got {refused:?}"
    );
    assert!(
        !holds(&emulator, over),
        "case \"one Region past {MAX_REGIONS}\": the refusal must keep no partition, and \
         {over} is held"
    );

    // "Holds something" is everything the partition holds, not its applications alone: a
    // deregistration that has completed takes the application with it and leaves the
    // record of the calls already made, so the Region is still one the store keeps.
    let deregistered = emulator
        .send_in_region(
            &planted[0],
            "/deregister-application",
            json!({ "ApplicationId": "reclaim-app" }),
        )
        .await;
    assert_eq!(
        deregistered.status, 200,
        "case \"a deregistration in the first Region\": expected HTTP 200, got {deregistered:?}"
    );
    settle().await;

    let gone = emulator
        .send_in_region(
            &planted[0],
            "/get-application",
            json!({ "ApplicationId": "reclaim-app" }),
        )
        .await;
    assert_eq!(
        gone.status, 400,
        "case \"the deregistered application\": expected HTTP 400 once the removal fell due, \
         got {gone:?}"
    );
    let history = emulator
        .send_in_region(
            &planted[0],
            "/get-operation",
            json!({ "OperationId": first_operation }),
        )
        .await;
    assert_eq!(
        history.status, 200,
        "case \"the record of the registration\": expected HTTP 200 after the deregistration \
         completed, got {history:?}"
    );
    assert!(
        holds(&emulator, &planted[0]),
        "case \"the emptied-of-applications Region\": a Region still holding the record of \
         its calls must keep its partition, and {} is not held",
        planted[0]
    );
    assert_eq!(
        partitions(&emulator),
        MAX_REGIONS,
        "case \"the emptied-of-applications Region\": expected {MAX_REGIONS} partitions still \
         held, got {}",
        partitions(&emulator)
    );
}
