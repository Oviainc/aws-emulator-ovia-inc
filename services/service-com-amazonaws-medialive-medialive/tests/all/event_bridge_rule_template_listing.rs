// SPDX-License-Identifier: CC0-1.0

//! `ListEventBridgeRuleTemplates`: which stored templates a listing reports, what
//! they look like read through the list shape, how a page is cut and which
//! templates a filter admits — end to end over the restJson1 wire.
//!
//! Every test drives the service the way a client does, through
//! `MultiServiceRouter::dispatch`, so what is asserted is the emulator's wire
//! behaviour rather than an internal function's return value. The suites are
//! TABLES: the recorded unit this issue carries (one `ListEventBridgeRuleTemplates`
//! call over a store a create and a rejected duplicate have written to) is ONE row
//! of the first two of them.

use serde_json::{json, Value};
use service_com_amazonaws_medialive_medialive::state::MAX_LIST_PAGE_SIZE;
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{entry_point, timestamp};
use crate::wire::{call_in_region, error_code, medialive, ACCOUNT, DEFAULT_REGION};

/// The collection URI `ListEventBridgeRuleTemplates` and
/// `CreateEventBridgeRuleTemplate` are bound to
/// (`smithy_query(query="operation", operation="ListEventBridgeRuleTemplates")`:
/// GET /prod/eventbridge-rule-templates).
const TEMPLATES: &str = "/prod/eventbridge-rule-templates";

/// The group collection every template here is created inside
/// (`smithy_query(query="operation",
/// operation="CreateEventBridgeRuleTemplateGroup")`).
const GROUPS: &str = "/prod/eventbridge-rule-template-groups";

/// The signal-map collection, whose `EventBridgeRuleTemplateGroupIdentifiers`
/// are what a `signalMapIdentifier` filter resolves through
/// (`smithy_query(query="operation", operation="CreateSignalMap")`).
const SIGNAL_MAPS: &str = "/prod/signal-maps";

/// Every `EventBridgeRuleTemplateEventType` the model declares, spelled exactly
/// as it declares them (`smithy_query(query="shape",
/// shape="EventBridgeRuleTemplateEventType")`). The rows below pick values the
/// issue did not cite, not only one.
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

/// The most targets one template may carry:
/// `EventBridgeRuleTemplateSummary$EventTargetCount` ("The number of targets
/// configured to send matching events") targets `__integerMax5`
/// (`smithy_query(query="shape", shape="EventBridgeRuleTemplateSummary")`).
const MAX_TARGETS: usize = 5;

/// `ListEventBridgeRuleTemplates` with `query` appended verbatim (all four input
/// members are `@httpQuery`), signed for `region`.
async fn list_in(router: &MultiServiceRouter, region: &str, query: &str) -> (u16, Value) {
    call_in_region(router, region, "GET", &format!("{TEMPLATES}{query}"), None).await
}

/// The same listing, signed for [`DEFAULT_REGION`].
async fn list(router: &MultiServiceRouter, query: &str) -> (u16, Value) {
    list_in(router, DEFAULT_REGION, query).await
}

/// The `eventBridgeRuleTemplates` member as a list, whatever its length.
fn summaries(case: &str, body: &Value) -> Vec<Value> {
    body["eventBridgeRuleTemplates"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected an eventBridgeRuleTemplates list, got {body}")
        })
}

/// `CreateEventBridgeRuleTemplateGroup` in `region`, answering the group's id.
async fn create_group(router: &MultiServiceRouter, region: &str, name: &str) -> String {
    let (status, group) =
        call_in_region(router, region, "POST", GROUPS, Some(json!({ "name": name }))).await;
    assert_eq!(
        status, 201,
        "setup: expected the group {name:?} in {region} to be created, got {status}: {group}"
    );
    group["id"].as_str().unwrap_or_default().to_string()
}

