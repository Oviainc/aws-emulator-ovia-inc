// SPDX-License-Identifier: CC0-1.0

//! The MediaLive eventbridge rule template GROUP slice, end to end over the
//! restJson1 wire.
//!
//! Every test drives the service through the shared `wire` harness — real HTTP
//! requests dispatched through `MultiServiceRouter` — so what is asserted is the
//! emulator's wire behaviour, not an internal function's return value.
//!
//! The suites are tables: the recorded case in the issue is ONE row of each.
//! The region is a COLUMN here, because a group's name is unique per region, so
//! these tests sign through the harness's `call_in_region`.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// `timestamp` reads a wire timestamp in either rendering the restJson1 codec
// may emit, so every group suite asserts the INSTANT the service reports rather
// than the serializer's choice of form. One copy, in `support`.
use crate::support::timestamp;
use crate::wire::{call_in_region, error_code, medialive, ACCOUNT, DEFAULT_REGION};

/// `CreateEventBridgeRuleTemplateGroup` / `ListEventBridgeRuleTemplateGroups`
/// URI, from `smithy_query(query="operation",
/// operation="CreateEventBridgeRuleTemplateGroup")`.
const GROUPS: &str = "/prod/eventbridge-rule-template-groups";

/// The bounds the input members' target shapes name:
/// `Name` is `__stringMin1Max255PatternS`, `Description` is
/// `__stringMin0Max1024` and `RequestId` is `__stringMin1Max256PatternS`
/// (`smithy_query(query="operation",
/// operation="CreateEventBridgeRuleTemplateGroup")`).
const MAX_NAME: usize = 255;
const MAX_DESCRIPTION: usize = 1024;
const MAX_REQUEST_ID: usize = 256;

async fn create(router: &MultiServiceRouter, region: &str, body: Value) -> (u16, Value) {
    call_in_region(router, region, "POST", GROUPS, Some(body)).await
}

async fn get(router: &MultiServiceRouter, region: &str, identifier: &str) -> (u16, Value) {
    call_in_region(
        router,
        region,
        "GET",
        &format!("{GROUPS}/{identifier}"),
        None,
    )
    .await
}

/// Rule: a create mints the resource's id, builds its ARN from the request's
/// region and that id, stores every member the request supplied verbatim, and
/// the reporting operation answers that stored record for either identifier the
/// model allows (`Identifier` is documented "Can be either be its id or current
/// name").
///
/// The rows differ along which optional members the request carries, along their
/// type (string, map) and along their size — up to the bounds the target shapes
/// name — because that is what the rule generalises over. Member names come from
/// `smithy_query(query="operation",
/// operation="CreateEventBridgeRuleTemplateGroup")`.
#[tokio::test]
async fn a_created_group_is_read_back_by_its_id_and_by_its_name() {
    let longest_name = "n".repeat(MAX_NAME);
    let longest_description = "d".repeat(MAX_DESCRIPTION);
    let rows: [(&str, Value); 7] = [
        ("name only", json!({ "name": "group-plain" })),
        (
            "name and description",
            json!({ "name": "group-described", "description": "nightly eventbridge rules" }),
        ),
        (
            "one tag",
            json!({ "name": "group-tagged", "tags": { "env": "test" } }),
        ),
        (
            "several tags",
            json!({
                "name": "group-tagged-more",
                "description": "",
                "tags": { "env": "test", "team": "video", "owner": "emulator" },
            }),
        ),
        (
            "non-ascii description",
            json!({ "name": "group-unicode", "description": "règles de nuit — 夜" }),
        ),
        ("shortest name", json!({ "name": "g" })),
        (
            "longest name and description",
            json!({ "name": longest_name, "description": longest_description }),
        ),
    ];

    for (case, request) in rows {
        let router = medialive();
        let region = DEFAULT_REGION;
        let name = request["name"].as_str().unwrap_or_default().to_string();

        let (status, created) = create(&router, region, request.clone()).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from CreateEventBridgeRuleTemplateGroup, got {status}: {created}"
        );

        let id = created["id"].as_str().unwrap_or_default().to_string();
        assert!(
            id.len() == 7 && id.chars().all(|c| c.is_ascii_digit()),
            "case {case:?}: expected a seven-digit group id, got {:?}",
            created["id"]
        );
        assert_eq!(
            created["arn"],
            json!(format!(
                "arn:aws:medialive:{region}:{ACCOUNT}:eventbridge-rule-template-group:{id}"
            )),
            "case {case:?}: expected the ARN built from the signed region and the minted id, got {:?}",
            created["arn"]
        );
        assert_eq!(
            created["name"],
            json!(name),
            "case {case:?}: expected the stored name, got {:?}",
            created["name"]
        );
        assert_eq!(
            created["description"], request["description"],
            "case {case:?}: expected the description the request supplied, got {:?}",
            created["description"]
        );
        assert_eq!(
            created["tags"], request["tags"],
            "case {case:?}: expected the tags the request supplied, got {:?}",
            created["tags"]
        );
        assert_eq!(
            timestamp(case, "createdAt", &created),
            timestamp(case, "modifiedAt", &created),
            "case {case:?}: expected a group that was never modified to report modifiedAt == createdAt, got {created}"
        );

        // The round trip: both identifiers the model allows answer the SAME
        // stored record the create reported.
        let (status, by_id) = get(&router, region, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 reading the group by id, got {status}: {by_id}"
        );
        assert_eq!(
            by_id, created,
            "case {case:?}: expected the stored group read back by id, got {by_id}"
        );

        let (status, by_name) = get(&router, region, &name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 reading the group by name, got {status}: {by_name}"
        );
        assert_eq!(
            by_name, created,
            "case {case:?}: expected the stored group read back by name, got {by_name}"
        );
    }
}

