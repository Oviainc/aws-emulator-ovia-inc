// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `ListNotifications` — "List lens notifications"
//! (`smithy_query(query="operation", operation="ListNotifications")`).
//!
//! The rules pinned here, each over a TABLE of inputs rather than one recorded request:
//!
//! 1. The list reports one `NotificationSummary` per LENS REVIEW — of a workload or of a
//!    review template — that is pinned at a version its lens has published past:
//!    `CurrentLensVersion` is the version the review pins, `LatestLensVersion` the version
//!    the lens is at, `Type` is `LENS_VERSION_UPGRADED`, and the resource members name the
//!    resource holding the review (`WorkloadId`/`WorkloadName` for a workload,
//!    `ResourceArn`/`ResourceName` for a review template, whose `ResourceArn` is patterned
//!    for a `review-template` ARN alone). A review against its lens's latest version — which
//!    every review of an AWS-official lens is, each carrying one version here — has nothing
//!    to report.
//! 2. `WorkloadId` narrows the report to the reviews OF that workload and `ResourceArn` to
//!    those of that review template; a request carrying neither asks about every resource.
//!    The operation models no `ResourceNotFoundException`
//!    (`smithy_query(query="operation", operation="ListNotifications")` publishes
//!    `AccessDenied`, `InternalServer`, `Throttling` and `Validation` only), so a selector no
//!    stored resource answers to selects nothing and gets an empty page — while a request
//!    setting BOTH selectors is refused, because "Only one of WorkloadID or ResourceARN
//!    should be specified" (`smithy_query(query="doc", shape="ListNotificationsInput",
//!    member="ResourceArn")`).
//! 3. `MaxResults`/`NextToken` page over the selection: however small the page, following
//!    `NextToken` enumerates every notification exactly once in one stable order, and the
//!    page that holds the rest of the selection carries no `NextToken`. The published
//!    `@range(1, 50)` bounds of `MaxResults` are served and a value past either of them is
//!    refused.
//! 4. A `NextToken` this service did not issue cannot be decoded, so it is a
//!    `400 ValidationException` classified `CANNOT_PARSE` and naming `NextToken` — not an
//!    empty page.
//! 5. A notification is DERIVED from the lens review, so upgrading that review — which moves
//!    it to the version the lens is at — clears it, and listing itself writes nothing: two
//!    identical reads report the same notifications.
//!
//! Every workload, review template, lens and published lens version here is made over HTTP
//! (`CreateWorkload`, `CreateReviewTemplate`, `ImportLens`, `CreateLensVersion`), so nothing
//! is seeded behind the API: requests go through the real `MultiServiceRouter` and routing,
//! restJson1 (de)serialization, boundary constraint validation and the handler all run as
//! they do in production. The HTTP client and the review-template seeding are the shared
//! `super::workload_harness`; the lens import and publish wire helpers are the ones
//! `super::custom_lens_import` and `super::custom_lens_versions` already spell.

use serde_json::{json, Value};

use super::custom_lens_import::{import, imported_arn, lens_document, wait_for_the_import_to_land};
use super::custom_lens_versions::create_version;
use super::workload_harness::{
    created_id, encode_path_segment, minimal_create, review_template, Answer, Client,
};

/// The alias of the one AWS-official lens every workload here reviews beside its custom
/// lens. An official lens carries exactly one version in this emulator, so a review of one
/// is never behind and raises no notification.
const FRAMEWORK_LENS: &str = "wellarchitected";

/// The one `NotificationType` a lens review behind its lens raises
/// (`smithy_query(query="shape", shape="NotificationType")`, which publishes
/// `LENS_VERSION_UPGRADED` and `LENS_VERSION_DEPRECATED`; the other cannot arise, because no
/// lens in either half of this emulator's catalog is ever retired).
const LENS_VERSION_UPGRADED: &str = "LENS_VERSION_UPGRADED";

/// Every member `NotificationSummary` declares, from `smithy_query(query="shape",
/// shape="NotificationSummary")`. A summary carries these and nothing else.
const SUMMARY_MEMBERS: [&str; 2] = ["LensUpgradeSummary", "Type"];

