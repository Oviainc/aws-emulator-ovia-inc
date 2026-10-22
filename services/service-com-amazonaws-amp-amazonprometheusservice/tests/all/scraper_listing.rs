// SPDX-License-Identifier: CC0-1.0

//! `ListScrapers`, end to end over HTTP: every input member is optional, so a listing
//! with no query at all is a success — the empty page while the account holds no
//! scraper — and the page it reports is computed from the scrapers the store HOLDS,
//! filtered and paged the way the model documents.
//!
//! The store is seeded directly here, through the same engine the mounted service reads
//! ([`Amp::store`]): `CreateScraper` is still scaffolded (its own stub issue), so seeding
//! is how a listing of MORE than the empty page is exercised at all — and it is the same
//! record the handler reads, not a second copy of it.

use crate::workspace_lifecycle::{text, Amp};
use service_com_amazonaws_amp_amazonprometheusservice::ops::types::{
    AmpConfiguration, CloudWatchConfiguration, Destination, EksConfiguration, Source,
};
use service_com_amazonaws_amp_amazonprometheusservice::state::{
    Scraper, SCRAPER_PAGE_DEFAULT, STATUS_ACTIVE, STATUS_CREATING,
};
use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

/// One seeded scraper. `scraperId` is what the listing orders and pages by.
fn scraper(
    scraper_id: &str,
    alias: Option<&str>,
    status_code: &str,
    source: Source,
    destination: Destination,
) -> Scraper {
    let created_at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    Scraper {
        scraper_id: scraper_id.to_string(),
        arn: format!(
            "arn:aws:aps:us-west-2:{}:scraper/{scraper_id}",
            engine::emulator_account_id()
        ),
        alias: alias.map(ToString::to_string),
        role_arn: format!(
            "arn:aws:iam::{}:role/aps-collector",
            engine::emulator_account_id()
        ),
        status_code: status_code.to_string(),
        status_reason: None,
        created_at,
        last_modified_at: created_at,
        tags: BTreeMap::from([("team".to_string(), "metrics".to_string())]),
        source,
        destination,
        role_configuration: None,
        exporters: Vec::new(),
    }
}

/// An EKS cluster source, named by the ARN the `sourceArn` filter matches on.
fn eks_source(cluster: &str) -> Source {
    Source::eksConfiguration(EksConfiguration {
        cluster_arn: format!(
            "arn:aws:eks:us-west-2:{}:cluster/{cluster}",
            engine::emulator_account_id()
        ),
        security_group_ids: None,
        subnet_ids: vec!["subnet-0123456789abcdef0".to_string()],
    })
}

/// An Amazon Managed Service for Prometheus destination, named by the workspace ARN the
/// `destinationArn` filter matches on.
fn workspace_destination(workspace_id: &str) -> Destination {
    Destination::ampConfiguration(AmpConfiguration {
        workspace_arn: format!(
            "arn:aws:aps:us-west-2:{}:workspace/{workspace_id}",
            engine::emulator_account_id()
        ),
    })
}

/// A CloudWatch dataset destination — the union's other variant, so the
/// `destinationArn` filter is exercised on both.
fn dataset_destination(dataset: &str) -> Destination {
    Destination::cloudWatchConfiguration(CloudWatchConfiguration {
        dataset_arn: format!(
            "arn:aws:cloudwatch:us-west-2:{}:dataset/{dataset}",
            engine::emulator_account_id()
        ),
    })
}

/// The `scraperId`s a listing reported, in the order it reported them.
fn listed(body: &serde_json::Value) -> Vec<String> {
    body["scrapers"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .map(|entry| text(&entry["scraperId"]))
                .collect()
        })
        .unwrap_or_default()
}