/// `CreateEventBridgeRuleTemplate` in `region`, answering the created template.
async fn create_template(router: &MultiServiceRouter, case: &str, region: &str, body: Value) -> Value {
    let (status, created) = call_in_region(router, region, "POST", TEMPLATES, Some(body)).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected CreateEventBridgeRuleTemplate to answer 201, got {status}: {created}"
    );
    created
}

/// `GetEventBridgeRuleTemplate` on `identifier`, signed for `region`.
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

/// `CreateSignalMap` associated with the template groups `rule_groups` names,
/// answering the created map's id.
async fn create_signal_map(
    router: &MultiServiceRouter,
    name: &str,
    entry: &str,
    rule_groups: &[&str],
) -> String {
    let mut body = json!({ "name": name, "discoveryEntryPointArn": entry });
    if !rule_groups.is_empty() {
        body["eventBridgeRuleTemplateGroupIdentifiers"] = json!(rule_groups);
    }
    let (status, created) = call_in_region(
        router,
        DEFAULT_REGION,
        "POST",
        SIGNAL_MAPS,
        Some(body),
    )
    .await;
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
            "case {case:?}: expected ListEventBridgeRuleTemplates to answer 200, \
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
            let template = create_template(
                &router,
                case,
                DEFAULT_REGION,
                json!({
                    "name": format!("tpl-{index}"),
                    "eventType": EVENT_TYPES[index % EVENT_TYPES.len()],
                    "groupIdentifier": "paging-group",
                }),
            )
            .await;
            created.push(template["id"].as_str().unwrap_or_default().to_string());
        }

        let (listed, pages) = every_page(&router, case, "", page_size).await;
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

/// One row of the summary table: what the create supplies beyond the name, the
/// group and the event type.
struct Stored {
    case: &'static str,
    event_type: &'static str,
    targets: usize,
    description: Option<&'static str>,
    tags: Option<Value>,
}

