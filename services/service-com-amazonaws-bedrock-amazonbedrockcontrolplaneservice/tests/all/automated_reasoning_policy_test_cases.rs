// SPDX-License-Identifier: CC0-1.0

//! The Automated Reasoning policy TEST resource end-to-end:
//! `CreateAutomatedReasoningPolicyTestCase` writes it and
//! `GetAutomatedReasoningPolicyTestCase` reads it back, over the same `MultiServiceRouter`
//! the emulator boots.
//!
//! The rules under test, each asserted over a SET of inputs rather than over one recorded
//! request:
//!
//! 1. **A created test round-trips**: every member the read reports comes back out of the
//!    stored record, character for character, for requests differing in the script and
//!    length of `guardContent`, in whether `queryContent` and `confidenceThreshold` are
//!    present, in the `AutomatedReasoningCheckResult` expected, and in the threshold's
//!    value at each published bound. The minted `testCaseId` is one
//!    `AutomatedReasoningPolicyTestCaseId` admits, and two creates mint two tests.
//! 2. **A replayed `clientRequestToken` mints nothing new**, and two distinct tokens mint
//!    two tests.
//! 3. **A test belongs to the policy it was created under**, so a policy ARN this account
//!    holds no policy under is 404 on both operations, and a `testCaseId` minted under
//!    another policy names no test of this one.
//! 4. **A listing reports every test of its policy and none of another's**, each item the
//!    SAME object the read of that one test reports, and its pages partition the listing:
//!    the rows span policies holding none, one and several tests, and page sizes of one,
//!    two and the whole listing.
//!
//! Every expectation is derived from the model, cited per row
//! (`smithy_query(query="operation",operation="CreateAutomatedReasoningPolicyTestCase,GetAutomatedReasoningPolicyTestCase,ListAutomatedReasoningPolicyTestCases")`
//! / `query="shape",shape="AutomatedReasoningPolicyTestCase,AutomatedReasoningCheckResult"`).
//! The requests go through `crate::support` — the ONE harness every suite in this crate
//! drives the emulator with.

use serde_json::{json, Value};

use crate::support::{date_time_seconds, emulator, label, Emulator, Reply, ACCOUNT, REGION};

/// How many characters `AutomatedReasoningPolicyTestCaseId` carries (`@length(max: 12)`,
/// pattern `^[0-9A-Z]{12}$`).
const TEST_CASE_ID_DIGITS: usize = 12;

/// The most characters `guardContent` admits (`@length(max: 2048)`).
const MAX_GUARD_CONTENT: usize = 2048;

/// The most characters `queryContent` admits (`@length(max: 1024)`).
const MAX_QUERY_CONTENT: usize = 1024;

/// Every value `AutomatedReasoningCheckResult` admits
/// (`smithy_query(query="shape",shape="AutomatedReasoningCheckResult")`), so the rule is
/// asserted over the whole enum rather than over the one value a recording carried.
const CHECK_RESULTS: &[&str] = &[
    "VALID",
    "INVALID",
    "SATISFIABLE",
    "IMPOSSIBLE",
    "TRANSLATION_AMBIGUOUS",
    "TOO_COMPLEX",
    "NO_TRANSLATION",
];

/// A policy created through `CreateAutomatedReasoningPolicy`, as its `policyArn`.
///
/// `pub(crate)` so the sibling suite that drives this operation's `@range`-constrained
/// member (`range_refusal_wording.rs`) reaches a policy the SAME way rather than carrying
/// a second copy of this request.
pub(crate) async fn create_policy(emu: &Emulator, name: &str) -> String {
    let reply = emu
        .call(
            "POST",
            "/automated-reasoning-policies",
            Some(json!({ "name": name })),
        )
        .await;
    assert_eq!(
        reply.status, 200,
        "case {name:?}: CreateAutomatedReasoningPolicy must answer 200, got {} {:?}",
        reply.status, reply.body
    );
    reply.text("policyArn")
}

async fn create_test_case(emu: &Emulator, policy_arn: &str, body: Value) -> Reply {
    emu.call(
        "POST",
        &format!(
            "/automated-reasoning-policies/{}/test-cases",
            label(policy_arn)
        ),
        Some(body),
    )
    .await
}

