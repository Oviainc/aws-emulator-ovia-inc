// SPDX-License-Identifier: CC0-1.0

//! The AWS-PROVIDED monitor template catalog, end to end over the restJson1
//! wire: the cloudwatch alarm template GROUPS and TEMPLATES the SERVICE
//! publishes, as `ListCloudWatchAlarmTemplateGroups`,
//! `ListCloudWatchAlarmTemplates`, `GetCloudWatchAlarmTemplateGroup` and
//! `GetCloudWatchAlarmTemplate` report them.
//!
//! The rule under test: a monitor template resource has two provenances —
//! `CloudWatchAlarmTemplateGroupSummary$Id` is "A cloudwatch alarm template
//! group's id. AWS provided template groups have ids that start with `aws-`",
//! and `CloudWatchAlarmTemplateSummary$Id` says the same of a template — so the
//! resources the SERVICE publishes exist in every account and every region
//! without anybody creating them: every read path reports them beside the
//! account's own, their ARNs name no account, their published values are the
//! same on every call and in every process, and no write of the account's
//! touches them.
//!
//! The suites are TABLES: the rows are the published groups (every one of them,
//! each resolved by both identifier forms), the published templates, the three
//! `Scope` options, the two account states a merge is read in (an account that
//! created nothing and one that created a group of its own), two regions, and —
//! for the read-only rule — the four writes a monitor template resource has
//! (`Update`/`Delete` of a group and of a template) crossed with both identifier
//! forms.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{
    account_owned, create_in as create_group_in, is_aws_provided, percent_encoded, timestamp,
    GROUPS,
};
use crate::wire::{call_in_region, error_code, error_message, medialive, DEFAULT_REGION};

/// The template collection (`smithy_query(query="operation",
/// operation="ListCloudWatchAlarmTemplates")`: GET
/// /prod/cloudwatch-alarm-templates).
const TEMPLATES: &str = "/prod/cloudwatch-alarm-templates";

/// The groups MediaLive publishes, one per media workflow it ships monitoring
/// for. This is the catalog's own data — a client resolves a published group by
/// exactly these names — spelled here so the suite pins the published set rather
/// than echoing the table the service builds it from.
const PUBLISHED_GROUPS: [&str; 10] = [
    "AWS-CloudFrontGeneralWorkflows",
    "AWS-MediaConnectArqProtocolWorkflows",
    "AWS-MediaConnectCdiJpegxsWorkflows",
    "AWS-MediaConnectInputFailoverWorkflows",
    "AWS-MediaConnectRtpWorkflows",
    "AWS-MediaLiveMultiplexWorkflows",
    "AWS-MediaLiveToMediaPackageWorkflows",
    "AWS-MediaLiveUdpAndRtmpWorkflows",
    "AWS-MediaPackageGeneralWorkflows",
    "AWS-MediaTailorAdInsertionWorkflows",
];

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

/// Every `CloudWatchAlarmTemplateComparisonOperator`, `...Statistic` and
/// `...TreatMissingData` the model declares.
const COMPARISON_OPERATORS: [&str; 4] = [
    "GreaterThanOrEqualToThreshold",
    "GreaterThanThreshold",
    "LessThanOrEqualToThreshold",
    "LessThanThreshold",
];
const STATISTICS: [&str; 5] = ["Average", "Maximum", "Minimum", "SampleCount", "Sum"];
const TREAT_MISSING_DATA: [&str; 4] = ["breaching", "ignore", "missing", "notBreaching"];

/// The bounds `Period` carries, from its target shape `__integerMin10Max86400`.
const MIN_PERIOD: i64 = 10;
const MAX_PERIOD: i64 = 86_400;

/// The longest `MetricName`, from its target shape `__stringMax64`.
const MAX_METRIC_NAME: usize = 64;

/// The bounds a published id must satisfy: `__stringMin7Max11PatternAws097`
/// admits seven characters at least and eleven at most, so an `aws-` prefixed id
/// is the prefix plus seven digits.
const PUBLISHED_ID_LEN: usize = 11;
const PUBLISHED_ID_PREFIX: &str = "aws-";

/// `ListCloudWatchAlarmTemplateGroups` with `query` appended verbatim, signed for
/// `region`.
async fn list_groups(router: &MultiServiceRouter, region: &str, query: &str) -> Vec<Value> {
    let (status, body) =
        call_in_region(router, region, "GET", &format!("{GROUPS}{query}"), None).await;
    assert_eq!(
        status, 200,
        "query {query:?}: expected ListCloudWatchAlarmTemplateGroups to answer 200, \
         got {status}: {body}"
    );
    body["cloudWatchAlarmTemplateGroups"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("query {query:?}: expected a group list, got {body}"))
}

