// SPDX-License-Identifier: CC0-1.0

//! `ListSignalMaps`: what the stored signal maps look like read through the list
//! shape, how a page is cut, and which maps a template-group filter admits —
//! end to end over the restJson1 wire.
//!
//! Every test drives the service the way a client does, through
//! `MultiServiceRouter::dispatch`, so what is asserted is the emulator's wire
//! behaviour. The suites are TABLES: the issue's recorded case (one
//! `ListSignalMaps` call on a store nothing has created a map in) is one row of
//! the first of them.

use serde_json::{json, Value};
use service_com_amazonaws_medialive_medialive::state::MAX_LIST_PAGE_SIZE;
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::wait_for_the_clock;
use crate::support::{entry_point, timestamp};
use crate::wire::{call, call_in_region, error_code, medialive, DEFAULT_REGION};

/// The collection URI `ListSignalMaps` is bound to
/// (`smithy_query(query="operation", operation="ListSignalMaps")`: GET
/// /prod/signal-maps).
const SIGNAL_MAPS: &str = "/prod/signal-maps";

/// `ListSignalMaps` with `query` appended verbatim (the four input members are
/// all `@httpQuery`), signed for `region`.
async fn list_in(router: &MultiServiceRouter, region: &str, query: &str) -> (u16, Value) {
    call_in_region(router, region, "GET", &format!("{SIGNAL_MAPS}{query}"), None).await
}

/// `ListSignalMaps` with `query` appended verbatim.
async fn list(router: &MultiServiceRouter, query: &str) -> (u16, Value) {
    list_in(router, DEFAULT_REGION, query).await
}

/// The `signalMaps` member as a list, whatever its length.
fn summaries(case: &str, body: &Value) -> Vec<Value> {
    body["signalMaps"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected a signalMaps list, got {body}"))
}

/// `CreateSignalMap` with `body`, answering the created map's id.
async fn create_signal_map(router: &MultiServiceRouter, case: &str, body: Value) -> String {
    let (status, created) = call(router, "POST", SIGNAL_MAPS, Some(body)).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected CreateSignalMap to answer 201, got {status}: {created}"
    );
    created["id"].as_str().unwrap_or_default().to_string()
}

/// `CreateCloudWatchAlarmTemplateGroup` / `CreateEventBridgeRuleTemplateGroup`
/// on `uri`, answering the created group's id.
async fn create_group(router: &MultiServiceRouter, uri: &str, name: &str) -> String {
    let (status, created) = call(router, "POST", uri, Some(json!({ "name": name }))).await;
    assert_eq!(
        status, 201,
        "create group {name:?} on {uri}: expected 201, got {status}: {created}"
    );
    created["id"].as_str().unwrap_or_default().to_string()
}