/// The members of `LensUpgradeSummary` every notification carries, whichever resource holds
/// the review (`smithy_query(query="shape", shape="LensUpgradeSummary")`).
const SHARED_UPGRADE_MEMBERS: [&str; 4] = [
    "CurrentLensVersion",
    "LatestLensVersion",
    "LensAlias",
    "LensArn",
];

/// The members of `LensUpgradeSummary` that name a WORKLOAD holding the review.
const WORKLOAD_MEMBERS: [&str; 2] = ["WorkloadId", "WorkloadName"];

/// The members of `LensUpgradeSummary` that name a REVIEW TEMPLATE holding the review.
const TEMPLATE_MEMBERS: [&str; 2] = ["ResourceArn", "ResourceName"];

/// A `WorkloadId`-shaped value (`^[0-9a-f]{32}$`) that no stored workload carries, so the
/// empty page it draws is the enumeration's and not the boundary validator's.
const ABSENT_WORKLOAD_ID: &str = "0123456789abcdef0123456789abcdef";

/// A `ResourceArn`-shaped value naming no stored review template: the modelled
/// `^arn:aws…:\d{12}:(review-template)/[a-f0-9]{32}$` form, inside the published
/// `@length(50, 250)`.
const ABSENT_TEMPLATE_ARN: &str =
    "arn:aws:wellarchitected:us-west-2:000000000000:review-template/0123456789abcdef0123456789abcdef";

/// `ListNotifications`: `POST /notifications`, with every member bound to the body (the
/// operation declares no HTTP bindings for them).
async fn list(client: &Client, body: Value) -> Answer {
    client.send("POST", "/notifications", Some(body)).await
}

/// The `NotificationSummaries` a successful `ListNotifications` answered with.
fn summaries(answer: &Answer, case: &str) -> Vec<Value> {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected ListNotifications 200, got {answer:?}"
    );
    answer
        .body
        .get("NotificationSummaries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: a successful list must carry NotificationSummaries, got {answer:?}")
        })
}