/// `ListCloudWatchAlarmTemplates` with `query` appended verbatim, signed for
/// `region`.
async fn list_templates(router: &MultiServiceRouter, region: &str, query: &str) -> Vec<Value> {
    let (status, body) =
        call_in_region(router, region, "GET", &format!("{TEMPLATES}{query}"), None).await;
    assert_eq!(
        status, 200,
        "query {query:?}: expected ListCloudWatchAlarmTemplates to answer 200, \
         got {status}: {body}"
    );
    body["cloudWatchAlarmTemplates"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("query {query:?}: expected a template list, got {body}"))
}

/// The published entries of a listing, keyed by `name`.
fn published_by_name(entries: &[Value]) -> BTreeMap<String, Value> {
    entries
        .iter()
        .filter(|entry| is_aws_provided(entry))
        .map(|entry| {
            (
                entry["name"].as_str().unwrap_or_default().to_string(),
                entry.clone(),
            )
        })
        .collect()
}

/// One `GetCloudWatchAlarmTemplateGroup` on `identifier`.
async fn get_group(router: &MultiServiceRouter, region: &str, identifier: &str) -> (u16, Value) {
    call_in_region(
        router,
        region,
        "GET",
        &format!("{GROUPS}/{}", percent_encoded(identifier)),
        None,
    )
    .await
}

/// One `GetCloudWatchAlarmTemplate` on `identifier`.
async fn get_template(router: &MultiServiceRouter, region: &str, identifier: &str) -> (u16, Value) {
    call_in_region(
        router,
        region,
        "GET",
        &format!("{TEMPLATES}/{}", percent_encoded(identifier)),
        None,
    )
    .await
}

/// Assert that `id` is a published id and `arn` is the service-owned ARN of
/// `resource_kind` in `region`: a resource the service publishes belongs to no
/// account, so the ARN's account-id field is EMPTY.
fn assert_published_identity(case: &str, region: &str, resource_kind: &str, entry: &Value) {
    let id = entry["id"].as_str().unwrap_or_default();
    assert!(
        id.starts_with(PUBLISHED_ID_PREFIX) && id.len() == PUBLISHED_ID_LEN,
        "case {case:?}: expected a published id of {PUBLISHED_ID_LEN} characters under \
         {PUBLISHED_ID_PREFIX:?}, got {id:?}"
    );
    assert!(
        id[PUBLISHED_ID_PREFIX.len()..]
            .chars()
            .all(|digit| digit.is_ascii_digit()),
        "case {case:?}: expected the published id's suffix to be digits, got {id:?}"
    );
    assert_eq!(
        entry["arn"],
        json!(format!("arn:aws:medialive:{region}::{resource_kind}:{id}")),
        "case {case:?}: expected the service-owned ARN, whose account-id field is empty, \
         got {:?}",
        entry["arn"]
    );
}

