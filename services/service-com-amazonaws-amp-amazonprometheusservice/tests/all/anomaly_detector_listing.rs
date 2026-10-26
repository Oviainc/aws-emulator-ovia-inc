// SPDX-License-Identifier: CC0-1.0

//! `ListAnomalyDetectors`, end to end over HTTP.
//!
//! The rule under test: the listing reports the anomaly detectors the NAMED workspace
//! holds, read off the persisted records, ordered by `anomalyDetectorId`, narrowed to the
//! detectors whose stored `alias` IS the request's `alias` when one is given, and served
//! one page of at most `maxResults` entries with `nextToken` as the cursor to the next
//! page.
//!
//! Every suite spans the inputs that rule generalises over — the alias filter (absent, an
//! alias two detectors share, one a prefix of another alias, the same letters in the
//! other case, one nothing carries), the page size (each published bound of `maxResults`
//! and one past it, plus the page an omitted `maxResults` serves), the number of stored
//! detectors, and the workspace a detector belongs to — rather than one recorded
//! request's values.
//!
//! The request harness (`Amp`, `Reply`) and the create/describe helpers are the ones
//! `workspace_lifecycle` and `anomaly_detector_lifecycle` already build.

use crate::anomaly_detector_lifecycle::{create_detector, describe_detector, minimal_configuration};
use crate::workspace_lifecycle::{encode_label, tags_of, text, wait_for_due_transition, Amp, Reply};
use service_com_amazonaws_amp_amazonprometheusservice::state::{
    ANOMALY_DETECTOR_PAGE_DEFAULT, ANOMALY_DETECTOR_PAGE_LIMIT, STATUS_ACTIVE, STATUS_CREATING,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// `GET /workspaces/{workspaceId}/anomalydetectors` — ListAnomalyDetectors, with the
/// `@httpQuery` members the request names.
///
/// `pub(crate)` so a suite that creates a detector another way reads it back through this
/// same listing rather than spelling the route out again.
pub(crate) async fn list_detectors(
    amp: &Amp,
    region: &str,
    workspace_id: &str,
    query: &[(&str, String)],
) -> Reply {
    let mut uri = format!("/workspaces/{workspace_id}/anomalydetectors");
    if !query.is_empty() {
        let encoded: Vec<String> = query
            .iter()
            .map(|(key, value)| format!("{key}={}", encode_label(value)))
            .collect();
        uri.push('?');
        uri.push_str(&encoded.join("&"));
    }
    amp.call("GET", &uri, region, "").await
}

/// The summaries one page carries, in the order the page carries them.
fn summaries(case: &str, reply: &Reply) -> Vec<Value> {
    reply.body["anomalyDetectors"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: anomalyDetectors must be a list, got {reply:?}"))
        .clone()
}

/// The `anomalyDetectorId` of every summary on one page, in page order.
fn ids_of(case: &str, reply: &Reply) -> Vec<String> {
    summaries(case, reply)
        .iter()
        .map(|entry| text(&entry["anomalyDetectorId"]))
        .collect()
}

/// The `alias` of every summary on one page, sorted, so a row states the aliases the
/// filter keeps without depending on which id the service minted for which detector.
fn sorted_aliases(case: &str, reply: &Reply) -> Vec<String> {
    let mut aliases: Vec<String> = summaries(case, reply)
        .iter()
        .map(|entry| text(&entry["alias"]))
        .collect();
    aliases.sort();
    aliases
}

/// The `nextToken` a page carries, `None` when it carries none.
fn next_token(reply: &Reply) -> Option<String> {
    reply.body["nextToken"]
        .as_str()
        .map(|token| token.to_string())
}

/// Walk every page the listing serves for one query, returning the ids in page order and
/// how many pages it took.
async fn walk_pages(
    amp: &Amp,
    region: &str,
    workspace_id: &str,
    case: &str,
    base_query: &[(&str, String)],
) -> (Vec<String>, usize) {
    let mut collected: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let mut query = base_query.to_vec();
        if let Some(cursor) = token.as_ref() {
            query.push(("nextToken", cursor.clone()));
        }
        let page = list_detectors(amp, region, workspace_id, &query).await;
        assert_eq!(
            page.status, 200,
            "case {case:?}: page {pages} must answer 200, got {page:?}"
        );
        collected.extend(ids_of(case, &page));
        pages += 1;
        token = next_token(&page);
        if token.is_none() {
            break;
        }
        assert!(
            pages < 1_000,
            "case {case:?}: the listing kept emitting a nextToken after {pages} pages"
        );
    }
    (collected, pages)
}

/// The aliases seeded for the filter table. They span what an alias filter can
/// distinguish: one alias TWO detectors share, aliases where one is a prefix of another,
/// the same letters in the other case, and punctuation the modelled
/// `AnomalyDetectorAlias` pattern `^[0-9A-Za-z][-.0-9A-Z_a-z]*$` allows.
const SEEDED_ALIASES: &[&str] = &[
    "alpha",
    "alphabet",
    "alpha.detector",
    "Alpha",
    "shared",
    "shared",
    "z9",
    "with_underscore",
];

/// Create one detector per entry of [`SEEDED_ALIASES`] in a fresh ACTIVE workspace,
/// returning its id and the `(alias, detectorId)` pairs in creation order.
async fn seeded_workspace(amp: &Amp, region: &str, alias: &str) -> (String, Vec<(String, String)>) {
    let workspace_id = amp.active_workspace(region, alias).await;
    let mut seeded = Vec::new();
    for (index, detector_alias) in SEEDED_ALIASES.iter().enumerate() {
        let created = create_detector(
            amp,
            region,
            &workspace_id,
            &json!({
                "alias": detector_alias,
                "configuration": minimal_configuration("avg(rate(requests_total[5m]))"),
                "tags": { "owner": format!("seed{index:02}") },
            }),
        )
        .await;
        assert_eq!(
            created.status, 202,
            "seeding {detector_alias:?}: the create must answer 202, got {created:?}"
        );
        seeded.push((
            (*detector_alias).to_string(),
            text(&created.body["anomalyDetectorId"]),
        ));
    }
    (workspace_id, seeded)
}

/// The documented alias filter — "Filters the results to anomaly detectors with the
/// specified alias" — one row per thing an alias can distinguish. The expected aliases of
/// each row are written out rather than computed, so the test states the rule instead of
/// restating the implementation.
#[tokio::test]
async fn the_alias_filter_keeps_only_the_detectors_carrying_that_alias() {
    let amp = Amp::new();
    let region = "us-west-2";
    let (workspace_id, seeded) = seeded_workspace(&amp, region, "listing-host").await;

    let mut every_alias: Vec<&str> = SEEDED_ALIASES.to_vec();
    every_alias.sort_unstable();

    let rows: Vec<(&str, Option<&str>, Vec<&str>)> = vec![
        ("no filter lists every detector", None, every_alias),
        (
            "an alias two detectors share reports both",
            Some("shared"),
            vec!["shared", "shared"],
        ),
        (
            "an alias that is also a prefix of another alias matches only itself",
            Some("alpha"),
            vec!["alpha"],
        ),
        ("a longer alias matches only itself", Some("alphabet"), vec!["alphabet"]),
        (
            "an alias spanning punctuation",
            Some("alpha.detector"),
            vec!["alpha.detector"],
        ),
        ("the filter is case-sensitive: upper", Some("Alpha"), vec!["Alpha"]),
        ("an alias bearing a digit", Some("z9"), vec!["z9"]),
        (
            "an alias bearing an underscore",
            Some("with_underscore"),
            vec!["with_underscore"],
        ),
        ("a proper prefix of a stored alias matches nothing", Some("alph"), vec![]),
        ("one character past a stored alias", Some("alphabets"), vec![]),
        ("the same letters in the other case", Some("ALPHA"), vec![]),
        ("an alias nothing carries", Some("nosuch"), vec![]),
    ];

    for (case, alias, expected) in rows {
        let query: Vec<(&str, String)> = alias
            .map(|value| vec![("alias", value.to_string())])
            .unwrap_or_default();
        let reply = list_detectors(&amp, region, &workspace_id, &query).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: the listing must answer 200, got {reply:?}"
        );
        assert_eq!(
            sorted_aliases(case, &reply),
            expected,
            "case {case:?}: expected {expected:?}, got {reply:?}"
        );
        assert_eq!(
            next_token(&reply),
            None,
            "case {case:?}: one page holds every match, so no nextToken is owed, got {reply:?}"
        );

        // The ids reported are the ones the creates minted for exactly those aliases.
        let mut expected_ids: Vec<String> = seeded
            .iter()
            .filter(|(seeded_alias, _)| {
                alias.is_none_or(|wanted| seeded_alias.as_str() == wanted)
            })
            .map(|(_, id)| id.clone())
            .collect();
        expected_ids.sort();
        let mut reported_ids = ids_of(case, &reply);
        reported_ids.sort();
        assert_eq!(
            reported_ids, expected_ids,
            "case {case:?}: expected the seeded ids {expected_ids:?}, got {reported_ids:?}"
        );
    }
}