async fn list_test_cases(emu: &Emulator, policy_arn: &str, query: &str) -> Reply {
    let path = format!(
        "/automated-reasoning-policies/{}/test-cases",
        label(policy_arn)
    );
    let path = if query.is_empty() {
        path
    } else {
        format!("{path}?{query}")
    };
    emu.call("GET", &path, None).await
}

async fn get_test_case(emu: &Emulator, policy_arn: &str, test_case_id: &str) -> Reply {
    emu.call(
        "GET",
        &format!(
            "/automated-reasoning-policies/{}/test-cases/{}",
            label(policy_arn),
            label(test_case_id)
        ),
        None,
    )
    .await
}

/// Assert `id` is one `AutomatedReasoningPolicyTestCaseId` admits.
fn assert_id_shape(case: &str, id: &str) {
    assert!(
        id.len() == TEST_CASE_ID_DIGITS
            && id
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit()),
        "case {case:?}: testCaseId must be {TEST_CASE_ID_DIGITS} characters of [0-9A-Z], \
         got {id:?} ({} chars)",
        id.len()
    );
}

/// The member `name` of a `testCase` object, or `Value::Null` when it is absent.
fn member(test_case: &Value, name: &str) -> Value {
    test_case.get(name).cloned().unwrap_or(Value::Null)
}

