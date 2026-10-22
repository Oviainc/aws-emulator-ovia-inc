// SPDX-License-Identifier: CC0-1.0

//! The MediaLive eventbridge rule TEMPLATE slice, end to end over the restJson1
//! wire.
//!
//! Every test drives the service through the shared `wire` harness — real HTTP
//! requests dispatched through `MultiServiceRouter` — so what is asserted is the
//! emulator's wire behaviour, not an internal function's return value.
//!
//! The suites are tables: the recorded case in the issue is ONE row of each. The
//! region is a COLUMN where it matters, because a template's name is unique per
//! region, so these tests sign through the harness's `call_in_region`.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{member, reported_tags, timestamp};
use crate::wire::{call_in_region, error_code, medialive, ACCOUNT, DEFAULT_REGION};

/// `CreateEventBridgeRuleTemplate` / `ListEventBridgeRuleTemplates` URI, and the
/// `GetEventBridgeRuleTemplate` collection it reads from
/// (`smithy_query(query="operation", operation="CreateEventBridgeRuleTemplate")`:
/// POST /prod/eventbridge-rule-templates).
const TEMPLATES: &str = "/prod/eventbridge-rule-templates";

/// The group collection `CreateEventBridgeRuleTemplateGroup` is bound to, which
/// every template here is created inside
/// (`smithy_query(query="operation",
/// operation="CreateEventBridgeRuleTemplateGroup")`).
const GROUPS: &str = "/prod/eventbridge-rule-template-groups";

/// The bounds the input members' target shapes name: `Name` is
/// `__stringMin1Max255PatternS`, `Description` is `__stringMin0Max1024`,
/// `RequestId` is `__stringMin1Max256PatternS` and an `EventTargets` entry's
/// `Arn` is `__stringMin1Max2048PatternArn` (`smithy_query(query="operation",
/// operation="CreateEventBridgeRuleTemplate")`,
/// `smithy_query(query="shape", shape="EventBridgeRuleTemplateTarget")`).
const MAX_NAME: usize = 255;
const MAX_DESCRIPTION: usize = 1024;
const MAX_REQUEST_ID: usize = 256;
const MAX_TARGET_ARN: usize = 2048;

/// The most targets a template may carry:
/// `EventBridgeRuleTemplateSummary$EventTargetCount` ("The number of targets
/// configured to send matching events") targets `__integerMax5`
/// (`smithy_query(query="shape", shape="EventBridgeRuleTemplateSummary")`).
const MAX_TARGETS: usize = 5;

/// Every `EventBridgeRuleTemplateEventType` the model declares
/// (`smithy_query(query="shape", shape="EventBridgeRuleTemplateEventType")`).
/// The rows below test values the issue did not cite, not only the recorded one.
const EVENT_TYPES: [&str; 13] = [
    "MEDIALIVE_MULTIPLEX_ALERT",
    "MEDIALIVE_MULTIPLEX_STATE_CHANGE",
    "MEDIALIVE_CHANNEL_ALERT",
    "MEDIALIVE_CHANNEL_INPUT_CHANGE",
    "MEDIALIVE_CHANNEL_STATE_CHANGE",
    "MEDIAPACKAGE_INPUT_NOTIFICATION",
    "MEDIAPACKAGE_KEY_PROVIDER_NOTIFICATION",
    "MEDIAPACKAGE_HARVEST_JOB_NOTIFICATION",
    "SIGNAL_MAP_ACTIVE_ALARM",
    "MEDIACONNECT_ALERT",
    "MEDIACONNECT_SOURCE_HEALTH",
    "MEDIACONNECT_OUTPUT_HEALTH",
    "MEDIACONNECT_FLOW_STATUS_CHANGE",
];

async fn create_group(router: &MultiServiceRouter, region: &str, name: &str) -> Value {
    let (status, group) = call_in_region(
        router,
        region,
        "POST",
        GROUPS,
        Some(json!({ "name": name })),
    )
    .await;
    assert_eq!(
        status, 201,
        "setup: expected the group {name:?} in {region} to be created, got {status}: {group}"
    );
    group
}

async fn create(router: &MultiServiceRouter, region: &str, body: Value) -> (u16, Value) {
    call_in_region(router, region, "POST", TEMPLATES, Some(body)).await
}