/// Every summary member is read off the persisted record: the ARN, alias and tags the
/// create minted, the status the clock owes, and the timestamps
/// `DescribeAnomalyDetector` reports for the same detector.
#[tokio::test]
async fn each_summary_reports_the_persisted_record() {
    let amp = Amp::new();
    let region = "eu-central-1";
    let (workspace_id, seeded) = seeded_workspace(&amp, region, "summary-host").await;

    // Straight after the creates the detectors are still in their transient status, and
    // the listing reports that rather than a status nothing reached yet.
    let fresh = list_detectors(&amp, region, &workspace_id, &[]).await;
    for entry in summaries("fresh", &fresh) {
        let id = text(&entry["anomalyDetectorId"]);
        assert_eq!(
            text(&entry["status"]["statusCode"]),
            STATUS_CREATING,
            "case {id:?}: a fresh detector lists as {STATUS_CREATING}, got {entry:?}"
        );
    }

    wait_for_due_transition();

    let listed = list_detectors(&amp, region, &workspace_id, &[]).await;
    let page = summaries("settled", &listed);
    assert_eq!(
        page.len(),
        seeded.len(),
        "the listing must report every stored detector, got {listed:?}"
    );

    for entry in &page {
        let id = text(&entry["anomalyDetectorId"]);
        let described = describe_detector(&amp, region, &workspace_id, &id).await;
        assert_eq!(
            described.status, 200,
            "case {id:?}: the listed detector must be readable, got {described:?}"
        );
        let record = &described.body["anomalyDetector"];
        assert_eq!(
            text(&entry["arn"]),
            text(&record["arn"]),
            "case {id:?}: the summary's arn must be the stored one, got {entry:?}"
        );
        assert_eq!(
            text(&entry["alias"]),
            text(&record["alias"]),
            "case {id:?}: the summary's alias must be the stored one, got {entry:?}"
        );
        assert_eq!(
            text(&entry["status"]["statusCode"]),
            STATUS_ACTIVE,
            "case {id:?}: the clock owes the move to {STATUS_ACTIVE}, got {entry:?}"
        );
        assert!(
            entry["status"]["statusReason"].is_null(),
            "case {id:?}: nothing failed, so statusReason must be absent, got {entry:?}"
        );
        assert_eq!(
            entry["createdAt"].as_f64(),
            record["createdAt"].as_f64(),
            "case {id:?}: createdAt must be the stored instant, got {entry:?}"
        );
        assert_eq!(
            entry["modifiedAt"].as_f64(),
            record["modifiedAt"].as_f64(),
            "case {id:?}: modifiedAt must be the stored instant, got {entry:?}"
        );
        assert_eq!(
            tags_of(&entry["tags"]),
            tags_of(&record["tags"]),
            "case {id:?}: the summary's tags must be the stored ones, got {entry:?}"
        );
        assert_eq!(
            tags_of(&entry["tags"]).keys().cloned().collect::<Vec<_>>(),
            vec!["owner".to_string()],
            "case {id:?}: the seeded tag key must be reported, got {entry:?}"
        );
    }

    // A summary carries no member the shape does not model, and the detector's own
    // configuration is `DescribeAnomalyDetector`'s to report, not the listing's.
    for entry in &page {
        let id = text(&entry["anomalyDetectorId"]);
        assert!(
            entry.get("configuration").is_none(),
            "case {id:?}: AnomalyDetectorSummary models no configuration, got {entry:?}"
        );
    }
}