// ---------------------------------------------------------------------------------------
// 1. What a create stores is what a read reports.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_created_test_round_trips_through_get() {
    // Rows differ along what the create carries: the script, length and composition of
    // `guardContent` (ASCII, multi-byte CJK, an emoji with a zero-width joiner, combining
    // marks, right-to-left text, the published maximum length), whether `queryContent` is
    // present and how long it is, the expected `AutomatedReasoningCheckResult`, and the
    // `confidenceThreshold` at each published bound of its `@range(0, 1)`.
    struct Row {
        case: &'static str,
        guard_content: String,
        query_content: Option<String>,
        expected: &'static str,
        confidence_threshold: Option<f64>,
    }

    let mut rows = vec![
        Row {
            case: "ascii only, nothing optional",
            guard_content: "All returns are accepted within 30 days.".to_string(),
            query_content: None,
            expected: "VALID",
            confidence_threshold: None,
        },
        Row {
            case: "multi-byte CJK",
            guard_content: "返品は30日以内に受け付けます。".to_string(),
            query_content: Some("返品について教えてください".to_string()),
            expected: "INVALID",
            confidence_threshold: Some(0.5),
        },
        Row {
            case: "emoji with a zero-width joiner",
            guard_content: "Refunds are approved \u{1f469}\u{200d}\u{1f4bb}\u{2705}".to_string(),
            query_content: Some("Is my refund approved? \u{1f914}".to_string()),
            expected: "SATISFIABLE",
            confidence_threshold: Some(0.0),
        },
        Row {
            case: "combining marks",
            guard_content: "e\u{301}le\u{300}ve n\u{303}o\u{308} cafe\u{301}".to_string(),
            query_content: Some("e\u{301}le\u{300}ve?".to_string()),
            expected: "TOO_COMPLEX",
            confidence_threshold: Some(1.0),
        },
        Row {
            case: "right-to-left text",
            guard_content: "الإرجاع مقبول خلال ٣٠ يوما".to_string(),
            query_content: Some("ما هي سياسة الإرجاع؟".to_string()),
            expected: "NO_TRANSLATION",
            confidence_threshold: Some(0.25),
        },
        Row {
            case: "guardContent at its published maximum length",
            guard_content: "g".repeat(MAX_GUARD_CONTENT),
            query_content: Some("q".repeat(MAX_QUERY_CONTENT)),
            expected: "IMPOSSIBLE",
            confidence_threshold: Some(0.75),
        },
    ];
    // One row per remaining enum value, so every `AutomatedReasoningCheckResult` is stored
    // and read back.
    for result in CHECK_RESULTS {
        if !rows.iter().any(|row| row.expected == *result) {
            rows.push(Row {
                case: "another expected result",
                guard_content: format!("the claim is {result}"),
                query_content: None,
                expected: result,
                confidence_threshold: None,
            });
        }
    }

    let emu = emulator();
    let policy_arn = create_policy(&emu, "round trip policy").await;
    let mut minted: Vec<String> = Vec::new();

    for row in &rows {
        let case = format!("{} ({})", row.case, row.expected);
        let mut body = json!({
            "guardContent": row.guard_content,
            "expectedAggregatedFindingsResult": row.expected,
        });
        if let Some(query_content) = &row.query_content {
            body["queryContent"] = json!(query_content);
        }
        if let Some(threshold) = row.confidence_threshold {
            body["confidenceThreshold"] = json!(threshold);
        }

        let created = create_test_case(&emu, &policy_arn, body).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreateAutomatedReasoningPolicyTestCase must answer 200, got {} {:?}",
            created.status, created.body
        );
        assert_eq!(
            created.text("policyArn"),
            policy_arn,
            "case {case:?}: the create must report the policy it created the test under"
        );
        let id = created.text("testCaseId");
        assert_id_shape(&case, &id);
        assert!(
            !minted.contains(&id),
            "case {case:?}: each create must mint a distinct testCaseId, got {id:?} twice"
        );
        minted.push(id.clone());

        let read = get_test_case(&emu, &policy_arn, &id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: GetAutomatedReasoningPolicyTestCase must answer 200, got {} {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.text("policyArn"),
            policy_arn,
            "case {case:?}: the read must report the policy the test belongs to"
        );
        let test_case = read.get("testCase");
        assert_eq!(
            member(&test_case, "testCaseId"),
            json!(id),
            "case {case:?}: the read must report the id the create minted, got {:?}",
            member(&test_case, "testCaseId")
        );
        assert_eq!(
            member(&test_case, "guardContent"),
            json!(row.guard_content),
            "case {case:?}: guardContent must come back exactly as it was written, got {:?}",
            member(&test_case, "guardContent")
        );
        assert_eq!(
            member(&test_case, "queryContent"),
            row.query_content
                .as_ref()
                .map_or(Value::Null, |text| json!(text)),
            "case {case:?}: queryContent must come back exactly as it was written, got {:?}",
            member(&test_case, "queryContent")
        );
        assert_eq!(
            member(&test_case, "expectedAggregatedFindingsResult"),
            json!(row.expected),
            "case {case:?}: the read must report the expected result that was stored, got {:?}",
            member(&test_case, "expectedAggregatedFindingsResult")
        );
        let threshold = member(&test_case, "confidenceThreshold");
        match row.confidence_threshold {
            Some(expected) => {
                let actual = threshold.as_f64().unwrap_or_else(|| {
                    panic!("case {case:?}: confidenceThreshold must be a number, got {threshold:?}")
                });
                assert!(
                    (actual - expected).abs() < f64::EPSILON,
                    "case {case:?}: expected confidenceThreshold {expected}, got {actual}"
                );
            }
            None => assert_eq!(
                threshold,
                Value::Null,
                "case {case:?}: a create that named no threshold reports none, got {threshold:?}"
            ),
        }
        let created_at = date_time_seconds(
            &case,
            member(&test_case, "createdAt").as_str().unwrap_or_else(|| {
                panic!(
                    "case {case:?}: createdAt is modelled @timestampFormat(\"date-time\"), got {:?}",
                    member(&test_case, "createdAt")
                )
            }),
        );
        let updated_at = date_time_seconds(
            &case,
            member(&test_case, "updatedAt").as_str().unwrap_or_else(|| {
                panic!(
                    "case {case:?}: updatedAt is modelled @timestampFormat(\"date-time\"), got {:?}",
                    member(&test_case, "updatedAt")
                )
            }),
        );
        assert!(
            (created_at - updated_at).abs() < f64::EPSILON,
            "case {case:?}: a test nothing has updated reports one instant for both, got \
             createdAt={created_at} updatedAt={updated_at}"
        );

        // A read changes nothing a later caller sees.
        let again = get_test_case(&emu, &policy_arn, &id).await;
        assert_eq!(
            again.body, read.body,
            "case {case:?}: a second read must report the same test"
        );
    }

    assert_eq!(
        minted.len(),
        rows.len(),
        "case all: {} creates must mint {} tests, got {minted:?}",
        rows.len(),
        rows.len()
    );
}