async fn get(router: &MultiServiceRouter, region: &str, identifier: &str) -> (u16, Value) {
    call_in_region(
        router,
        region,
        "GET",
        &format!("{TEMPLATES}/{identifier}"),
        None,
    )
    .await
}

/// An SNS topic ARN of `length` characters, which is one of the two ARN kinds
/// `EventBridgeRuleTemplateTarget$Arn` documents ("Target ARNs must be either an
/// SNS topic or CloudWatch log group").
fn sns_arn(length: usize) -> String {
    let prefix = format!("arn:aws:sns:{DEFAULT_REGION}:{ACCOUNT}:");
    let padding = length.saturating_sub(prefix.len());
    format!("{prefix}{}", "t".repeat(padding))
}

/// Rule: a create mints the resource's id, builds its ARN from the request's
/// region and that id, resolves its parent by the identifier the model allows,
/// stores every member the request supplied verbatim, and the reporting operation
/// answers that stored record for either identifier the model allows
/// (`Identifier` is documented "Can be either be its id or current name").
///
/// The rows differ along which optional members the request carries, along their
/// type (string, list, map), along the NUMBER of targets — up to the bound the
/// model's `EventTargetCount` names — along the `EventType` value, and along the
/// form of the `GroupIdentifier` ("Can be either be its id or current name"),
/// because that is what the rule generalises over.
#[tokio::test]
async fn a_created_template_is_read_back_by_its_id_and_by_its_name() {
    let longest_name = "n".repeat(MAX_NAME);
    let longest_description = "d".repeat(MAX_DESCRIPTION);
    let longest_arn = sns_arn(MAX_TARGET_ARN);
    let five_targets: Vec<Value> = (0..MAX_TARGETS)
        .map(|index| json!({ "arn": format!("arn:aws:sns:{DEFAULT_REGION}:{ACCOUNT}:topic-{index}") }))
        .collect();

    // (case, by group name rather than id, the request's members besides the
    // group identifier)
    let rows: [(&str, bool, Value); 8] = [
        (
            "required members only",
            false,
            json!({ "name": "tpl-plain", "eventType": EVENT_TYPES[2] }),
        ),
        (
            "group named rather than identified",
            true,
            json!({ "name": "tpl-by-group-name", "eventType": EVENT_TYPES[0] }),
        ),
        (
            "one target and a description",
            false,
            json!({
                "name": "tpl-one-target",
                "eventType": EVENT_TYPES[8],
                "description": "page the on-call",
                "eventTargets": [{ "arn": format!("arn:aws:sns:{DEFAULT_REGION}:{ACCOUNT}:alarms") }],
            }),
        ),
        (
            "a CloudWatch log group target",
            false,
            json!({
                "name": "tpl-log-target",
                "eventType": EVENT_TYPES[12],
                "eventTargets": [{ "arn": format!("arn:aws:logs:{DEFAULT_REGION}:{ACCOUNT}:log-group:/aws/medialive") }],
            }),
        ),
        (
            "the most targets the model can count",
            false,
            json!({
                "name": "tpl-five-targets",
                "eventType": EVENT_TYPES[10],
                "eventTargets": five_targets,
            }),
        ),
        (
            "tags and an empty description",
            false,
            json!({
                "name": "tpl-tagged",
                "eventType": EVENT_TYPES[5],
                "description": "",
                "tags": { "env": "test", "team": "video" },
            }),
        ),
        (
            "non-ascii description",
            false,
            json!({ "name": "tpl-unicode", "eventType": EVENT_TYPES[7], "description": "règles — 夜" }),
        ),
        (
            "longest name, description and target ARN",
            false,
            json!({
                "name": longest_name,
                "eventType": EVENT_TYPES[1],
                "description": longest_description,
                "eventTargets": [{ "arn": longest_arn }],
            }),
        ),
    ];

    for (case, by_group_name, members) in rows {
        let router = medialive();
        let region = DEFAULT_REGION;
        let group = create_group(&router, region, "parent-group").await;
        let group_id = group["id"].as_str().unwrap_or_default().to_string();
        let group_identifier = if by_group_name {
            "parent-group".to_string()
        } else {
            group_id.clone()
        };

        let mut request = members.clone();
        request["groupIdentifier"] = json!(group_identifier);
        let name = request["name"].as_str().unwrap_or_default().to_string();

        let (status, created) = create(&router, region, request.clone()).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from CreateEventBridgeRuleTemplate, got {status}: {created}"
        );

        let id = created["id"].as_str().unwrap_or_default().to_string();
        assert!(
            id.len() == 7 && id.chars().all(|c| c.is_ascii_digit()),
            "case {case:?}: expected a seven-digit template id, got {:?}",
            created["id"]
        );
        assert_ne!(
            id, group_id,
            "case {case:?}: expected the template's id to be its own, got the group's id {group_id}"
        );
        assert_eq!(
            created["arn"],
            json!(format!(
                "arn:aws:medialive:{region}:{ACCOUNT}:eventbridge-rule-template:{id}"
            )),
            "case {case:?}: expected the ARN built from the signed region and the minted id, got {:?}",
            created["arn"]
        );
        assert_eq!(
            created["groupId"],
            json!(group_id),
            "case {case:?}: expected the id of the group the identifier {group_identifier:?} names, got {:?}",
            created["groupId"]
        );
        assert_eq!(
            created["name"],
            json!(name),
            "case {case:?}: expected the stored name, got {:?}",
            created["name"]
        );
        assert_eq!(
            created["eventType"], request["eventType"],
            "case {case:?}: expected the eventType the request supplied, got {:?}",
            created["eventType"]
        );
        assert_eq!(
            member(&created, "description"),
            member(&request, "description"),
            "case {case:?}: expected the description the request supplied, got {:?}",
            created["description"]
        );
        // `Tags` is always reported, as the map it is modelled as: a template
        // created with no tags reports the empty map.
        assert_eq!(
            member(&created, "tags"),
            reported_tags(request.get("tags")),
            "case {case:?}: expected the tags the request supplied, got {:?}",
            created["tags"]
        );
        assert_eq!(
            member(&created, "eventTargets"),
            member(&request, "eventTargets"),
            "case {case:?}: expected the targets the request listed, in order, got {:?}",
            created["eventTargets"]
        );
        assert_eq!(
            timestamp(case, "createdAt", &created),
            timestamp(case, "modifiedAt", &created),
            "case {case:?}: expected a template that was never modified to report modifiedAt == createdAt, got {created}"
        );

        // The round trip: both identifiers the model allows answer the SAME
        // stored record the create reported.
        let (status, by_id) = get(&router, region, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 reading the template by id, got {status}: {by_id}"
        );
        assert_eq!(
            by_id, created,
            "case {case:?}: expected the stored template read back by id, got {by_id}"
        );

        let (status, by_name) = get(&router, region, &name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 reading the template by name, got {status}: {by_name}"
        );
        assert_eq!(
            by_name, created,
            "case {case:?}: expected the stored template read back by name, got {by_name}"
        );
    }
}