/// Every member of `ListScrapers`'s input is optional, so a listing is a SUCCESS however
/// little of it the request carries — and an account with no scrapers has an empty page,
/// not an error. One row per shape of optional input the model allows.
#[tokio::test]
async fn an_account_with_no_scrapers_lists_an_empty_page() {
    let region = "us-west-2";
    let amp = Amp::new();

    let cases = [
        ("no query at all", "/scrapers"),
        ("maxResults alone", "/scrapers?maxResults=10"),
        ("nextToken alone", "/scrapers?nextToken=scraper-zzzz"),
        ("one filter key", "/scrapers?status=ACTIVE"),
        (
            "every documented filter key",
            "/scrapers?status=ACTIVE&alias=a&sourceArn=b&destinationArn=c",
        ),
        (
            "a filter beside the pagination members",
            "/scrapers?status=ACTIVE&maxResults=5",
        ),
    ];

    for (case, uri) in cases {
        let reply = amp.call("GET", uri, region, "").await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: a listing with optional input must answer 200, got {reply:?}"
        );
        assert_eq!(
            reply.body["scrapers"],
            serde_json::json!([]),
            "case {case:?}: `scrapers` is @required and must be reported as an empty \
             list, got {reply:?}"
        );
        assert!(
            reply.body.get("nextToken").is_none_or(serde_json::Value::is_null),
            "case {case:?}: a single empty page must carry no nextToken, got {reply:?}"
        );
    }
}

/// The listing reports what the store HOLDS, and reports it from the stored record: each
/// summary's members are the ones seeded, including the union source and destination.
#[tokio::test]
async fn the_listing_reports_the_stored_record() {
    let region = "us-west-2";
    let amp = Amp::new();
    let seeded = scraper(
        "scraper-abc",
        Some("collector"),
        STATUS_ACTIVE,
        eks_source("metrics-cluster"),
        workspace_destination("ws-11111111-1111-1111-1111-111111111111"),
    );
    amp.store()
        .lock()
        .scrapers
        .insert(seeded.scraper_id.clone(), seeded.clone());

    let reply = amp.call("GET", "/scrapers", region, "").await;
    assert_eq!(reply.status, 200, "the listing must answer 200: {reply:?}");
    let summaries = reply.body["scrapers"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        summaries.len(),
        1,
        "the listing must report the one stored scraper, got {reply:?}"
    );
    let summary = &summaries[0];
    let expected_source_arn = match &seeded.source {
        Source::eksConfiguration(eks) => eks.cluster_arn.clone(),
        Source::vpcConfiguration(_) => unreachable!("this row seeds an EKS source"),
    };
    let expected_destination_arn = match &seeded.destination {
        Destination::ampConfiguration(amp_destination) => amp_destination.workspace_arn.clone(),
        Destination::cloudWatchConfiguration(_) => {
            unreachable!("this row seeds a workspace destination")
        }
    };
    let rows: Vec<(&str, String, String)> = vec![
        ("scraperId", text(&summary["scraperId"]), seeded.scraper_id.clone()),
        ("arn", text(&summary["arn"]), seeded.arn.clone()),
        ("alias", text(&summary["alias"]), "collector".to_string()),
        ("roleArn", text(&summary["roleArn"]), seeded.role_arn.clone()),
        (
            "status.statusCode",
            text(&summary["status"]["statusCode"]),
            STATUS_ACTIVE.to_string(),
        ),
        (
            "source.eksConfiguration.clusterArn",
            text(&summary["source"]["eksConfiguration"]["clusterArn"]),
            expected_source_arn,
        ),
        (
            "destination.ampConfiguration.workspaceArn",
            text(&summary["destination"]["ampConfiguration"]["workspaceArn"]),
            expected_destination_arn,
        ),
        (
            "tags.team",
            text(&summary["tags"]["team"]),
            "metrics".to_string(),
        ),
    ];
    for (member, reported, expected) in rows {
        assert_eq!(
            reported, expected,
            "case {member:?}: the summary must report the stored value {expected:?}, \
             got {reported:?} in {summary}"
        );
    }
    assert!(
        summary["createdAt"].is_number() && summary["lastModifiedAt"].is_number(),
        "createdAt and lastModifiedAt are @required timestamps, got {summary}"
    );
}