/// Rule: every group the service publishes is reported by a listing on an
/// account that has created nothing, each carrying a published id, the
/// service-owned ARN of its region, the number of templates currently in it and
/// the instant it was published; and a `GetCloudWatchAlarmTemplateGroup` on
/// either identifier form reports the same entry.
///
/// The rows are the ten published groups, each read through the listing, through
/// a get by id and through a get by name.
#[tokio::test]
async fn a_fresh_account_reads_every_published_group() {
    let router = medialive();
    let listed = list_groups(&router, DEFAULT_REGION, "").await;
    let published = published_by_name(&listed);
    let templates = list_templates(&router, DEFAULT_REGION, "").await;

    assert_eq!(
        published.keys().cloned().collect::<Vec<String>>(),
        PUBLISHED_GROUPS.map(ToString::to_string).to_vec(),
        "expected the published catalog, got {:?}",
        published.keys().collect::<Vec<&String>>()
    );
    assert!(
        account_owned(&listed).is_empty(),
        "expected an account that created nothing to own no group, got {listed:?}"
    );

    for name in PUBLISHED_GROUPS {
        let case = name;
        let summary = &published[name];
        assert_published_identity(case, DEFAULT_REGION, "cloudwatch-alarm-template-group", summary);

        // `TemplateCount` is "The number of templates in a group", derived from
        // the templates that exist, so it counts the published templates the
        // listing beside it reports for this group.
        let id = summary["id"].as_str().unwrap_or_default().to_string();
        let held = templates
            .iter()
            .filter(|template| template["groupId"] == json!(id))
            .count();
        assert!(
            held > 0,
            "case {case:?}: expected the published group to publish templates of its own"
        );
        assert_eq!(
            summary["templateCount"],
            json!(held),
            "case {case:?}: expected the count of the templates in the group, got {:?}",
            summary["templateCount"]
        );

        // Both identifier forms resolve to the same published group ("Can be
        // either be its id or current name").
        for identifier in [id.as_str(), name] {
            let (status, read) = get_group(&router, DEFAULT_REGION, identifier).await;
            assert_eq!(
                status, 200,
                "case {case:?}/{identifier}: expected the get to answer 200, got {status}: {read}"
            );
            for member in ["arn", "id", "name", "tags"] {
                assert_eq!(
                    read.get(member).cloned().unwrap_or(Value::Null),
                    summary.get(member).cloned().unwrap_or(Value::Null),
                    "case {case:?}/{identifier}: expected the get's {member} to equal the \
                     listed one, got {:?} vs {:?}",
                    read.get(member),
                    summary.get(member)
                );
            }
            assert_eq!(
                timestamp(case, "createdAt", &read),
                timestamp(case, "createdAt", summary),
                "case {case:?}/{identifier}: expected the get and the listing to report one \
                 published instant"
            );
        }
    }
}

/// Rule: every template the service publishes is reported by a listing on an
/// account that has created nothing, each carrying a published id, the
/// service-owned ARN of its region, the id of the published group it belongs to
/// and a complete, modelled alarm definition; and a `GetCloudWatchAlarmTemplate`
/// on either identifier form reports the same entry.
///
/// The rows are every published template.
#[tokio::test]
async fn a_fresh_account_reads_every_published_template() {
    let router = medialive();
    let groups = list_groups(&router, DEFAULT_REGION, "").await;
    let group_ids: BTreeSet<String> = groups
        .iter()
        .map(|group| group["id"].as_str().unwrap_or_default().to_string())
        .collect();
    let listed = list_templates(&router, DEFAULT_REGION, "").await;
    let published = published_by_name(&listed);

    assert!(
        published.len() >= PUBLISHED_GROUPS.len(),
        "expected at least one published template per published group, got {}",
        published.len()
    );
    assert!(
        account_owned(&listed).is_empty(),
        "expected an account that created nothing to own no template, got {listed:?}"
    );

    for (name, template) in &published {
        let case = name.as_str();
        assert_published_identity(case, DEFAULT_REGION, "cloudwatch-alarm-template", template);

        let group_id = template["groupId"].as_str().unwrap_or_default().to_string();
        assert!(
            group_ids.contains(&group_id),
            "case {case:?}: expected the groupId to name a group the listing reports, \
             got {group_id:?}"
        );

        // Every member of the alarm definition is one the model declares, so a
        // published template is one CloudWatch could really be driven from.
        for (member, values) in [
            ("comparisonOperator", COMPARISON_OPERATORS.to_vec()),
            ("statistic", STATISTICS.to_vec()),
            ("targetResourceType", TARGET_RESOURCE_TYPES.to_vec()),
            ("treatMissingData", TREAT_MISSING_DATA.to_vec()),
        ] {
            let reported = template[member].as_str().unwrap_or_default();
            assert!(
                values.contains(&reported),
                "case {case:?}: expected {member} to be one of {values:?}, got {reported:?}"
            );
        }
        let metric = template["metricName"].as_str().unwrap_or_default();
        assert!(
            !metric.is_empty() && metric.len() <= MAX_METRIC_NAME,
            "case {case:?}: expected a metric name of at most {MAX_METRIC_NAME} characters, \
             got {metric:?}"
        );
        let period = template["period"].as_i64().unwrap_or_default();
        assert!(
            (MIN_PERIOD..=MAX_PERIOD).contains(&period),
            "case {case:?}: expected a period in {MIN_PERIOD}..={MAX_PERIOD}, got {period}"
        );
        let evaluation_periods = template["evaluationPeriods"].as_i64().unwrap_or_default();
        let datapoints = template["datapointsToAlarm"].as_i64().unwrap_or_default();
        assert!(
            evaluation_periods >= 1 && (1..=evaluation_periods).contains(&datapoints),
            "case {case:?}: expected an M-of-N alarm with 1 <= M <= N, got {datapoints} \
             of {evaluation_periods}"
        );
        assert!(
            template["threshold"].as_f64().is_some(),
            "case {case:?}: expected a numeric threshold, got {:?}",
            template["threshold"]
        );
        assert!(
            template["description"]
                .as_str()
                .is_some_and(|description| !description.is_empty()),
            "case {case:?}: expected a description, got {:?}",
            template["description"]
        );

        let id = template["id"].as_str().unwrap_or_default().to_string();
        for identifier in [id.as_str(), name.as_str()] {
            let (status, read) = get_template(&router, DEFAULT_REGION, identifier).await;
            assert_eq!(
                status, 200,
                "case {case:?}/{identifier}: expected the get to answer 200, got {status}: {read}"
            );
            for member in [
                "arn",
                "id",
                "groupId",
                "name",
                "description",
                "comparisonOperator",
                "datapointsToAlarm",
                "evaluationPeriods",
                "metricName",
                "period",
                "statistic",
                "targetResourceType",
                "threshold",
                "treatMissingData",
            ] {
                assert_eq!(
                    read.get(member).cloned().unwrap_or(Value::Null),
                    template.get(member).cloned().unwrap_or(Value::Null),
                    "case {case:?}/{identifier}: expected the get's {member} to equal the \
                     listed one, got {:?} vs {:?}",
                    read.get(member),
                    template.get(member)
                );
            }
        }
    }
}