/// Rule: every value of a closed enum member is accepted and reported back
/// unchanged.
///
/// The rows are the whole published catalog of
/// `EventBridgeRuleTemplateEventType`, so the values the issue never cited are
/// covered too.
#[tokio::test]
async fn every_modelled_event_type_is_accepted_and_reported_back() {
    let router = medialive();
    let region = DEFAULT_REGION;
    let group = create_group(&router, region, "all-event-types").await;
    let group_identifier = group["id"].as_str().unwrap_or_default().to_string();

    for event_type in EVENT_TYPES {
        let (status, created) = create(
            &router,
            region,
            json!({
                "name": format!("tpl-{event_type}"),
                "eventType": event_type,
                "groupIdentifier": group_identifier,
            }),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {event_type:?}: expected 201, got {status}: {created}"
        );
        assert_eq!(
            created["eventType"],
            json!(event_type),
            "case {event_type:?}: expected the event type stored, got {:?}",
            created["eventType"]
        );

        let id = created["id"].as_str().unwrap_or_default().to_string();
        let (status, read) = get(&router, region, &id).await;
        assert_eq!(
            status, 200,
            "case {event_type:?}: expected to read the template back, got {status}: {read}"
        );
        assert_eq!(
            read["eventType"],
            json!(event_type),
            "case {event_type:?}: expected the stored event type on the read, got {:?}",
            read["eventType"]
        );
    }
}