/// One member of a JSON object as a string, empty when the object carries none.
fn field(value: &Value, name: &str) -> String {
    value
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The `LensUpgradeSummary` of one notification, with its `Type` checked on the way.
fn upgrade_summary(summary: &Value, case: &str) -> Value {
    assert_eq!(
        field(summary, "Type"),
        LENS_VERSION_UPGRADED,
        "case {case:?}: a lens review behind its lens is LENS_VERSION_UPGRADED, got {summary:?}"
    );
    let members: Vec<String> = summary
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: a summary is an object, got {summary:?}"))
        .keys()
        .cloned()
        .collect();
    let mut expected: Vec<String> = SUMMARY_MEMBERS.iter().map(ToString::to_string).collect();
    expected.sort();
    let mut carried = members;
    carried.sort();
    assert_eq!(
        carried, expected,
        "case {case:?}: a summary carries exactly {expected:?}, got {summary:?}"
    );
    summary
        .get("LensUpgradeSummary")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: LensUpgradeSummary must be present, got {summary:?}"))
}

/// What names one notification: the key of the resource holding the review — the workload's
/// `WorkloadId` or the template's `ResourceArn` — and the `LensAlias` of the review.
type Review = (String, String);

/// The [`Review`] one notification is about, read off its `LensUpgradeSummary`.
fn reviewed(summary: &Value, case: &str) -> Review {
    let upgrade = upgrade_summary(summary, case);
    let workload_id = field(&upgrade, "WorkloadId");
    let resource_arn = field(&upgrade, "ResourceArn");
    let key = if workload_id.is_empty() {
        resource_arn
    } else {
        workload_id
    };
    (key, field(&upgrade, "LensAlias"))
}

/// Import a fresh custom lens carrying `document`, wait for the import to land and publish
/// it as `version`. Answers the lens ARN.
async fn custom_lens_at(client: &Client, case: &str, document: &str, version: &str) -> String {
    let imported = import(client, document, None, &format!("import-{case}"), None).await;
    let arn = imported_arn(&imported, case);
    wait_for_the_import_to_land();
    let published = create_version(client, &arn, version, Some(true), &format!("pub-{case}")).await;
    assert_eq!(
        published.status, 200,
        "case {case:?}: expected CreateLensVersion 200, got {published:?}"
    );
    arn
}

/// Replace the lens's draft with fresh content and publish it as `version`: the publish that
/// supersedes whatever version a review of this lens is against.
async fn publish_later_version(client: &Client, arn: &str, case: &str, label: &str, version: &str) {
    let document = lens_document(
        &format!("Lens {case} as of {version}"),
        Some("a later revision of the lens"),
        &[("governance", 2, 2)],
    )
    .to_string();
    let update = import(
        client,
        &document,
        Some(arn),
        &format!("reimport-{case}-{label}"),
        None,
    )
    .await;
    assert_eq!(
        update.status, 200,
        "case {case:?}: the update import must be accepted, got {update:?}"
    );
    wait_for_the_import_to_land();
    let published = create_version(
        client,
        arn,
        version,
        Some(false),
        &format!("pub-{case}-{label}"),
    )
    .await;
    assert_eq!(
        published.status, 200,
        "case {case:?}: publishing {version} must succeed, got {published:?}"
    );
}

/// A workload created over HTTP reviewing `lenses`, and its id.
async fn workload(client: &Client, name: &str, lenses: &[&str]) -> String {
    let mut request = minimal_create(name, &format!("tok-{name}"));
    request["Lenses"] = json!(lenses);
    created_id(&client.create(request).await, name)
}

/// `PUT /workloads/{WorkloadId}/lensReviews/{LensAlias}/upgrade`, the wire form of
/// `UpgradeLensReview`: `MilestoneName` is `@required`, so every upgrade records one.
async fn upgrade_workload_review(client: &Client, workload_id: &str, lens: &str, case: &str) {
    let answer = client
        .send(
            "PUT",
            &format!(
                "/workloads/{}/lensReviews/{}/upgrade",
                encode_path_segment(workload_id),
                encode_path_segment(lens)
            ),
            Some(json!({ "MilestoneName": format!("upgraded for {case}") })),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected UpgradeLensReview 200, got {answer:?}"
    );
}

/// `PUT /reviewTemplates/{TemplateArn}/lensReviews/{LensAlias}/upgrade`, the wire form of
/// `UpgradeReviewTemplateLensReview`.
async fn upgrade_template_review(client: &Client, template_arn: &str, lens: &str, case: &str) {
    let answer = client
        .send(
            "PUT",
            &format!(
                "/reviewTemplates/{}/lensReviews/{}/upgrade",
                encode_path_segment(template_arn),
                encode_path_segment(lens)
            ),
            Some(json!({ "ClientRequestToken": format!("tok-upgrade-{case}") })),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected UpgradeReviewTemplateLensReview 200, got {answer:?}"
    );
}

/// Every notification the request selects, read by following `NextToken` from the first page
/// to the last with pages of `page_size` (or in one request when `page_size` is `None`), with
/// `selector` (the body members narrowing the report, empty for none) carried by each
/// request.
async fn walk(client: &Client, case: &str, page_size: Option<i64>, selector: &Value) -> Vec<Value> {
    let mut walked: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    // One more request than there can be pages, so a handler that never stops paging fails
    // here instead of hanging.
    for request in 0..64 {
        let mut body = selector.clone();
        if let Some(size) = page_size {
            body["MaxResults"] = json!(size);
        }
        if let Some(cursor) = &token {
            body["NextToken"] = json!(cursor);
        }
        let answer = list(client, body).await;
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

/// Rule 1. The rows differ along how many versions were published after the lens was applied
/// and the form of their names — one later version, two, a dated one — plus the row that
/// publishes NONE, where every review is against its lens's latest version and nothing is
/// reported. Each row covers a workload's review and a review template's review of the same
/// lens, so both kinds of resource are asserted against the same rule.
#[tokio::test]
async fn a_lens_review_behind_its_lens_raises_one_notification() {
    let rows: [(&str, &[&str]); 4] = [
        ("one-later-version", &["2.0"]),
        ("two-later-versions", &["2.0", "3.0"]),
        ("dated-later-version", &["2024-01-01"]),
        ("no-later-version", &[]),
    ];

    for (case, later) in rows {
        let client = Client::new();
        let applied_version = "1.0";
        let document = lens_document(
            &format!("Lens applied for {case}"),
            None,
            &[("governance", 1, 2)],
        )
        .to_string();
        let lens_arn = custom_lens_at(&client, case, &document, applied_version).await;

        // Both resources review the lens as it stands, so each review is pinned at the
        // version the lens's content is published as right now.
        let workload_id = workload(&client, case, &[FRAMEWORK_LENS, lens_arn.as_str()]).await;
        let template_name = format!("template-{case}");
        let template_arn = review_template(&client, &template_name, &[lens_arn.as_str()]).await;

        for (index, version) in later.iter().enumerate() {
            publish_later_version(&client, &lens_arn, case, &index.to_string(), version).await;
        }

        let reported = summaries(&list(&client, json!({})).await, case);
        let expected = if later.is_empty() { 0 } else { 2 };
        assert_eq!(
            reported.len(),
            expected,
            "case {case:?}: expected {expected} notification(s) — one per superseded lens \
             review — got {reported:?}"
        );
        if later.is_empty() {
            continue;
        }

        let latest = *later.last().expect("the row published a later version");
        // The report covers one workload review and one template review of the same lens, so
        // each notification is found by the resource it names.
        let by_key: Vec<(Review, Value)> = reported
            .iter()
            .map(|summary| (reviewed(summary, case), upgrade_summary(summary, case)))
            .collect();

        for (kind, key, name_member, name) in [
            (
                "workload",
                workload_id.clone(),
                "WorkloadName",
                case.to_string(),
            ),
            (
                "review-template",
                template_arn.clone(),
                "ResourceName",
                template_name.clone(),
            ),
        ] {
            let (_, upgrade) = by_key
                .iter()
                .find(|((resource, alias), _)| *resource == key && *alias == lens_arn)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: the {kind} review must be reported, got {by_key:?}")
                });
            for (member, want) in [
                ("LensAlias", lens_arn.as_str()),
                // A custom lens is named by its ARN, which is the only `LensAlias` it has.
                ("LensArn", lens_arn.as_str()),
                ("CurrentLensVersion", applied_version),
                ("LatestLensVersion", latest),
                (name_member, name.as_str()),
            ] {
                assert_eq!(
                    field(upgrade, member),
                    want,
                    "case {case:?}: expected the {kind} summary's {member} to be {want:?}, \
                     got {upgrade:?}"
                );
            }
            // The resource members of the OTHER kind are left off rather than filled with a
            // value of the wrong kind.
            let mut expected_members: Vec<&str> = SHARED_UPGRADE_MEMBERS.to_vec();
            expected_members.extend(if kind == "workload" {
                WORKLOAD_MEMBERS
            } else {
                TEMPLATE_MEMBERS
            });
            expected_members.sort_unstable();
            let mut carried: Vec<&str> = upgrade
                .as_object()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: a LensUpgradeSummary is an object, got {upgrade:?}")
                })
                .keys()
                .map(String::as_str)
                .collect();
            carried.sort_unstable();
            assert_eq!(
                carried, expected_members,
                "case {case:?}: the {kind} summary must carry exactly {expected_members:?}, \
                 got {upgrade:?}"
            );
        }
    }
}