/// `filters` is documented "Filters on the same key are `OR`'d together, and filters on
/// different keys are `AND`'d together", over the four keys the member names — and
/// `filters` is bound `@httpQueryParams`, so a key that is not one of those four (the
/// pagination members, or anything a caller invents) is not a filter at all.
#[tokio::test]
async fn the_listing_filters_the_way_the_model_documents() {
    let region = "us-west-2";
    let amp = Amp::new();
    let active_eks = eks_source("active-cluster");
    let creating_eks = eks_source("creating-cluster");
    let seeded = [
        scraper(
            "scraper-active",
            Some("edge"),
            STATUS_ACTIVE,
            active_eks.clone(),
            workspace_destination("ws-11111111-1111-1111-1111-111111111111"),
        ),
        scraper(
            "scraper-creating",
            Some("edge"),
            STATUS_CREATING,
            creating_eks.clone(),
            workspace_destination("ws-22222222-2222-2222-2222-222222222222"),
        ),
        scraper(
            "scraper-dataset",
            None,
            STATUS_ACTIVE,
            active_eks.clone(),
            dataset_destination("fleet-metrics"),
        ),
    ];
    {
        let store = amp.store();
        let mut guard = store.lock();
        for record in &seeded {
            guard
                .scrapers
                .insert(record.scraper_id.clone(), record.clone());
        }
    }

    let active_cluster_arn = match &active_eks {
        Source::eksConfiguration(eks) => eks.cluster_arn.clone(),
        Source::vpcConfiguration(_) => unreachable!("an EKS source is seeded"),
    };
    let first_workspace_arn = match &seeded[0].destination {
        Destination::ampConfiguration(destination) => destination.workspace_arn.clone(),
        Destination::cloudWatchConfiguration(_) => unreachable!("a workspace destination"),
    };
    let dataset_arn = match &seeded[2].destination {
        Destination::cloudWatchConfiguration(destination) => destination.dataset_arn.clone(),
        Destination::ampConfiguration(_) => unreachable!("a dataset destination"),
    };

    let cases: Vec<(&str, String, Vec<&str>)> = vec![
        (
            "no filter lists everything, ordered by scraperId",
            "/scrapers".to_string(),
            vec!["scraper-active", "scraper-creating", "scraper-dataset"],
        ),
        (
            "one value of one key",
            "/scrapers?status=CREATING".to_string(),
            vec!["scraper-creating"],
        ),
        (
            "two values of the same key are OR'd",
            "/scrapers?status=ACTIVE&status=CREATING".to_string(),
            vec!["scraper-active", "scraper-creating", "scraper-dataset"],
        ),
        (
            "two keys are AND'd",
            "/scrapers?status=ACTIVE&alias=edge".to_string(),
            vec!["scraper-active"],
        ),
        (
            "an AND that nothing satisfies",
            "/scrapers?status=CREATING&alias=nope".to_string(),
            vec![],
        ),
        (
            "a scraper with no alias matches no alias filter",
            "/scrapers?alias=edge".to_string(),
            vec!["scraper-active", "scraper-creating"],
        ),
        (
            "sourceArn names the EKS cluster the scraper reads",
            format!("/scrapers?sourceArn={active_cluster_arn}"),
            vec!["scraper-active", "scraper-dataset"],
        ),
        (
            "destinationArn names the workspace the scraper writes to",
            format!("/scrapers?destinationArn={first_workspace_arn}"),
            vec!["scraper-active"],
        ),
        (
            "destinationArn names a CloudWatch dataset",
            format!("/scrapers?destinationArn={dataset_arn}"),
            vec!["scraper-dataset"],
        ),
        (
            "a value no scraper carries matches nothing",
            "/scrapers?status=DELETING".to_string(),
            vec![],
        ),
        (
            "the pagination members are not filters",
            format!("/scrapers?maxResults={SCRAPER_PAGE_DEFAULT}"),
            vec!["scraper-active", "scraper-creating", "scraper-dataset"],
        ),
        (
            "a key the service does not define is not a filter",
            "/scrapers?somethingElse=whatever".to_string(),
            vec!["scraper-active", "scraper-creating", "scraper-dataset"],
        ),
    ];

    for (case, uri, expected) in cases {
        let reply = amp.call("GET", &uri, region, "").await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: a filtered listing must answer 200, got {reply:?}"
        );
        assert_eq!(
            listed(&reply.body),
            expected,
            "case {case:?}: expected {expected:?}, got {reply:?}"
        );
    }
}

