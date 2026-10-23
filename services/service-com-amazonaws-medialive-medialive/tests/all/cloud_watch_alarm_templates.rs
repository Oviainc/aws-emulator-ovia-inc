// SPDX-License-Identifier: CC0-1.0

//! The MediaLive cloudwatch alarm TEMPLATE slice, end to end over the restJson1
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

use crate::support::{
    create_in as create_group_in, epoch_seconds, get_in as get_group_in, member, percent_encoded,
    reported_tags, timestamp, GROUPS,
};
use crate::wire::{call_in_region, error_code, medialive, ACCOUNT, DEFAULT_REGION};

/// `CreateCloudWatchAlarmTemplate`'s collection, which
/// `GetCloudWatchAlarmTemplate` reads an instance from
/// (`smithy_query(query="operation", operation="CreateCloudWatchAlarmTemplate")`:
/// POST /prod/cloudwatch-alarm-templates).
const TEMPLATES: &str = "/prod/cloudwatch-alarm-templates";

/// The bounds the input members' target shapes name: `Name` is
/// `__stringMin1Max255PatternS`, `Description` is `__stringMin0Max1024`,
/// `RequestId` is `__stringMin1Max256PatternS` and `MetricName` is
/// `__stringMax64` (`smithy_query(query="operation",
/// operation="CreateCloudWatchAlarmTemplate")`).
const MAX_NAME: usize = 255;
const MAX_DESCRIPTION: usize = 1024;
const MAX_REQUEST_ID: usize = 256;
const MAX_METRIC_NAME: usize = 64;

/// The bounds `Period` carries, from its target shape `__integerMin10Max86400`
/// ("The period, in seconds, over which the specified statistic is applied").
const MIN_PERIOD: i64 = 10;
const MAX_PERIOD: i64 = 86_400;

/// Every `CloudWatchAlarmTemplateComparisonOperator` the model declares
/// (`smithy_query(query="shape",
/// shape="CloudWatchAlarmTemplateComparisonOperator")`).
const COMPARISON_OPERATORS: [&str; 4] = [
    "GreaterThanOrEqualToThreshold",
    "GreaterThanThreshold",
    "LessThanOrEqualToThreshold",
    "LessThanThreshold",
];

/// Every `CloudWatchAlarmTemplateStatistic` the model declares
/// (`smithy_query(query="shape", shape="CloudWatchAlarmTemplateStatistic")`).
const STATISTICS: [&str; 5] = ["Average", "Maximum", "Minimum", "SampleCount", "Sum"];

/// Every `CloudWatchAlarmTemplateTargetResourceType` the model declares
/// (`smithy_query(query="shape",
/// shape="CloudWatchAlarmTemplateTargetResourceType")`).
const TARGET_RESOURCE_TYPES: [&str; 10] = [
    "CLOUDFRONT_DISTRIBUTION",
    "ELEMENTAL_INFERENCE_FEED",
    "MEDIACONNECT_FLOW",
    "MEDIALIVE_CHANNEL",
    "MEDIALIVE_INPUT_DEVICE",
    "MEDIALIVE_MULTIPLEX",
    "MEDIAPACKAGE_CHANNEL",
    "MEDIAPACKAGE_ORIGIN_ENDPOINT",
    "MEDIATAILOR_PLAYBACK_CONFIGURATION",
    "S3_BUCKET",
];

/// Every `CloudWatchAlarmTemplateTreatMissingData` the model declares
/// (`smithy_query(query="shape",
/// shape="CloudWatchAlarmTemplateTreatMissingData")`).
const TREAT_MISSING_DATA: [&str; 4] = ["breaching", "ignore", "missing", "notBreaching"];

/// The first id the alarm-template store mints, so a test can assert that a
/// refused create minted nothing.
const FIRST_MINTED_ID: &str = "2000000";