/// Rule: a resource's name is unique "within the scope of a resource type in a
/// specific region" (the `Name` member's documentation), so a create naming a
/// name that region already holds is refused with the operation's modelled
/// `ConflictException` and creates nothing, while the same name in another region
/// names a different resource.
///
/// The rows differ along the form of the name, because the rule generalises over
/// the value, along the region pair, because the rule's scope is the region, and
/// along whether the second create targets the same GROUP, because the scope the
/// documentation names is the resource type in the region rather than one group
/// of it.
#[tokio::test]
async fn a_template_name_is_unique_within_one_region() {
    // (case, name, first region, second region, second group)
    let rows: [(&str, &str, &str, &str, &str); 5] = [
        ("plain name, same group", "duplicated", "us-west-2", "us-west-2", "first-group"),
        ("dotted name, same group", "dup.with.dots-1", "eu-west-1", "eu-west-1", "first-group"),
        ("plain name, other group", "duplicated", "us-west-2", "us-west-2", "second-group"),
        ("plain name, other region", "duplicated", "us-west-2", "eu-west-1", "first-group"),
        ("dotted name, other region", "dup.with.dots-1", "ap-south-1", "us-east-1", "first-group"),
    ];

    for (case, name, first_region, second_region, second_group) in rows {
        let router = medialive();
        // One group pair per region: when both creates are in the same region
        // that is one region, not two.
        let regions: Vec<&str> = if first_region == second_region {
            vec![first_region]
        } else {
            vec![first_region, second_region]
        };
        for region in regions {
            create_group(&router, region, "first-group").await;
            create_group(&router, region, "second-group").await;
        }

        let (status, first) = create(
            &router,
            first_region,
            json!({
                "name": name,
                "eventType": EVENT_TYPES[4],
                "groupIdentifier": "first-group",
            }),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected the first create to succeed, got {status}: {first}"
        );
        let first_id = first["id"].as_str().unwrap_or_default().to_string();

        let (status, second) = create(
            &router,
            second_region,
            json!({
                "name": name,
                "eventType": EVENT_TYPES[4],
                "groupIdentifier": second_group,
            }),
        )
        .await;

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
            // The refusal created nothing: the name still names the first
            // template.
            let (status, by_name) = get(&router, first_region, name).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the first template to survive the refusal, got {status}: {by_name}"
            );
            assert_eq!(
                by_name["id"],
                json!(first_id),
                "case {case:?}: expected the name to still name the first template, got {:?}",
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
                "case {case:?}: expected two regions' templates to have different ids, got {second_id} twice"
            );
            for (region, expected) in [(first_region, &first_id), (second_region, &second_id)] {
                let (status, by_name) = get(&router, region, name).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected {region} to hold the template named {name}, got {status}: {by_name}"
                );
                assert_eq!(
                    by_name["id"],
                    json!(expected),
                    "case {case:?}: expected {region} to report its own template {expected}, got {:?}",
                    by_name["id"]
                );
            }
        }
    }
}

/// Rule: a `RequestId` (`@idempotencyToken`) "ensures idempotency when creating
/// resources", so every create repeating a token answers the resource that token
/// already created and mints no second one; and a token is idempotent for the
/// regional endpoint it was presented to, so one token presented in several
/// regions creates one resource in each.
///
/// The rows differ along the token's form and length — up to the bound
/// `__stringMin1Max256PatternS` names — and along how many times the create is
/// retried, because that is what the rule generalises over.
#[tokio::test]
async fn a_retried_create_returns_the_template_its_request_id_already_created() {
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
        create_group(&router, region, "idempotent-group").await;
        let request = json!({
            "name": "idempotent-template",
            "eventType": EVENT_TYPES[3],
            "groupIdentifier": "idempotent-group",
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
                "case {case:?}: retry {attempt}: expected the template the token already created, got {retried}"
            );
        }

        // Nothing else was minted: the name is still taken, by that one
        // template.
        let (status, conflict) = create(
            &router,
            region,
            json!({
                "name": "idempotent-template",
                "eventType": EVENT_TYPES[3],
                "groupIdentifier": "idempotent-group",
                "requestId": "a-different-token",
            }),
        )
        .await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected a fresh token reusing the name to conflict, got {status}: {conflict}"
        );
    }
}

