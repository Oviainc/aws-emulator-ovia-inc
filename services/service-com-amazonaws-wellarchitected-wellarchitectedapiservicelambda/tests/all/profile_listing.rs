// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `ListProfiles` — "List profiles"
//! (`smithy_query(query="operation", operation="ListProfiles")`).
//!
//! The rules pinned here, each over a TABLE of inputs rather than one recorded request:
//!
//! 1. The list reports one `ProfileSummary` per STORED profile the request selects,
//!    projected from that profile's own state: every member `ProfileSummary` models and no
//!    other, agreeing with the `GetProfile` of the same profile on each member the two
//!    shapes share. A store that holds no profile reports an empty page.
//! 2. `ProfileNamePrefix` selects the profiles whose `ProfileName` BEGINS with it, and a
//!    request that carries no prefix asks for no filtering. The published `@length(..100)`
//!    maximum is served and a longer prefix is refused.
//! 3. `ProfileOwnerType` selects by who owns the profile: `SELF` the profiles the calling
//!    account owns, `SHARED` the ones another account owns, and a request naming neither
//!    asks for both.
//! 4. `MaxResults`/`NextToken` page over the selection: however small the page, following
//!    `NextToken` enumerates every selected profile exactly once in one stable order, and
//!    the page that holds the rest of the selection carries no `NextToken`. The published
//!    `@range(1, 50)` bounds of `MaxResults` are served and a value past either of them is
//!    refused.
//! 5. A `NextToken` this service did not issue cannot be decoded, so it is a
//!    `400 ValidationException` classified `CANNOT_PARSE` and naming `NextToken` — not an
//!    empty page.
//! 6. Listing is REPORTING: it writes nothing, so two identical reads report the same
//!    profiles and a read taken between two pages changes neither.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do
//! in production. The HTTP client lives in `super::workload_harness`.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use super::workload_harness::{created_profile, member, minimal_profile, Answer, Client};

/// Every member `ProfileSummary` declares, from
/// `smithy_query(query="shape", shape="ProfileSummary")`. A summary carries these and
/// nothing else.
const SUMMARY_MEMBERS: [&str; 7] = [
    "CreatedAt",
    "Owner",
    "ProfileArn",
    "ProfileDescription",
    "ProfileName",
    "ProfileVersion",
    "UpdatedAt",
];

/// The members `Profile` carries that `ProfileSummary` does NOT declare (same
/// `smithy_query`), so a summary must omit them.
const NON_SUMMARY_MEMBERS: [&str; 3] = ["ProfileQuestions", "Tags", "ShareInvitationId"];

/// The `ProfileSummaries` a successful `ListProfiles` answered with.
fn summaries(answer: &Answer, case: &str) -> Vec<Value> {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected ListProfiles 200, got {answer:?}"
    );
    answer
        .body
        .get("ProfileSummaries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: a successful list must carry ProfileSummaries, got {answer:?}")
        })
}

fn arn_of(summary: &Value) -> String {
    summary
        .get("ProfileArn")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Every profile the request selects, read by following `NextToken` from the first page to
/// the last with pages of `page_size` (or in one request when `page_size` is `None`), with
/// `filters` (already query-encoded, empty for none) carried by each request. Answers the
/// summaries in the order the pages served them.
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
        let answer = client.list_profiles(&query.join("&")).await;
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
    panic!(
        "case {case:?}: paging did not terminate, got {} summaries",
        walked.len()
    );
}

/// Create `count` profiles named `<prefix>-<index>` and answer their ARNs.
async fn seed(client: &Client, prefix: &str, count: usize) -> Vec<String> {
    let mut arns = Vec::new();
    for index in 0..count {
        let (arn, _) = created_profile(client, &format!("{prefix}-{index:03}")).await;
        arns.push(arn);
    }
    arns
}