/// Rule: a resource's name is unique "within the scope of a resource type in a
/// specific region" (the `Name` member's documentation), so a create naming a
/// name that region already holds is refused with the operation's modelled
/// `ConflictException` and creates nothing, while the same name in another
/// region names a different resource.
///
/// The rows differ along the form of the name, because the rule generalises over
/// the value, and along the region pair, because the rule's scope is the region.
#[tokio::test]
async fn a_group_name_is_unique_within_one_region() {
    // (case, name, first region, second region)
    let rows: [(&str, &str, &str, &str); 4] = [
        ("plain name, same region", "duplicated", "us-west-2", "us-west-2"),
        ("dotted name, same region", "dup.with.dots-1", "eu-west-1", "eu-west-1"),
        ("plain name, other region", "duplicated", "us-west-2", "eu-west-1"),
        ("dotted name, other region", "dup.with.dots-1", "ap-south-1", "us-east-1"),
    ];

    for (case, name, first_region, second_region) in rows {
        let router = medialive();
        let (status, first) = create(&router, first_region, json!({ "name": name })).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected the first create to succeed, got {status}: {first}"
        );
        let first_id = first["id"].as_str().unwrap_or_default().to_string();

        let (status, second) = create(&router, second_region, json!({ "name": name })).await;
        if first_region == second_region {
            assert_eq!(
                status, 409,
                "case {case:?}: expected a name {first_region} already holds to conflict, got {status}: {second}"
            );
            assert_eq!(
                error_code(status, &second),
                "ConflictException",
                "case {case:?}: expected ConflictException, got {second}"
            );
            // The refusal created nothing: the name still names the first group.
            let (status, by_name) = get(&router, first_region, name).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the first group to survive the refusal, got {status}: {by_name}"
            );
            assert_eq!(
                by_name["id"],
                json!(first_id),
                "case {case:?}: expected the name to still name the first group, got {:?}",
                by_name["id"]
            );
        } else {
            assert_eq!(
                status, 201,
                "case {case:?}: expected the same name in {second_region} to be free, got {status}: {second}"
            );
            let second_id = second["id"].as_str().unwrap_or_default().to_string();
            assert_ne!(
                first_id, second_id,
                "case {case:?}: expected two regions' groups to have different ids, got {second_id} twice"
            );
            // Each region reports its own group, and only its own.
            for (region, expected) in [(first_region, &first_id), (second_region, &second_id)] {
                let (status, by_name) = get(&router, region, name).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected {region} to hold the group named {name}, got {status}: {by_name}"
                );
                assert_eq!(
                    by_name["id"],
                    json!(expected),
                    "case {case:?}: expected {region} to report its own group {expected}, got {:?}",
                    by_name["id"]
                );
            }
        }
    }
}

/// Rule: a `RequestId` (`@idempotencyToken`) "ensures idempotency when creating
/// resources", so every create repeating a token answers the resource that token
/// already created and mints no second one.
///
/// The rows differ along the token's form and length — up to the bound
/// `__stringMin1Max256PatternS` names — and along how many times the create is
/// retried, because that is what the rule generalises over.
#[tokio::test]
async fn a_retried_create_returns_the_group_its_request_id_already_created() {
    let longest_token = "t".repeat(MAX_REQUEST_ID);
    // (case, token, retries)
    let rows: [(&str, &str, usize); 4] = [
        ("uuid token", "4f6c6d6e-0000-4000-8000-0123456789ab", 1),
        ("short token", "r", 1),
        ("longest token", longest_token.as_str(), 2),
        ("many retries", "retry-token", 5),
    ];

    for (case, token, retries) in rows {
        let router = medialive();
        let region = DEFAULT_REGION;
        let request = json!({
            "name": "idempotent-group",
            "description": "created once",
            "requestId": token,
        });

        let (status, first) = create(&router, region, request.clone()).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected the first create to succeed, got {status}: {first}"
        );

        for attempt in 1..=retries {
            let (status, retried) = create(&router, region, request.clone()).await;
            assert_eq!(
                status, 201,
                "case {case:?}: retry {attempt}: expected 201, got {status}: {retried}"
            );
            assert_eq!(
                retried, first,
                "case {case:?}: retry {attempt}: expected the group the token already created, got {retried}"
            );
        }

        // Nothing else was minted: the name is still taken, by that one group.
        let (status, conflict) = create(
            &router,
            region,
            json!({ "name": "idempotent-group", "requestId": "a-different-token" }),
        )
        .await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected a fresh token reusing the name to conflict, got {status}: {conflict}"
        );
    }
}

