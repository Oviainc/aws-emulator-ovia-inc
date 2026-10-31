// SPDX-License-Identifier: CC0-1.0

//! What `DeleteEndpoint` does to an endpoint, end to end over HTTP.
//!
//! The rule under test: an action this control plane documents as taking time to finish is
//! ACCEPTED rather than performed, so the resource it names keeps being reported — with the
//! transient `EndpointStatus` value that action puts it in, and still holding everything it
//! committed — until the clock completes the action, and only then does the record
//! disappear. For a deletion that value is `Deleting`, which the model enumerates beside
//! `Pending`: inside the span the endpoint is a record like any other (every listing that
//! serves its Outpost reports it, its members do not move, another delete of it resolves,
//! and the subnet, security
//! group and CIDR it holds are not free for a new endpoint), and past the span it is gone
//! (no listing reports it, naming it is the modelled 404, and what it held is available
//! again).
//!
//! The rows differ along what the rule generalises over: the status the endpoint was in
//! when the delete arrived (one the clock had already made `Available`, one still
//! `Pending`), how many times the delete is repeated inside the span, and which Outpost of
//! the fixture the endpoint sits on — one this account owns, or one another account shared
//! with it, which is also the Outpost the Outpost-scoped listing serves. Every assertion
//! goes through the router, so each row exercises routing, deserialization, the handler,
//! the persisted state and serialization.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_s3outposts_s3outposts::state::ENDPOINT_TRANSITION;

use crate::endpoint_lifecycle::{
    assert_not_found, create, delete_endpoint, emulator, endpoint_id_of, endpoint_with_arn,
    list_endpoints, list_shared, owned_outposts, reported_arns, shared_outpost, shared_query,
    string_at,
};

/// The status the endpoint carries when the delete arrives.
#[derive(Debug, Clone, Copy)]
enum Arrives {
    /// After the create's own span, so the clock has already made it `Available`.
    WhenAvailable,
    /// Inside the create's own span, while it is still `Pending`.
    WhilePending,
}

/// One row of the table: what state the endpoint is in when the delete arrives, how many
/// deletes are issued inside the teardown span, and which Outpost it sits on — the two this
/// account owns, or the one another account shared with it, which is also the Outpost the
/// Outpost-scoped listing serves (`shared_endpoint_listing.rs`).
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    arrives: Arrives,
    deletes: usize,
    outpost: usize,
}

/// The members of an endpoint that identify what it holds and when it was created — none
/// of which a teardown may move while it is still running.
fn held_members(endpoint: &Value) -> Vec<Value> {
    [
        "CidrBlock",
        "VpcId",
        "SubnetId",
        "SecurityGroupId",
        "CreationTime",
        "AccessType",
        "OutpostsId",
    ]
    .iter()
    .map(|name| endpoint[*name].clone())
    .collect()
}