/// `maxResults` bounds one page and `nextToken` carries the walk to the next: every page
/// size concatenates to the same single list, ordered by `anomalyDetectorId`, and only a
/// page with more behind it carries a token. One row per page size, including each
/// published bound of the modelled `@range(min: 1, max: 1000)`.
#[tokio::test]
async fn pagination_walks_the_same_ordered_list_whatever_the_page_size() {
    let amp = Amp::new();
    let region = "ap-south-1";
    let (workspace_id, seeded) = seeded_workspace(&amp, region, "paging-host").await;
    let total = seeded.len();
    let mut ordered: Vec<String> = seeded.iter().map(|(_, id)| id.clone()).collect();
    ordered.sort();

    for page_size in [1i32, 2, 3, 7, 8, 9, ANOMALY_DETECTOR_PAGE_LIMIT] {
        let case = format!("maxResults {page_size}");
        let (ids, pages) = walk_pages(
            &amp,
            region,
            &workspace_id,
            &case,
            &[("maxResults", page_size.to_string())],
        )
        .await;
        assert_eq!(
            ids, ordered,
            "case {case:?}: the walk must report every detector once, in id order, got {ids:?}"
        );
        let expected_pages = total.div_ceil(page_size.clamp(1, i32::MAX) as usize).max(1);
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: {total} detectors at {page_size} per page is {expected_pages} \
             pages, walked {pages}"
        );
    }

    // The filter holds across the page boundary too: the other parameters stay the same
    // as the initial call, so a cursor never widens the page back to the whole workspace.
    let mut shared_ids: Vec<String> = seeded
        .iter()
        .filter(|(alias, _)| alias == "shared")
        .map(|(_, id)| id.clone())
        .collect();
    shared_ids.sort();
    let (filtered, pages) = walk_pages(
        &amp,
        region,
        &workspace_id,
        "filtered, one per page",
        &[("maxResults", "1".to_string()), ("alias", "shared".to_string())],
    )
    .await;
    assert_eq!(
        filtered, shared_ids,
        "case \"filtered, one per page\": the alias must hold on every page, got {filtered:?}"
    );
    assert_eq!(
        pages, 2,
        "case \"filtered, one per page\": two matches at one per page is two pages, walked {pages}"
    );

    // A page that holds every remaining entry owes no token.
    let whole = list_detectors(
        &amp,
        region,
        &workspace_id,
        &[("maxResults", total.to_string())],
    )
    .await;
    assert_eq!(
        next_token(&whole),
        None,
        "case \"maxResults equals the total\": the last page carries no nextToken, got {whole:?}"
    );

    // The cursor is exclusive: a token is the last id of the page before it, so the page
    // it opens starts after that id and nothing is reported twice.
    let first = list_detectors(&amp, region, &workspace_id, &[("maxResults", "3".to_string())]).await;
    let cursor = next_token(&first).expect("a token while entries remain");
    assert_eq!(
        cursor,
        ordered[2],
        "case \"cursor\": the token must be the last id of the page, got {first:?}"
    );
    let second = list_detectors(
        &amp,
        region,
        &workspace_id,
        &[("maxResults", "3".to_string()), ("nextToken", cursor)],
    )
    .await;
    assert_eq!(
        ids_of("cursor", &second),
        ordered[3..6].to_vec(),
        "case \"cursor\": the next page must start after the token, got {second:?}"
    );
}

