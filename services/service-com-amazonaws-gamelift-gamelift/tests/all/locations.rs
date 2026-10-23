// SPDX-License-Identifier: CC0-1.0

//! The custom-location contract, over the wire: `CreateLocation` persists the location
//! the request names, a name a location already holds is refused with
//! `ConflictException` — including when several creates of one name run at once, where
//! exactly ONE wins — and `ListLocations` reports each stored location exactly once,
//! beside the Amazon Web Services locations this store hosts in.
//!
//! Beside those, the three bounds a create is subject to, each asserted to store nothing
//! when it refuses: the modelled constraint on `LocationName`, the key-value-map rule its
//! `Tags` obey, and the account's custom-location quota.
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

/// The lowest and highest `LocationName` length a create may carry, from
/// `CustomInputLocationStringModel` (`smithy_query shape=CustomInputLocationStringModel`:
/// len 8..64).
const LOCATION_NAME_LENGTH: (usize, usize) = (8, 64);

/// The largest `Tags` list a create may carry, from `TagList` (`smithy_query
/// shape=TagList`: len 0..200).
const MAX_TAGS: usize = 200;

/// A name of exactly `length` characters that satisfies the modelled pattern
/// (`^custom-[A-Za-z0-9\-]+$`).
fn name_of_length(length: usize) -> String {
    format!("custom-{}", "a".repeat(length - "custom-".len()))
}

/// `count` tags, each carrying a distinct key, as `Tag` models them (`Key` len 1..128,
/// `Value` len 0..256).
fn tag_list(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| json!({"Key": format!("key-{index}"), "Value": format!("value-{index}")}))
            .collect(),
    )
}

/// Whether `ListLocations` reports a custom location named `name`.
async fn location_exists(gamelift: &GameLift, name: &str) -> bool {
    listed_names(gamelift, json!({"Filters": ["CUSTOM"], "Limit": 200}))
        .await
        .iter()
        .any(|listed| listed == name)
}

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

/// A create carries a `LocationName` the model accepts or it is refused and stores
/// nothing: the name's length sits inside `CustomInputLocationStringModel`'s published
/// bounds and its characters satisfy that shape's pattern.
///
/// The rows differ along what the constraint generalises over: the LENGTH, at each
/// published bound and one past it, and the FORM — the required `custom-` prefix, its
/// case, and the character classes the pattern allows and excludes.
#[tokio::test]
async fn a_create_takes_only_a_location_name_the_model_accepts() {
    let (shortest, longest) = LOCATION_NAME_LENGTH;
    struct Case {
        label: &'static str,
        name: Option<String>,
        /// The wire error code the create must answer with, or `None` when it is served.
        refused_with: Option<&'static str>,
    }

    let cases = vec![
        Case {
            label: "a name on the lowest published length bound",
            name: Some(name_of_length(shortest)),
            refused_with: None,
        },
        Case {
            label: "a name on the highest published length bound",
            name: Some(name_of_length(longest)),
            refused_with: None,
        },
        Case {
            label: "digits, upper case and inner hyphens, which the pattern allows",
            name: Some("custom-Rack-7-B".to_string()),
            refused_with: None,
        },
        Case {
            label: "the prefix alone: one character below the lowest published length bound, \
                    and nothing for the pattern's + to match",
            name: Some("custom-".to_string()),
            refused_with: Some("ValidationException"),
        },
        Case {
            label: "one character past the highest published length bound",
            name: Some(name_of_length(longest + 1)),
            refused_with: Some("ValidationException"),
        },
        Case {
            label: "an underscore, which the pattern's character class excludes",
            name: Some("custom-rack_7".to_string()),
            refused_with: Some("ValidationException"),
        },
        Case {
            label: "a space, which the pattern's character class excludes",
            name: Some("custom-rack 7".to_string()),
            refused_with: Some("ValidationException"),
        },
        Case {
            label: "no custom- prefix at all",
            name: Some("my-own-location".to_string()),
            refused_with: Some("ValidationException"),
        },
        Case {
            label: "the prefix in the wrong case",
            name: Some("Custom-rack-7".to_string()),
            refused_with: Some("ValidationException"),
        },
        Case {
            label: "no LocationName at all, which the model makes required",
            name: None,
            refused_with: Some("ValidationException"),
        },
    ];

    let gamelift = GameLift::new();
    for case in &cases {
        let label = case.label;
        let mut request = json!({});
        if let Some(name) = &case.name {
            request["LocationName"] = json!(name);
        }

        match case.refused_with {
            None => {
                let name = case.name.clone().expect("an accepted row names a location");
                let created = gamelift.ok("CreateLocation", request).await;
                assert_eq!(
                    created["Location"]["LocationName"],
                    json!(&name),
                    "case {label:?}: the create must report the stored location, got {}",
                    created["Location"]
                );
                assert!(
                    location_exists(&gamelift, &name).await,
                    "case {label:?}: the accepted location must be stored, got no {name} in the \
                     list"
                );
            }
            Some(expected) => {
                let (status, code) = gamelift.err("CreateLocation", request).await;
                assert_eq!(
                    (status, code.as_str()),
                    (400, expected),
                    "case {label:?}: a name outside the modelled constraint must be refused, got \
                     {status} {code}"
                );
                if let Some(name) = &case.name {
                    assert!(
                        !location_exists(&gamelift, name).await,
                        "case {label:?}: a refused create must store nothing, got {name} in the \
                         list"
                    );
                }
            }
        }
    }

    // Only the accepted rows are stored, so a refusal left the collection untouched.
    let accepted: usize = cases
        .iter()
        .filter(|case| case.refused_with.is_none())
        .count();
    let listed = listed_names(&gamelift, json!({"Filters": ["CUSTOM"], "Limit": 200})).await;
    assert_eq!(
        listed.len(),
        accepted,
        "exactly the accepted names must be stored, got {listed:?}"
    );
}

