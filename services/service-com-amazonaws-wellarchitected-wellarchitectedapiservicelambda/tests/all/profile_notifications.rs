// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `ListProfileNotifications` — "List profile notifications"
//! (`smithy_query(query="operation", operation="ListProfileNotifications")`).
//!
//! The rules pinned here, each over a TABLE of inputs rather than one recorded request:
//!
//! 1. The list reports one `ProfileNotificationSummary` per ASSOCIATION of a profile with a
//!    workload whose pinned `ProfileVersion` is not the version that profile is now at:
//!    `CurrentProfileVersion` is the version the association is pinned at,
//!    `LatestProfileVersion` the version the profile is at, `Type` is
//!    `PROFILE_ANSWERS_UPDATED`, and the ARN, name, workload id and workload name are read
//!    off the profile and the workload the association joins. An association already at the
//!    profile's version has nothing to report, and a store with no association at all
//!    reports an empty page.
//! 2. `WorkloadId` narrows the report to the association OF that workload; a request
//!    carrying none asks about every workload. The operation models no
//!    `ResourceNotFoundException`
//!    (`smithy_query(query="operation", operation="ListProfileNotifications")` publishes
//!    `AccessDenied`, `InternalServer`, `Throttling` and `Validation` only), so a
//!    `WorkloadId` no stored workload answers to selects nothing and gets an empty page.
//! 3. `MaxResults`/`NextToken` page over the selection: however small the page, following
//!    `NextToken` enumerates every notification exactly once in one stable order, and the
//!    page that holds the rest of the selection carries no `NextToken`. The published
//!    `@range(1, 50)` bounds of `MaxResults` are served and a value past either of them is
//!    refused.
//! 4. A `NextToken` this service did not issue cannot be decoded, so it is a
//!    `400 ValidationException` classified `CANNOT_PARSE` and naming `NextToken` — not an
//!    empty page.
//! 5. A notification is DERIVED from the association, so `UpgradeProfileVersion` — which
//!    moves the association to the version the profile is at — clears it, and listing
//!    itself writes nothing: two identical reads report the same notifications.
//!
//! Every workload, profile and association here is made over HTTP (`CreateWorkload`,
//! `CreateProfile`, `AssociateProfiles`). One half of the starting situation has no HTTP
//! path yet: a profile REVISION, which is what puts a profile ahead of the version an
//! association pinned — associating pins the version the profile is at, and minting a
//! profile's next version belongs to `UpdateProfile`, still a stub with its own issue — so
//! that revision is seeded through the shared state by the harness's
//! `Client::revise_profile_version`, exactly as `super::workload_profile_upgrade` seeds it.
//! Everything asserted crosses the HTTP boundary: requests go through the real
//! `MultiServiceRouter`, so routing, restJson1 (de)serialization, boundary constraint
//! validation and the handler all run as they do in production.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use super::workload_harness::{
    created_id, created_profile, encode_path_segment, minimal_create, Answer, Client,
};

/// Every member `ProfileNotificationSummary` declares, from
/// `smithy_query(query="shape", shape="ProfileNotificationSummary")`. A summary carries
/// these and nothing else.
const SUMMARY_MEMBERS: [&str; 7] = [
    "CurrentProfileVersion",
    "LatestProfileVersion",
    "ProfileArn",
    "ProfileName",
    "Type",
    "WorkloadId",
    "WorkloadName",
];

/// The one `ProfileNotificationType` an association behind its profile raises
/// (`smithy_query(query="shape", shape="ProfileNotificationType")`, which publishes
/// `PROFILE_ANSWERS_UPDATED` and `PROFILE_DELETED`; the other cannot arise, because
/// deleting a profile drops every association that named it — pinned by
/// `super::profile_deletion`).
const PROFILE_ANSWERS_UPDATED: &str = "PROFILE_ANSWERS_UPDATED";

/// A `WorkloadId`-shaped value (`^[0-9a-f]{32}$`) that no stored workload carries, so the
/// empty page it draws is the enumeration's and not the boundary validator's.
const ABSENT_WORKLOAD_ID: &str = "0123456789abcdef0123456789abcdef";