/// Rule: a listed `EventBridgeRuleTemplateSummary` is the stored template read
/// through the list shape — the same `Arn`, `Id`, `Name`, `Description`,
/// `EventType`, `GroupId`, `Tags` and timestamps `GetEventBridgeRuleTemplate`
/// reports for it — and its `EventTargetCount` is the number of targets the
/// template carries.
///
/// The rows differ along the NUMBER of targets (none, one, and the most the model
/// counts), the event type, and which optional members the stored template
/// carries, because that is what the rule generalises over.
#[tokio::test]
async fn a_listed_summary_reads_the_stored_template() {
    let table = [
        Stored {
            case: "required members only",
            event_type: EVENT_TYPES[2],
            targets: 0,
            description: None,
            tags: None,
        },
        Stored {
            case: "one target",
            event_type: EVENT_TYPES[8],
            targets: 1,
            description: None,
            tags: None,
        },
        Stored {
            case: "described and tagged, two targets",
            event_type: EVENT_TYPES[11],
            targets: 2,
            description: Some("page the on-call"),
            tags: Some(json!({ "team": "video", "tier": "gold" })),
        },
        Stored {
            case: "the most targets the model counts",
            event_type: EVENT_TYPES[0],
            targets: MAX_TARGETS,
            description: Some(""),
            tags: Some(json!({ "env": "test" })),
        },
    ];

    for Stored {
        case,
        event_type,
        targets,
        description,
        tags,
    } in table
    {
        let router = medialive();
        let group_id = create_group(&router, DEFAULT_REGION, "summary-group").await;
        let name = format!("tpl-{}", case.replace(' ', "-"));

        let mut body = json!({
            "name": name,
            "eventType": event_type,
            "groupIdentifier": "summary-group",
        });
        if targets > 0 {
            let arns: Vec<Value> = (0..targets)
                .map(|index| {
                    json!({ "arn": format!("arn:aws:sns:{DEFAULT_REGION}:{ACCOUNT}:topic-{index}") })
                })
                .collect();
            body["eventTargets"] = json!(arns);
        }
        if let Some(description) = description {
            body["description"] = json!(description);
        }
        if let Some(tags) = &tags {
            body["tags"] = tags.clone();
        }
        let created = create_template(&router, case, DEFAULT_REGION, body).await;
        let id = created["id"].as_str().unwrap_or_default().to_string();

        let (status, listed) = list(&router, "").await;
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
            "case {case:?}: expected GetEventBridgeRuleTemplate to answer 200, \
             got {status}: {read}"
        );

        // Every member the summary shares with the flat template shape
        // (`smithy_query(query="shape", shape="EventBridgeRuleTemplateSummary")`)
        // reports the same stored value the read does.
        for member in [
            "arn",
            "id",
            "name",
            "description",
            "eventType",
            "groupId",
            "tags",
        ] {
            assert_eq!(
                summary.get(member).cloned().unwrap_or(Value::Null),
                read.get(member).cloned().unwrap_or(Value::Null),
                "case {case:?}: expected the listed {member} to equal the one \
                 GetEventBridgeRuleTemplate reports, got {:?} vs {:?}",
                summary.get(member),
                read.get(member)
            );
        }
        for member in ["createdAt", "modifiedAt"] {
            assert_eq!(
                timestamp(case, member, summary),
                timestamp(case, member, &read),
                "case {case:?}: expected the listed {member} to equal the one \
                 GetEventBridgeRuleTemplate reports, got {:?} vs {:?}",
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
        assert_eq!(
            summary["eventTargetCount"],
            json!(targets),
            "case {case:?}: expected the number of stored targets, got {:?}",
            summary["eventTargetCount"]
        );
        // The count is the length of the target list the read reports, so the
        // two readings of one stored template cannot disagree.
        let read_targets = read["eventTargets"].as_array().map_or(0, Vec::len);
        assert_eq!(
            read_targets, targets,
            "case {case:?}: expected GetEventBridgeRuleTemplate to report {targets} targets, \
             got {:?}",
            read["eventTargets"]
        );
    }
}

/// Rule: a `GroupIdentifier` narrows the listing to the templates stored in the
/// group it resolves to, a `SignalMapIdentifier` narrows it to the templates of
/// the groups that signal map is associated with, each identifier resolving by
/// the resource's id or its current name, and naming both narrows it to the
/// templates both admit.
///
/// The rows differ along WHICH filter is named, in which of the two forms, and
/// over a store whose templates are spread across three groups of which a signal
/// map names two, because that is what the rule generalises over. Each row is
/// walked a page at a time as well, so the filter and the page cut are exercised
/// together.
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
    for (index, (name, group)) in stored.into_iter().enumerate() {
        create_template(
            &router,
            name,
            DEFAULT_REGION,
            json!({
                "name": name,
                "eventType": EVENT_TYPES[index],
                "groupIdentifier": group,
            }),
        )
        .await;
    }

    // One map associated with both named groups, one with only the second, one
    // with no group at all.
    let both = create_signal_map(&router, "map-both", &entry, &["group-one", &second]).await;
    let only_second = create_signal_map(&router, "map-second", &entry, &["group-two"]).await;
    let unassociated = create_signal_map(&router, "map-none", &entry, &[]).await;

    // (case, query, the template names the filter must admit)
    let table: [(&str, String, &[&str]); 11] = [
        (
            "no filter",
            String::new(),
            &["one-a", "one-b", "spare-a", "two-a", "two-b"],
        ),
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
            "both filters, the group among the map's",
            format!("?groupIdentifier={first}&signalMapIdentifier={both}"),
            &["one-a", "one-b"],
        ),
        (
            "both filters, the group outside the map's",
            format!("?groupIdentifier={spare}&signalMapIdentifier={both}"),
            &[],
        ),
    ];

    for (case, query, expected) in table {
        let (status, body) = list(&router, &query).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status}: {body}"
        );
        let mut names: Vec<String> = summaries(case, &body)
            .iter()
            .map(|summary| summary["name"].as_str().unwrap_or_default().to_string())
            .collect();
        names.sort();
        assert_eq!(
            names, expected,
            "case {case:?}: expected the filter to admit {expected:?}, got {names:?}"
        );

        // The same filter, paged one template at a time, admits the same set:
        // the page cut narrows nothing of its own.
        let (ids, pages) = every_page(&router, case, &query, Some(1)).await;
        assert_eq!(
            ids.len(),
            expected.len(),
            "case {case:?}: expected {} templates across the pages {pages:?}, got {ids:?}",
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
        for (index, name) in names.iter().enumerate() {
            create_template(
                &router,
                name,
                region,
                json!({
                    "name": name,
                    "eventType": EVENT_TYPES[index],
                    "groupIdentifier": "regional-group",
                }),
            )
            .await;
        }
    }

    for (region, names) in regions {
        let (status, body) = list_in(&router, region, "").await;
        assert_eq!(
            status, 200,
            "case {region:?}: expected 200, got {status}: {body}"
        );
        let mut listed: Vec<String> = summaries(region, &body)
            .iter()
            .map(|summary| summary["name"].as_str().unwrap_or_default().to_string())
            .collect();
        listed.sort();
        assert_eq!(
            listed, names,
            "case {region:?}: expected only the templates created in {region}, got {listed:?}"
        );
    }
}