/// A request that names no `maxResults` still gets a bounded page — the size
/// `ANOMALY_DETECTOR_PAGE_DEFAULT` names — with a token for the rest; naming the
/// published maximum gets them all in one page.
#[tokio::test]
async fn an_omitted_max_results_serves_a_bounded_default_page() {
    let amp = Amp::new();
    let region = "us-east-2";
    let workspace_id = amp.active_workspace(region, "default-page-host").await;

    let total = ANOMALY_DETECTOR_PAGE_DEFAULT as usize + 3;
    let mut expected: Vec<String> = Vec::new();
    for index in 0..total {
        let created = create_detector(
            &amp,
            region,
            &workspace_id,
            &json!({
                "alias": format!("bulk{index:03}"),
                "configuration": minimal_configuration("sum(rate(requests_total[5m]))"),
            }),
        )
        .await;
        assert_eq!(
            created.status, 202,
            "seeding {index}: the create must answer 202, got {created:?}"
        );
        expected.push(text(&created.body["anomalyDetectorId"]));
    }
    expected.sort();

    let first = list_detectors(&amp, region, &workspace_id, &[]).await;
    assert_eq!(
        first.status, 200,
        "case \"no maxResults\": the listing must answer 200, got {first:?}"
    );
    let first_ids = ids_of("no maxResults", &first);
    assert_eq!(
        first_ids.len(),
        ANOMALY_DETECTOR_PAGE_DEFAULT as usize,
        "case \"no maxResults\": the default page holds {ANOMALY_DETECTOR_PAGE_DEFAULT} entries, \
         got {}",
        first_ids.len()
    );
    assert_eq!(
        next_token(&first).as_deref(),
        first_ids.last().map(|id| id.as_str()),
        "case \"no maxResults\": the token is the cursor past the last entry, got {first:?}"
    );

    let (walked, pages) = walk_pages(&amp, region, &workspace_id, "no maxResults", &[]).await;
    assert_eq!(
        walked, expected,
        "case \"no maxResults\": the walk must report all {total} detectors in id order"
    );
    assert_eq!(
        pages, 2,
        "case \"no maxResults\": {total} detectors at {ANOMALY_DETECTOR_PAGE_DEFAULT} per page \
         is two pages, walked {pages}"
    );

    let whole = list_detectors(
        &amp,
        region,
        &workspace_id,
        &[("maxResults", ANOMALY_DETECTOR_PAGE_LIMIT.to_string())],
    )
    .await;
    assert_eq!(
        ids_of("maxResults at the published maximum", &whole),
        expected,
        "case \"maxResults at the published maximum\": one page holds all {total}, got {whole:?}"
    );
}

