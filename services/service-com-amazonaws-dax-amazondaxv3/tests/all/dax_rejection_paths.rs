// SPDX-License-Identifier: CC0-1.0

//! The DAX rejection paths, over the wire: which subnet identifiers a subnet group may
//! name, which `DescribeEvents` windows are readable, and the message each fault
//! answers with.
//!
//! Every case drives `POST /` with `X-Amz-Target: AmazonDAXV3.<Operation>` through the
//! same `MultiServiceRouter` the production router builds, so what is asserted is the
//! HTTP behaviour a DAX SDK client sees.
//!
//! The tables below span the CLASS each rule covers; the one request the issue reported
//! is a single row.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_dax_amazondaxv3::{state, EmulatedService};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for: the Region whose network a subnet
/// identifier is resolved in.
const REGION: &str = "us-west-2";

/// A role ARN to pass as the required `IamRoleArn`.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/DAXRole";

/// A node type to pass as the required `NodeType`.
const NODE_TYPE: &str = "dax.r4.large";

/// Wait until every clock-owed status move started before the call is due.
async fn settle() {
    tokio::time::sleep(state::STATUS_SETTLE_DELAY + Duration::from_millis(400)).await;
}

/// A router carrying only this service, over its own engine, so each test starts from
/// an empty store.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Call one operation and return its HTTP status and parsed JSON body.
async fn call(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("AmazonDAXV3.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/dax/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The wire error code of a failed call: the `__type` member, without any namespace.
fn error_code(body: &Value) -> String {
    body["__type"]
        .as_str()
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The wire error message of a failed call, under either casing an error envelope uses.
fn error_message(body: &Value) -> String {
    body["message"]
        .as_str()
        .or_else(|| body["Message"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// The subnets that exist in `region`'s network.
fn subnets_in(region: &str) -> Vec<String> {
    state::region_subnets(region)
        .into_iter()
        .map(|subnet| subnet.subnet_id)
        .collect()
}

/// A `CreateCluster` body with the required members filled in.
fn create_cluster_body(name: &str) -> Value {
    json!({
        "ClusterName": name,
        "NodeType": NODE_TYPE,
        "ReplicationFactor": 1,
        "IamRoleArn": ROLE_ARN,
    })
}

/// Seconds since the epoch, `ago` before now — the wire form of a `TStamp` on this
/// service's JSON protocol.
fn seconds_ago(ago: Duration) -> f64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch");
    now.saturating_sub(ago).as_secs() as f64
}

// ---------------------------------------------------------------------------
// Cause 1. Rule: `CreateSubnetGroup` resolves every identifier in `SubnetIds` against
// the subnets that exist in the Region the request was signed for, and refuses a
// request naming any identifier that resolves to no subnet — HTTP 400
// `InvalidParameterValueException`, listing the identifiers it could not resolve —
// storing nothing.
// Source: `state::region_subnets` (the Region's network) and the recorded AWS answer
// for a subnet identifier that names nothing.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_subnet_group_is_refused_unless_every_identifier_names_an_existing_subnet() {
    let router = emulator();
    let here = subnets_in(REGION);
    let elsewhere = subnets_in("eu-west-1");

    // Rows differ in WHY an identifier resolves to nothing: the long EC2 form that
    // names no subnet (the reported request), the short EC2 form, a string that is not
    // a subnet id at all, a real subnet of ANOTHER Region, and one bad identifier
    // hidden among good ones. The accepted rows span one subnet, every subnet of the
    // Region, and a repeated identifier.
    let accepted: [(&str, Vec<String>); 3] = [
        ("one-subnet", vec![here[0].clone()]),
        ("every-subnet", here.clone()),
        ("repeated", vec![here[1].clone(), here[1].clone()]),
    ];
    let refused: [(&str, Vec<String>, Vec<String>); 5] = [
        (
            "bogus-long",
            vec!["subnet-0000000000deadbee".to_string()],
            vec!["subnet-0000000000deadbee".to_string()],
        ),
        (
            "bogus-short",
            vec!["subnet-0a1b2c3d".to_string()],
            vec!["subnet-0a1b2c3d".to_string()],
        ),
        (
            "not-a-subnet-id",
            vec!["potato".to_string()],
            vec!["potato".to_string()],
        ),
        (
            "other-region",
            vec![elsewhere[0].clone()],
            vec![elsewhere[0].clone()],
        ),
        (
            "one-bad-among-good",
            vec![
                here[0].clone(),
                "subnet-0000000000deadbee".to_string(),
                here[1].clone(),
            ],
            vec!["subnet-0000000000deadbee".to_string()],
        ),
    ];

    for (case, subnet_ids) in accepted {
        let (status, body) = call(
            &router,
            "CreateSubnetGroup",
            json!({ "SubnetGroupName": case, "SubnetIds": subnet_ids }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: an existing subnet is accepted, got {status} {body:?}"
        );
        // Round trip: the group must read back out of the store.
        let (status, read) = call(
            &router,
            "DescribeSubnetGroups",
            json!({ "SubnetGroupNames": [case] }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeSubnetGroups got {status} {read:?}"
        );
        assert_eq!(
            read["SubnetGroups"][0], body["SubnetGroup"],
            "case {case:?}: the stored group must read back as created, got {read:?}"
        );
    }

    for (case, subnet_ids, invalid) in refused {
        let (status, body) = call(
            &router,
            "CreateSubnetGroup",
            json!({ "SubnetGroupName": case, "SubnetIds": subnet_ids.clone() }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: {subnet_ids:?} names no existing subnet, expected a refusal, \
             got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValueException",
            "case {case:?}: expected InvalidParameterValueException, got {body:?}"
        );
        assert_eq!(
            error_message(&body),
            format!("Some input subnets in :[{}] are invalid.", invalid.join(", ")),
            "case {case:?}: the message must list the identifiers that resolve to nothing, \
             got {body:?}"
        );
        // Nothing was stored: the group does not exist.
        let (status, read) = call(
            &router,
            "DescribeSubnetGroups",
            json!({ "SubnetGroupNames": [case] }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: a refused create leaves nothing behind, got {status} {read:?}"
        );
        assert_eq!(
            error_code(&read),
            "SubnetGroupNotFoundFault",
            "case {case:?}: expected SubnetGroupNotFoundFault, got {read:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Cause 2. Rule: `DescribeEvents` reports the events this service recorded for its own
// resources inside the requested window, filtered to one source when the request names
// one, and refuses a window reaching further back than the events DAX retains
// (`state::MAX_EVENT_DURATION` minutes) with HTTP 400
// `InvalidParameterCombinationException`.
// Source: `DescribeEvents` ("By default, only the events occurring within the last 24
// hours are returned; however, you can retrieve up to 14 days' worth of events").
// ---------------------------------------------------------------------------

#[tokio::test]
async fn describe_events_refuses_a_window_past_the_retained_days_and_reads_the_rest() {
    let router = emulator();

    // Rows: every minute count inside the retained window, including both published
    // bounds, against the counts past the upper bound and a negative one.
    let accepted = [0_i64, 1, 60, 1_440, i64::from(state::MAX_EVENT_DURATION)];
    let refused = [
        i64::from(state::MAX_EVENT_DURATION) + 1,
        i64::from(state::MAX_EVENT_DURATION) * 2,
        1_000_000,
    ];

    for duration in accepted {
        let (status, body) = call(&router, "DescribeEvents", json!({ "Duration": duration })).await;
        assert_eq!(
            status, 200,
            "case Duration={duration}: inside the retained window, got {status} {body:?}"
        );
        assert!(
            body["Events"].is_array(),
            "case Duration={duration}: the answer carries an Events list, got {body:?}"
        );
    }
    for duration in refused {
        let (status, body) = call(&router, "DescribeEvents", json!({ "Duration": duration })).await;
        assert_eq!(
            status, 400,
            "case Duration={duration}: past the retained window, expected a refusal, \
             got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterCombinationException",
            "case Duration={duration}: expected InvalidParameterCombinationException, \
             got {body:?}"
        );
        assert_eq!(
            error_message(&body),
            format!(
                "Events occurring more than {} days in the past are not available.",
                state::MAX_EVENT_DURATION / (24 * 60)
            ),
            "case Duration={duration}: wrong message, got {body:?}"
        );
    }

    // The same rule through `StartTime`: a start inside the retained window is read, a
    // start before it is refused with the same fault.
    let (status, body) = call(
        &router,
        "DescribeEvents",
        json!({ "StartTime": seconds_ago(state::EVENT_RETENTION / 2) }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case StartTime inside the window: got {status} {body:?}"
    );
    let (status, body) = call(
        &router,
        "DescribeEvents",
        json!({ "StartTime": seconds_ago(state::EVENT_RETENTION + Duration::from_secs(86_400)) }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case StartTime before the window: expected a refusal, got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidParameterCombinationException",
        "case StartTime before the window: expected InvalidParameterCombinationException, \
         got {body:?}"
    );

    // A window that ends before it starts is a combination no events can fall in.
    let (status, body) = call(
        &router,
        "DescribeEvents",
        json!({
            "StartTime": seconds_ago(Duration::from_secs(60)),
            "EndTime": seconds_ago(Duration::from_secs(3_600)),
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case StartTime after EndTime: expected a refusal, got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidParameterCombinationException",
        "case StartTime after EndTime: expected InvalidParameterCombinationException, \
         got {body:?}"
    );

    // A timestamp this service cannot place is REFUSED rather than converted. Rows: an
    // instant before the epoch, and one further from the epoch than an instant reaches,
    // on each of the two members that carry one.
    let impossible: [(&str, f64); 4] = [
        ("StartTime", -1.0),
        ("EndTime", -1.0),
        ("StartTime", 1e300),
        ("EndTime", 1e300),
    ];
    for (member, value) in impossible {
        let (status, body) = call(&router, "DescribeEvents", json!({ member: value })).await;
        assert_eq!(
            status, 400,
            "case {member}={value}: an unplaceable timestamp is refused, got {status} {body:?}"
        );
    }
}

#[tokio::test]
async fn describe_events_reports_the_events_the_store_recorded() {
    let router = emulator();
    let subnet = subnets_in(REGION).swap_remove(0);

    let (status, body) = call(
        &router,
        "CreateSubnetGroup",
        json!({ "SubnetGroupName": "evt-subnets", "SubnetIds": [subnet] }),
    )
    .await;
    assert_eq!(status, 200, "CreateSubnetGroup got {status} {body:?}");
    let (status, body) = call(&router, "CreateCluster", create_cluster_body("evt")).await;
    assert_eq!(status, 200, "CreateCluster got {status} {body:?}");
    let (status, body) = call(
        &router,
        "CreateParameterGroup",
        json!({ "ParameterGroupName": "evt-pg" }),
    )
    .await;
    assert_eq!(status, 200, "CreateParameterGroup got {status} {body:?}");
    settle().await;

    // Rows: each filter the operation documents — one source type, one source name,
    // both together, and a source nothing was recorded for.
    let (status, body) = call(
        &router,
        "DescribeEvents",
        json!({ "SourceType": state::SOURCE_TYPE_SUBNET_GROUP }),
    )
    .await;
    assert_eq!(status, 200, "case SourceType=SUBNET_GROUP: got {body:?}");
    assert_eq!(
        body["Events"],
        json!([{
            "SourceName": "evt-subnets",
            "SourceType": state::SOURCE_TYPE_SUBNET_GROUP,
            "Message": state::EVENT_SUBNET_GROUP_CREATED,
            "Date": body["Events"][0]["Date"],
        }]),
        "case SourceType=SUBNET_GROUP: only the subnet group's own event, got {body:?}"
    );

    let (status, body) = call(
        &router,
        "DescribeEvents",
        json!({ "SourceType": state::SOURCE_TYPE_PARAMETER_GROUP }),
    )
    .await;
    assert_eq!(status, 200, "case SourceType=PARAMETER_GROUP: got {body:?}");
    let messages: Vec<String> = body["Events"]
        .as_array()
        .expect("Events is a list")
        .iter()
        .map(|event| event["Message"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        messages,
        vec![state::EVENT_PARAMETER_GROUP_CREATED.to_string()],
        "case SourceType=PARAMETER_GROUP: only the parameter group's own event, got {body:?}"
    );

    let (status, body) = call(
        &router,
        "DescribeEvents",
        json!({ "SourceName": "evt", "SourceType": state::SOURCE_TYPE_CLUSTER }),
    )
    .await;
    assert_eq!(status, 200, "case SourceName=evt: got {body:?}");
    let messages: Vec<String> = body["Events"]
        .as_array()
        .expect("Events is a list")
        .iter()
        .map(|event| event["Message"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        messages,
        vec![
            state::EVENT_CLUSTER_CREATED.to_string(),
            state::EVENT_CLUSTER_AVAILABLE.to_string(),
        ],
        "case SourceName=evt: the create and the clock-owed settle, in order, got {body:?}"
    );

    let (status, body) = call(
        &router,
        "DescribeEvents",
        json!({ "SourceName": "nothing-recorded-here" }),
    )
    .await;
    assert_eq!(status, 200, "case an unrecorded source: got {body:?}");
    assert_eq!(
        body["Events"],
        json!([]),
        "case an unrecorded source: no event matches, got {body:?}"
    );

    // A `Duration` of 0 minutes selects the instant of the call, not the whole log.
    let (status, body) = call(&router, "DescribeEvents", json!({ "Duration": 0 })).await;
    assert_eq!(status, 200, "case Duration=0: got {body:?}");
    assert_eq!(
        body["Events"],
        json!([]),
        "case Duration=0: a zero-minute window holds the events of this instant only, \
         got {body:?}"
    );

    // A `SourceType` outside the enum names no event source.
    let (status, body) = call(
        &router,
        "DescribeEvents",
        json!({ "SourceType": "NOT_A_SOURCE_TYPE" }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case SourceType outside the enum: expected a refusal, got {status} {body:?}"
    );
}

#[tokio::test]
async fn a_truncated_event_page_continues_through_its_next_token() {
    let router = emulator();
    // Each cluster records a create event now and an available event when the clock
    // settles it, so this fills more than one page of the minimum page size.
    let clusters = state::MIN_MAX_RESULTS + 2;
    for index in 0..clusters {
        let name = format!("evt-{index:02}");
        let (status, body) = call(&router, "CreateCluster", create_cluster_body(&name)).await;
        assert_eq!(status, 200, "case {name}: CreateCluster got {body:?}");
    }
    settle().await;

    let expected = usize::try_from(clusters * 2).expect("an event count fits in usize");
    let page_size = usize::try_from(state::MIN_MAX_RESULTS).expect("a page size fits in usize");
    let mut seen: Vec<(String, String)> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let mut request = json!({ "MaxResults": state::MIN_MAX_RESULTS });
        if let Some(carried) = &token {
            request["NextToken"] = json!(carried);
        }
        let (status, answered) = call(&router, "DescribeEvents", request).await;
        assert_eq!(
            status, 200,
            "case page {pages}: DescribeEvents got {status} {answered:?}"
        );
        let events = answered["Events"].as_array().expect("Events is a list");
        assert!(
            !events.is_empty() && events.len() <= page_size,
            "case page {pages}: a page holds between one and MaxResults events, got {answered:?}"
        );
        for event in events {
            seen.push((
                event["SourceName"].as_str().unwrap_or_default().to_string(),
                event["Message"].as_str().unwrap_or_default().to_string(),
            ));
        }
        pages += 1;
        token = answered["NextToken"].as_str().map(str::to_string);
        if token.is_none() {
            break;
        }
        assert!(
            pages < 10,
            "case page {pages}: the pages must run out, got {answered:?}"
        );
    }

    assert!(
        pages > 1,
        "case page count: {expected} events at a page size of {page_size} must truncate, \
         got {pages}"
    );
    let mut unique = seen.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        (seen.len(), unique.len()),
        (expected, expected),
        "every event must appear exactly once across the pages, got {seen:?}"
    );

    // A token this service did not issue is refused, not answered with some page.
    for token in ["not-a-token!!", "/w==", "", "bm90LWEtbnVtYmVy"] {
        let (status, body) = call(
            &router,
            "DescribeEvents",
            json!({ "MaxResults": state::MIN_MAX_RESULTS, "NextToken": token }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case NextToken={token:?}: expected a refusal, got {status} {body:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Cause 3. Rule: a DAX fault's message comes from ONE template owned by that fault, so
// every operation raising it answers the same text; the template echoes the requested
// identifier only where that fault's template does.
// Source: the recorded DAX answers — `ClusterNotFoundFault` "Cluster not found.",
// `SubnetGroupNotFoundFault` "Subnet group <name> not found.", and the `MaxResults`
// refusal "Invalid value for max records. Must be between 20 and 100".
// ---------------------------------------------------------------------------

#[tokio::test]
async fn one_fault_answers_one_message_whatever_operation_raises_it() {
    let router = emulator();

    // Rows: every implemented operation that resolves a cluster by name, over names
    // that differ in length and casing, none of which is stored.
    let missing = ["gone", "sim2real-dax-missing-5516309b87dc", "MixedCaseGone"];
    for name in missing {
        let cases: [(&str, Value); 4] = [
            ("DeleteCluster", json!({ "ClusterName": name })),
            ("UpdateCluster", json!({ "ClusterName": name })),
            (
                "IncreaseReplicationFactor",
                json!({ "ClusterName": name, "NewReplicationFactor": 2 }),
            ),
            ("DescribeClusters", json!({ "ClusterNames": [name] })),
        ];
        for (operation, body) in cases {
            let (status, answered) = call(&router, operation, body).await;
            assert_eq!(
                status, 404,
                "case {operation} {name:?}: expected a not-found, got {status} {answered:?}"
            );
            assert_eq!(
                error_code(&answered),
                "ClusterNotFoundFault",
                "case {operation} {name:?}: expected ClusterNotFoundFault, got {answered:?}"
            );
            assert_eq!(
                error_message(&answered),
                "Cluster not found.",
                "case {operation} {name:?}: every site answers this fault's own template, \
                 got {answered:?}"
            );
        }
    }

    // `SubnetGroupNotFoundFault`'s template DOES name the group, lower-cased the way
    // the group is stored.
    let groups: [(&str, &str); 2] = [("absent-sg", "absent-sg"), ("Absent-SG", "absent-sg")];
    for (asked, stored) in groups {
        let cases: [(&str, Value); 2] = [
            (
                "DescribeSubnetGroups",
                json!({ "SubnetGroupNames": [asked] }),
            ),
            ("CreateCluster", {
                let mut body = create_cluster_body("needs-sg");
                body["SubnetGroupName"] = json!(asked);
                body
            }),
        ];
        for (operation, body) in cases {
            let (status, answered) = call(&router, operation, body).await;
            assert_eq!(
                status, 404,
                "case {operation} {asked:?}: expected a not-found, got {status} {answered:?}"
            );
            assert_eq!(
                error_code(&answered),
                "SubnetGroupNotFoundFault",
                "case {operation} {asked:?}: expected SubnetGroupNotFoundFault, got {answered:?}"
            );
            assert_eq!(
                error_message(&answered),
                format!("Subnet group {stored} not found."),
                "case {operation} {asked:?}: wrong message, got {answered:?}"
            );
        }
    }

    // The `MaxResults` refusal reports the documented window, not the value given.
    for operation in ["DescribeClusters", "DescribeSubnetGroups", "DescribeEvents"] {
        for max_results in [
            i64::from(state::MIN_MAX_RESULTS) - 1,
            0,
            -1,
            i64::from(state::MAX_MAX_RESULTS) + 1,
            1_000,
        ] {
            let (status, body) =
                call(&router, operation, json!({ "MaxResults": max_results })).await;
            assert_eq!(
                status, 400,
                "case {operation} MaxResults={max_results}: expected a refusal, \
                 got {status} {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidParameterValueException",
                "case {operation} MaxResults={max_results}: expected \
                 InvalidParameterValueException, got {body:?}"
            );
            assert_eq!(
                error_message(&body),
                format!(
                    "Invalid value for max records. Must be between {} and {}",
                    state::MIN_MAX_RESULTS,
                    state::MAX_MAX_RESULTS
                ),
                "case {operation} MaxResults={max_results}: every site answers this \
                 fault's own template, got {body:?}"
            );
        }
    }
}