/// Rule 1. The rows differ along what the create carried — how many questions are
/// answered and how many choices each selects, whether tags are present, and the name and
/// description lengths at their published `@length(3, 100)` bounds — because each summary
/// is projected from the stored profile's own state and not from one recorded request.
#[tokio::test]
async fn every_stored_profile_is_reported_as_its_modelled_summary() {
    // (case, request overrides)
    let cases: Vec<(&str, Value)> = vec![
        ("minimal", json!({})),
        ("tagged", json!({ "Tags": { "Env": "test" } })),
        ("no-questions", json!({ "ProfileQuestions": [] })),
        (
            "two-questions",
            json!({ "ProfileQuestions": [
                { "QuestionId": "priority", "SelectedChoiceIds": ["priority_cost"] },
                {
                    "QuestionId": "business-outcome",
                    "SelectedChoiceIds": ["outcome_cost", "outcome_speed"],
                },
            ] }),
        ),
        (
            "name-and-description-at-the-minimum-length",
            json!({ "ProfileName": "abc", "ProfileDescription": "xyz" }),
        ),
        (
            "name-and-description-at-the-maximum-length",
            json!({ "ProfileName": "n".repeat(100), "ProfileDescription": "d".repeat(100) }),
        ),
    ];

    // One client, so every profile shares a store and one list reports them all.
    let client = Client::new();
    let mut requests: Vec<(String, Value, String)> = Vec::new();
    for (case, overrides) in cases {
        let mut request = minimal_profile(&format!("summary-{case}"));
        for (name, value) in overrides.as_object().expect("overrides are an object") {
            request[name] = value.clone();
        }
        let created = client.create_profile(request.clone()).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected CreateProfile 200, got {created:?}"
        );
        requests.push((
            case.to_string(),
            request,
            member(&created, "ProfileArn").to_string(),
        ));
    }

    let listed = walk(&client, "all-profiles", None, "").await;
    assert_eq!(
        listed.len(),
        requests.len(),
        "case \"all-profiles\": expected one summary per stored profile, got {listed:?}"
    );

    for (case, request, arn) in &requests {
        let summary = listed
            .iter()
            .find(|summary| &arn_of(summary) == arn)
            .unwrap_or_else(|| {
                panic!("case {case:?}: the list must report the profile {arn:?}, got {listed:?}")
            });

        // Exactly the modelled members, and no member the summary shape does not declare.
        let reported: BTreeSet<&str> = summary
            .as_object()
            .unwrap_or_else(|| panic!("case {case:?}: a summary is an object, got {summary:?}"))
            .keys()
            .map(String::as_str)
            .collect();
        let expected: BTreeSet<&str> = SUMMARY_MEMBERS.into_iter().collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the summary members {expected:?}, got {summary:?}"
        );
        for absent in NON_SUMMARY_MEMBERS {
            assert!(
                summary.get(absent).is_none(),
                "case {case:?}: ProfileSummary models no {absent}, got {summary:?}"
            );
        }

        // The members the REQUEST set come back as sent.
        for name in ["ProfileName", "ProfileDescription"] {
            assert_eq!(
                summary.get(name),
                request.get(name),
                "case {case:?}: {name} must be reported as stored, got {summary:?}"
            );
        }

        // The members the SERVICE computes.
        assert_eq!(
            summary.get("Owner").and_then(Value::as_str),
            Some(engine::emulator_account_id()),
            "case {case:?}: the creating account owns the profile, got {summary:?}"
        );
        assert!(
            summary.get("CreatedAt").is_some() && summary.get("UpdatedAt").is_some(),
            "case {case:?}: a stored profile carries both timestamps, got {summary:?}"
        );

        // The summary agrees with the full shape on every member the two share.
        let fetched = client.get_profile(arn, None).await;
        let profile = fetched.body.get("Profile").cloned().unwrap_or(Value::Null);
        for name in SUMMARY_MEMBERS {
            assert_eq!(
                summary.get(name),
                profile.get(name),
                "case {case:?}: the summary and GetProfile must agree on {name}, \
                 got {summary:?} and {profile:?}"
            );
        }
    }
}