/// Rule: an `@idempotencyToken` is idempotent for the regional endpoint it was
/// presented to, so one token presented in several regions creates one resource
/// in each and every retry resolves the resource its own region's call created.
///
/// The rows are the regions the one token is presented in; nothing about them
/// matters except that they differ.
#[tokio::test]
async fn one_request_id_is_idempotent_within_each_region() {
    let regions = ["us-west-2", "eu-central-1", "ap-south-1"];
    let token = "token-shared-across-regions";
    let name = "same-name-in-every-region";

    let router = medialive();
    let mut created: Vec<Value> = Vec::new();

    for region in regions {
        let (status, group) = create(
            &router,
            region,
            json!({ "name": name, "requestId": token }),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {region}: expected the create to succeed, got {status}: {group}"
        );
        assert!(
            !created.iter().any(|earlier| earlier["id"] == group["id"]),
            "case {region}: expected a group of its own, got {:?} which another region already has",
            group["id"]
        );
        created.push(group);
    }

    for (region, expected) in regions.iter().zip(&created) {
        let (status, retried) = create(
            &router,
            region,
            json!({ "name": name, "requestId": token }),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {region} retry: expected 201, got {status}: {retried}"
        );
        assert_eq!(
            &retried, expected,
            "case {region} retry: expected the group this region's token created, got {retried}"
        );
    }
}

/// Rule: a reporting operation answers its modelled `NotFoundException` for an
/// identifier no resource in the request's region has, and reports nothing about
/// another region's resources.
///
/// The rows differ along the FORM of the miss: an id nothing minted, a name
/// nothing used, and the id and the name of a group that exists in a different
/// region.
#[tokio::test]
async fn an_identifier_no_group_in_the_region_has_is_not_found() {
    let router = medialive();
    let (status, elsewhere) = create(&router, "eu-west-1", json!({ "name": "far-away" })).await;
    assert_eq!(
        status, 201,
        "setup: expected the eu-west-1 group to be created, got {status}: {elsewhere}"
    );
    let far_id = elsewhere["id"].as_str().unwrap_or_default().to_string();

    let rows: [(&str, &str); 4] = [
        ("unminted id", "9999999"),
        ("unused name", "no-such-group"),
        ("other region's id", far_id.as_str()),
        ("other region's name", "far-away"),
    ];

    for (case, identifier) in rows {
        let (status, body) = get(&router, "us-west-2", identifier).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 for identifier {identifier:?}, got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {body}"
        );
    }
}

/// Rule: a member whose length falls outside the bounds its target shape names
/// is refused with a 400 and nothing is created; a member AT either bound is
/// accepted.
///
/// The rows walk each published bound and one past it: `Name` is
/// `__stringMin1Max255PatternS`, `Description` is `__stringMin0Max1024` and
/// `RequestId` is `__stringMin1Max256PatternS`
/// (`smithy_query(query="operation",
/// operation="CreateEventBridgeRuleTemplateGroup")`).
#[tokio::test]
async fn a_member_outside_the_bounds_its_shape_names_is_refused() {
    let at_name_bound = "n".repeat(MAX_NAME);
    let past_name_bound = "n".repeat(MAX_NAME + 1);
    let at_description_bound = "d".repeat(MAX_DESCRIPTION);
    let past_description_bound = "d".repeat(MAX_DESCRIPTION + 1);
    let at_request_id_bound = "t".repeat(MAX_REQUEST_ID);
    let past_request_id_bound = "t".repeat(MAX_REQUEST_ID + 1);

    // (case, request, expected status)
    let rows: [(&str, Value, u16); 9] = [
        ("name below the minimum", json!({ "name": "" }), 400),
        ("name at the minimum", json!({ "name": "n" }), 201),
        ("name at the maximum", json!({ "name": at_name_bound }), 201),
        ("name past the maximum", json!({ "name": past_name_bound }), 400),
        (
            "description at the maximum",
            json!({ "name": "described", "description": at_description_bound }),
            201,
        ),
        (
            "description past the maximum",
            json!({ "name": "described", "description": past_description_bound }),
            400,
        ),
        (
            "request id below the minimum",
            json!({ "name": "tokened", "requestId": "" }),
            400,
        ),
        (
            "request id at the maximum",
            json!({ "name": "tokened", "requestId": at_request_id_bound }),
            201,
        ),
        (
            "request id past the maximum",
            json!({ "name": "tokened", "requestId": past_request_id_bound }),
            400,
        ),
    ];

    for (case, request, expected) in rows {
        let router = medialive();
        let region = DEFAULT_REGION;
        let (status, body) = create(&router, region, request).await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected {expected}, got {status}: {body}"
        );

        if expected == 400 {
            let code = error_code(status, &body);
            assert!(
                code == "BadRequestException" || code == "ValidationException",
                "case {case:?}: expected the request to be refused as a client error, got {code} ({body})"
            );
            // The refusal created nothing, so no identifier it named exists.
            let (status, listed) = get(&router, region, "1000000").await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected the refused create to mint nothing, got {status}: {listed}"
            );
        }
    }
}