// ---------------------------------------------------------------------------------------
// 2. The idempotency token.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_replayed_client_request_token_mints_nothing_new() {
    // Tokens differing in the characters `IdempotencyToken`
    // (`^[a-zA-Z0-9]([-a-zA-Z0-9]{0,254}[a-zA-Z0-9])?$`) admits.
    let tokens = ["token1", "A-token-2", "0123456789abcdef"];

    let emu = emulator();
    let policy_arn = create_policy(&emu, "token policy").await;
    let mut minted: Vec<String> = Vec::new();

    for (index, token) in tokens.iter().enumerate() {
        let body = json!({
            "guardContent": format!("claim {index}"),
            "queryContent": format!("query {index}"),
            "expectedAggregatedFindingsResult": CHECK_RESULTS[index % CHECK_RESULTS.len()],
            "clientRequestToken": token,
        });

        let first = create_test_case(&emu, &policy_arn, body.clone()).await;
        assert_eq!(
            first.status, 200,
            "case {token:?}: the first create must answer 200, got {} {:?}",
            first.status, first.body
        );
        let replay = create_test_case(&emu, &policy_arn, body).await;
        assert_eq!(
            replay.status, 200,
            "case {token:?}: a replay must answer 200, got {} {:?}",
            replay.status, replay.body
        );
        assert_eq!(
            replay.body, first.body,
            "case {token:?}: a replay must answer with the test the token already minted"
        );

        let id = first.text("testCaseId");
        assert!(
            !minted.contains(&id),
            "case {token:?}: a distinct token must mint a distinct test, got {id:?} twice"
        );
        minted.push(id);
    }

    // Three tokens, each replayed once: six creates, three tests. The replay answered with
    // the id the first create minted, and each of those ids still reads back.
    for (token, id) in tokens.iter().zip(&minted) {
        let read = get_test_case(&emu, &policy_arn, id).await;
        assert_eq!(
            read.status, 200,
            "case {token:?}: the test the token minted must read back, got {} {:?}",
            read.status, read.body
        );
    }
    assert_eq!(
        minted.len(),
        tokens.len(),
        "case all: {} distinct tokens must mint {} tests, got {minted:?}",
        tokens.len(),
        tokens.len()
    );
}

// ---------------------------------------------------------------------------------------
// 3. A test belongs to the policy it was created under.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_test_is_addressed_by_its_policy_and_its_id() {
    let emu = emulator();
    let policy_arn = create_policy(&emu, "owning policy").await;
    let other_policy_arn = create_policy(&emu, "other policy").await;

    let created = create_test_case(
        &emu,
        &policy_arn,
        json!({
            "guardContent": "the claim under test",
            "expectedAggregatedFindingsResult": "VALID",
        }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case setup: CreateAutomatedReasoningPolicyTestCase must answer 200, got {} {:?}",
        created.status, created.body
    );
    let id = created.text("testCaseId");

    // Rows: an ARN of a policy nothing minted, a well-formed ARN of another region, and
    // the policy's own versioned ARN — `AutomatedReasoningPolicyArn` admits a `:<version>`
    // suffix and this policy has no versions. Each names no policy of this account, so
    // both operations answer 404.
    let unknown_policies: Vec<(&str, String)> = vec![
        (
            "a policy id nothing minted",
            format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:automated-reasoning-policy/00000000000z"),
        ),
        (
            "a policy ARN of another region",
            policy_arn.replace(REGION, "eu-west-1"),
        ),
        ("a version nothing minted", format!("{policy_arn}:1")),
    ];

    for (case, arn) in &unknown_policies {
        let read = get_test_case(&emu, arn, &id).await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: {arn:?} names no policy of this account, so the read must answer \
             404, got {} {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.error.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            read.error
        );

        let create = create_test_case(
            &emu,
            arn,
            json!({
                "guardContent": "a claim for a policy that is not there",
                "expectedAggregatedFindingsResult": "VALID",
            }),
        )
        .await;
        assert_eq!(
            create.status, 404,
            "case {case:?}: a create under {arn:?} must answer 404, got {} {:?}",
            create.status, create.body
        );
        assert_eq!(
            create.error.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            create.error
        );
    }

    // Rows: an id nothing minted, and the minted id read under ANOTHER policy this account
    // does hold — a test belongs to one policy, so neither names a test here.
    let unknown_tests: Vec<(&str, &str, String)> = vec![
        ("an id nothing minted", policy_arn.as_str(), "AAAAAAAAAAAA".to_string()),
        (
            "the minted id under another policy",
            other_policy_arn.as_str(),
            id.clone(),
        ),
    ];
    for (case, arn, test_case_id) in &unknown_tests {
        let read = get_test_case(&emu, arn, test_case_id).await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: {test_case_id:?} names no test of {arn:?}, so the read must answer \
             404, got {} {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.error.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            read.error
        );
    }

    // The pair the create minted still reads back, so the rows above refused on the pair
    // rather than on the store.
    let read = get_test_case(&emu, &policy_arn, &id).await;
    assert_eq!(
        read.status, 200,
        "case the minted pair: the read must answer 200, got {} {:?}",
        read.status, read.body
    );
}