/// What names one notification: the `WorkloadId` of the workload it is about and the
/// `ProfileArn` of the association's profile — the pair the emulator orders and pages the
/// report by.
type Association = (String, String);

/// `ListProfileNotifications`: `GET /profileNotifications`, with every member bound to the
/// query string. `query` is passed as sent, empty for a request that names none.
async fn list(client: &Client, query: &str) -> Answer {
    let uri = if query.is_empty() {
        "/profileNotifications".to_string()
    } else {
        format!("/profileNotifications?{query}")
    };
    client.send("GET", &uri, None).await
}

/// The `NotificationSummaries` a successful `ListProfileNotifications` answered with.
fn summaries(answer: &Answer, case: &str) -> Vec<Value> {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected ListProfileNotifications 200, got {answer:?}"
    );
    answer
        .body
        .get("NotificationSummaries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: a successful list must carry NotificationSummaries, got {answer:?}"
            )
        })
}

/// One member of a summary as a string, empty when the summary carries none.
fn field(summary: &Value, name: &str) -> String {
    summary
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// `AssociateProfiles`: `PATCH /workloads/{WorkloadId}/associateProfiles`.
async fn associate(client: &Client, workload_id: &str, profile_arn: &str, case: &str) {
    let answer = client
        .send(
            "PATCH",
            &format!("/workloads/{workload_id}/associateProfiles"),
            Some(json!({ "ProfileArns": [profile_arn] })),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected AssociateProfiles 200, got {answer:?}"
    );
}

/// A workload created over HTTP with `name`, and its id.
async fn workload(client: &Client, name: &str) -> String {
    let answer = client.create(minimal_create(name, &format!("tok-{name}"))).await;
    created_id(&answer, name)
}

/// One workload reviewed against one profile: the workload's id and name, and the profile's
/// ARN and the version the association pinned. `revision` is the version the profile is
/// moved to after the association, or `None` to leave the profile where the association
/// pinned it.
async fn reviewed(
    client: &Client,
    name: &str,
    revision: Option<&str>,
) -> (String, String, String) {
    let workload_id = workload(client, name).await;
    let (profile_arn, pinned) = created_profile(client, &format!("profile-{name}")).await;
    associate(client, &workload_id, &profile_arn, name).await;
    if let Some(version) = revision {
        client.revise_profile_version(&profile_arn, version);
    }
    (workload_id, profile_arn, pinned)
}

/// Every notification the request selects, read by following `NextToken` from the first
/// page to the last with pages of `page_size` (or in one request when `page_size` is
/// `None`), with `filters` (already query-encoded, empty for none) carried by each request.
async fn walk(client: &Client, case: &str, page_size: Option<i64>, filters: &str) -> Vec<Value> {
    let mut walked: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    // One more request than there can be pages, so a handler that never stops paging
    // fails here instead of hanging.
    for request in 0..64 {
        let mut query: Vec<String> = Vec::new();
        if !filters.is_empty() {
            query.push(filters.to_string());
        }
        if let Some(size) = page_size {
            query.push(format!("MaxResults={size}"));
        }
        if let Some(cursor) = &token {
            query.push(format!("NextToken={cursor}"));
        }
        let answer = list(client, &query.join("&")).await;
        let page = summaries(&answer, case);
        if let Some(size) = page_size {
            assert!(
                i64::try_from(page.len()).expect("a page fits in an i64") <= size,
                "case {case:?}: request {request} returned {} summaries for MaxResults={size}",
                page.len()
            );
        }
        walked.extend(page);
        token = match answer.body.get("NextToken").and_then(Value::as_str) {
            Some(cursor) => {
                assert!(
                    !cursor.is_empty()
                        && cursor.chars().all(|character| character.is_ascii_alphanumeric()
                            || matches!(character, '+' | '/' | '=' | '_' | '-')),
                    "case {case:?}: NextToken must match ^[A-Za-z0-9+/=_-]+$, got {cursor:?}"
                );
                Some(cursor.to_string())
            }
            None => return walked,
        };
    }
    panic!("case {case:?}: the pages never ran out");
}

/// Rule 1. The rows differ along the version the profile was revised to — one revision, a
/// tenth revision, a version carrying the `-` its `^[A-Za-z0-9-]+$` pattern allows and one
/// at the published `@length(1, 32)` maximum — plus the row where the profile was NOT
/// revised, which must report nothing.
#[tokio::test]
async fn an_association_behind_its_profile_raises_one_notification() {
    // The published `ProfileVersion` maximum: `@length(min: 1, max: 32)`
    // (`smithy_query(query="shape", shape="ProfileVersion")`).
    let longest = "9".repeat(32);

    let rows: Vec<(&str, Option<&str>)> = vec![
        ("one-revision-ahead", Some("2")),
        ("many-revisions-ahead", Some("10")),
        ("hyphenated-version", Some("2-0")),
        ("longest-version", Some(longest.as_str())),
        ("not-revised", None),
    ];

    for (case, revision) in rows {
        let client = Client::new();
        let (workload_id, profile_arn, pinned) = reviewed(&client, case, revision).await;

        let reported = summaries(&list(&client, "").await, case);
        let expected = usize::from(revision.is_some());
        assert_eq!(
            reported.len(),
            expected,
            "case {case:?}: expected {expected} notification(s), got {reported:?}"
        );

        let Some(latest) = revision else {
            continue;
        };
        let summary = &reported[0];
        for (member, want) in [
            ("CurrentProfileVersion", pinned.as_str()),
            ("LatestProfileVersion", latest),
            ("Type", PROFILE_ANSWERS_UPDATED),
            ("ProfileArn", profile_arn.as_str()),
            ("ProfileName", &format!("profile-{case}")),
            ("WorkloadId", workload_id.as_str()),
            ("WorkloadName", case),
        ] {
            assert_eq!(
                field(summary, member),
                want,
                "case {case:?}: expected {member} {want:?}, got {summary:?}"
            );
        }
        // A summary carries exactly the members the shape declares.
        let carried: BTreeSet<&str> = summary
            .as_object()
            .unwrap_or_else(|| panic!("case {case:?}: a summary is an object, got {summary:?}"))
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            carried,
            SUMMARY_MEMBERS.into_iter().collect::<BTreeSet<&str>>(),
            "case {case:?}: unexpected member set, got {summary:?}"
        );
    }
}

