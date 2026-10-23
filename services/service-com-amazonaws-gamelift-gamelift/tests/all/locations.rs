// SPDX-License-Identifier: CC0-1.0

//! The custom-location contract, over the wire: `CreateLocation` persists the location
//! the request names, a name a location already holds is refused with
//! `ConflictException` — including when several creates of one name run at once, where
//! exactly ONE wins — and `ListLocations` reports each stored location exactly once,
//! beside the Amazon Web Services locations this store hosts in.
//!
//! The harness (`GameLift`, `REGION`, `gamelift_arn`, `ec2_fleet`, `create_fleet`) is the
//! one `hosting_loop` already drives the production dispatch path with.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::hosting_loop::{
    anywhere_fleet, create_fleet, ec2_fleet, gamelift_arn, GameLift, REGION,
};

/// The ARN a custom location named `name` is assigned. `LocationModel$LocationArn`'s
/// modelled pattern is `^arn:.*:location\/custom-\S+$`, so the resource is the
/// location's own name.
fn location_arn(name: &str) -> String {
    gamelift_arn(&format!("location/{name}"))
}

/// The locations `ListLocations` reports for `request`.
async fn list_locations(gamelift: &GameLift, request: Value) -> Vec<Value> {
    let listed = gamelift.ok("ListLocations", request).await;
    listed["Locations"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("ListLocations must report a list, got {listed}"))
}