// ---------------------------------------------------------------------------------------
// 4. The listing.
// ---------------------------------------------------------------------------------------

/// Create `count` tests under `policy_arn`, each with its own content, and hand back the
/// ids the creates minted in ASCENDING order — the order a listing walks.
async fn minted_tests(emu: &Emulator, policy_arn: &str, count: usize) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    for index in 0..count {
        let reply = create_test_case(
            emu,
            policy_arn,
            json!({
                "guardContent": format!("claim {index}"),
                "queryContent": format!("query {index}"),
                "expectedAggregatedFindingsResult": CHECK_RESULTS[index % CHECK_RESULTS.len()],
                "confidenceThreshold": (index as f64) / (count as f64),
            }),
        )
        .await;
        assert_eq!(
            reply.status, 200,
            "case test {index}: the create must answer 200, got {} {:?}",
            reply.status, reply.body
        );
        ids.push(reply.text("testCaseId"));
    }
    ids.sort();
    ids
}

/// The `testCaseId` of every item a listing reply carries, in the order it carries them.
fn listed_ids(reply: &Reply) -> Vec<String> {
    reply
        .get("testCases")
        .as_array()
        .unwrap_or_else(|| panic!("testCases is modelled @required, got {:?}", reply.body))
        .iter()
        .map(|test_case| {
            member(test_case, "testCaseId")
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

#[tokio::test]
async fn a_listing_reports_every_test_of_its_policy_and_none_of_anothers() {
    let emu = emulator();
    // Policies holding a different NUMBER of tests, which is what the rule generalises
    // over: none, one, and several.
    let empty = create_policy(&emu, "listing empty policy").await;
    let single = create_policy(&emu, "listing single policy").await;
    let several = create_policy(&emu, "listing several policy").await;
    let one = minted_tests(&emu, &single, 1).await;
    let many = minted_tests(&emu, &several, 5).await;

    for (case, policy_arn, expected) in [
        ("a policy holding no test", &empty, Vec::new()),
        ("a policy holding one test", &single, one.clone()),
        ("a policy holding several tests", &several, many.clone()),
    ] {
        let listing = list_test_cases(&emu, policy_arn, "").await;
        assert_eq!(
            listing.status, 200,
            "case {case:?}: the listing must answer 200, got {} {:?}",
            listing.status, listing.body
        );
        assert_eq!(
            listed_ids(&listing),
            expected,
            "case {case:?}: the listing reports every test of its policy and none of \
             another's, got {:?}",
            listing.body
        );
        assert_eq!(
            listing.get("nextToken"),
            Value::Null,
            "case {case:?}: a listing that fits one page carries no nextToken, got {:?}",
            listing.body
        );

        // Each listed item is the SAME object the read of that one test reports, member
        // for member — one mapping, two operations.
        let items = listing.get("testCases");
        for id in &expected {
            let read = get_test_case(&emu, policy_arn, id).await;
            let listed = items
                .as_array()
                .expect("testCases is a list")
                .iter()
                .find(|item| member(item, "testCaseId") == json!(id))
                .unwrap_or_else(|| panic!("case {case:?}: {id:?} must be listed"));
            assert_eq!(
                *listed,
                read.get("testCase"),
                "case {case:?}: the listed test {id:?} must match the read, got {listed:?}"
            );
        }
    }

    // A listing changes nothing a later caller sees.
    let again = list_test_cases(&emu, &several, "").await;
    assert_eq!(
        listed_ids(&again),
        many,
        "case \"a second listing\": a read reports the same tests, got {:?}",
        again.body
    );
}

#[tokio::test]
async fn the_pages_of_a_listing_partition_its_tests() {
    let emu = emulator();
    let policy_arn = create_policy(&emu, "listing pages policy").await;
    let minted = minted_tests(&emu, &policy_arn, 5).await;

    // Page sizes differing along what the rule generalises over: one test per page, two
    // (so the last page is short), and the whole listing in one page.
    for page_size in [1_usize, 2, minted.len()] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match &token {
                Some(token) => format!("maxResults={page_size}&nextToken={token}"),
                None => format!("maxResults={page_size}"),
            };
            let page = list_test_cases(&emu, &policy_arn, &query).await;
            assert_eq!(
                page.status, 200,
                "case page size {page_size}: a page must answer 200, got {} {:?}",
                page.status, page.body
            );
            let ids = listed_ids(&page);
            assert!(
                ids.len() <= page_size,
                "case page size {page_size}: a page holds at most {page_size} tests, got \
                 {ids:?}"
            );
            walked.extend(ids);
            pages += 1;
            assert!(
                pages <= minted.len() + 1,
                "case page size {page_size}: the walk must terminate, got {walked:?}"
            );
            match page.get("nextToken").as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            walked, minted,
            "case page size {page_size}: the pages partition the listing with neither a gap \
             nor a repeat, got {walked:?}"
        );
    }
}

#[tokio::test]
async fn a_listing_of_a_policy_this_account_holds_none_of_is_not_found() {
    let emu = emulator();
    let policy_arn = create_policy(&emu, "listing owner policy").await;
    minted_tests(&emu, &policy_arn, 2).await;

    // The same rows the single read refuses on: a policy nothing minted, one of another
    // region, and a version nothing minted.
    let rows: Vec<(&str, String)> = vec![
        (
            "a policy id nothing minted",
            format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:automated-reasoning-policy/00000000000z"),
        ),
        (
            "a policy ARN of another region",
            policy_arn.replace(REGION, "eu-west-1"),
        ),
        ("a version nothing minted", format!("{policy_arn}:1")),
    ];

    for (case, arn) in rows {
        let listing = list_test_cases(&emu, &arn, "").await;
        assert_eq!(
            listing.status, 404,
            "case {case:?}: {arn:?} names no policy of this account, so the listing must \
             answer 404, got {} {:?}",
            listing.status, listing.body
        );
        assert_eq!(
            listing.error.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            listing.error
        );
    }
}

#[tokio::test]
async fn a_next_token_outside_the_cursor_form_is_refused() {
    let emu = emulator();
    let policy_arn = create_policy(&emu, "listing token policy").await;
    minted_tests(&emu, &policy_arn, 3).await;

    // Rows differing along what a cursor is NOT: too short, too long, lower case (outside
    // `AutomatedReasoningPolicyTestCaseId`'s `^[0-9A-Z]{12}$`), and a token shaped like
    // another listing's cursor. A token this listing could never have issued is refused;
    // a WELL-FORMED one names a position in the id order, which the pagination test walks.
    for (case, token) in [
        ("too short", "AAAA"),
        ("too long", "AAAAAAAAAAAAA"),
        ("lower case", "aaaaaaaaaaaa"),
        ("another listing's cursor", "abc123/00000000"),
    ] {
        let listing = list_test_cases(&emu, &policy_arn, &format!("nextToken={token}")).await;
        assert_eq!(
            (listing.status, listing.error.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: a token outside the cursor form is refused, got {} {:?}",
            listing.status,
            listing.body
        );
    }
}