/// One cloudwatch alarm template group called `name` in `region`, answering the
/// created group.
async fn create_group(router: &MultiServiceRouter, region: &str, name: &str) -> Value {
    let (status, group) = create_group_in(router, region, json!({ "name": name })).await;
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

/// Every REQUIRED member of `CreateCloudWatchAlarmTemplate` besides
/// `GroupIdentifier`, with one modelled value each, so a row that varies ONE
/// member still sends a complete request.
fn required_members(name: &str) -> Value {
    json!({
        "name": name,
        "comparisonOperator": COMPARISON_OPERATORS[1],
        "evaluationPeriods": 1,
        "metricName": "InputVideoFrameRate",
        "period": 60,
        "statistic": STATISTICS[0],
        "targetResourceType": TARGET_RESOURCE_TYPES[3],
        "threshold": 29.97,
        "treatMissingData": TREAT_MISSING_DATA[3],
    })
}

/// A request made of `required_members` with `overrides` merged over it, so a row
/// states only what it varies.
fn request_with(name: &str, overrides: &Value) -> Value {
    let mut request = required_members(name);
    if let Some(members) = overrides.as_object() {
        for (key, value) in members {
            request[key.as_str()] = value.clone();
        }
    }
    request
}

/// A numeric member read as an f64, so an integral `threshold` and a fractional
/// one compare the same way whichever JSON number form each travelled as.
fn number(case: &str, body: &Value, name: &str) -> f64 {
    body[name].as_f64().unwrap_or_else(|| {
        panic!(
            "case {case:?}: expected {name} as a JSON number, got {:?}",
            body[name]
        )
    })
}

/// Rule: a create mints the resource's id, builds its ARN from the request's
/// region and that id, resolves its parent by either identifier form the model
/// allows, stores every member the request supplied verbatim, and the reporting
/// operation answers that stored record for either identifier form the model
/// allows (`Identifier` is documented "Can be either be its id or current name").
///
/// The rows differ along which optional members the request carries, along their
/// type (string, number, map), along the enum values chosen, along the numeric
/// values — including each published bound of `Period` — and along the form of
/// the `GroupIdentifier` ("Can be either be its id or current name"), because
/// that is what the rule generalises over.
#[tokio::test]
async fn a_created_alarm_template_is_read_back_by_its_id_and_by_its_name() {
    let longest_name = "n".repeat(MAX_NAME);
    let longest_description = "d".repeat(MAX_DESCRIPTION);
    let longest_metric_name = "m".repeat(MAX_METRIC_NAME);

    // (case, by group name rather than id, the members this row overrides)
    let rows: [(&str, bool, Value); 8] = [
        ("required members only", false, json!({})),
        ("group named rather than identified", true, json!({})),
        (
            "a description and tags",
            false,
            json!({ "description": "page the on-call", "tags": { "env": "test", "team": "video" } }),
        ),
        (
            "the optional datapointsToAlarm",
            false,
            json!({ "datapointsToAlarm": 3, "evaluationPeriods": 5 }),
        ),
        (
            "period at its published minimum",
            false,
            json!({ "period": MIN_PERIOD, "statistic": STATISTICS[3], "threshold": 0.0 }),
        ),
        (
            "period at its published maximum",
            false,
            json!({ "period": MAX_PERIOD, "statistic": STATISTICS[4], "threshold": -1.5 }),
        ),
        (
            "an empty description and a non-ascii name",
            false,
            json!({ "name": "alarmes — 夜", "description": "" }),
        ),
        (
            "longest name, description and metric name",
            false,
            json!({
                "name": longest_name,
                "description": longest_description,
                "metricName": longest_metric_name,
                "comparisonOperator": COMPARISON_OPERATORS[3],
                "targetResourceType": TARGET_RESOURCE_TYPES[9],
                "treatMissingData": TREAT_MISSING_DATA[0],
            }),
        ),
    ];

    for (index, (case, by_group_name, overrides)) in rows.into_iter().enumerate() {
        let router = medialive();
        let region = DEFAULT_REGION;
        let group = create_group(&router, region, "parent-group").await;
        let group_id = group["id"].as_str().unwrap_or_default().to_string();
        let group_identifier = if by_group_name {
            "parent-group".to_string()
        } else {
            group_id.clone()
        };

        let mut request = request_with(&format!("alarm-{index}"), &overrides);
        request["groupIdentifier"] = json!(group_identifier);
        let name = request["name"].as_str().unwrap_or_default().to_string();

        let (status, created) = create(&router, region, request.clone()).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from CreateCloudWatchAlarmTemplate, got {status}: {created}"
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
                "arn:aws:medialive:{region}:{ACCOUNT}:cloudwatch-alarm-template:{id}"
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

        // Every member the request supplied, reported back out of the store.
        for reported in [
            "name",
            "comparisonOperator",
            "metricName",
            "statistic",
            "targetResourceType",
            "treatMissingData",
            "evaluationPeriods",
            "period",
        ] {
            assert_eq!(
                created[reported], request[reported],
                "case {case:?}: expected the {reported} the request supplied, got {:?}",
                created[reported]
            );
        }
        assert_eq!(
            number(case, &created, "threshold"),
            number(case, &request, "threshold"),
            "case {case:?}: expected the threshold the request supplied, got {:?}",
            created["threshold"]
        );
        assert_eq!(
            member(&created, "description"),
            member(&request, "description"),
            "case {case:?}: expected the description the request supplied, got {:?}",
            created["description"]
        );
        // `DatapointsToAlarm` is optional on the request, so a template created
        // without one reports it absent rather than inventing a value.
        assert_eq!(
            member(&created, "datapointsToAlarm"),
            member(&request, "datapointsToAlarm"),
            "case {case:?}: expected the datapointsToAlarm the request supplied, got {:?}",
            created["datapointsToAlarm"]
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

        let (status, by_name) = get(&router, region, &percent_encoded(&name)).await;
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
/// unchanged, and a value outside its catalog is refused as a client error.
///
/// The rows are the WHOLE published catalog of each of the four enum members, so
/// the values the issue never cited are covered too.
#[tokio::test]
async fn every_modelled_enum_value_is_accepted_and_reported_back() {
    // (the request member, its published catalog)
    let members: [(&str, &[&str]); 4] = [
        ("comparisonOperator", &COMPARISON_OPERATORS),
        ("statistic", &STATISTICS),
        ("targetResourceType", &TARGET_RESOURCE_TYPES),
        ("treatMissingData", &TREAT_MISSING_DATA),
    ];

    let router = medialive();
    let region = DEFAULT_REGION;
    create_group(&router, region, "all-enum-values").await;

    for (name, catalog) in members {
        for value in catalog {
            let case = format!("{name} = {value}");
            let mut request = required_members(&format!("alarm-{name}-{value}"));
            request[name] = json!(value);
            request["groupIdentifier"] = json!("all-enum-values");

            let (status, created) = create(&router, region, request).await;
            assert_eq!(
                status, 201,
                "case {case:?}: expected 201, got {status}: {created}"
            );
            assert_eq!(
                created[name],
                json!(value),
                "case {case:?}: expected the value stored, got {:?}",
                created[name]
            );

            let id = created["id"].as_str().unwrap_or_default().to_string();
            let (status, read) = get(&router, region, &id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected to read the template back, got {status}: {read}"
            );
            assert_eq!(
                read[name],
                json!(value),
                "case {case:?}: expected the stored value on the read, got {:?}",
                read[name]
            );
        }

        // A value the enum does not declare names nothing the service can
        // evaluate, whatever else the request carries.
        for stray in ["", "average", "NOT_A_MODELLED_VALUE"] {
            let case = format!("{name} = {stray:?}");
            let router = medialive();
            create_group(&router, region, "stray-values").await;
            let mut request = required_members("stray");
            request[name] = json!(stray);
            request["groupIdentifier"] = json!("stray-values");

            let (status, body) = create(&router, region, request).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected an unmodelled enum value to be refused, got {status}: {body}"
            );
            let code = error_code(status, &body);
            assert!(
                code == "BadRequestException" || code == "ValidationException",
                "case {case:?}: expected a client error, got {code} ({body})"
            );
        }
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
async fn an_alarm_template_name_is_unique_within_one_region() {
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

        let mut request = request_with(name, &json!({}));
        request["groupIdentifier"] = json!("first-group");
        let (status, first) = create(&router, first_region, request).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected the first create to succeed, got {status}: {first}"
        );
        let first_id = first["id"].as_str().unwrap_or_default().to_string();

        let mut retry = request_with(name, &json!({}));
        retry["groupIdentifier"] = json!(second_group);
        let (status, second) = create(&router, second_region, retry).await;

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
        let mut request = request_with(
            "idempotent-template",
            &json!({ "description": "created once", "requestId": token }),
        );
        request["groupIdentifier"] = json!("idempotent-group");

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
        let mut other = request_with(
            "idempotent-template",
            &json!({ "requestId": "a-different-token" }),
        );
        other["groupIdentifier"] = json!("idempotent-group");
        let (status, conflict) = create(&router, region, other).await;
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
        let mut request = request_with(name, &json!({ "requestId": token }));
        request["groupIdentifier"] = json!("shared-group");
        let (status, template) = create(&router, region, request).await;
        assert_eq!(
            status, 201,
            "case {region}: expected the create to succeed, got {status}: {template}"
        );
        assert!(
            !created.iter().any(|earlier| earlier["id"] == template["id"]),
            "case {region}: expected a template of its own, got {:?} which another region already has",
            template["id"]
        );
        created.push(template);
    }

    for (region, expected) in regions.iter().zip(&created) {
        let mut request = request_with(name, &json!({ "requestId": token }));
        request["groupIdentifier"] = json!("shared-group");
        let (status, retried) = create(&router, region, request).await;
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
/// nothing used, and the id and the name of a group that exists in a DIFFERENT
/// region.
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
        let mut request = request_with("orphan", &json!({}));
        request["groupIdentifier"] = json!(identifier);
        let (status, body) = create(&router, "us-west-2", request).await;
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
async fn an_identifier_no_alarm_template_in_the_region_has_is_not_found() {
    let router = medialive();
    let group = create_group(&router, "eu-west-1", "far-away-group").await;
    let group_id = group["id"].as_str().unwrap_or_default().to_string();
    let mut request = request_with("far-away-template", &json!({}));
    request["groupIdentifier"] = json!(group_id);
    let (status, elsewhere) = create(&router, "eu-west-1", request).await;
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

/// Rule: a member outside the constraint its model carries — a string length
/// bound or a numeric range — is refused as a client error and nothing is
/// created; a member AT a published bound is accepted.
///
/// The rows walk each published bound and one past it: `Name` is
/// `__stringMin1Max255PatternS`, `Description` is `__stringMin0Max1024`,
/// `RequestId` is `__stringMin1Max256PatternS`, `MetricName` is `__stringMax64`,
/// `Period` is `__integerMin10Max86400` and `EvaluationPeriods` and
/// `DatapointsToAlarm` are both `__integerMin1`.
#[tokio::test]
async fn a_member_outside_the_constraint_its_model_carries_is_refused() {
    let at_name_bound = "n".repeat(MAX_NAME);
    let past_name_bound = "n".repeat(MAX_NAME + 1);
    let at_description_bound = "d".repeat(MAX_DESCRIPTION);
    let past_description_bound = "d".repeat(MAX_DESCRIPTION + 1);
    let at_request_id_bound = "t".repeat(MAX_REQUEST_ID);
    let past_request_id_bound = "t".repeat(MAX_REQUEST_ID + 1);
    let at_metric_name_bound = "m".repeat(MAX_METRIC_NAME);
    let past_metric_name_bound = "m".repeat(MAX_METRIC_NAME + 1);

    // (case, the members this row overrides, expected status)
    let rows: [(&str, Value, u16); 16] = [
        ("name below the minimum", json!({ "name": "" }), 400),
        ("name at the minimum", json!({ "name": "n" }), 201),
        ("name at the maximum", json!({ "name": at_name_bound }), 201),
        ("name past the maximum", json!({ "name": past_name_bound }), 400),
        (
            "description at the maximum",
            json!({ "description": at_description_bound }),
            201,
        ),
        (
            "description past the maximum",
            json!({ "description": past_description_bound }),
            400,
        ),
        ("request id below the minimum", json!({ "requestId": "" }), 400),
        (
            "request id at the maximum",
            json!({ "requestId": at_request_id_bound }),
            201,
        ),
        (
            "request id past the maximum",
            json!({ "requestId": past_request_id_bound }),
            400,
        ),
        (
            "metric name at the maximum",
            json!({ "metricName": at_metric_name_bound }),
            201,
        ),
        (
            "metric name past the maximum",
            json!({ "metricName": past_metric_name_bound }),
            400,
        ),
        ("period below the minimum", json!({ "period": MIN_PERIOD - 1 }), 400),
        ("period past the maximum", json!({ "period": MAX_PERIOD + 1 }), 400),
        (
            "evaluation periods below the minimum",
            json!({ "evaluationPeriods": 0 }),
            400,
        ),
        (
            "datapoints to alarm below the minimum",
            json!({ "datapointsToAlarm": 0 }),
            400,
        ),
        (
            "datapoints to alarm at the minimum",
            json!({ "datapointsToAlarm": 1 }),
            201,
        ),
    ];

    for (case, overrides, expected) in rows {
        let router = medialive();
        let region = DEFAULT_REGION;
        create_group(&router, region, "bounds-group").await;

        let mut request = request_with("bounded", &overrides);
        request["groupIdentifier"] = json!("bounds-group");

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
            let (status, read) = get(&router, region, FIRST_MINTED_ID).await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected the refused create to mint nothing, got {status}: {read}"
            );
        }
    }
}

/// Rule: a group delete whose documented precondition is unmet is refused with
/// the operation's modelled `ConflictException` (HTTP 409) and removes nothing —
/// `DeleteCloudWatchAlarmTemplateGroup` documents "You must ... ensure its
/// existing templates are moved to another group or deleted", and now that a
/// group can hold cloudwatch alarm templates that half of the precondition is
/// checkable.
///
/// The rows differ along the NUMBER of templates the group still holds, because
/// that is what the rule generalises over.
#[tokio::test]
async fn an_alarm_template_group_delete_is_refused_while_it_still_holds_templates() {
    for held in [1usize, 2, 5] {
        let case = format!("group holding {held} template(s)");
        let router = medialive();
        let region = DEFAULT_REGION;
        let group = create_group(&router, region, "crowded-group").await;
        let group_id = group["id"].as_str().unwrap_or_default().to_string();

        let mut templates: Vec<String> = Vec::new();
        for index in 0..held {
            let mut request = request_with(&format!("member-{index}"), &json!({}));
            request["groupIdentifier"] = json!("crowded-group");
            let (status, template) = create(&router, region, request).await;
            assert_eq!(
                status, 201,
                "case {case:?}: expected member {index} to be created, got {status}: {template}"
            );
            templates.push(template["id"].as_str().unwrap_or_default().to_string());
        }

        for identifier in [group_id.as_str(), "crowded-group"] {
            let (status, refused) = call_in_region(
                &router,
                region,
                "DELETE",
                &format!("{GROUPS}/{}", percent_encoded(identifier)),
                None,
            )
            .await;
            assert_eq!(
                status, 409,
                "case {case:?}: expected deleting {identifier:?} to be refused, got {status}: {refused}"
            );
            assert_eq!(
                error_code(status, &refused),
                "ConflictException",
                "case {case:?}: expected ConflictException, got {refused}"
            );
        }

        // Nothing was removed: the group and every member still read back.
        let (status, read) = call_in_region(
            &router,
            region,
            "GET",
            &format!("{GROUPS}/{group_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the refused delete to leave the group, got {status}: {read}"
        );
        assert_eq!(
            read, group,
            "case {case:?}: expected the group unchanged, got {read}"
        );
        for id in &templates {
            let (status, read) = get(&router, region, id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected member {id} to survive, got {status}: {read}"
            );
        }
    }
}

/// Rule: every create mints the template its OWN identity — an id no other
/// template of the store carries, in the bare (caller-created) form of
/// `__stringMin7Max11PatternAws097` rather than the `aws-` form the model
/// reserves for AWS-provided templates, plus an ARN built from the signed region
/// and that id — files it under the id of the group its `GroupIdentifier`
/// resolves to, and writes NOTHING else: every template the store already holds
/// still reads back exactly as its own create reported it, in creation order by
/// `CreatedAt`, and so does every group, none of whose members this request
/// carries.
///
/// The rows differ along how many templates one sequence creates, along how those
/// templates are spread over groups (one group, two, one each), and along the
/// region signed for, because that is what the rule generalises over: the
/// recorded case — a single create into a single group — is one row of it.
#[tokio::test]
async fn each_create_mints_its_own_identity_and_leaves_what_is_stored_untouched() {
    // (case, region, the groups the sequence creates templates in, how many templates)
    let rows: [(&str, &str, &[&str], usize); 5] = [
        ("one template, one group", DEFAULT_REGION, &["solo"], 1),
        ("two templates, one group", DEFAULT_REGION, &["shared"], 2),
        (
            "five templates alternating over two groups",
            DEFAULT_REGION,
            &["left", "right"],
            5,
        ),
        ("five templates in another region", "eu-central-1", &["eu"], 5),
        (
            "one template in each of three groups",
            "ap-southeast-2",
            &["a", "b", "c"],
            3,
        ),
    ];

    for (case, region, group_names, count) in rows {
        let router = medialive();

        // The parent groups, and what each reads back as BEFORE it holds anything.
        let mut group_ids: Vec<String> = Vec::new();
        let mut groups_before: Vec<Value> = Vec::new();
        for name in group_names {
            let group = create_group(&router, region, name).await;
            group_ids.push(group["id"].as_str().unwrap_or_default().to_string());
            groups_before.push(group);
        }

        let mut created: Vec<Value> = Vec::new();
        for index in 0..count {
            let group_index = index % group_names.len();
            let mut request = request_with(&format!("alarm-{index}"), &json!({}));
            request["groupIdentifier"] = json!(group_names[group_index]);

            let (status, template) = create(&router, region, request).await;
            assert_eq!(
                status, 201,
                "case {case:?}: expected template {index} to be created, got {status}: {template}"
            );

            let id = template["id"].as_str().unwrap_or_default().to_string();
            assert!(
                (7..=11).contains(&id.len()) && id.chars().all(|c| c.is_ascii_digit()),
                "case {case:?}: expected template {index}'s id in the caller-created form of __stringMin7Max11PatternAws097, got {:?}",
                template["id"]
            );
            assert_eq!(
                template["arn"],
                json!(format!(
                    "arn:aws:medialive:{region}:{ACCOUNT}:cloudwatch-alarm-template:{id}"
                )),
                "case {case:?}: expected template {index}'s ARN built from the signed region and its minted id, got {:?}",
                template["arn"]
            );
            assert_eq!(
                template["groupId"],
                json!(group_ids[group_index]),
                "case {case:?}: expected template {index} filed under the group {:?} names, got {:?}",
                group_names[group_index],
                template["groupId"]
            );

            for (earlier_index, earlier) in created.iter().enumerate() {
                assert_ne!(
                    earlier["id"], template["id"],
                    "case {case:?}: expected template {index} to get an id of its own, got template {earlier_index}'s {:?}",
                    template["id"]
                );
                assert_ne!(
                    earlier["arn"], template["arn"],
                    "case {case:?}: expected template {index} to get an ARN of its own, got template {earlier_index}'s {:?}",
                    template["arn"]
                );
                let before = epoch_seconds(timestamp(case, "createdAt", earlier));
                let after = epoch_seconds(timestamp(case, "createdAt", &template));
                assert!(
                    before <= after,
                    "case {case:?}: expected createdAt not to run backwards from template {earlier_index} to {index}, got {before} then {after}"
                );
            }

            created.push(template);
        }

        let minted: std::collections::BTreeSet<&str> = created
            .iter()
            .map(|template| template["id"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(
            minted.len(),
            count,
            "case {case:?}: expected {count} distinct minted ids, got {minted:?}"
        );

        // Nothing a later create wrote reached an earlier template: each one
        // still reads back as the record its own create reported.
        for (index, template) in created.iter().enumerate() {
            let id = template["id"].as_str().unwrap_or_default();
            let (status, read) = get(&router, region, id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected template {index} ({id}) to read back, got {status}: {read}"
            );
            assert_eq!(
                &read, template,
                "case {case:?}: expected template {index} unchanged by the creates after it, got {read}"
            );
        }

        // Nor did it reach a group: a create carries no member of the group it
        // files the template under, so each group keeps the record its own create
        // reported.
        for (index, before) in groups_before.iter().enumerate() {
            let (status, read) = get_group_in(&router, region, &group_ids[index]).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected group {index} to read back, got {status}: {read}"
            );
            assert_eq!(
                &read, before,
                "case {case:?}: expected group {index} unchanged by the templates created in it, got {read}"
            );
        }
    }
}
