// SPDX-License-Identifier: CC0-1.0

//! `ListCloudWatchAlarmTemplates`: which stored templates a listing reports, what
//! they look like read through the list shape, how a page is cut and which
//! templates a filter admits — end to end over the restJson1 wire.
//!
//! Every test drives the service the way a client does, through
//! `MultiServiceRouter::dispatch`, so what is asserted is the emulator's wire
//! behaviour rather than an internal function's return value. The suites are
//! TABLES whose rows differ along what the rule generalises over: the number of
//! stored templates and the page size asked for, the members a stored template
//! carries, which filter is named and in which of its two identifier forms, and
//! the region the request is signed for.

use serde_json::{json, Value};
use service_com_amazonaws_medialive_medialive::state::MAX_LIST_PAGE_SIZE;
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{
    account_owned_names, create_in as create_group_in, entry_point, timestamp,
    AWS_PROVIDED_ID_PREFIX,
};
use crate::wire::{call_in_region, error_code, medialive, DEFAULT_REGION};

/// The `Scope` that narrows a listing to the templates LOCAL to this account,
/// which the suites whose subject is what the CALLER created list with: the
/// service publishes a catalog of its own templates in every region, and those
/// are the subject of `managed_monitor_catalog.rs` instead.
const LOCAL_ONLY: &str = "?scope=LOCAL";

/// The collection URI `ListCloudWatchAlarmTemplates` and
/// `CreateCloudWatchAlarmTemplate` are bound to
/// (`smithy_query(query="operation", operation="ListCloudWatchAlarmTemplates")`:
/// GET /prod/cloudwatch-alarm-templates).
const TEMPLATES: &str = "/prod/cloudwatch-alarm-templates";

/// The signal-map collection, whose `CloudWatchAlarmTemplateGroupIdentifiers`
/// are what a `signalMapIdentifier` filter resolves through
/// (`smithy_query(query="operation", operation="CreateSignalMap")`).
const SIGNAL_MAPS: &str = "/prod/signal-maps";

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

/// The bounds `Period` carries, from its target shape `__integerMin10Max86400`
/// ("The period, in seconds, over which the specified statistic is applied").
const MIN_PERIOD: i64 = 10;
const MAX_PERIOD: i64 = 86_400;

/// `ListCloudWatchAlarmTemplates` with `query` appended verbatim (every input
/// member is an `@httpQuery`), signed for `region`.
async fn list_in(router: &MultiServiceRouter, region: &str, query: &str) -> (u16, Value) {
    call_in_region(router, region, "GET", &format!("{TEMPLATES}{query}"), None).await
}

/// The same listing, signed for [`DEFAULT_REGION`].
async fn list(router: &MultiServiceRouter, query: &str) -> (u16, Value) {
    list_in(router, DEFAULT_REGION, query).await
}

/// The `cloudWatchAlarmTemplates` member as a list, whatever its length.
fn summaries(case: &str, body: &Value) -> Vec<Value> {
    body["cloudWatchAlarmTemplates"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a cloudWatchAlarmTemplates list, got {body}")
        })
}

/// `CreateCloudWatchAlarmTemplateGroup` in `region`, answering the group's id.
async fn create_group(router: &MultiServiceRouter, region: &str, name: &str) -> String {
    let (status, group) = create_group_in(router, region, json!({ "name": name })).await;
    assert_eq!(
        status, 201,
        "setup: expected the group {name:?} in {region} to be created, got {status}: {group}"
    );
    group["id"].as_str().unwrap_or_default().to_string()
}

/// Every REQUIRED member of `CreateCloudWatchAlarmTemplate` besides
/// `GroupIdentifier`, with one modelled value each, so a row that varies ONE
/// member still sends a complete request.
pub fn required_members(name: &str) -> Value {
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

/// `CreateCloudWatchAlarmTemplate` in `region`, answering the created template.
async fn create_template(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    body: Value,
) -> Value {
    let (status, created) = call_in_region(router, region, "POST", TEMPLATES, Some(body)).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected CreateCloudWatchAlarmTemplate to answer 201, got {status}: {created}"
    );
    created
}