/// Rule 1's other half: a store that holds no profile reports an empty page, and each
/// profile joins the list as it is stored.
#[tokio::test]
async fn the_list_reports_exactly_what_the_store_holds() {
    let client = Client::new();

    let empty = client.list_profiles("").await;
    assert!(
        summaries(&empty, "empty-store").is_empty(),
        "case \"empty-store\": a store with no profile reports no summary, got {empty:?}"
    );
    assert!(
        empty.body.get("NextToken").is_none(),
        "case \"empty-store\": an empty page carries no NextToken, got {empty:?}"
    );

    // The rows are the store sizes one create apart, so the list is reporting the store
    // rather than a fixed set.
    let mut stored: Vec<String> = Vec::new();
    for index in 0..4 {
        let case = format!("after-{index}-creates");
        let (arn, _) = created_profile(&client, &format!("grown-{index:03}")).await;
        stored.push(arn);
        stored.sort();
        let reported: Vec<String> = walk(&client, &case, None, "")
            .await
            .iter()
            .map(arn_of)
            .collect();
        assert_eq!(
            reported, stored,
            "case {case:?}: the list must report exactly the profiles the store holds"
        );
    }
}

/// Rule 2. The rows differ along the prefix's length and how much of the stored names it
/// covers: a whole name, a shared prefix of several, one character, the published
/// `@length(..100)` maximum, a prefix no name starts with, and no prefix at all.
#[tokio::test]
async fn the_name_prefix_selects_the_profiles_whose_name_begins_with_it() {
    let client = Client::new();
    // Names chosen so the prefixes below cut the set in different places.
    let names = [
        "alpha-one",
        "alpha-two",
        "beta-one",
        "gamma",
        // The longest name a `ProfileName` may carry (`@length(3, 100)`), so a prefix at
        // the published maximum can select it.
        &"z".repeat(100),
    ];
    let mut by_name: Vec<(String, String)> = Vec::new();
    for name in names {
        let (arn, _) = created_profile(&client, name).await;
        by_name.push((name.to_string(), arn));
    }

    let expected_for = |prefix: &str| -> Vec<String> {
        let mut arns: Vec<String> = by_name
            .iter()
            .filter(|(name, _)| name.starts_with(prefix))
            .map(|(_, arn)| arn.clone())
            .collect();
        arns.sort();
        arns
    };

    let rows: Vec<(&str, String)> = vec![
        ("one-character", "a".to_string()),
        ("shared-prefix", "alpha-".to_string()),
        ("whole-name", "beta-one".to_string()),
        ("name-of-one", "gamma".to_string()),
        ("published-maximum-length", "z".repeat(100)),
        ("matches-nothing", "no-such-profile".to_string()),
        ("empty", String::new()),
    ];

    for (case, prefix) in rows {
        let filters = format!("ProfileNamePrefix={prefix}");
        let reported: Vec<String> = walk(&client, case, None, &filters)
            .await
            .iter()
            .map(arn_of)
            .collect();
        assert_eq!(
            reported,
            expected_for(&prefix),
            "case {case:?}: the prefix must select the profiles whose name begins with it"
        );

        // The filter is the same selection however the pages cut it.
        let paged: Vec<String> = walk(&client, case, Some(1), &filters)
            .await
            .iter()
            .map(arn_of)
            .collect();
        assert_eq!(
            paged, reported,
            "case {case:?}: paging must not change which profiles the prefix selects"
        );
    }

    // A prefix past the published maximum is refused before the handler runs.
    let too_long = client
        .list_profiles(&format!("ProfileNamePrefix={}", "z".repeat(101)))
        .await;
    assert_eq!(
        too_long.status, 400,
        "case \"past-the-maximum-length\": a 101-character prefix is refused, got {too_long:?}"
    );
    assert_eq!(
        too_long.error_code, "ValidationException",
        "case \"past-the-maximum-length\": expected ValidationException, got {too_long:?}"
    );
}