/// `nextToken` and `maxResults` are the `@httpQuery` members the model declares, so a
/// repeat of either takes its FIRST value (as every other operation's scalar query member
/// does) and neither is ever read as a filter — while a `maxResults` that is not an
/// integer is a member the request cannot provide, refused as one.
#[tokio::test]
async fn the_pagination_members_are_bound_as_the_model_declares() {
    let region = "us-west-2";
    let amp = Amp::new();
    let ids = ["scraper-0", "scraper-1", "scraper-2"];
    {
        let store = amp.store();
        let mut guard = store.lock();
        for id in ids {
            guard.scrapers.insert(
                id.to_string(),
                scraper(
                    id,
                    Some("bound"),
                    STATUS_ACTIVE,
                    eks_source("bound-cluster"),
                    workspace_destination("ws-44444444-4444-4444-4444-444444444444"),
                ),
            );
        }
    }

    let accepted: Vec<(&str, &str, Vec<&str>)> = vec![
        (
            "a repeated maxResults takes the first value",
            "/scrapers?maxResults=1&maxResults=3",
            vec!["scraper-0"],
        ),
        (
            "a repeated nextToken takes the first value",
            "/scrapers?nextToken=scraper-0&nextToken=scraper-1",
            vec!["scraper-1", "scraper-2"],
        ),
        (
            "neither pagination member filters the listing",
            // A cursor that sorts before every seeded id, so the page is the whole
            // listing: the point of the row is that the two members are not filters.
            "/scrapers?maxResults=3&nextToken=scraper-",
            vec!["scraper-0", "scraper-1", "scraper-2"],
        ),
    ];
    for (case, uri, expected) in accepted {
        let reply = amp.call("GET", uri, region, "").await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: must answer 200, got {reply:?}"
        );
        assert_eq!(
            listed(&reply.body),
            expected,
            "case {case:?}: expected {expected:?}, got {reply:?}"
        );
    }

    for (case, uri) in [
        ("a maxResults that is not a number", "/scrapers?maxResults=many"),
        ("a maxResults that is not an integer", "/scrapers?maxResults=2.5"),
    ] {
        let reply = amp.call("GET", uri, region, "").await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: a member the request cannot provide must be refused with \
             400, got {reply:?}"
        );
        assert_eq!(
            reply.error_code(),
            "ValidationException",
            "case {case:?}: expected the modelled ValidationException, got {reply:?}"
        );
        assert_eq!(
            text(&reply.body["fieldList"][0]["name"]),
            "maxResults",
            "case {case:?}: the refusal must name the member, got {reply:?}"
        );
    }
}

/// `maxResults` bounds one page and `nextToken` continues it: the pages partition the
/// listing in `scraperId` order, with no repeat and nothing left out.
#[tokio::test]
async fn the_listing_pages_by_max_results_and_next_token() {
    let region = "us-west-2";
    let amp = Amp::new();
    let ids: Vec<String> = (0..5).map(|index| format!("scraper-{index}")).collect();
    {
        let store = amp.store();
        let mut guard = store.lock();
        for id in &ids {
            guard.scrapers.insert(
                id.clone(),
                scraper(
                    id,
                    Some("paged"),
                    STATUS_ACTIVE,
                    eks_source("paged-cluster"),
                    workspace_destination("ws-33333333-3333-3333-3333-333333333333"),
                ),
            );
        }
    }

    for page_size in [1usize, 2, 5] {
        let case = format!("maxResults={page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut cursor: Option<String> = None;
        // One more request than there can be pages, so a token that never clears fails
        // here instead of looping.
        for _ in 0..=ids.len() {
            let uri = match &cursor {
                Some(token) => format!("/scrapers?maxResults={page_size}&nextToken={token}"),
                None => format!("/scrapers?maxResults={page_size}"),
            };
            let reply = amp.call("GET", &uri, region, "").await;
            assert_eq!(
                reply.status, 200,
                "case {case:?}: a page must answer 200, got {reply:?}"
            );
            let page = listed(&reply.body);
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page must hold at most {page_size} summaries, got {page:?}"
            );
            seen.extend(page.clone());
            cursor = reply.body["nextToken"].as_str().map(ToString::to_string);
            if cursor.is_none() {
                break;
            }
            assert_eq!(
                cursor.as_deref(),
                page.last().map(String::as_str),
                "case {case:?}: the cursor must be the last id of the page it ends, \
                 got {reply:?}"
            );
        }
        assert_eq!(
            cursor, None,
            "case {case:?}: paging must terminate, got a cursor {cursor:?} after every page"
        );
        assert_eq!(
            seen, ids,
            "case {case:?}: the pages must partition the listing in scraperId order, \
             got {seen:?}"
        );
    }
}