/// One template of the required members only, in the group `group` of `region`.
async fn create_plain(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    name: &str,
    group: &str,
) -> Value {
    let mut body = required_members(name);
    body["groupIdentifier"] = json!(group);
    create_template(router, case, region, body).await
}

/// `GetCloudWatchAlarmTemplate` on `identifier`, signed for `region`.
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

/// `CreateSignalMap` associated with the alarm template groups `groups` names,
/// answering the created map's id.
async fn create_signal_map(
    router: &MultiServiceRouter,
    name: &str,
    entry: &str,
    groups: &[&str],
) -> String {
    let mut body = json!({ "name": name, "discoveryEntryPointArn": entry });
    if !groups.is_empty() {
        body["cloudWatchAlarmTemplateGroupIdentifiers"] = json!(groups);
    }
    let (status, created) =
        call_in_region(router, DEFAULT_REGION, "POST", SIGNAL_MAPS, Some(body)).await;
    assert_eq!(
        status, 201,
        "setup: expected the signal map {name:?} to be created, got {status}: {created}"
    );
    created["id"].as_str().unwrap_or_default().to_string()
}

/// Walk every page of a listing, answering the ids reported in order and the
/// size of each page.
async fn every_page(
    router: &MultiServiceRouter,
    case: &str,
    query: &str,
    page_size: Option<i32>,
) -> (Vec<String>, Vec<usize>) {
    let mut ids: Vec<String> = Vec::new();
    let mut pages: Vec<usize> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut full = query.to_string();
        if let Some(max_results) = page_size {
            full.push(if full.is_empty() { '?' } else { '&' });
            full.push_str(&format!("maxResults={max_results}"));
        }
        if let Some(next) = &token {
            full.push(if full.is_empty() { '?' } else { '&' });
            full.push_str(&format!("nextToken={next}"));
        }

        let (status, body) = list(router, &full).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListCloudWatchAlarmTemplates to answer 200, \
             got {status}: {body}"
        );
        let page = summaries(case, &body);
        let allowed = page_size.map_or(MAX_LIST_PAGE_SIZE, |size| {
            (size as usize).min(MAX_LIST_PAGE_SIZE)
        });
        assert!(
            page.len() <= allowed,
            "case {case:?}: expected at most {allowed} summaries in a page, got {}",
            page.len()
        );
        pages.push(page.len());
        for summary in &page {
            ids.push(summary["id"].as_str().unwrap_or_default().to_string());
        }

        token = body["nextToken"].as_str().map(ToString::to_string);
        // A token is handed out only when something is left over, so an empty
        // page can never carry one.
        if token.is_some() {
            assert!(
                !page.is_empty(),
                "case {case:?}: expected no nextToken beside an empty page, got {body}"
            );
        }
        if token.is_none() {
            break;
        }
        assert!(
            pages.len() <= 64,
            "case {case:?}: expected pagination to terminate, got pages {pages:?}"
        );
    }
    (ids, pages)
}

/// One row of the pagination table: how many templates the store holds and the
/// page size the request asks for.
struct Paging {
    case: &'static str,
    templates: usize,
    page_size: Option<i32>,
}