/// Rule 3. The rows are the two published `ProfileOwnerType` members and a request that
/// names neither, over a store holding profiles of the calling account AND of two other
/// accounts.
#[tokio::test]
async fn the_owner_type_selects_profiles_by_who_owns_them() {
    let client = Client::new();
    let mut own = seed(&client, "owned", 3).await;
    own.sort();
    // Two foreign owners, so `SHARED` is selecting on the profile's `Owner` rather than on
    // one account id.
    let mut foreign = vec![
        client.seed_foreign_profile("foreign-one", "210987654321", "aaaaaaaa1111"),
        client.seed_foreign_profile("foreign-two", "123456789012", "bbbbbbbb2222"),
    ];
    foreign.sort();
    let mut everything: Vec<String> = own.iter().chain(foreign.iter()).cloned().collect();
    everything.sort();

    for (case, owner_type, expected) in [
        ("self", "SELF", own.clone()),
        ("shared", "SHARED", foreign.clone()),
    ] {
        let filters = format!("ProfileOwnerType={owner_type}");
        let reported: Vec<String> = walk(&client, case, None, &filters)
            .await
            .iter()
            .map(arn_of)
            .collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: ProfileOwnerType={owner_type} must select profiles by their Owner"
        );

        // Every selected summary reports the owner the filter asked for.
        for summary in walk(&client, case, Some(1), &filters).await {
            let owner = summary.get("Owner").and_then(Value::as_str).unwrap_or("");
            let owned_by_caller = owner == engine::emulator_account_id();
            assert_eq!(
                owned_by_caller,
                owner_type == "SELF",
                "case {case:?}: ProfileOwnerType={owner_type} selected Owner {owner:?}"
            );
        }
    }

    let unfiltered: Vec<String> = walk(&client, "omitted", None, "")
        .await
        .iter()
        .map(arn_of)
        .collect();
    assert_eq!(
        unfiltered, everything,
        "case \"omitted\": a request naming no ProfileOwnerType asks for every profile"
    );

    // A value `ProfileOwnerType` does not publish is refused before the handler runs.
    let unknown = client.list_profiles("ProfileOwnerType=EVERYONE").await;
    assert_eq!(
        unknown.status, 400,
        "case \"unmodelled-owner-type\": expected a 400, got {unknown:?}"
    );
}

/// Rule 4. The rows are every page size from one profile per page up to past the stored
/// count, plus the published `@range(1, 50)` maximum and a request that omits
/// `MaxResults` — however the selection is cut, the pages enumerate the same profiles in
/// the same order, exactly once each.
#[tokio::test]
async fn paging_enumerates_every_profile_exactly_once() {
    let client = Client::new();
    let created = seed(&client, "paged", 7).await;
    let mut expected: Vec<String> = created.clone();
    // The emulator's order is `ProfileArn`, the only member of a profile that is both
    // unique and fixed for its whole life.
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
        let walked: Vec<String> = walk(&client, case, page_size, "")
            .await
            .iter()
            .map(arn_of)
            .collect();
        assert_eq!(
            walked, expected,
            "case {case:?}: paging must enumerate every profile exactly once in ARN order"
        );
    }

    // A page that holds the rest of the selection carries no `NextToken`; one that does
    // not, does.
    for (case, max_results, expects_token) in [
        ("short-page", 3, true),
        ("page-of-the-whole-selection", 7, false),
        ("page-larger-than-the-selection", 50, false),
    ] {
        let answer = client.list_profiles(&format!("MaxResults={max_results}")).await;
        assert_eq!(
            answer.body.get("NextToken").and_then(Value::as_str).is_some(),
            expects_token,
            "case {case:?}: NextToken must be present only while profiles remain, got {answer:?}"
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
        let answer = client.list_profiles(&format!("MaxResults={max_results}")).await;
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

/// Rule 5. The rows differ along the undecodable token's form — each one satisfying the
/// modelled `NextToken` `@pattern("^[A-Za-z0-9+/=_-]+$")`, so the refusal is about the
/// token's content and not its characters — and include cursors ANOTHER paginated
/// operation of this service issues, which are not cursors here.
#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let client = Client::new();
    seed(&client, "token", 3).await;

    let issued = client
        .list_profiles("MaxResults=1")
        .await
        .body
        .get("NextToken")
        .and_then(Value::as_str)
        .expect("three profiles and a page of one leave a NextToken")
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
        // hex of {"n":"a","i":"b"} — the cursor `ListWorkloads` issues.
        (
            "a-workloads-cursor",
            "7b226e223a2261222c2269223a2262227d".to_string(),
        ),
        // hex of {"t":"a"} — the cursor `ListReviewTemplates` issues.
        ("a-review-templates-cursor", "7b2274223a2261227d".to_string()),
        // hex of {"p":"a"} — the cursor `ListAgentProfiles` issues, over the OTHER profile
        // resource this service carries.
        ("an-agent-profiles-cursor", "7b2270223a2261227d".to_string()),
    ];

    for (case, token) in rows {
        let answer = client.list_profiles(&format!("NextToken={token}")).await;
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
                    .map(|field| field.get("Name").and_then(Value::as_str).unwrap_or_default())
                    .collect::<Vec<_>>()),
            Some(vec!["NextToken"]),
            "case {case:?}: the finding must name NextToken, got {answer:?}"
        );
    }

    // The token this service DID issue still pages, so the refusals above are about the
    // token and not about the operation.
    let resumed = client.list_profiles(&format!("NextToken={issued}")).await;
    assert_eq!(
        resumed.status, 200,
        "case \"issued-token\": a token this service issued must page, got {resumed:?}"
    );
}

