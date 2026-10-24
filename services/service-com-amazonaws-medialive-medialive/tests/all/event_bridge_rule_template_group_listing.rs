// SPDX-License-Identifier: CC0-1.0

//! `ListEventBridgeRuleTemplateGroups`: which stored groups a listing reports,
//! what one looks like read through the list shape, how a page is cut and which
//! groups the `signalMapIdentifier` filter admits — end to end over the restJson1
//! wire.
//!
//! Every test drives the service the way a client does, through
//! `MultiServiceRouter::dispatch`, so what is asserted is the emulator's wire
//! behaviour rather than an internal function's return value. The suites are
//! TABLES spanning what each rule generalises over: the count of stored groups
//! and the page size asked for, the members a stored group carries and the number
//! of templates inside it, which form the filter's identifier takes, and the
//! region the request is signed for.

use serde_json::{json, Value};
use service_com_amazonaws_medialive_medialive::state::MAX_LIST_PAGE_SIZE;
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{entry_point, timestamp};
use crate::wire::{call_in_region, error_code, medialive, ACCOUNT, DEFAULT_REGION};

/// The collection URI `ListEventBridgeRuleTemplateGroups` and
/// `CreateEventBridgeRuleTemplateGroup` are bound to
/// (`smithy_query(query="operation",
/// operation="ListEventBridgeRuleTemplateGroups")`: GET
/// /prod/eventbridge-rule-template-groups).
const GROUPS: &str = "/prod/eventbridge-rule-template-groups";

/// The template collection, whose members are what a group's `templateCount`
/// counts (`smithy_query(query="operation",
/// operation="CreateEventBridgeRuleTemplate")`).
const TEMPLATES: &str = "/prod/eventbridge-rule-templates";

/// The signal-map collection, whose
/// `EventBridgeRuleTemplateGroupIdentifiers` are what a `signalMapIdentifier`
/// filter resolves through (`smithy_query(query="operation",
/// operation="CreateSignalMap")`).
const SIGNAL_MAPS: &str = "/prod/signal-maps";

/// Three `EventBridgeRuleTemplateEventType` values the model declares, spelled
/// exactly as it declares them (`smithy_query(query="shape",
/// shape="EventBridgeRuleTemplateEventType")`). Only the templates' existence
/// matters here, so the rows pick values the issue did not cite.
const EVENT_TYPES: [&str; 3] = [
    "MEDIAPACKAGE_HARVEST_JOB_NOTIFICATION",
    "MEDIACONNECT_SOURCE_HEALTH",
    "MEDIALIVE_CHANNEL_INPUT_CHANGE",
];

/// `ListEventBridgeRuleTemplateGroups` with `query` appended verbatim (all three
/// input members are `@httpQuery`), signed for `region`.
async fn list_in(router: &MultiServiceRouter, region: &str, query: &str) -> (u16, Value) {
    call_in_region(router, region, "GET", &format!("{GROUPS}{query}"), None).await
}

/// The same listing, signed for [`DEFAULT_REGION`].
async fn list(router: &MultiServiceRouter, query: &str) -> (u16, Value) {
    list_in(router, DEFAULT_REGION, query).await
}

/// The `eventBridgeRuleTemplateGroups` member as a list, whatever its length.
fn summaries(case: &str, body: &Value) -> Vec<Value> {
    body["eventBridgeRuleTemplateGroups"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected an eventBridgeRuleTemplateGroups list, got {body}")
        })
}

/// `CreateEventBridgeRuleTemplateGroup` in `region`, answering the created group.
async fn create_group(router: &MultiServiceRouter, case: &str, region: &str, body: Value) -> Value {
    let (status, created) = call_in_region(router, region, "POST", GROUPS, Some(body)).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected CreateEventBridgeRuleTemplateGroup to answer 201, \
         got {status}: {created}"
    );
    created
}

/// The same create with only a name, answering the group's id.
async fn named_group(router: &MultiServiceRouter, region: &str, name: &str) -> String {
    let created = create_group(router, name, region, json!({ "name": name })).await;
    created["id"].as_str().unwrap_or_default().to_string()
}

/// `GetEventBridgeRuleTemplateGroup` on `identifier`, signed for `region`.
async fn get_group(router: &MultiServiceRouter, region: &str, identifier: &str) -> (u16, Value) {
    call_in_region(router, region, "GET", &format!("{GROUPS}/{identifier}"), None).await
}