/// Rule 1, continued, and rule 2. The rows are the `WorkloadId`s a request can name over
/// one store holding workloads that differ along what they have to report: one behind its
/// profile, another behind a different profile, one at its profile's version, and one
/// reviewed against no profile at all.
#[tokio::test]
async fn workload_id_narrows_the_report_to_one_workload() {
    let client = Client::new();
    let (behind_one, arn_one, _) = reviewed(&client, "behind-one", Some("2")).await;
    let (behind_two, arn_two, _) = reviewed(&client, "behind-two", Some("3")).await;
    let (current, _, _) = reviewed(&client, "current", None).await;
    let unreviewed = workload(&client, "unreviewed").await;

    // The emulator's order is the pair that names the association — the workload's id, then
    // the profile's ARN — the only members of it fixed for the resources' whole lives.
    let mut everything: Vec<Association> = vec![
        (behind_one.clone(), arn_one.clone()),
        (behind_two.clone(), arn_two.clone()),
    ];
    everything.sort();

    let rows: Vec<(&str, String, Vec<Association>)> = vec![
        ("no-filter", String::new(), everything),
        (
            "the-first-workload-behind",
            format!("WorkloadId={behind_one}"),
            vec![(behind_one.clone(), arn_one)],
        ),
        (
            "the-second-workload-behind",
            format!("WorkloadId={behind_two}"),
            vec![(behind_two.clone(), arn_two)],
        ),
        (
            "a-workload-at-its-profile-version",
            format!("WorkloadId={current}"),
            Vec::new(),
        ),
        (
            "a-workload-reviewed-against-no-profile",
            format!("WorkloadId={unreviewed}"),
            Vec::new(),
        ),
        (
            "a-workload-that-is-not-stored",
            format!("WorkloadId={ABSENT_WORKLOAD_ID}"),
            Vec::new(),
        ),
    ];

    for (case, query, expected) in rows {
        let answer = list(&client, &query).await;
        let reported: Vec<Association> = summaries(&answer, case)
            .iter()
            .map(|summary| (field(summary, "WorkloadId"), field(summary, "ProfileArn")))
            .collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the associations {expected:?}, got {answer:?}"
        );
    }
}