/// Rule: a `MaxResults` that asks for no results at all is refused with
/// `BadRequestException` (400), and an identifier that resolves to no resource of
/// the filtered type where the request can see it is refused with
/// `NotFoundException` (404) naming the resource type and the identifier.
///
/// The rows differ along the page size asked for, which filter carries the
/// unresolvable identifier, and WHY it is unresolvable — never created, the id of
/// a resource of another type, or a group that exists in another region.
#[tokio::test]
async fn a_bad_page_size_or_an_unresolvable_filter_is_refused() {
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

    // (case, query member, identifier, the resource type the message names)
    let unresolvable: [(&str, &str, String, &str); 6] = [
        (
            "group never created",
            "groupIdentifier",
            "no-such-group".to_string(),
            "eventbridge rule template group",
        ),
        (
            "group filter naming a signal map's id",
            "groupIdentifier",
            map.clone(),
            "eventbridge rule template group",
        ),
        (
            "group by a name nothing holds",
            "groupIdentifier",
            "map-one".to_string(),
            "eventbridge rule template group",
        ),
        (
            "signal map never created",
            "signalMapIdentifier",
            "no-such-map".to_string(),
            "Signal map",
        ),
        (
            "signal map filter naming a group's id",
            "signalMapIdentifier",
            group.clone(),
            "Signal map",
        ),
        (
            "signal map by a name nothing holds",
            "signalMapIdentifier",
            "group-one".to_string(),
            "Signal map",
        ),
    ];

    for (case, member, identifier, resource_type) in unresolvable {
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
        assert_eq!(
            body["message"],
            json!(format!("{resource_type} {identifier} not found")),
            "case {case:?}: expected the message to name the resource type then the \
             identifier, got {:?}",
            body["message"]
        );
    }

    // A group belongs to the region it was created in, so the same identifier
    // resolves in the region that created it and nowhere else.
    let query = format!("?groupIdentifier={group}");
    let (status, body) = list(&router, &query).await;
    assert_eq!(
        status, 200,
        "case \"group of its own region\": expected the filter to resolve where the \
         group was created, got {status}: {body}"
    );
    let (status, body) = list_in(&router, "eu-west-1", &query).await;
    assert_eq!(
        status, 404,
        "case \"group of another region\": expected a group of another region to \
         resolve to nothing, got {status}: {body}"
    );
    assert_eq!(
        error_code(status, &body),
        "NotFoundException",
        "case \"group of another region\": expected NotFoundException, got {body}"
    );
}