/// The `LocationName`s `ListLocations` reports for `request`, in order.
async fn listed_names(gamelift: &GameLift, request: Value) -> Vec<String> {
    list_locations(gamelift, request)
        .await
        .iter()
        .map(|location| {
            location["LocationName"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// Every `LocationFilter` the model declares (`smithy_query shape=LocationFilter`), so
/// the filter rows below are drawn from the modelled catalog rather than from one
/// example.
const LOCATION_FILTERS: [&str; 2] = ["AWS", "CUSTOM"];

/// A created custom location is reported as stored by the create and by every later
/// list, and the name it holds cannot be taken a second time.
///
/// The rows differ along what the rule generalises over: the LENGTH of the name at each
/// modelled bound (`CustomInputLocationStringModel`, len 8..64), the characters its
/// pattern allows (`^custom-[A-Za-z0-9\-]+$`: digits, upper case, inner hyphens), and
/// whether the create carries tags.
#[tokio::test]
async fn a_created_location_is_reported_once_and_its_name_cannot_be_taken_twice() {
    struct Case {
        label: &'static str,
        name: String,
        request: Value,
    }

    let cases = vec![
        Case {
            label: "the shortest name the model allows (8 characters)",
            name: "custom-a".to_string(),
            request: json!({}),
        },
        Case {
            label: "the longest name the model allows (64 characters)",
            name: format!("custom-{}", "a".repeat(57)),
            request: json!({}),
        },
        Case {
            label: "digits and upper case, which the pattern allows",
            name: "custom-WEST-2a".to_string(),
            request: json!({}),
        },
        Case {
            label: "several inner hyphens",
            name: "custom-rack-7-row-2".to_string(),
            request: json!({}),
        },
        Case {
            label: "a location created with tags",
            name: "custom-tagged".to_string(),
            request: json!({"Tags": [{"Key": "owner", "Value": "liveops"}]}),
        },
    ];

    let gamelift = GameLift::new();
    for case in &cases {
        let label = case.label;
        let mut request = case.request.clone();
        request["LocationName"] = json!(&case.name);

        let created = gamelift.ok("CreateLocation", request.clone()).await;
        assert_eq!(
            created["Location"],
            json!({
                "LocationName": &case.name,
                "LocationArn": location_arn(&case.name),
            }),
            "case {label:?}: CreateLocation must report the stored location, got {}",
            created["Location"]
        );

        // "The requested operation would cause a conflict with the current state of a
        // service resource": the name is taken, so a second create is refused and the
        // stored location is untouched.
        let (status, code) = gamelift.err("CreateLocation", request).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ConflictException"),
            "case {label:?}: a repeated location name must be refused, got {status} {code}"
        );

        // The list path serves the SAME record, and reports it exactly once.
        let custom = list_locations(&gamelift, json!({"Filters": ["CUSTOM"]})).await;
        let matching: Vec<&Value> = custom
            .iter()
            .filter(|location| location["LocationName"] == json!(&case.name))
            .collect();
        assert_eq!(
            matching.len(),
            1,
            "case {label:?}: the created location must be listed exactly once, got {custom:?}"
        );
        assert_eq!(
            *matching[0],
            created["Location"],
            "case {label:?}: the listed location must be the created record, got {}",
            matching[0]
        );
    }

    // Reading the locations changed nothing: the same list, run again, reports exactly
    // the same collection, and it holds every created location once.
    let first = listed_names(&gamelift, json!({"Filters": ["CUSTOM"], "Limit": 200})).await;
    let second = listed_names(&gamelift, json!({"Filters": ["CUSTOM"], "Limit": 200})).await;
    assert_eq!(
        first, second,
        "listing the locations must not change what a later list reports, got {second:?}"
    );
    let mut expected: Vec<String> = cases.iter().map(|case| case.name.clone()).collect();
    expected.sort();
    let mut reported = first.clone();
    reported.sort();
    assert_eq!(
        reported, expected,
        "every created location must be reported once, got {reported:?}"
    );
}

/// Several creates of ONE name, running at once, produce exactly one success and one
/// stored location: the uniqueness check and the insert happen under the one state lock.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_creates_of_one_name_have_exactly_one_winner() {
    // The rows differ along how many creates race for the name: one, a pair, and the
    // eight-way race a client that provisions a shared location performs.
    for racers in [1_usize, 2, 8] {
        let gamelift = Arc::new(GameLift::new());
        let name = format!("custom-race-{racers}");
        let mut tasks = Vec::with_capacity(racers);
        for _ in 0..racers {
            let gamelift = Arc::clone(&gamelift);
            let name = name.clone();
            tasks.push(tokio::spawn(async move {
                gamelift
                    .call("CreateLocation", &json!({"LocationName": name}))
                    .await
            }));
        }

        let mut winners = 0_usize;
        let mut conflicts = Vec::new();
        for task in tasks {
            let (status, body) = task.await.expect("the create task must not panic");
            if status == 200 {
                winners += 1;
                assert_eq!(
                    body["Location"]["LocationArn"],
                    json!(location_arn(&name)),
                    "case {racers} racers: the winner must report the stored ARN, got {body}"
                );
            } else {
                conflicts.push((status, crate::hosting_loop::error_code(&body)));
            }
        }

        assert_eq!(
            winners, 1,
            "case {racers} racers: exactly one create must win, got {winners} winners and \
             losers {conflicts:?}"
        );
        for (status, code) in &conflicts {
            assert_eq!(
                (*status, code.as_str()),
                (400, "ConflictException"),
                "case {racers} racers: every loser must be refused with ConflictException, got \
                 {status} {code}"
            );
        }
        assert_eq!(
            conflicts.len(),
            racers - 1,
            "case {racers} racers: every create but one must lose, got {conflicts:?}"
        );

        // And the name exists exactly once afterwards.
        let listed = listed_names(&gamelift, json!({"Filters": ["CUSTOM"]})).await;
        assert_eq!(
            listed,
            vec![name.clone()],
            "case {racers} racers: the location must be stored exactly once, got {listed:?}"
        );
    }
}

/// `Filters` narrows the list to one kind of location: the custom ones a caller created,
/// or the Amazon Web Services Regions this store hosts in — and an unfiltered list holds
/// both.
#[tokio::test]
async fn the_filters_select_the_kind_of_location() {
    let gamelift = GameLift::new();
    gamelift
        .ok("CreateLocation", json!({"LocationName": "custom-filtered"}))
        .await;
    // A fleet deployed to a remote Region makes that Region a location this store hosts
    // in, beside the Region the requests are signed for.
    create_fleet(&gamelift, ec2_fleet("filter-home-fleet")).await;
    create_fleet(
        &gamelift,
        anywhere_fleet("filter-remote-fleet", &["eu-central-1", "ap-south-1"]),
    )
    .await;

    let rows: Vec<(&str, Value, Vec<&str>, Vec<&str>)> = vec![
        (
            "only the custom locations",
            json!({"Filters": ["CUSTOM"]}),
            vec!["custom-filtered"],
            vec![REGION, "eu-central-1", "ap-south-1"],
        ),
        (
            "only the Amazon Web Services locations",
            json!({"Filters": ["AWS"]}),
            vec![REGION, "eu-central-1", "ap-south-1"],
            vec!["custom-filtered"],
        ),
        (
            "both kinds, named explicitly",
            json!({"Filters": LOCATION_FILTERS}),
            vec![REGION, "eu-central-1", "ap-south-1", "custom-filtered"],
            vec![],
        ),
        (
            "no filter at all, which narrows nothing",
            json!({}),
            vec![REGION, "eu-central-1", "ap-south-1", "custom-filtered"],
            vec![],
        ),
    ];

    for (label, request, present, absent) in &rows {
        let listed = listed_names(&gamelift, request.clone()).await;
        for name in present {
            assert!(
                listed.iter().any(|listed| listed == name),
                "case {label:?}: {name} must be listed, got {listed:?}"
            );
        }
        for name in absent {
            assert!(
                !listed.iter().any(|listed| listed == name),
                "case {label:?}: {name} must not be listed, got {listed:?}"
            );
        }
    }

    // Only a custom location carries an ARN: "the ARN that is assigned to a custom
    // location resource".
    for location in list_locations(&gamelift, json!({})).await {
        let name = location["LocationName"].as_str().unwrap_or_default();
        let has_arn = location.get("LocationArn").is_some();
        assert_eq!(
            has_arn,
            name.starts_with("custom-"),
            "case {name:?}: only a custom location carries a LocationArn, got {location}"
        );
    }
}

/// `ListLocations` pages the collection: a `Limit` bounds the page, the `NextToken` it
/// hands back resumes exactly where that page ended, and a token this collection did not
/// issue is refused.
#[tokio::test]
async fn the_list_pages_the_collection_and_refuses_a_foreign_token() {
    let gamelift = GameLift::new();
    let names: Vec<String> = (0..5).map(|index| format!("custom-paged-{index}")).collect();
    for name in &names {
        gamelift
            .ok("CreateLocation", json!({"LocationName": name}))
            .await;
    }

    for page_size in [1_usize, 2, 5, 7] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut request = json!({"Filters": ["CUSTOM"], "Limit": page_size});
            if let Some(token) = &token {
                request["NextToken"] = json!(token);
            }
            let response = gamelift.ok("ListLocations", request).await;
            let page = response["Locations"].as_array().cloned().unwrap_or_default();
            assert!(
                page.len() <= page_size,
                "case page size {page_size}: a page must hold at most Limit locations, got {}",
                page.len()
            );
            walked.extend(page.iter().map(|location| {
                location["LocationName"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            }));
            match response["NextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            walked, names,
            "case page size {page_size}: the paged walk must report every location once, in \
             order, got {walked:?}"
        );
    }

    // A token minted for another collection is not an offset into this one.
    create_fleet(&gamelift, ec2_fleet("location-token-fleet")).await;
    let fleet_page = gamelift
        .ok("DescribeFleetAttributes", json!({"Limit": 1}))
        .await;
    let foreign = fleet_page["NextToken"].as_str().map(str::to_string);
    let mut tokens = vec!["not-a-token-this-service-issued".to_string()];
    tokens.extend(foreign);
    for token in tokens {
        let (status, code) = gamelift
            .err("ListLocations", json!({"NextToken": token}))
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidRequestException"),
            "case token {token:?}: a token this collection did not issue must be refused, got \
             {status} {code}"
        );
    }

    // `Limit` is modelled `ListLocationsLimit` with range 1..200: the bounds are the
    // model's, and one past each of them is refused by the boundary walk.
    let largest = gamelift
        .ok("ListLocations", json!({"Filters": ["CUSTOM"], "Limit": 200}))
        .await;
    assert_eq!(
        largest["Locations"].as_array().map(Vec::len),
        Some(names.len()),
        "a Limit at the modelled maximum must be served, got {largest}"
    );
    for limit in [0, 201] {
        let (status, code) = gamelift
            .err("ListLocations", json!({"Limit": limit}))
            .await;
        assert_eq!(
            status, 400,
            "case Limit {limit}: a Limit outside the modelled range must be refused with 400, \
             got {status} {code}"
        );
    }
}