/// Rules 1 and 2. One store holding resources that differ along what they have to report —
/// two workloads behind different lenses, a workload whose only lens review is current, a
/// review template behind one of those lenses and a template whose review is current — read
/// through every selector a request can name, including the pair the model refuses together.
#[tokio::test]
async fn the_selectors_narrow_the_report_to_one_resource() {
    let client = Client::new();
    let first_lens = custom_lens_at(
        &client,
        "first",
        &lens_document("First lens", None, &[("governance", 1, 2)]).to_string(),
        "1.0",
    )
    .await;
    let second_lens = custom_lens_at(
        &client,
        "second",
        &lens_document("Second lens", None, &[("security", 1, 2)]).to_string(),
        "1.0",
    )
    .await;

    let behind_first = workload(&client, "behind-first", &[first_lens.as_str()]).await;
    let behind_second = workload(&client, "behind-second", &[second_lens.as_str()]).await;
    let current_workload = workload(&client, "current-workload", &[FRAMEWORK_LENS]).await;
    let behind_template = review_template(&client, "behind-template", &[first_lens.as_str()]).await;
    let current_template = review_template(&client, "current-template", &[FRAMEWORK_LENS]).await;

    publish_later_version(&client, &first_lens, "first", "later", "2.0").await;
    publish_later_version(&client, &second_lens, "second", "later", "2.0").await;

    let mut everything: Vec<Review> = vec![
        (behind_first.clone(), first_lens.clone()),
        (behind_second.clone(), second_lens.clone()),
        (behind_template.clone(), first_lens.clone()),
    ];
    everything.sort();

    let rows: Vec<(&str, Value, Vec<Review>)> = vec![
        ("no-selector", json!({}), everything),
        (
            "the-first-workload-behind",
            json!({ "WorkloadId": behind_first }),
            vec![(behind_first.clone(), first_lens.clone())],
        ),
        (
            "the-second-workload-behind",
            json!({ "WorkloadId": behind_second }),
            vec![(behind_second.clone(), second_lens)],
        ),
        (
            "a-workload-whose-reviews-are-current",
            json!({ "WorkloadId": current_workload }),
            Vec::new(),
        ),
        (
            "a-workload-that-is-not-stored",
            json!({ "WorkloadId": ABSENT_WORKLOAD_ID }),
            Vec::new(),
        ),
        (
            "the-template-behind",
            json!({ "ResourceArn": behind_template }),
            vec![(behind_template.clone(), first_lens)],
        ),
        (
            "a-template-whose-reviews-are-current",
            json!({ "ResourceArn": current_template }),
            Vec::new(),
        ),
        (
            "a-template-that-is-not-stored",
            json!({ "ResourceArn": ABSENT_TEMPLATE_ARN }),
            Vec::new(),
        ),
    ];

    for (case, body, expected) in rows {
        let answer = list(&client, body).await;
        let reported: Vec<Review> = summaries(&answer, case)
            .iter()
            .map(|summary| reviewed(summary, case))
            .collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the reviews {expected:?}, got {answer:?}"
        );
    }

    // "Only one of WorkloadID or ResourceARN should be specified": a request setting both is
    // refused, and the finding names both members a caller could drop.
    let both = list(
        &client,
        json!({ "WorkloadId": behind_first, "ResourceArn": behind_template }),
    )
    .await;
    assert_eq!(
        (both.status, both.error_code.as_str()),
        (400, "ValidationException"),
        "case \"both-selectors\": the two selectors are mutually exclusive, got {both:?}"
    );
    assert_eq!(
        both.body
            .get("Fields")
            .and_then(Value::as_array)
            .map(|fields| fields
                .iter()
                .map(|entry| field(entry, "Name"))
                .collect::<Vec<String>>()),
        Some(vec!["WorkloadId".to_string(), "ResourceArn".to_string()]),
        "case \"both-selectors\": the finding must name both selectors, got {both:?}"
    );
}