/// `CreateEventBridgeRuleTemplate` named `name` inside `group`, answering its id.
async fn create_template(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    group: &str,
    event_type: &str,
) -> String {
    let (status, created) = call_in_region(
        router,
        region,
        "POST",
        TEMPLATES,
        Some(json!({
            "name": name,
            "eventType": event_type,
            "groupIdentifier": group,
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "setup: expected the template {name:?} to be created, got {status}: {created}"
    );
    created["id"].as_str().unwrap_or_default().to_string()
}

/// `DeleteEventBridgeRuleTemplate` on `identifier`.
async fn delete_template(router: &MultiServiceRouter, region: &str, identifier: &str) {
    let (status, body) = call_in_region(
        router,
        region,
        "DELETE",
        &format!("{TEMPLATES}/{identifier}"),
        None,
    )
    .await;
    assert!(
        (200..300).contains(&status),
        "setup: expected the template {identifier:?} to be deleted, got {status}: {body}"
    );
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
            "case {case:?}: expected ListEventBridgeRuleTemplateGroups to answer 200, \
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

/// One row of the pagination table: how many groups the store holds and the page
/// size the request asks for.
struct Paging {
    case: &'static str,
    groups: usize,
    page_size: Option<i32>,
}

/// Rule: a group listing reports the eventbridge rule template groups the store
/// holds in the region the request was signed for, each exactly once, in the
/// order their ids were minted, cut into pages of at most `MaxResults` (bounded
/// by the service's own ceiling) whose `NextToken` resumes at the first group not
/// yet reported and is absent on the last page.
///
/// The rows differ along the COUNT of stored groups and the requested page size —
/// including none stored, a page size of one, a size that divides the store
/// unevenly, no size at all and a size past the service's ceiling — because that
/// is what the rule generalises over.
#[tokio::test]
async fn listing_pages_through_every_stored_group_in_mint_order() {
    let table = [
        Paging {
            case: "empty store",
            groups: 0,
            page_size: None,
        },
        Paging {
            case: "empty store, page of one",
            groups: 0,
            page_size: Some(1),
        },
        Paging {
            case: "one group, page of one",
            groups: 1,
            page_size: Some(1),
        },
        Paging {
            case: "one group, no page size",
            groups: 1,
            page_size: None,
        },
        Paging {
            case: "three groups, page of one",
            groups: 3,
            page_size: Some(1),
        },
        Paging {
            case: "five groups, page of two",
            groups: 5,
            page_size: Some(2),
        },
        Paging {
            case: "four groups, default page size",
            groups: 4,
            page_size: None,
        },
        Paging {
            case: "three groups, page size past the ceiling",
            groups: 3,
            page_size: Some(MAX_LIST_PAGE_SIZE as i32 + 500),
        },
    ];

    for Paging {
        case,
        groups,
        page_size,
    } in table
    {
        let router = medialive();

        let mut created: Vec<String> = Vec::new();
        for index in 0..groups {
            created.push(named_group(&router, DEFAULT_REGION, &format!("grp-{index}")).await);
        }

        let (listed, pages) = every_page(&router, case, "", page_size).await;
        assert_eq!(
            listed, created,
            "case {case:?}: expected the listing to report every stored group once, \
             in mint order; pages were {pages:?}"
        );

        // The page cut itself: with a page size of `n` every page but the last
        // carries exactly `n`.
        if let Some(size) = page_size.map(|size| (size as usize).min(MAX_LIST_PAGE_SIZE)) {
            let expected_pages = if groups == 0 { 1 } else { groups.div_ceil(size) };
            assert_eq!(
                pages.len(),
                expected_pages,
                "case {case:?}: expected {expected_pages} pages for {groups} groups \
                 of {size}, got {pages:?}"
            );
            for (index, page) in pages.iter().enumerate() {
                let expected = if index + 1 == pages.len() {
                    groups - size * index
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

/// One row of the summary table: what the create supplies beyond the name, and
/// how many templates are stored inside the group.
struct Stored {
    case: &'static str,
    description: Option<&'static str>,
    tags: Option<Value>,
    templates: usize,
}

/// Rule: a listed `EventBridgeRuleTemplateGroupSummary` is the stored group read
/// through the list shape — the same `Arn`, `Id`, `Name`, `Description`, `Tags`
/// and timestamps `GetEventBridgeRuleTemplateGroup` reports for it — and its
/// `TemplateCount` is the number of templates currently stored in that group, so
/// it follows every later template create and delete.
///
/// The rows differ along which optional members the stored group carries and the
/// NUMBER of templates inside it (none, one, several), because that is what the
/// rule generalises over.
#[tokio::test]
async fn a_listed_summary_reads_the_stored_group() {
    let table = [
        Stored {
            case: "required members only",
            description: None,
            tags: None,
            templates: 0,
        },
        Stored {
            case: "described, one template",
            description: Some("rules for the on-call"),
            tags: None,
            templates: 1,
        },
        Stored {
            case: "described and tagged, three templates",
            description: Some("fleet wide"),
            tags: Some(json!({ "team": "video", "tier": "gold" })),
            templates: 3,
        },
        Stored {
            case: "an empty description, two templates",
            description: Some(""),
            tags: Some(json!({ "env": "test" })),
            templates: 2,
        },
    ];

    for Stored {
        case,
        description,
        tags,
        templates,
    } in table
    {
        let router = medialive();
        let name = format!("grp-{}", case.replace(' ', "-"));

        let mut body = json!({ "name": name });
        if let Some(description) = description {
            body["description"] = json!(description);
        }
        if let Some(tags) = &tags {
            body["tags"] = tags.clone();
        }
        let created = create_group(&router, case, DEFAULT_REGION, body).await;
        let id = created["id"].as_str().unwrap_or_default().to_string();

        let mut template_ids: Vec<String> = Vec::new();
        for index in 0..templates {
            template_ids.push(
                create_template(
                    &router,
                    DEFAULT_REGION,
                    &format!("{name}-tpl-{index}"),
                    &name,
                    EVENT_TYPES[index % EVENT_TYPES.len()],
                )
                .await,
            );
        }

        let (status, listed) = list(&router, "").await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status}: {listed}"
        );
        let page = summaries(case, &listed);
        assert_eq!(
            page.len(),
            1,
            "case {case:?}: expected the one stored group, got {listed}"
        );
        let summary = &page[0];

        let (status, read) = get_group(&router, DEFAULT_REGION, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetEventBridgeRuleTemplateGroup to answer 200, \
             got {status}: {read}"
        );

        // Every member the summary shares with the flat group shape
        // (`smithy_query(query="shape",
        // shape="EventBridgeRuleTemplateGroupSummary")`) reports the same stored
        // value the read does.
        for member in ["arn", "id", "name", "description", "tags"] {
            assert_eq!(
                summary.get(member).cloned().unwrap_or(Value::Null),
                read.get(member).cloned().unwrap_or(Value::Null),
                "case {case:?}: expected the listed {member} to equal the one \
                 GetEventBridgeRuleTemplateGroup reports, got {:?} vs {:?}",
                summary.get(member),
                read.get(member)
            );
        }
        for member in ["createdAt", "modifiedAt"] {
            assert_eq!(
                timestamp(case, member, summary),
                timestamp(case, member, &read),
                "case {case:?}: expected the listed {member} to equal the one \
                 GetEventBridgeRuleTemplateGroup reports, got {:?} vs {:?}",
                summary.get(member),
                read.get(member)
            );
        }
        assert_eq!(
            summary["templateCount"],
            json!(templates),
            "case {case:?}: expected the number of templates stored in the group, got {:?}",
            summary["templateCount"]
        );

        // The count is derived from the templates that exist, so deleting one
        // lowers it on the very next read.
        if let Some(gone) = template_ids.first() {
            delete_template(&router, DEFAULT_REGION, gone).await;
            let (status, listed) = list(&router, "").await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200 after a template delete, got {status}: {listed}"
            );
            let page = summaries(case, &listed);
            assert_eq!(
                page[0]["templateCount"],
                json!(templates - 1),
                "case {case:?}: expected the count to drop with the deleted template, got {:?}",
                page[0]["templateCount"]
            );
        }
    }
}

/// Rule: a `SignalMapIdentifier` narrows the listing to the eventbridge rule
/// template groups that signal map is associated with, the identifier resolving
/// by the map's id or its current name.
///
/// The rows differ along WHICH map the filter names, in which of the two forms,
/// and how many groups that map is associated with — including a map associated
/// with none — because that is what the rule generalises over. Each row is walked
/// a page at a time as well, so the filter and the page cut are exercised
/// together.
#[tokio::test]
async fn a_signal_map_filter_lists_only_the_groups_it_is_associated_with() {
    let router = medialive();
    let entry = entry_point(&router, "entry").await;

    let first = named_group(&router, DEFAULT_REGION, "group-one").await;
    let second = named_group(&router, DEFAULT_REGION, "group-two").await;
    named_group(&router, DEFAULT_REGION, "group-spare").await;

    // One map associated with both named groups, one with only the second, one
    // with no group at all.
    let both = create_signal_map(&router, "map-both", &entry, &["group-one", &second]).await;
    let only_second = create_signal_map(&router, "map-second", &entry, &["group-two"]).await;
    let unassociated = create_signal_map(&router, "map-none", &entry, &[]).await;

    // (case, query, the group names the filter must admit)
    let table: [(&str, String, &[&str]); 6] = [
        (
            "no filter",
            String::new(),
            &["group-one", "group-spare", "group-two"],
        ),
        (
            "signal map by id, two groups",
            format!("?signalMapIdentifier={both}"),
            &["group-one", "group-two"],
        ),
        (
            "signal map by name, two groups",
            "?signalMapIdentifier=map-both".to_string(),
            &["group-one", "group-two"],
        ),
        (
            "signal map naming one group",
            format!("?signalMapIdentifier={only_second}"),
            &["group-two"],
        ),
        (
            "signal map naming one group, by name",
            "?signalMapIdentifier=map-second".to_string(),
            &["group-two"],
        ),
        (
            "signal map naming no group",
            format!("?signalMapIdentifier={unassociated}"),
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

        // The same filter, paged one group at a time, admits the same set: the
        // page cut narrows nothing of its own.
        let (ids, pages) = every_page(&router, case, &query, Some(1)).await;
        assert_eq!(
            ids.len(),
            expected.len(),
            "case {case:?}: expected {} groups across the pages {pages:?}, got {ids:?}",
            expected.len()
        );
    }

    // The ids the unfiltered listing reports are the ones the creates minted,
    // whichever filter is applied on top of them.
    let (_, body) = list(&router, "").await;
    let listed: Vec<String> = summaries("no filter", &body)
        .iter()
        .map(|summary| summary["id"].as_str().unwrap_or_default().to_string())
        .collect();
    assert!(
        listed.contains(&first) && listed.contains(&second),
        "case \"no filter\": expected the created ids among {listed:?}"
    );
    // Every reported ARN names this service, this region and this account.
    for summary in summaries("no filter", &body) {
        let arn = summary["arn"].as_str().unwrap_or_default().to_string();
        assert!(
            arn.starts_with(&format!("arn:aws:medialive:{DEFAULT_REGION}:{ACCOUNT}:")),
            "case \"no filter\": expected an ARN of this region and account, got {arn:?}"
        );
    }
}

/// Rule: a group is visible only to a request signed for the region it was
/// created in, so the same listing reports a different set per region.
///
/// The rows are the regions the groups are created in; nothing about them matters
/// except that they differ, and that one region holds none.
#[tokio::test]
async fn a_listing_reports_only_the_groups_of_its_own_region() {
    let router = medialive();
    // (region, the names created there)
    let regions: [(&str, &[&str]); 3] = [
        ("us-west-2", &["west-a", "west-b"]),
        ("eu-central-1", &["central-a"]),
        ("ap-south-1", &[]),
    ];

    for (region, names) in regions {
        for name in names {
            named_group(&router, region, name).await;
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
            "case {region:?}: expected only the groups created in {region}, got {listed:?}"
        );
    }
}

/// Rule: a `MaxResults` that asks for no results at all is refused with
/// `BadRequestException` (400), and a `SignalMapIdentifier` that resolves to no
/// signal map is refused with `NotFoundException` (404) whose message names the
/// resource type and the identifier.
///
/// The rows differ along the page size asked for, and along WHY the identifier is
/// unresolvable — never created, the id of a resource of another type, or a name
/// nothing of that type holds.
#[tokio::test]
async fn a_bad_page_size_or_an_unresolvable_filter_is_refused() {
    let router = medialive();
    let group = named_group(&router, DEFAULT_REGION, "group-one").await;

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

    // (case, the identifier the filter carries)
    let unresolvable: [(&str, String); 3] = [
        ("signal map never created", "no-such-map".to_string()),
        ("the id of a group, not a map", group.clone()),
        ("a name nothing of that type holds", "group-one".to_string()),
    ];

    for (case, identifier) in unresolvable {
        let (status, body) = list(&router, &format!("?signalMapIdentifier={identifier}")).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected an identifier naming no signal map to be refused, \
             got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {body}"
        );
        assert_eq!(
            body["message"],
            json!(format!("Signal map {identifier} not found")),
            "case {case:?}: expected the message to name the resource type then the \
             identifier, got {:?}",
            body["message"]
        );
    }
}