/// The collection URIs of the two template-group resources a signal map can be
/// associated with (`smithy_query(query="operation",
/// operation="CreateCloudWatchAlarmTemplateGroup,CreateEventBridgeRuleTemplateGroup")`).
const ALARM_GROUPS: &str = "/prod/cloudwatch-alarm-template-groups";
const RULE_GROUPS: &str = "/prod/eventbridge-rule-template-groups";

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
            "case {case:?}: expected ListSignalMaps to answer 200, got {status}: {body}"
        );
        let page = summaries(case, &body);
        let allowed = page_size
            .map_or(MAX_LIST_PAGE_SIZE, |size| {
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

/// One row of the pagination table: how many signal maps the store holds and
/// the page size the request asks for.
struct Paging {
    case: &'static str,
    maps: usize,
    page_size: Option<i32>,
}

/// Rule: `ListSignalMaps` reports the signal maps this service's store holds,
/// each exactly once, in the order their ids were minted, cut into pages of at
/// most `MaxResults` (bounded by the service's own ceiling) whose `NextToken`
/// resumes at the first map not yet reported and is absent on the last page.
///
/// The rows differ along the COUNT of stored maps and the requested page size —
/// including none stored (the issue's recorded case: a list call on a store no
/// create has written to), a page size of one, a size that divides the store
/// unevenly, no size at all, and a size past the service's ceiling — because
/// that is what the rule generalises over.
#[tokio::test]
async fn list_signal_maps_pages_through_every_stored_map_in_mint_order() {
    let table = [
        Paging {
            case: "empty store",
            maps: 0,
            page_size: None,
        },
        Paging {
            case: "empty store, page of one",
            maps: 0,
            page_size: Some(1),
        },
        Paging {
            case: "one map, page of one",
            maps: 1,
            page_size: Some(1),
        },
        Paging {
            case: "three maps, page of one",
            maps: 3,
            page_size: Some(1),
        },
        Paging {
            case: "five maps, page of two",
            maps: 5,
            page_size: Some(2),
        },
        Paging {
            case: "four maps, default page size",
            maps: 4,
            page_size: None,
        },
        Paging {
            case: "three maps, page size past the ceiling",
            maps: 3,
            page_size: Some(MAX_LIST_PAGE_SIZE as i32 + 500),
        },
    ];

    for Paging {
        case,
        maps,
        page_size,
    } in table
    {
        let router = medialive();
        let entry = entry_point(&router, "entry").await;

        let mut created: Vec<String> = Vec::new();
        for index in 0..maps {
            created.push(
                create_signal_map(
                    &router,
                    case,
                    json!({
                        "name": format!("map-{index}"),
                        "discoveryEntryPointArn": entry,
                    }),
                )
                .await,
            );
        }

        let (listed, pages) = every_page(&router, case, "", page_size).await;
        assert_eq!(
            listed, created,
            "case {case:?}: expected the listing to report every stored map once, \
             in mint order; pages were {pages:?}"
        );

        // The page cut itself: with a page size of `n` every page but the last
        // carries exactly `n`.
        if let Some(size) = page_size.map(|size| (size as usize).min(MAX_LIST_PAGE_SIZE)) {
            let expected_pages = if maps == 0 { 1 } else { maps.div_ceil(size) };
            assert_eq!(
                pages.len(),
                expected_pages,
                "case {case:?}: expected {expected_pages} pages for {maps} maps of {size}, \
                 got {pages:?}"
            );
            for (index, page) in pages.iter().enumerate() {
                let expected = if index + 1 == pages.len() {
                    maps - size * index
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

/// One row of the summary table: what the create supplies beyond the name and
/// the entry point.
struct Stored {
    case: &'static str,
    description: Option<&'static str>,
    tags: Option<Value>,
}

/// Rule: a listed `SignalMapSummary` is the stored signal map read through the
/// list shape — the same `Arn`, `Id`, `Name`, `Description`, `Status`, `Tags`
/// and timestamps `GetSignalMap` reports for it, with the monitor deployment
/// status of a signal map no monitor was ever deployed for.
///
/// The rows differ along WHICH optional members the stored map carries, and
/// each row is listed both before the clock has made discovery due and after,
/// because the rule covers the transient status too.
#[tokio::test]
async fn a_listed_summary_reads_the_stored_signal_map() {
    let table = [
        Stored {
            case: "name only",
            description: None,
            tags: None,
        },
        Stored {
            case: "described",
            description: Some("a described map"),
            tags: None,
        },
        Stored {
            case: "tagged",
            description: None,
            tags: Some(json!({ "team": "video" })),
        },
        Stored {
            case: "described and tagged",
            description: Some("both"),
            tags: Some(json!({ "team": "video", "tier": "gold" })),
        },
    ];

    for Stored {
        case,
        description,
        tags,
    } in table
    {
        let router = medialive();
        let entry = entry_point(&router, "entry").await;
        let name = format!("map-{}", case.replace(' ', "-"));

        let mut body = json!({ "name": name, "discoveryEntryPointArn": entry });
        if let Some(description) = description {
            body["description"] = json!(description);
        }
        if let Some(tags) = &tags {
            body["tags"] = tags.clone();
        }
        let id = create_signal_map(&router, case, body).await;

        // Before discovery is due the listing shows the transient status, the
        // same one `CreateSignalMap` reported.
        let (status, listed) = list(&router, "").await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status}: {listed}"
        );
        let page = summaries(case, &listed);
        assert_eq!(
            page.len(),
            1,
            "case {case:?}: expected the one stored map, got {listed}"
        );
        assert_eq!(
            page[0]["status"],
            json!("CREATE_IN_PROGRESS"),
            "case {case:?}: expected a freshly created map to list as CREATE_IN_PROGRESS, got {:?}",
            page[0]["status"]
        );

        wait_for_the_clock();

        let (status, listed) = list(&router, "").await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status}: {listed}"
        );
        let summary = summaries(case, &listed).remove(0);
        let (status, read) = call(&router, "GET", &format!("{SIGNAL_MAPS}/{id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetSignalMap to answer 200, got {status}: {read}"
        );

        // Every member `SignalMapSummary` shares with the flat signal-map shape
        // (`smithy_query(query="shape", shape="SignalMapSummary")`) must report
        // the same stored value the read does.
        for member in ["arn", "id", "name", "description", "status", "tags"] {
            assert_eq!(
                summary.get(member).cloned().unwrap_or(Value::Null),
                read.get(member).cloned().unwrap_or(Value::Null),
                "case {case:?}: expected the listed {member} to equal the one GetSignalMap \
                 reports, got {:?} vs {:?}",
                summary.get(member),
                read.get(member)
            );
        }
        for member in ["createdAt", "modifiedAt"] {
            assert_eq!(
                timestamp(case, member, &summary),
                timestamp(case, member, &read),
                "case {case:?}: expected the listed {member} to equal the one GetSignalMap \
                 reports, got {:?} vs {:?}",
                summary.get(member),
                read.get(member)
            );
        }
        assert_eq!(
            summary["id"],
            json!(id),
            "case {case:?}: expected the created id, got {:?}",
            summary["id"]
        );
        assert_eq!(
            summary["status"],
            json!("CREATE_COMPLETE"),
            "case {case:?}: expected discovery to have settled, got {:?}",
            summary["status"]
        );
        // `SignalMapMonitorDeploymentStatus` values come from
        // `smithy_query(query="shape", shape="SignalMapMonitorDeploymentStatus")`;
        // nothing has deployed a monitor here, which is NOT_DEPLOYED.
        assert_eq!(
            summary["monitorDeploymentStatus"],
            json!("NOT_DEPLOYED"),
            "case {case:?}: expected NOT_DEPLOYED for a map no monitor was deployed for, got {:?}",
            summary["monitorDeploymentStatus"]
        );
        assert_eq!(
            summary["description"],
            description.map_or(Value::Null, |text| json!(text)),
            "case {case:?}: expected the stored description, got {:?}",
            summary["description"]
        );
        assert_eq!(
            summary["tags"],
            tags.clone().unwrap_or(json!({})),
            "case {case:?}: expected the stored tags, got {:?}",
            summary["tags"]
        );
    }
}

/// Rule: a template-group identifier in the request narrows the listing to the
/// signal maps associated with the group it resolves to (by the group's id or
/// its current name), and naming both narrows it to the maps associated with
/// both.
///
/// The rows differ along WHICH filter is named, in which of the two forms, and
/// over a store whose maps carry every combination of associations, because
/// that is what the rule generalises over. Each row is walked with a page size
/// of one as well, so the filter and the page cut are exercised together.
#[tokio::test]
async fn a_group_filter_lists_only_the_maps_associated_with_that_group() {
    let router = medialive();
    let entry = entry_point(&router, "entry").await;

    let alarm_one = create_group(&router, ALARM_GROUPS, "alarm-one").await;
    let alarm_two = create_group(&router, ALARM_GROUPS, "alarm-two").await;
    let alarm_spare = create_group(&router, ALARM_GROUPS, "alarm-spare").await;
    let rule_one = create_group(&router, RULE_GROUPS, "rule-one").await;
    let rule_two = create_group(&router, RULE_GROUPS, "rule-two").await;

    // (name, alarm group identifiers, rule group identifiers)
    let stored: [(&str, &[&str], &[&str]); 5] = [
        ("unassociated", &[], &[]),
        ("alarm-one-only", &["alarm-one"], &[]),
        ("rule-one-only", &[], &["rule-one"]),
        ("both-one", &["alarm-one"], &["rule-one"]),
        ("the-other-groups", &["alarm-two"], &["rule-two"]),
    ];
    for (name, alarms, rules) in stored {
        let mut body = json!({ "name": name, "discoveryEntryPointArn": entry });
        if !alarms.is_empty() {
            body["cloudWatchAlarmTemplateGroupIdentifiers"] = json!(alarms);
        }
        if !rules.is_empty() {
            body["eventBridgeRuleTemplateGroupIdentifiers"] = json!(rules);
        }
        create_signal_map(&router, name, body).await;
    }

    // (case, query, the names the filter must admit)
    let table: [(&str, String, &[&str]); 9] = [
        (
            "no filter",
            String::new(),
            &[
                "alarm-one-only",
                "both-one",
                "rule-one-only",
                "the-other-groups",
                "unassociated",
            ],
        ),
        (
            "alarm group by name",
            "?cloudWatchAlarmTemplateGroupIdentifier=alarm-one".to_string(),
            &["alarm-one-only", "both-one"],
        ),
        (
            "alarm group by id",
            format!("?cloudWatchAlarmTemplateGroupIdentifier={alarm_one}"),
            &["alarm-one-only", "both-one"],
        ),
        (
            "the other alarm group",
            format!("?cloudWatchAlarmTemplateGroupIdentifier={alarm_two}"),
            &["the-other-groups"],
        ),
        (
            "an alarm group no map names",
            format!("?cloudWatchAlarmTemplateGroupIdentifier={alarm_spare}"),
            &[],
        ),
        (
            "rule group by name",
            "?eventBridgeRuleTemplateGroupIdentifier=rule-one".to_string(),
            &["both-one", "rule-one-only"],
        ),
        (
            "rule group by id",
            format!("?eventBridgeRuleTemplateGroupIdentifier={rule_one}"),
            &["both-one", "rule-one-only"],
        ),
        (
            "both filters, one map in both groups",
            format!(
                "?cloudWatchAlarmTemplateGroupIdentifier={alarm_one}\
                 &eventBridgeRuleTemplateGroupIdentifier={rule_one}"
            ),
            &["both-one"],
        ),
        (
            "both filters, no map in both groups",
            format!(
                "?cloudWatchAlarmTemplateGroupIdentifier={alarm_one}\
                 &eventBridgeRuleTemplateGroupIdentifier={rule_two}"
            ),
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

        // The same filter, paged one map at a time, admits the same set: the
        // page cut narrows nothing of its own.
        let (ids, pages) = every_page(&router, case, &query, Some(1)).await;
        assert_eq!(
            ids.len(),
            expected.len(),
            "case {case:?}: expected {} maps across the pages {pages:?}, got {ids:?}",
            expected.len()
        );
    }
}

/// Rule: a `MaxResults` that asks for no results at all is refused with
/// `BadRequestException` (400), and a template-group identifier that resolves to
/// no group of that type in the region the request was signed for is refused
/// with `NotFoundException` (404) naming the resource type and the identifier.
///
/// The rows differ along the page size asked for, which filter carries the
/// unresolvable identifier, and WHY it is unresolvable — never created, the id
/// of a group of the other type, or a group that exists in another region.
#[tokio::test]
async fn a_bad_page_size_or_an_unresolvable_group_filter_is_refused() {
    let router = medialive();
    let alarm = create_group(&router, ALARM_GROUPS, "alarm-one").await;
    let rule = create_group(&router, RULE_GROUPS, "rule-one").await;

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

    // (case, query member, identifier, the message the refusal carries — a
    //  template-group 404 of EITHER monitor hierarchy leaves the member unset,
    //  which is those hierarchies' own rule (pinned by
    //  `crate::event_bridge_rule_template_not_found` and
    //  `crate::not_found_refusal_wording`), so every row here is unset
    //  and the `Option` is what keeps the assertion honest for a family that
    //  does state its message)
    let unresolvable: [(&str, &str, String, Option<&str>); 5] = [
        (
            "alarm group never created",
            "cloudWatchAlarmTemplateGroupIdentifier",
            "no-such-group".to_string(),
            None,
        ),
        (
            "alarm filter naming a rule group's id",
            "cloudWatchAlarmTemplateGroupIdentifier",
            rule.clone(),
            None,
        ),
        (
            "rule group never created",
            "eventBridgeRuleTemplateGroupIdentifier",
            "no-such-group".to_string(),
            None,
        ),
        (
            "rule filter naming an alarm group's id",
            "eventBridgeRuleTemplateGroupIdentifier",
            alarm.clone(),
            None,
        ),
        (
            "alarm group by a name no group holds",
            "cloudWatchAlarmTemplateGroupIdentifier",
            "rule-one".to_string(),
            None,
        ),
    ];

    for (case, member, identifier, resource_type) in unresolvable {
        let (status, body) = list(&router, &format!("?{member}={identifier}")).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected an identifier naming no group to be refused, \
             got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {body}"
        );
        match resource_type {
            Some(resource_type) => assert_eq!(
                body["message"],
                json!(format!("{resource_type} {identifier} not found")),
                "case {case:?}: expected the message to name the resource type then the \
                 identifier, got {:?}",
                body["message"]
            ),
            None => assert_eq!(
                body.get("message"),
                Some(&json!(null)),
                "case {case:?}: expected a template group's refusal to report the \
                 message member with no value, got {:?}",
                body.get("message")
            ),
        }
    }

    // A group belongs to the region it was created in, so the same identifier
    // resolves in `us-west-2` and nowhere else.
    for (case, member, identifier) in [
        (
            "alarm group of another region",
            "cloudWatchAlarmTemplateGroupIdentifier",
            alarm,
        ),
        (
            "rule group of another region",
            "eventBridgeRuleTemplateGroupIdentifier",
            rule,
        ),
    ] {
        let query = format!("?{member}={identifier}");
        let (status, body) = list(&router, &query).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the filter to resolve in the region that created \
             the group, got {status}: {body}"
        );
        let (status, body) = list_in(&router, "eu-west-1", &query).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected a group of another region to resolve to nothing, \
             got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {body}"
        );
    }
}