/// Rule 3. The rows are every page size from one notification per page up to past the
/// stored count, plus the published `@range(1, 50)` maximum and a request that omits
/// `MaxResults` — however the selection is cut, the pages enumerate the same notifications
/// in the same order, exactly once each.
#[tokio::test]
async fn paging_enumerates_every_notification_exactly_once() {
    let client = Client::new();
    let count = 7;
    let mut expected: Vec<Association> = Vec::new();
    for index in 0..count {
        // The rows of this fixture differ along the version each profile was revised to, so
        // one page size is exercised over several `LatestProfileVersion`s.
        let (workload_id, profile_arn, _) = reviewed(
            &client,
            &format!("paged-{index}"),
            Some(&format!("{}", index + 2)),
        )
        .await;
        expected.push((workload_id, profile_arn));
    }
    expected.sort();

    for (case, page_size) in [
        ("omitted", None),
        ("published-minimum", Some(1)),
        ("two-per-page", Some(2)),
        ("three-per-page", Some(3)),
        ("one-short-of-the-count", Some(6)),
        ("exactly-the-count", Some(7)),
        ("more-than-the-count", Some(8)),
        ("published-maximum", Some(50)),
    ] {
        let walked: Vec<Association> = walk(&client, case, page_size, "")
            .await
            .iter()
            .map(|summary| (field(summary, "WorkloadId"), field(summary, "ProfileArn")))
            .collect();
        assert_eq!(
            walked, expected,
            "case {case:?}: paging must enumerate every notification exactly once in \
             (WorkloadId, ProfileArn) order"
        );
    }

    // A page that holds the rest of the selection carries no `NextToken`; one that does
    // not, does.
    for (case, max_results, expects_token) in [
        ("short-page", 3, true),
        ("page-of-the-whole-selection", 7, false),
        ("page-larger-than-the-selection", 50, false),
    ] {
        let answer = list(&client, &format!("MaxResults={max_results}")).await;
        assert_eq!(
            answer.body.get("NextToken").and_then(Value::as_str).is_some(),
            expects_token,
            "case {case:?}: NextToken must be present only while notifications remain, \
             got {answer:?}"
        );
    }

    // The published bounds are served; a value past either of them is refused before the
    // handler runs.
    for (case, max_results, expected_status) in [
        ("published-minimum", 1, 200),
        ("published-maximum", 50, 200),
        ("below-the-minimum", 0, 400),
        ("above-the-maximum", 51, 400),
    ] {
        let answer = list(&client, &format!("MaxResults={max_results}")).await;
        assert_eq!(
            answer.status, expected_status,
            "case {case:?}: expected HTTP {expected_status} for MaxResults={max_results}, \
             got {answer:?}"
        );
        if expected_status == 400 {
            assert_eq!(
                answer.error_code, "ValidationException",
                "case {case:?}: expected ValidationException, got {answer:?}"
            );
        }
    }
}