/// `maxResults` is modelled `@range(min: 1, max: 1000)` and `alias` `@length(1..64)` with
/// a pattern: a value past a published bound is refused with the modelled
/// `ValidationException` (400) rather than served.
#[tokio::test]
async fn an_input_past_a_published_bound_is_refused() {
    let amp = Amp::new();
    let region = "eu-west-1";
    let (workspace_id, _) = seeded_workspace(&amp, region, "bounds-host").await;

    for (case, value) in [
        ("one below the published minimum", 0i32),
        ("negative", -1),
        ("one past the published maximum", ANOMALY_DETECTOR_PAGE_LIMIT + 1),
    ] {
        let refused = list_detectors(
            &amp,
            region,
            &workspace_id,
            &[("maxResults", value.to_string())],
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: maxResults {value} must be refused with 400, got {refused:?}"
        );
        assert_eq!(
            refused.error_code(),
            "ValidationException",
            "case {case:?}: expected the modelled ValidationException, got {refused:?}"
        );
    }

    // Each published bound of `maxResults` itself is accepted.
    for (case, value) in [
        ("the published minimum", 1i32),
        ("the published maximum", ANOMALY_DETECTOR_PAGE_LIMIT),
    ] {
        let served = list_detectors(
            &amp,
            region,
            &workspace_id,
            &[("maxResults", value.to_string())],
        )
        .await;
        assert_eq!(
            served.status, 200,
            "case {case:?}: maxResults {value} is inside the modelled range, got {served:?}"
        );
    }

    // `AnomalyDetectorAlias`: length 1..64, pattern `^[0-9A-Za-z][-.0-9A-Z_a-z]*$`.
    for (case, alias) in [
        ("an alias one character past the length bound", "a".repeat(65)),
        ("an alias the pattern rejects", "-leading-dash".to_string()),
    ] {
        let refused = list_detectors(&amp, region, &workspace_id, &[("alias", alias.clone())]).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: alias {alias:?} must be refused with 400, got {refused:?}"
        );
        assert_eq!(
            refused.error_code(),
            "ValidationException",
            "case {case:?}: expected the modelled ValidationException, got {refused:?}"
        );
    }

    // The length bound itself is accepted, and matches nothing.
    let served = list_detectors(
        &amp,
        region,
        &workspace_id,
        &[("alias", "a".repeat(64))],
    )
    .await;
    assert_eq!(
        served.status, 200,
        "case \"an alias at the length bound\": inside the modelled length, got {served:?}"
    );
    assert_eq!(
        sorted_aliases("an alias at the length bound", &served),
        Vec::<String>::new(),
        "case \"an alias at the length bound\": nothing carries it, got {served:?}"
    );
}