/// Rule: a template listing reports the templates the store holds in the region
/// the request was signed for, each exactly once, in the order their ids were
/// minted, cut into pages of at most `MaxResults` (bounded by the service's own
/// ceiling) whose `NextToken` resumes at the first template not yet reported and
/// is absent on the last page.
///
/// The rows differ along the COUNT of stored templates and the requested page
/// size — including none stored, a page size of one, a size that divides the
/// store unevenly, no size at all and a size past the service's ceiling —
/// because that is what the rule generalises over.
#[tokio::test]
async fn listing_pages_through_every_stored_template_in_mint_order() {
    let table = [
        Paging {
            case: "empty store",
            templates: 0,
            page_size: None,
        },
        Paging {
            case: "empty store, page of one",
            templates: 0,
            page_size: Some(1),
        },
        Paging {
            case: "one template, page of one",
            templates: 1,
            page_size: Some(1),
        },
        Paging {
            case: "one template, no page size",
            templates: 1,
            page_size: None,
        },
        Paging {
            case: "three templates, page of one",
            templates: 3,
            page_size: Some(1),
        },
        Paging {
            case: "five templates, page of two",
            templates: 5,
            page_size: Some(2),
        },
        Paging {
            case: "four templates, default page size",
            templates: 4,
            page_size: None,
        },
        Paging {
            case: "three templates, page size past the ceiling",
            templates: 3,
            page_size: Some(MAX_LIST_PAGE_SIZE as i32 + 500),
        },
    ];

    for Paging {
        case,
        templates,
        page_size,
    } in table
    {
        let router = medialive();
        create_group(&router, DEFAULT_REGION, "paging-group").await;

        let mut created: Vec<String> = Vec::new();
        for index in 0..templates {
            let template = create_plain(
                &router,
                case,
                DEFAULT_REGION,
                &format!("tpl-{index}"),
                "paging-group",
            )
            .await;
            created.push(template["id"].as_str().unwrap_or_default().to_string());
        }

        let (listed, pages) = every_page(&router, case, LOCAL_ONLY, page_size).await;
        assert_eq!(
            listed, created,
            "case {case:?}: expected the listing to report every stored template once, \
             in mint order; pages were {pages:?}"
        );

        // The page cut itself: with a page size of `n` every page but the last
        // carries exactly `n`.
        if let Some(size) = page_size.map(|size| (size as usize).min(MAX_LIST_PAGE_SIZE)) {
            let expected_pages = if templates == 0 {
                1
            } else {
                templates.div_ceil(size)
            };
            assert_eq!(
                pages.len(),
                expected_pages,
                "case {case:?}: expected {expected_pages} pages for {templates} templates \
                 of {size}, got {pages:?}"
            );
            for (index, page) in pages.iter().enumerate() {
                let expected = if index + 1 == pages.len() {
                    templates - size * index
                } else {
                    size
                };
                assert_eq!(
                    *page, expected,
                    "case {case:?}: expected page {index} to carry {expected} summaries, \
                     got {pages:?}"
                );
            }
        }
    }
}

/// One row of the summary table: what the create varies beyond the name and the
/// group.
struct Stored {
    case: &'static str,
    overrides: Value,
}