/// Rule 3. The rows are every page size from one notification per page up to past the stored
/// count, plus the published `@range(1, 50)` maximum and a request that omits `MaxResults` —
/// however the selection is cut, the pages enumerate the same notifications in the same
/// order, exactly once each.
#[tokio::test]
async fn paging_enumerates_every_notification_exactly_once() {
    let client = Client::new();
    let lens_arn = custom_lens_at(
        &client,
        "paged",
        &lens_document("Paged lens", None, &[("governance", 1, 2)]).to_string(),
        "1.0",
    )
    .await;

    let mut expected: Vec<Review> = Vec::new();
    for index in 0..5 {
        let workload_id = workload(
            &client,
            &format!("paged-workload-{index}"),
            &[FRAMEWORK_LENS, lens_arn.as_str()],
        )
        .await;
        expected.push((workload_id, lens_arn.clone()));
    }
    for index in 0..2 {
        let template_arn = review_template(
            &client,
            &format!("paged-template-{index}"),
            &[lens_arn.as_str()],
        )
        .await;
        expected.push((template_arn, lens_arn.clone()));
    }
    publish_later_version(&client, &lens_arn, "paged", "later", "2.0").await;
    expected.sort();
    let count = expected.len();

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
        let walked: Vec<Review> = walk(&client, case, page_size, &json!({}))
            .await
            .iter()
            .map(|summary| reviewed(summary, case))
            .collect();
        assert_eq!(
            walked, expected,
            "case {case:?}: paging must enumerate every one of the {count} notifications \
             exactly once in (resource key, LensAlias) order"
        );
    }

    // A page that holds the rest of the selection carries no `NextToken`; one that does not,
    // does.
    for (case, max_results, expects_token) in [
        ("short-page", 3, true),
        ("page-of-the-whole-selection", 7, false),
        ("page-larger-than-the-selection", 50, false),
    ] {
        let answer = list(&client, json!({ "MaxResults": max_results })).await;
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
        let answer = list(&client, json!({ "MaxResults": max_results })).await;
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
    let lens_arn = custom_lens_at(
        &client,
        "token",
        &lens_document("Token lens", None, &[("governance", 1, 2)]).to_string(),
        "1.0",
    )
    .await;
    for index in 0..3 {
        workload(
            &client,
            &format!("token-workload-{index}"),
            &[lens_arn.as_str()],
        )
        .await;
    }
    publish_later_version(&client, &lens_arn, "token", "later", "2.0").await;

    let issued = list(&client, json!({ "MaxResults": 1 }))
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
        // hex of {"nr":"a"} — half of this operation's own cursor, which names a review by
        // BOTH the resource holding it and the lens.
        ("half-a-cursor", "7b226e72223a2261227d".to_string()),
        // hex of {"r":"a"} — the cursor `ListLensReviews` issues over the same lens reviews.
        ("a-lens-reviews-cursor", "7b2272223a2261227d".to_string()),
        // hex of {"w":"a","f":"b"} — the cursor `ListProfileNotifications` issues, the other
        // notification enumeration of this service.
        (
            "a-profile-notifications-cursor",
            "7b2277223a2261222c2266223a2262227d".to_string(),
        ),
    ];

    for (case, token) in rows {
        let answer = list(&client, json!({ "NextToken": token })).await;
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
                    .map(|entry| field(entry, "Name"))
                    .collect::<Vec<String>>()),
            Some(vec!["NextToken".to_string()]),
            "case {case:?}: the finding must name NextToken, got {answer:?}"
        );
    }

    // The token this service DID issue still pages, so the refusals above are about the token
    // and not about the operation.
    let resumed = list(&client, json!({ "NextToken": issued })).await;
    assert_eq!(
        resumed.status, 200,
        "case \"issued-token\": a token this service issued must page, got {resumed:?}"
    );
}