/// A resource's tags are a key-value MAP ("Tags are developer-defined key-value pairs"),
/// so a create that assigns one key twice is refused with the modelled
/// `TaggingFailedException` ("This may be due to invalid tag format") and stores nothing;
/// a list longer than `TagList`'s published maximum is refused one step earlier, by the
/// boundary walk.
///
/// The rows differ along where the repeated key sits and how many tags surround it, and
/// along which published bound of the list the row sits on.
#[tokio::test]
async fn a_create_whose_tags_are_not_a_key_value_map_is_refused_and_stores_nothing() {
    struct Case {
        label: &'static str,
        tags: Value,
        /// The wire error code the create must answer with, or `None` when it is served.
        refused_with: Option<&'static str>,
    }

    let mut repeated_among_many: Vec<Value> = (0..MAX_TAGS - 1)
        .map(|index| json!({"Key": format!("key-{index}"), "Value": format!("value-{index}")}))
        .collect();
    repeated_among_many.push(json!({"Key": "key-0", "Value": "again"}));

    let cases = [
        Case {
            label: "no tags at all, the lowest published bound of the list",
            tags: json!([]),
            refused_with: None,
        },
        Case {
            label: "one tag",
            tags: tag_list(1),
            refused_with: None,
        },
        Case {
            label: "a tag whose value is empty, which the model allows",
            tags: json!([{"Key": "empty-value", "Value": ""}]),
            refused_with: None,
        },
        Case {
            label: "two keys that differ only in case, which are two keys",
            tags: json!([{"Key": "stage", "Value": "test"}, {"Key": "Stage", "Value": "prod"}]),
            refused_with: None,
        },
        Case {
            label: "as many tags as the published maximum allows",
            tags: tag_list(MAX_TAGS),
            refused_with: None,
        },
        Case {
            label: "one key assigned two different values",
            tags: json!([{"Key": "stage", "Value": "test"}, {"Key": "stage", "Value": "prod"}]),
            refused_with: Some("TaggingFailedException"),
        },
        Case {
            label: "one key assigned the same value twice",
            tags: json!([{"Key": "stage", "Value": "test"}, {"Key": "stage", "Value": "test"}]),
            refused_with: Some("TaggingFailedException"),
        },
        Case {
            label: "a repeated key at the end of a full tag list",
            tags: Value::Array(repeated_among_many),
            refused_with: Some("TaggingFailedException"),
        },
        Case {
            label: "one tag past the published maximum",
            tags: tag_list(MAX_TAGS + 1),
            refused_with: Some("ValidationException"),
        },
    ];

    let gamelift = GameLift::new();
    for (index, case) in cases.iter().enumerate() {
        let label = case.label;
        let name = format!("custom-tag-row-{index}");
        let request = json!({"LocationName": &name, "Tags": case.tags.clone()});

        match case.refused_with {
            None => {
                let created = gamelift.ok("CreateLocation", request).await;
                assert_eq!(
                    created["Location"],
                    json!({"LocationName": &name, "LocationArn": location_arn(&name)}),
                    "case {label:?}: the create must report the stored location, got {}",
                    created["Location"]
                );
                assert!(
                    location_exists(&gamelift, &name).await,
                    "case {label:?}: the accepted location must be stored, got no {name} in the \
                     list"
                );
            }
            Some(expected) => {
                let (status, code) = gamelift.err("CreateLocation", request).await;
                assert_eq!(
                    (status, code.as_str()),
                    (400, expected),
                    "case {label:?}: the create must be refused, got {status} {code}"
                );
                assert!(
                    !location_exists(&gamelift, &name).await,
                    "case {label:?}: a refused create must store nothing, got {name} in the list"
                );
            }
        }
    }
}

/// The account's custom-location quota bounds the stored collection: once it is reached
/// the next create answers the modelled `LimitExceededException` and stores nothing,
/// while a create of a name that is already taken stays the `ConflictException` it is at
/// any size of the collection.
#[tokio::test]
async fn the_custom_location_quota_is_the_bound_on_the_stored_collection() {
    let quota = service_com_amazonaws_gamelift_gamelift::state::MAX_CUSTOM_LOCATIONS_PER_ACCOUNT;
    let gamelift = GameLift::new();
    for index in 0..quota {
        gamelift
            .ok(
                "CreateLocation",
                json!({"LocationName": format!("custom-quota-{index}")}),
            )
            .await;
    }

    let (status, code) = gamelift
        .err(
            "CreateLocation",
            json!({"LocationName": "custom-one-too-many"}),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "LimitExceededException"),
        "a create past the quota must be refused, got {status} {code}"
    );

    // A name that is taken conflicts whatever the size of the collection: the uniqueness
    // of the name is decided before the quota is.
    let (status, code) = gamelift
        .err("CreateLocation", json!({"LocationName": "custom-quota-0"}))
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ConflictException"),
        "a name that is taken must conflict even at the quota, got {status} {code}"
    );

    // Neither refused create changed the collection: it still holds exactly the
    // locations that filled the quota, each once.
    let mut listed = listed_names(&gamelift, json!({"Filters": ["CUSTOM"], "Limit": 200})).await;
    listed.sort();
    let mut expected: Vec<String> = (0..quota)
        .map(|index| format!("custom-quota-{index}"))
        .collect();
    expected.sort();
    assert_eq!(
        listed, expected,
        "the refused creates must leave the collection as it was, got {} locations",
        listed.len()
    );
}