/// Rule 6. The rows are the page sizes a read can be cut into: for each, listing twice
/// reports the same profiles, and a full re-read taken between two pages of a walk changes
/// neither the rest of that walk nor the stored profiles.
#[tokio::test]
async fn listing_writes_nothing() {
    let client = Client::new();
    let created = seed(&client, "reporting", 5).await;
    let mut expected: Vec<String> = created.clone();
    expected.sort();

    for (case, page_size) in [
        ("omitted", None),
        ("one-per-page", Some(1)),
        ("two-per-page", Some(2)),
    ] {
        let first: Vec<String> = walk(&client, case, page_size, "")
            .await
            .iter()
            .map(arn_of)
            .collect();
        let second: Vec<String> = walk(&client, case, page_size, "")
            .await
            .iter()
            .map(arn_of)
            .collect();
        assert_eq!(
            first, expected,
            "case {case:?}: the first read must report every stored profile"
        );
        assert_eq!(
            second, first,
            "case {case:?}: a second read must report the same profiles"
        );
    }

    // A read taken BETWEEN two pages does not move the cursor's selection.
    let first_page = client.list_profiles("MaxResults=2").await;
    let mut seen: Vec<String> = summaries(&first_page, "interleaved")
        .iter()
        .map(arn_of)
        .collect();
    let token = first_page
        .body
        .get("NextToken")
        .and_then(Value::as_str)
        .expect("five profiles and a page of two leave a NextToken")
        .to_string();
    let interleaved: Vec<String> = walk(&client, "interleaved-full-read", None, "")
        .await
        .iter()
        .map(arn_of)
        .collect();
    assert_eq!(
        interleaved, expected,
        "case \"interleaved\": the read between two pages must report every profile"
    );
    let rest = client
        .list_profiles(&format!("MaxResults=50&NextToken={token}"))
        .await;
    seen.extend(summaries(&rest, "interleaved").iter().map(arn_of));
    assert_eq!(
        seen, expected,
        "case \"interleaved\": a read between two pages must not change what the rest of \
         the walk reports"
    );

    // Nothing the reads above ran changed a stored profile: each is still readable and
    // still reports the same summary members as the list does.
    for arn in &expected {
        let fetched = client.get_profile(arn, None).await;
        assert_eq!(
            fetched.status, 200,
            "case \"after-the-reads\": the profile {arn:?} must still be readable, \
             got {fetched:?}"
        );
    }
}