/// Rule: a listed `CloudWatchAlarmTemplateSummary` is the stored template read
/// through the list shape — every member the summary declares reports the same
/// stored value `GetCloudWatchAlarmTemplate` reports for that template.
///
/// The rows differ along WHICH of the template's settings the create varies and
/// which optional members it carries — each modelled enum taken at an entry the
/// issue did not cite, the `Period` bounds at both published ends, and a
/// described and tagged template beside a bare one — because that is what the
/// rule generalises over.
#[tokio::test]
async fn a_listed_summary_reads_the_stored_template() {
    let table = [
        Stored {
            case: "required members only",
            overrides: json!({}),
        },
        Stored {
            case: "described and tagged",
            overrides: json!({
                "description": "page the on-call",
                "tags": { "team": "video", "tier": "gold" },
            }),
        },
        Stored {
            case: "the other enum entries, m out of n",
            overrides: json!({
                "comparisonOperator": COMPARISON_OPERATORS[2],
                "statistic": STATISTICS[4],
                "targetResourceType": TARGET_RESOURCE_TYPES[8],
                "treatMissingData": TREAT_MISSING_DATA[0],
                "evaluationPeriods": 5,
                "datapointsToAlarm": 2,
            }),
        },
        Stored {
            case: "the shortest period",
            overrides: json!({ "period": MIN_PERIOD, "threshold": 0.0 }),
        },
        Stored {
            case: "the longest period",
            overrides: json!({
                "period": MAX_PERIOD,
                "threshold": -17.5,
                "metricName": "ActiveAlerts",
                "description": "",
            }),
        },
    ];

    for Stored { case, overrides } in table {
        let router = medialive();
        let group_id = create_group(&router, DEFAULT_REGION, "summary-group").await;
        let name = format!("tpl-{}", case.replace(' ', "-"));

        let mut body = required_members(&name);
        body["groupIdentifier"] = json!("summary-group");
        if let Some(members) = overrides.as_object() {
            for (key, value) in members {
                body[key.as_str()] = value.clone();
            }
        }
        let created = create_template(&router, case, DEFAULT_REGION, body).await;
        let id = created["id"].as_str().unwrap_or_default().to_string();

        let (status, listed) = list(&router, LOCAL_ONLY).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status}: {listed}"
        );
        let page = summaries(case, &listed);
        assert_eq!(
            page.len(),
            1,
            "case {case:?}: expected the one stored template, got {listed}"
        );
        let summary = &page[0];

        let (status, read) = get(&router, DEFAULT_REGION, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetCloudWatchAlarmTemplate to answer 200, \
             got {status}: {read}"
        );

        // Every member `CloudWatchAlarmTemplateSummary` declares
        // (`smithy_query(query="shape", shape="CloudWatchAlarmTemplateSummary")`)
        // reports the same stored value the read does.
        for member in [
            "arn",
            "comparisonOperator",
            "datapointsToAlarm",
            "description",
            "evaluationPeriods",
            "groupId",
            "id",
            "metricName",
            "name",
            "period",
            "statistic",
            "tags",
            "targetResourceType",
            "treatMissingData",
        ] {
            assert_eq!(
                summary.get(member).cloned().unwrap_or(Value::Null),
                read.get(member).cloned().unwrap_or(Value::Null),
                "case {case:?}: expected the listed {member} to equal the one \
                 GetCloudWatchAlarmTemplate reports, got {:?} vs {:?}",
                summary.get(member),
                read.get(member)
            );
        }
        // `Threshold` is a `__double`, so the two readings are compared as
        // numbers: an integral value and a fractional one travel as different
        // JSON number forms.
        let listed_threshold = summary["threshold"].as_f64();
        assert_eq!(
            listed_threshold,
            read["threshold"].as_f64(),
            "case {case:?}: expected the listed threshold to equal the one \
             GetCloudWatchAlarmTemplate reports, got {:?} vs {:?}",
            summary["threshold"],
            read["threshold"]
        );
        for member in ["createdAt", "modifiedAt"] {
            assert_eq!(
                timestamp(case, member, summary),
                timestamp(case, member, &read),
                "case {case:?}: expected the listed {member} to equal the one \
                 GetCloudWatchAlarmTemplate reports, got {:?} vs {:?}",
                summary.get(member),
                read.get(member)
            );
        }
        assert_eq!(
            summary["groupId"],
            json!(group_id),
            "case {case:?}: expected the id of the group the template was created in, got {:?}",
            summary["groupId"]
        );
    }
}