/// Rule: the published catalog is the SERVICE's, so it is the same wherever and
/// whenever it is read — two independently constructed emulators report the same
/// ids and the same published instants for it, and two regions report the same
/// entries with only the ARN's region field differing. Nothing about it is taken
/// from the local clock, which is what makes a value a caller cached about one
/// entry stay true.
///
/// The rows are the two regions, crossed with the two emulators.
#[tokio::test]
async fn the_published_catalog_is_the_same_everywhere() {
    let regions = ["us-west-2", "eu-central-1"];
    let first = medialive();
    let second = medialive();

    for region in regions {
        let here = published_by_name(&list_groups(&first, region, "").await);
        let there = published_by_name(&list_groups(&second, region, "").await);
        assert_eq!(
            here, there,
            "case {region:?}: expected two emulators to publish one catalog"
        );

        let templates_here = published_by_name(&list_templates(&first, region, "").await);
        let templates_there = published_by_name(&list_templates(&second, region, "").await);
        assert_eq!(
            templates_here, templates_there,
            "case {region:?}: expected two emulators to publish one template catalog"
        );
    }

    // Across regions: the same ids and published instants, and an ARN that names
    // the region it was read through.
    let [here, there] = regions;
    let in_here = published_by_name(&list_groups(&first, here, "").await);
    let in_there = published_by_name(&list_groups(&first, there, "").await);
    for name in PUBLISHED_GROUPS {
        let left = &in_here[name];
        let right = &in_there[name];
        assert_eq!(
            left["id"], right["id"],
            "case {name:?}: expected one published id in every region, got {:?} vs {:?}",
            left["id"], right["id"]
        );
        assert_eq!(
            timestamp(name, "createdAt", left),
            timestamp(name, "createdAt", right),
            "case {name:?}: expected one published instant in every region"
        );
        assert_published_identity(name, there, "cloudwatch-alarm-template-group", right);
    }
}