/// Rule: one `@idempotencyToken` presented in several regions creates one
/// resource in each, and every retry resolves the resource its own region's call
/// created.
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
        create_group(&router, region, "shared-group").await;
        let request = json!({
            "name": name,
            "eventType": EVENT_TYPES[6],
            "groupIdentifier": "shared-group",
            "requestId": token,
        });
        let (status, template) = create(&router, region, request).await;
        assert_eq!(
            status, 201,
            "case {region}: expected the create to succeed, got {status}: {template}"
        );
        assert!(
            !created
                .iter()
                .any(|earlier| earlier["id"] == template["id"]),
            "case {region}: expected a template of its own, got {:?} which another region already has",
            template["id"]
        );
        created.push(template);
    }

    for (region, expected) in regions.iter().zip(&created) {
        let (status, retried) = create(
            &router,
            region,
            json!({
                "name": name,
                "eventType": EVENT_TYPES[6],
                "groupIdentifier": "shared-group",
                "requestId": token,
            }),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {region} retry: expected 201, got {status}: {retried}"
        );
        assert_eq!(
            &retried, expected,
            "case {region} retry: expected the template this region's token created, got {retried}"
        );
    }
}

/// Rule: a create whose parent identifier names no resource of the parent's type
/// in the request's region is refused with the operation's modelled
/// `NotFoundException` and creates nothing.
///
/// The rows differ along the FORM of the miss: an id nothing minted, a name
/// nothing used, the empty identifier, and the id and the name of a group that
/// exists in a DIFFERENT region.
#[tokio::test]
async fn a_group_identifier_no_group_in_the_region_has_is_not_found() {
    let router = medialive();
    let elsewhere = create_group(&router, "eu-west-1", "far-away-group").await;
    let far_id = elsewhere["id"].as_str().unwrap_or_default().to_string();

    let rows: [(&str, &str); 4] = [
        ("unminted group id", "9999999"),
        ("unused group name", "no-such-group"),
        ("other region's group id", far_id.as_str()),
        ("other region's group name", "far-away-group"),
    ];

    for (case, identifier) in rows {
        let (status, body) = create(
            &router,
            "us-west-2",
            json!({
                "name": "orphan",
                "eventType": EVENT_TYPES[9],
                "groupIdentifier": identifier,
            }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 for group identifier {identifier:?}, got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {body}"
        );

        // The refusal created nothing: the name it asked for is still free.
        let (status, read) = get(&router, "us-west-2", "orphan").await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected the refused create to store nothing, got {status}: {read}"
        );
    }
}