/// Rule: a `GroupIdentifier` narrows the listing to the templates stored in the
/// group it resolves to, a `SignalMapIdentifier` narrows it to the templates of
/// the groups that signal map is associated with, a `Scope` narrows it to the
/// templates of that provenance, each identifier resolving by the resource's id
/// or its current name, and naming several narrows the listing to the templates
/// all of them admit.
///
/// The rows differ along WHICH filter is named, in which of the two identifier
/// forms, in which case a scope option is spelled, and over a store whose
/// templates are spread across three groups of which a signal map names two,
/// because that is what the rule generalises over. Each row is walked a page at
/// a time as well, so the filter and the page cut are exercised together.
#[tokio::test]
async fn a_filter_lists_only_the_templates_it_admits() {
    let router = medialive();
    let entry = entry_point(&router, "entry").await;

    let first = create_group(&router, DEFAULT_REGION, "group-one").await;
    let second = create_group(&router, DEFAULT_REGION, "group-two").await;
    let spare = create_group(&router, DEFAULT_REGION, "group-spare").await;

    // (template name, the group it is created in)
    let stored: [(&str, &str); 5] = [
        ("one-a", "group-one"),
        ("one-b", "group-one"),
        ("two-a", "group-two"),
        ("two-b", "group-two"),
        ("spare-a", "group-spare"),
    ];
    for (name, group) in stored {
        create_plain(&router, name, DEFAULT_REGION, name, group).await;
    }

    // One map associated with both named groups, one with only the second, one
    // with no group at all.
    let both = create_signal_map(&router, "map-both", &entry, &["group-one", &second]).await;
    let only_second = create_signal_map(&router, "map-second", &entry, &["group-two"]).await;
    let unassociated = create_signal_map(&router, "map-none", &entry, &[]).await;

    let every = ["one-a", "one-b", "spare-a", "two-a", "two-b"];

    // (case, query, the template names the filter must admit)
    let table: [(&str, String, &[&str]); 18] = [
        ("no filter", String::new(), &every),
        (
            "group by name",
            "?groupIdentifier=group-one".to_string(),
            &["one-a", "one-b"],
        ),
        (
            "group by id",
            format!("?groupIdentifier={first}"),
            &["one-a", "one-b"],
        ),
        (
            "the other group",
            format!("?groupIdentifier={second}"),
            &["two-a", "two-b"],
        ),
        (
            "a group holding one template",
            format!("?groupIdentifier={spare}"),
            &["spare-a"],
        ),
        (
            "signal map by id, two groups",
            format!("?signalMapIdentifier={both}"),
            &["one-a", "one-b", "two-a", "two-b"],
        ),
        (
            "signal map by name, two groups",
            "?signalMapIdentifier=map-both".to_string(),
            &["one-a", "one-b", "two-a", "two-b"],
        ),
        (
            "signal map naming one group",
            format!("?signalMapIdentifier={only_second}"),
            &["two-a", "two-b"],
        ),
        (
            "signal map naming no group",
            format!("?signalMapIdentifier={unassociated}"),
            &[],
        ),
        (
            "both identifier filters, the group among the map's",
            format!("?groupIdentifier={first}&signalMapIdentifier={both}"),
            &["one-a", "one-b"],
        ),
        (
            "both identifier filters, the group outside the map's",
            format!("?groupIdentifier={spare}&signalMapIdentifier={both}"),
            &[],
        ),
        // `Scope` is the provenance filter: every template this emulator MINTS
        // is created by the caller, so the local scope admits them all and the
        // AWS-provided scope admits none of them (it admits the service's own
        // published templates, which `managed_monitor_catalog.rs` asserts).
        ("every scope", "?scope=ALL".to_string(), &every),
        ("the local scope", "?scope=LOCAL".to_string(), &every),
        ("the AWS-provided scope", "?scope=AWS".to_string(), &[]),
        // The model fixes no spelling for the option words, so each is named
        // whatever case the caller spells it in.
        ("every scope, lower case", "?scope=all".to_string(), &every),
        (
            "the local scope, mixed case",
            "?scope=Local".to_string(),
            &every,
        ),
        (
            "the AWS-provided scope, lower case",
            "?scope=aws".to_string(),
            &[],
        ),
        (
            "a scope beside a group filter",
            format!("?scope=LOCAL&groupIdentifier={second}"),
            &["two-a", "two-b"],
        ),
    ];

    for (case, query, expected) in table {
        let (status, body) = list(&router, &query).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status}: {body}"
        );
        // The ACCOUNT's templates among what the listing reports: the service's
        // own published templates are admitted by every row that does not narrow
        // the provenance or name one of the caller's groups, and which of the
        // CALLER's templates a filter admits is this suite's subject.
        let names = account_owned_names(&summaries(case, &body));
        assert_eq!(
            names, expected,
            "case {case:?}: expected the filter to admit {expected:?}, got {names:?}"
        );

        // The same filter, paged one template at a time, admits the same set:
        // the page cut narrows nothing of its own.
        let (ids, pages) = every_page(&router, case, &query, Some(1)).await;
        let account = ids
            .iter()
            .filter(|id| !id.starts_with(AWS_PROVIDED_ID_PREFIX))
            .count();
        assert_eq!(
            account,
            expected.len(),
            "case {case:?}: expected {} of the account's templates across the pages \
             {pages:?}, got {ids:?}",
            expected.len()
        );
    }
}

/// Rule: a template is visible only to a request signed for the region it was
/// created in, so the same listing reports a different set per region.
///
/// The rows are the regions the templates are created in; nothing about them
/// matters except that they differ, and that one region holds none.
#[tokio::test]
async fn a_listing_reports_only_the_templates_of_its_own_region() {
    let router = medialive();
    // (region, the names created there)
    let regions: [(&str, &[&str]); 3] = [
        ("us-west-2", &["west-a", "west-b"]),
        ("eu-central-1", &["central-a"]),
        ("ap-south-1", &[]),
    ];

    for (region, names) in regions {
        create_group(&router, region, "regional-group").await;
        for name in names {
            create_plain(&router, name, region, name, "regional-group").await;
        }
    }

    for (region, names) in regions {
        let (status, body) = list_in(&router, region, "").await;
        assert_eq!(
            status, 200,
            "case {region:?}: expected 200, got {status}: {body}"
        );
        let listed = account_owned_names(&summaries(region, &body));
        assert_eq!(
            listed, names,
            "case {region:?}: expected only the templates created in {region}, got {listed:?}"
        );
    }
}