#[tokio::test]
async fn a_deleted_endpoint_reports_deleting_until_the_clock_completes_the_teardown() {
    let rows = [
        Row {
            case: "an Available endpoint, deleted once",
            arrives: Arrives::WhenAvailable,
            deletes: 1,
            outpost: 0,
        },
        Row {
            case: "an endpoint still Pending when the delete arrives, on a shared Outpost",
            arrives: Arrives::WhilePending,
            deletes: 1,
            outpost: 2,
        },
        Row {
            case: "the delete repeated three times inside the span",
            arrives: Arrives::WhenAvailable,
            deletes: 3,
            outpost: 1,
        },
        Row {
            case: "a Pending endpoint on a shared Outpost, the delete repeated inside the span",
            arrives: Arrives::WhilePending,
            deletes: 2,
            outpost: 2,
        },
    ];

    for row in rows {
        // A fresh emulator per row, so each row's timing and store are its own.
        let router = emulator();
        let mut outposts = owned_outposts(&router).await;
        outposts.push(shared_outpost(&router).await);
        let shared = outposts[outposts.len() - 1].clone();
        let outpost_id = outposts[row.outpost % outposts.len()].clone();
        // The Outpost-scoped listing serves an Outpost another account shared with this
        // caller, so a row on an Outpost this account owns reads the record through its
        // own listing alone.
        let on_shared_outpost = outpost_id == shared;
        let request = json!({
            "OutpostId": outpost_id,
            "SubnetId": "subnet-0d1e2f30415263748",
            "SecurityGroupId": "sg-0d1e2f3041526374a",
        });

        let created = create(&router, request.clone()).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: CreateEndpoint must answer 200, got {} with {:?}",
            row.case, created.status, created.body
        );
        let arn = string_at(&created.body, "EndpointArn");
        let endpoint_id = endpoint_id_of(&arn);
        let outpost_query = shared_query(&outpost_id);

        if matches!(row.arrives, Arrives::WhenAvailable) {
            std::thread::sleep(ENDPOINT_TRANSITION + Duration::from_millis(200));
            let ready = list_endpoints(&router, "?maxResults=100").await;
            assert_eq!(
                endpoint_with_arn(&ready, &arn)["Status"],
                json!("Available"),
                "case {:?}: the endpoint must be Available before the delete, got {:?}",
                row.case,
                endpoint_with_arn(&ready, &arn)["Status"]
            );
        }

        let before = endpoint_with_arn(&list_endpoints(&router, "?maxResults=100").await, &arn);

        let started = Instant::now();
        for attempt in 0..row.deletes {
            let deleted = delete_endpoint(&router, &outpost_id, &endpoint_id).await;
            assert_eq!(
                deleted.status, 200,
                "case {:?}: delete {attempt} must answer 200 for a record that resolves, \
                 got {} with {:?}",
                row.case, deleted.status, deleted.body
            );
        }

        let listed = list_endpoints(&router, "?maxResults=100").await;
        let on_outpost = if on_shared_outpost {
            Some(list_shared(&router, &outpost_query).await)
        } else {
            None
        };
        // Everything below is about the endpoint INSIDE the teardown span; a run slow
        // enough to have left it is not evidence either way, so it is asserted only while
        // the span is demonstrably still open.
        let inside = started.elapsed() < ENDPOINT_TRANSITION;
        if inside {
            for (listing, answer) in [
                ("ListEndpoints", Some(&listed)),
                ("ListSharedEndpoints", on_outpost.as_ref()),
            ] {
                let Some(answer) = answer else {
                    continue;
                };
                let endpoint = endpoint_with_arn(answer, &arn);
                assert_eq!(
                    endpoint["Status"],
                    json!("Deleting"),
                    "case {:?}: {listing} must report the endpoint as Deleting, got {:?}",
                    row.case,
                    endpoint["Status"]
                );
                assert_eq!(
                    held_members(&endpoint),
                    held_members(&before),
                    "case {:?}: {listing} must report the same members the endpoint had \
                     before the delete, got {endpoint:?}",
                    row.case
                );
            }

            // The resources the endpoint committed are still its own, so a create
            // repeating all three is the modelled conflict.
            let conflict = create(&router, request.clone()).await;
            assert_eq!(
                (conflict.status, conflict.error_code()),
                (409, "ConflictException"),
                "case {:?}: a create repeating what the tearing-down endpoint holds must \
                 be refused, got {} with {:?}",
                row.case,
                conflict.status,
                conflict.body
            );
        }

        // The completion is owed by the clock, not by a call: a repeated delete does not
        // push it out, so the wait is measured from the FIRST one.
        let elapsed = started.elapsed();
        if elapsed < ENDPOINT_TRANSITION + Duration::from_millis(200) {
            std::thread::sleep(ENDPOINT_TRANSITION + Duration::from_millis(200) - elapsed);
        }

        let after = list_endpoints(&router, "?maxResults=100").await;
        let after_on_outpost = if on_shared_outpost {
            Some(list_shared(&router, &outpost_query).await)
        } else {
            None
        };
        for (listing, answer) in [
            ("ListEndpoints", Some(&after)),
            ("ListSharedEndpoints", after_on_outpost.as_ref()),
        ] {
            let Some(answer) = answer else {
                continue;
            };
            assert!(
                !reported_arns(answer).contains(&arn),
                "case {:?}: {listing} must not report {arn} once the teardown completed, \
                 got {:?}",
                row.case,
                answer.body
            );
        }

        let gone = delete_endpoint(&router, &outpost_id, &endpoint_id).await;
        assert_not_found(
            &format!(
                "{}: a delete naming an endpoint whose teardown completed",
                row.case
            ),
            &gone,
            &[outpost_id.as_str(), endpoint_id.as_str()],
        );

        // And what the removed endpoint held is free again: the very create that was the
        // conflict inside the span is now accepted.
        let recreated = create(&router, request).await;
        assert_eq!(
            recreated.status, 200,
            "case {:?}: a create repeating what the removed endpoint held must be \
             accepted, got {} with {:?}",
            row.case, recreated.status, recreated.body
        );
        assert_ne!(
            string_at(&recreated.body, "EndpointArn"),
            arn,
            "case {:?}: the new endpoint must be its own record, not the removed one",
            row.case
        );
    }
}