/// A listing is a read: it reports what the store holds and changes nothing a later
/// caller sees. It is also scoped to ONE workspace, and a workspace the account does not
/// hold is the modelled `ResourceNotFoundException`.
#[tokio::test]
async fn the_listing_is_scoped_to_its_workspace_and_changes_nothing() {
    let amp = Amp::new();
    let region = "us-west-2";
    let first = amp.active_workspace(region, "first").await;
    let second = amp.active_workspace(region, "second").await;

    // A workspace that holds nothing lists the empty page, not an error.
    let empty = list_detectors(&amp, region, &first, &[]).await;
    assert_eq!(
        empty.status, 200,
        "case \"empty workspace\": the listing must answer 200, got {empty:?}"
    );
    assert_eq!(
        ids_of("empty workspace", &empty),
        Vec::<String>::new(),
        "case \"empty workspace\": nothing is stored, so the page is empty, got {empty:?}"
    );
    assert_eq!(
        next_token(&empty),
        None,
        "case \"empty workspace\": an empty page owes no nextToken, got {empty:?}"
    );

    let mut owned: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for (case, workspace_id, aliases) in [
        ("first workspace", &first, vec!["only-here", "shared"]),
        ("second workspace", &second, vec!["shared"]),
    ] {
        let mut ids = Vec::new();
        for alias in &aliases {
            let created = create_detector(
                &amp,
                region,
                workspace_id,
                &json!({
                    "alias": alias,
                    "configuration": minimal_configuration("max(rate(requests_total[5m]))"),
                }),
            )
            .await;
            assert_eq!(
                created.status, 202,
                "case {case:?}: seeding {alias:?} must answer 202, got {created:?}"
            );
            ids.push(text(&created.body["anomalyDetectorId"]));
        }
        ids.sort();
        let listed = list_detectors(&amp, region, workspace_id, &[]).await;
        let mut reported = ids_of(case, &listed);
        reported.sort();
        assert_eq!(
            reported, ids,
            "case {case:?}: the listing reports only this workspace's detectors, got {listed:?}"
        );
        owned.insert(case, ids);
    }

    // A cursor another workspace's listing minted can only DROP entries from this page:
    // it is a filter over the records this workspace holds, never a key into the store.
    let foreign_cursor = owned["second workspace"][0].clone();
    let with_foreign_cursor = list_detectors(
        &amp,
        region,
        &first,
        &[("nextToken", foreign_cursor.clone())],
    )
    .await;
    assert_eq!(
        with_foreign_cursor.status, 200,
        "case \"foreign cursor\": the listing must answer 200, got {with_foreign_cursor:?}"
    );
    let reported = ids_of("foreign cursor", &with_foreign_cursor);
    for id in &reported {
        assert!(
            owned["first workspace"].contains(id)
                && id.as_str() > foreign_cursor.as_str(),
            "case \"foreign cursor\": only this workspace's entries past the cursor may be \
             reported, got {reported:?}"
        );
    }

    // Reading twice reports the same page, and a describe afterwards still finds every
    // detector: nothing was consumed by the read.
    let repeated = list_detectors(&amp, region, &first, &[]).await;
    let mut repeated_ids = ids_of("read twice", &repeated);
    repeated_ids.sort();
    assert_eq!(
        repeated_ids, owned["first workspace"],
        "case \"read twice\": a listing must not change what a later caller sees, got {repeated:?}"
    );
    for id in &owned["first workspace"] {
        let described = describe_detector(&amp, region, &first, id).await;
        assert_eq!(
            described.status, 200,
            "case {id:?}: the detector must survive the listing, got {described:?}"
        );
    }

    // A workspace id of the shape the service mints that no workspace holds.
    let absent = list_detectors(
        &amp,
        region,
        "ws-00000000-0000-0000-0000-000000000000",
        &[],
    )
    .await;
    assert_eq!(
        absent.status, 404,
        "case \"absent workspace\": expected 404, got {absent:?}"
    );
    assert_eq!(
        absent.error_code(),
        "ResourceNotFoundException",
        "case \"absent workspace\": expected ResourceNotFoundException, got {absent:?}"
    );
}