/// Rule 4. The rows are the ways a `NextToken` can fail to be one this service issued: not
/// its encoding at all, its encoding but not a cursor, a truncated or extended token it did
/// issue, a cursor carrying only half of this operation's key, and the cursors ANOTHER
/// paginated operation of this service issues.
#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let client = Client::new();
    for index in 0..3 {
        reviewed(&client, &format!("token-{index}"), Some("2")).await;
    }
    let issued = list(&client, "MaxResults=1")
        .await
        .body
        .get("NextToken")
        .and_then(Value::as_str)
        .expect("three notifications and a page of one leave a NextToken")
        .to_string();

    let rows: Vec<(&str, String)> = vec![
        ("not-hex", "zzzz".to_string()),
        ("hex-but-not-a-cursor", "6162".to_string()),
        ("single-character", "a".to_string()),
        (
            "truncated-issued-token",
            issued[..issued.len() / 2].to_string(),
        ),
        ("issued-token-with-a-suffix", format!("{issued}00")),
        // hex of {"w":"a"} — half of this operation's own cursor, which names an
        // association by BOTH the workload and the profile.
        ("half-a-cursor", "7b2277223a2261227d".to_string()),
        // hex of {"o":"a"} — the cursor `ListProfiles` issues over the same profiles.
        ("a-profiles-cursor", "7b226f223a2261227d".to_string()),
        // hex of {"n":"a","i":"b"} — the cursor `ListWorkloads` issues.
        (
            "a-workloads-cursor",
            "7b226e223a2261222c2269223a2262227d".to_string(),
        ),
    ];

    for (case, token) in rows {
        let answer = list(&client, &format!("NextToken={token}")).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: a token this service did not issue is a 400, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case {case:?}: expected ValidationException, got {answer:?}"
        );
        assert_eq!(
            answer.body.get("Reason").and_then(Value::as_str),
            Some("CANNOT_PARSE"),
            "case {case:?}: an opaque token that will not decode is CANNOT_PARSE, got {answer:?}"
        );
        assert_eq!(
            answer
                .body
                .get("Fields")
                .and_then(Value::as_array)
                .map(|fields| fields
                    .iter()
                    .map(|entry| entry.get("Name").and_then(Value::as_str).unwrap_or_default())
                    .collect::<Vec<_>>()),
            Some(vec!["NextToken"]),
            "case {case:?}: the finding must name NextToken, got {answer:?}"
        );
    }

    // The token this service DID issue still pages, so the refusals above are about the
    // token and not about the operation.
    let resumed = list(&client, &format!("NextToken={issued}")).await;
    assert_eq!(
        resumed.status, 200,
        "case \"issued-token\": a token this service issued must page, got {resumed:?}"
    );
}

/// Rule 5. The rows are the versions a profile can be ahead by: for each, the notification
/// the stale association raises survives repeated reads unchanged and is GONE once
/// `UpgradeProfileVersion` has moved that association to the profile's version — the read
/// reports state, so nothing but a write to the association clears it.
#[tokio::test]
async fn upgrading_the_association_clears_its_notification() {
    for (case, revision) in [
        ("one-revision-ahead", "2"),
        ("many-revisions-ahead", "12"),
        ("hyphenated-version", "3-1"),
    ] {
        let client = Client::new();
        let (workload_id, profile_arn, _) = reviewed(&client, case, Some(revision)).await;

        // Reporting writes nothing: two identical reads report the same notification.
        let first = summaries(&list(&client, "").await, case);
        let second = summaries(&list(&client, "").await, case);
        assert_eq!(
            first, second,
            "case {case:?}: two identical reads must report the same notifications"
        );
        assert_eq!(
            first.len(),
            1,
            "case {case:?}: an association behind its profile raises one notification, \
             got {first:?}"
        );

        let upgraded = client
            .send(
                "PUT",
                &format!(
                    "/workloads/{workload_id}/profiles/{}/upgrade",
                    encode_path_segment(&profile_arn)
                ),
                Some(json!({ "ClientRequestToken": format!("upgrade-{case}") })),
            )
            .await;
        assert_eq!(
            upgraded.status, 200,
            "case {case:?}: expected UpgradeProfileVersion 200, got {upgraded:?}"
        );

        let cleared = summaries(&list(&client, "").await, case);
        assert!(
            cleared.is_empty(),
            "case {case:?}: an upgraded association is reviewed against the profile as it \
             stands and reports nothing, got {cleared:?}"
        );
    }
}