/// Rule 5. The rows are the versions a lens can be ahead by: for each, the notifications its
/// stale reviews raise survive repeated reads unchanged and are GONE once the reviews have
/// been upgraded to the version the lens is at — the read reports state, so nothing but a
/// write to the review clears it.
#[tokio::test]
async fn upgrading_the_review_clears_its_notification() {
    for (case, later) in [
        ("one-version-ahead", vec!["2.0"]),
        ("three-versions-ahead", vec!["2.0", "2.1", "3.0"]),
    ] {
        let client = Client::new();
        let lens_arn = custom_lens_at(
            &client,
            case,
            &lens_document(&format!("Lens for {case}"), None, &[("governance", 1, 2)]).to_string(),
            "1.0",
        )
        .await;
        let workload_id = workload(&client, case, &[FRAMEWORK_LENS, lens_arn.as_str()]).await;
        let template_arn =
            review_template(&client, &format!("template-{case}"), &[lens_arn.as_str()]).await;
        for (index, version) in later.iter().enumerate() {
            publish_later_version(&client, &lens_arn, case, &index.to_string(), version).await;
        }

        // Reporting writes nothing: two identical reads report the same notifications.
        let first = summaries(&list(&client, json!({})).await, case);
        let second = summaries(&list(&client, json!({})).await, case);
        assert_eq!(
            first, second,
            "case {case:?}: two identical reads must report the same notifications"
        );
        assert_eq!(
            first.len(),
            2,
            "case {case:?}: both superseded reviews raise a notification, got {first:?}"
        );

        upgrade_workload_review(&client, &workload_id, &lens_arn, case).await;
        let after_workload: Vec<Review> = summaries(&list(&client, json!({})).await, case)
            .iter()
            .map(|summary| reviewed(summary, case))
            .collect();
        assert_eq!(
            after_workload,
            vec![(template_arn.clone(), lens_arn.clone())],
            "case {case:?}: upgrading the workload's review clears only its own notification"
        );

        upgrade_template_review(&client, &template_arn, &lens_arn, case).await;
        let cleared = summaries(&list(&client, json!({})).await, case);
        assert!(
            cleared.is_empty(),
            "case {case:?}: an upgraded review is a review of the lens as it stands and \
             reports nothing, got {cleared:?}"
        );
    }
}