/// Rule: a reporting operation answers its modelled `NotFoundException` for an
/// identifier no resource in the request's region has, and reports nothing about
/// another region's resources.
///
/// The rows differ along the FORM of the miss: an id nothing minted, a name
/// nothing used, the id and the name of a template that exists in a different
/// region, and the id of the template's own GROUP (a different resource type).
#[tokio::test]
async fn an_identifier_no_template_in_the_region_has_is_not_found() {
    let router = medialive();
    let group = create_group(&router, "eu-west-1", "far-away-group").await;
    let group_id = group["id"].as_str().unwrap_or_default().to_string();
    let (status, elsewhere) = create(
        &router,
        "eu-west-1",
        json!({
            "name": "far-away-template",
            "eventType": EVENT_TYPES[11],
            "groupIdentifier": group_id,
        }),
    )
    .await;
    assert_eq!(
        status, 201,
        "setup: expected the eu-west-1 template to be created, got {status}: {elsewhere}"
    );
    let far_id = elsewhere["id"].as_str().unwrap_or_default().to_string();

    let rows: [(&str, &str); 5] = [
        ("unminted id", "9999999"),
        ("unused name", "no-such-template"),
        ("other region's id", far_id.as_str()),
        ("other region's name", "far-away-template"),
        ("another resource type's id", group_id.as_str()),
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

/// Rule: a member outside the constraint its model carries — a length bound, the
/// closed set of an enum, the number of targets the service can count, or the
/// kind of ARN a target may name — is refused as a client error and nothing is
/// created; a member AT a published bound is accepted.
///
/// The rows walk each published bound and one past it: `Name` is
/// `__stringMin1Max255PatternS`, `Description` is `__stringMin0Max1024`,
/// `RequestId` is `__stringMin1Max256PatternS`, an `EventTargets` entry's `Arn`
/// is `__stringMin1Max2048PatternArn`, `EventTargetCount` is `__integerMax5` and
/// `EventType` is the `EventBridgeRuleTemplateEventType` enum.
#[tokio::test]
async fn a_member_outside_the_constraint_its_model_carries_is_refused() {
    let at_name_bound = "n".repeat(MAX_NAME);
    let past_name_bound = "n".repeat(MAX_NAME + 1);
    let at_description_bound = "d".repeat(MAX_DESCRIPTION);
    let past_description_bound = "d".repeat(MAX_DESCRIPTION + 1);
    let at_request_id_bound = "t".repeat(MAX_REQUEST_ID);
    let past_request_id_bound = "t".repeat(MAX_REQUEST_ID + 1);
    let at_arn_bound = sns_arn(MAX_TARGET_ARN);
    let past_arn_bound = sns_arn(MAX_TARGET_ARN + 1);
    let five_targets: Vec<Value> = (0..MAX_TARGETS)
        .map(|index| json!({ "arn": format!("arn:aws:sns:{DEFAULT_REGION}:{ACCOUNT}:topic-{index}") }))
        .collect();
    let six_targets: Vec<Value> = (0..=MAX_TARGETS)
        .map(|index| json!({ "arn": format!("arn:aws:sns:{DEFAULT_REGION}:{ACCOUNT}:topic-{index}") }))
        .collect();

    // (case, the request's members besides the group identifier, expected status)
    let rows: [(&str, Value, u16); 14] = [
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
        (
            "target arn at the maximum",
            json!({ "name": "targeted", "eventTargets": [{ "arn": at_arn_bound }] }),
            201,
        ),
        (
            "target arn past the maximum",
            json!({ "name": "targeted", "eventTargets": [{ "arn": past_arn_bound }] }),
            400,
        ),
        (
            "as many targets as the model counts",
            json!({ "name": "targeted", "eventTargets": five_targets }),
            201,
        ),
        (
            "one target past what the model counts",
            json!({ "name": "targeted", "eventTargets": six_targets }),
            400,
        ),
        (
            "a target that is neither an SNS topic nor a log group",
            json!({
                "name": "targeted",
                "eventTargets": [{ "arn": format!("arn:aws:s3:::bucket-{ACCOUNT}") }],
            }),
            400,
        ),
    ];

    for (case, members, expected) in rows {
        let router = medialive();
        let region = DEFAULT_REGION;
        create_group(&router, region, "bounds-group").await;

        let mut request = members;
        request["groupIdentifier"] = json!("bounds-group");
        // Every row above varies ONE member, so the event type is the same
        // modelled value throughout; the enum itself is a row below.
        request["eventType"] = json!(EVENT_TYPES[2]);

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
            // The refusal created nothing, so the first id the store would mint
            // names no template.
            let (status, read) = get(&router, region, "6000000").await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected the refused create to mint nothing, got {status}: {read}"
            );
        }
    }

    // The enum's own row: a value `EventBridgeRuleTemplateEventType` does not
    // declare is refused, whatever else the request carries.
    for event_type in ["", "medialive_channel_alert", "NOT_AN_EVENT_TYPE"] {
        let router = medialive();
        let region = DEFAULT_REGION;
        create_group(&router, region, "bounds-group").await;
        let (status, body) = create(
            &router,
            region,
            json!({
                "name": "enum-row",
                "eventType": event_type,
                "groupIdentifier": "bounds-group",
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {event_type:?}: expected an unmodelled event type to be refused, got {status}: {body}"
        );
        let code = error_code(status, &body);
        assert!(
            code == "BadRequestException" || code == "ValidationException",
            "case {event_type:?}: expected a client error, got {code} ({body})"
        );
    }
}