/// Rule: a `MaxResults` that asks for no results at all, and a `Scope` naming
/// none of the options its member documents, are refused with
/// `BadRequestException` (400); an identifier that resolves to no resource of
/// the filtered type where the request can see it is refused with
/// `NotFoundException` (404) — carrying no `message` member when the
/// unresolvable resource belongs to the cloudwatch alarm-template hierarchy
/// (the `GroupIdentifier` filter), and this family's own message, the resource's
/// label then the identifier then `not found`, when it belongs to another family
/// (the `SignalMapIdentifier` filter).
///
/// The rows differ along the page size asked for, the scope spelling, which
/// filter carries the unresolvable identifier and WHY it is unresolvable — never
/// created, the id of a resource of another type, or a name nothing in this
/// region holds.
#[tokio::test]
async fn a_bad_page_size_scope_or_unresolvable_filter_is_refused() {
    let router = medialive();
    let entry = entry_point(&router, "entry").await;
    let group = create_group(&router, DEFAULT_REGION, "group-one").await;
    let map = create_signal_map(&router, "map-one", &entry, &["group-one"]).await;

    for max_results in [0, -1, -50] {
        let (status, body) = list(&router, &format!("?maxResults={max_results}")).await;
        assert_eq!(
            status, 400,
            "case {max_results}: expected a non-positive maxResults to be refused, \
             got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "BadRequestException",
            "case {max_results}: expected BadRequestException, got {body}"
        );
    }

    for scope in ["SCOPE_ALL", "", "GLOBAL", "LOCALS", "AWS-PROVIDED", "any"] {
        let (status, body) = list(&router, &format!("?scope={scope}")).await;
        assert_eq!(
            status, 400,
            "case {scope:?}: expected a scope naming none of the documented options to be \
             refused, got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "BadRequestException",
            "case {scope:?}: expected BadRequestException, got {body}"
        );
    }

    // (case, query member, identifier, the message the refusal carries — absent
    //  for a resource of the alarm-template hierarchy)
    let unresolvable: [(&str, &str, String, Option<&str>); 6] = [
        (
            "group never created",
            "groupIdentifier",
            "no-such-group".to_string(),
            None,
        ),
        (
            "group filter naming a signal map's id",
            "groupIdentifier",
            map.clone(),
            None,
        ),
        (
            "group by a name nothing holds",
            "groupIdentifier",
            "map-one".to_string(),
            None,
        ),
        (
            "signal map never created",
            "signalMapIdentifier",
            "no-such-map".to_string(),
            Some("Signal map"),
        ),
        (
            "signal map filter naming a group's id",
            "signalMapIdentifier",
            group.clone(),
            Some("Signal map"),
        ),
        (
            "signal map by a name nothing holds",
            "signalMapIdentifier",
            "group-one".to_string(),
            Some("Signal map"),
        ),
    ];

    for (case, member, identifier, label) in unresolvable {
        let (status, body) = list(&router, &format!("?{member}={identifier}")).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected an identifier naming nothing to be refused, \
             got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {body}"
        );
        let expected = match label {
            Some(label) => json!(format!("{label} {identifier} not found")),
            None => Value::Null,
        };
        assert_eq!(
            body["message"], expected,
            "case {case:?}: expected the message {expected:?}, got {:?}",
            body["message"]
        );
    }

    // A refused listing wrote nothing: the store still holds the templates the
    // setup created, which is none of them.
    let (status, body) = list(&router, LOCAL_ONLY).await;
    assert_eq!(status, 200, "the unfiltered listing got {status}: {body}");
    assert!(
        summaries("after refusals", &body).is_empty(),
        "expected the refused listings to have stored no template, got {body}"
    );
}