/// Rule: `Scope` narrows a listing by PROVENANCE — "options for all scopes, AWS
/// provided resources, or local resources" — and the published catalog is
/// reported beside the account's own resources on every call, whether the account
/// has created anything or not.
///
/// The rows are the scope options (including a request that names none) crossed
/// with the two account states a merge can be read in.
#[tokio::test]
async fn the_published_catalog_is_merged_with_the_accounts_own() {
    let router = medialive();
    let published = PUBLISHED_GROUPS.len();

    // (case, the scope query, how many published groups it must admit, how many
    // of the account's own)
    let table: [(&str, &str, usize, usize); 8] = [
        ("no scope, nothing created", "", published, 0),
        ("every scope, nothing created", "?scope=ALL", published, 0),
        ("the AWS scope, nothing created", "?scope=AWS", published, 0),
        ("the local scope, nothing created", "?scope=LOCAL", 0, 0),
        ("no scope, one group created", "", published, 1),
        ("every scope, one group created", "?scope=ALL", published, 1),
        ("the AWS scope, one group created", "?scope=AWS", published, 0),
        ("the local scope, one group created", "?scope=LOCAL", 0, 1),
    ];

    let mut created = false;
    for (case, query, published_count, own_count) in table {
        if own_count > 0 && !created {
            let (status, group) =
                create_group_in(&router, DEFAULT_REGION, json!({ "name": "my-own-group" })).await;
            assert_eq!(
                status, 201,
                "case {case:?}: expected the account's own group to be created, \
                 got {status}: {group}"
            );
            created = true;
        }

        let listed = list_groups(&router, DEFAULT_REGION, query).await;
        assert_eq!(
            published_by_name(&listed).len(),
            published_count,
            "case {case:?}: expected {published_count} published groups, got {listed:?}"
        );
        assert_eq!(
            account_owned(&listed).len(),
            own_count,
            "case {case:?}: expected {own_count} of the account's own groups, got {listed:?}"
        );

        // A listing reports the resources of a kind in the order they came into
        // being, and the published catalog was published before the account
        // created anything, so every published entry precedes every entry of the
        // account's own.
        let provenances: Vec<bool> = listed.iter().map(is_aws_provided).collect();
        assert!(
            provenances.windows(2).all(|pair| pair[0] || !pair[1]),
            "case {case:?}: expected the published groups to precede the account's own, \
             got {provenances:?}"
        );
    }

    // The page cut follows that order: a page the size of the published catalog
    // carries exactly it, and the token resumes at the account's own group.
    let page_size = PUBLISHED_GROUPS.len();
    let (status, body) = call_in_region(
        &router,
        DEFAULT_REGION,
        "GET",
        &format!("{GROUPS}?maxResults={page_size}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "expected the paged listing to answer 200: {body}");
    let page = body["cloudWatchAlarmTemplateGroups"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        published_by_name(&page).len(),
        page_size,
        "expected a page the size of the published catalog to carry exactly it, got {page:?}"
    );
    let next = body["nextToken"].as_str().unwrap_or_default().to_string();
    let resumed = list_groups(&router, DEFAULT_REGION, &format!("?nextToken={next}")).await;
    assert_eq!(
        account_owned(&resumed).len(),
        1,
        "expected the next page to carry the account's own group, got {resumed:?}"
    );
}

/// Rule: an account's own template may be created INSIDE a published group — the
/// documented starting point for MediaLive monitoring — so the group's
/// `TemplateCount` follows that create, the created template is the ACCOUNT's
/// (its id carries no published prefix) and the published group's own templates
/// are untouched. A create whose `Name` collides with a published group's is
/// refused, because `Name` is unique per resource type in a region.
///
/// The rows are the identifier forms the create names the published group by,
/// and the names a colliding create asks for.
#[tokio::test]
async fn an_account_template_can_be_created_in_a_published_group() {
    for (case, by_name) in [("by the group's name", true), ("by the group's id", false)] {
        let router = medialive();
        let groups = published_by_name(&list_groups(&router, DEFAULT_REGION, "").await);
        let group = &groups[PUBLISHED_GROUPS[0]];
        let group_id = group["id"].as_str().unwrap_or_default().to_string();
        let before = group["templateCount"].as_i64().unwrap_or_default();
        let identifier = if by_name {
            PUBLISHED_GROUPS[0].to_string()
        } else {
            group_id.clone()
        };

        let mut body = crate::cloud_watch_alarm_template_listing::required_members("my-own-alarm");
        body["groupIdentifier"] = json!(identifier);
        let (status, created) =
            call_in_region(&router, DEFAULT_REGION, "POST", TEMPLATES, Some(body)).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected a create in a published group to answer 201, \
             got {status}: {created}"
        );
        assert_eq!(
            created["groupId"],
            json!(group_id),
            "case {case:?}: expected the created template to record the published group's id, \
             got {:?}",
            created["groupId"]
        );
        assert!(
            !is_aws_provided(&created),
            "case {case:?}: expected the account's own template to carry no published id, \
             got {created}"
        );

        let after = published_by_name(&list_groups(&router, DEFAULT_REGION, "").await)
            [PUBLISHED_GROUPS[0]]["templateCount"]
            .as_i64()
            .unwrap_or_default();
        assert_eq!(
            after,
            before + 1,
            "case {case:?}: expected the published group's templateCount to follow the create"
        );
        let own = account_owned(&list_templates(&router, DEFAULT_REGION, "").await);
        assert_eq!(
            own.len(),
            1,
            "case {case:?}: expected the one template the account created, got {own:?}"
        );
    }

    // A create that asks for a name a published group already holds is the
    // name-uniqueness conflict, not a second group.
    let router = medialive();
    for name in [PUBLISHED_GROUPS[0], PUBLISHED_GROUPS[9]] {
        let (status, body) = create_group_in(&router, DEFAULT_REGION, json!({ "name": name })).await;
        assert_eq!(
            status, 409,
            "case {name:?}: expected a name a published group holds to be refused, \
             got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "ConflictException",
            "case {name:?}: expected ConflictException, got {body}"
        );
    }
}

/// Rule: a published monitor template resource belongs to the SERVICE, so none
/// of the account's writes change it: an update or a delete naming one is refused
/// with the modelled `BadRequestException` (400) whose message names the resource
/// type and the identifier the request used, and the catalog still reports the
/// resource exactly as it did before.
///
/// The rows are the four writes this resource family has, crossed with both
/// identifier forms.
#[tokio::test]
async fn a_published_resource_is_read_only() {
    let router = medialive();
    let group = published_by_name(&list_groups(&router, DEFAULT_REGION, "").await)
        [PUBLISHED_GROUPS[5]]
        .clone();
    let templates = list_templates(&router, DEFAULT_REGION, "").await;
    let template = templates
        .iter()
        .find(|template| template["groupId"] == group["id"])
        .expect("a published group publishes templates")
        .clone();

    let group_id = group["id"].as_str().unwrap_or_default().to_string();
    let group_name = group["name"].as_str().unwrap_or_default().to_string();
    let template_id = template["id"].as_str().unwrap_or_default().to_string();
    let template_name = template["name"].as_str().unwrap_or_default().to_string();

    // (case, method, collection, identifier, the resource type the message names)
    let writes: [(&str, &str, &str, &str, &str); 8] = [
        (
            "update a group by id",
            "PATCH",
            GROUPS,
            &group_id,
            "cloudwatch alarm template group",
        ),
        (
            "update a group by name",
            "PATCH",
            GROUPS,
            &group_name,
            "cloudwatch alarm template group",
        ),
        (
            "delete a group by id",
            "DELETE",
            GROUPS,
            &group_id,
            "cloudwatch alarm template group",
        ),
        (
            "delete a group by name",
            "DELETE",
            GROUPS,
            &group_name,
            "cloudwatch alarm template group",
        ),
        (
            "update a template by id",
            "PATCH",
            TEMPLATES,
            &template_id,
            "cloudwatch alarm template",
        ),
        (
            "update a template by name",
            "PATCH",
            TEMPLATES,
            &template_name,
            "cloudwatch alarm template",
        ),
        (
            "delete a template by id",
            "DELETE",
            TEMPLATES,
            &template_id,
            "cloudwatch alarm template",
        ),
        (
            "delete a template by name",
            "DELETE",
            TEMPLATES,
            &template_name,
            "cloudwatch alarm template",
        ),
    ];

    for (case, method, collection, identifier, resource_type) in writes {
        let body = if method == "PATCH" {
            Some(json!({ "description": "mine now" }))
        } else {
            None
        };
        let (status, answered) = call_in_region(
            &router,
            DEFAULT_REGION,
            method,
            &format!("{collection}/{}", percent_encoded(identifier)),
            body,
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a write to a published resource to be refused, \
             got {status}: {answered}"
        );
        assert_eq!(
            error_code(status, &answered),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {answered}"
        );
        assert_eq!(
            error_message(&answered),
            format!("{resource_type} {identifier} is provided by AWS and cannot be modified"),
            "case {case:?}: expected the message to name the resource type and the \
             identifier, got {:?}",
            error_message(&answered)
        );
    }

    // Every refused write left the catalog exactly as it was.
    assert_eq!(
        published_by_name(&list_groups(&router, DEFAULT_REGION, "").await)[PUBLISHED_GROUPS[5]],
        group,
        "expected the refused writes to leave the published group as it was"
    );
    let (status, read) = get_template(&router, DEFAULT_REGION, &template_id).await;
    assert_eq!(status, 200, "expected the published template to survive: {read}");
    for member in ["arn", "id", "groupId", "name", "description", "threshold"] {
        assert_eq!(
            read.get(member).cloned().unwrap_or(Value::Null),
            template.get(member).cloned().unwrap_or(Value::Null),
            "expected the refused writes to leave the published template's {member} as it was"
        );
    }
}
