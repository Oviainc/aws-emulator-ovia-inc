// SPDX-License-Identifier: CC0-1.0

//! `UpdateAutomatedReasoningPolicyTestCase` end-to-end, over the same `MultiServiceRouter`
//! the emulator boots.
//!
//! The rules under test, each asserted over a SET of inputs rather than over one recorded
//! request:
//!
//! 1. **A mutation addressed to a test the store does not hold fails on its IDENTITY**: the
//!    answer is `ResourceNotFoundException` (404) carrying the resource's own wording, the
//!    SAME status, code and message a read of that pair gives — never a 400 that reports the
//!    request's parameters as bad. The rows span a test a delete removed, an id nothing ever
//!    minted, an id minted under ANOTHER policy, and a `policyArn` naming no policy of this
//!    account, each driven with several spellings of `lastUpdatedAt` so the identity is
//!    shown to decide ahead of the concurrency token.
//! 2. **An accepted update rewrites exactly the test's content members**: a later
//!    `GetAutomatedReasoningPolicyTestCase` reports what the update carried, character for
//!    character; an optional member the update omits is removed; `testCaseId` and
//!    `createdAt` are the test's identity and do not move, `updatedAt` names the instant of
//!    the write, and no other test of the policy changes. The rows differ in the script and
//!    length of the content, in the `AutomatedReasoningCheckResult` expected (the whole
//!    enum), in the threshold at each published bound, and in whether the optional members
//!    arrive, go away or stay away.
//! 3. **`lastUpdatedAt` is a concurrency token, weighed only once the record exists**: an
//!    update naming any instant other than the write the test holds is `ConflictException`
//!    (400) and leaves the test exactly as it was — over instants on both sides of the
//!    stored one, at millisecond and second distance, and at the epoch and far in the
//!    future.
//! 4. **A replayed `clientRequestToken` is ignored**: a retry of an update that already
//!    landed — the same token beside the `lastUpdatedAt` the retried request was built from
//!    — answers success and writes nothing a second time, while the same stale instant
//!    under a token nothing landed is the refusal of rule 3.
//!
//! Every expectation is derived from the model, cited per row
//! (`smithy_query(query="operation",operation="UpdateAutomatedReasoningPolicyTestCase")`:
//! `@http(code: 200, method: "PATCH", uri: "/automated-reasoning-policies/{policyArn}/test-cases/{testCaseId}")`,
//! `ResourceNotFoundException` 404, `ConflictException` 400, `guardContent`
//! `@length(max: 2048)`, `queryContent` `@length(max: 1024)`, `confidenceThreshold`
//! `@range(min: 0, max: 1)`) and from its documentation (`lastUpdatedAt` "is used as a
//! concurrency token to prevent conflicting modifications"; `confidenceThreshold` "If null
//! is provided, the threshold will be removed"; `clientRequestToken` "If this token matches
//! a previous request, Amazon Bedrock ignores the request, but does not return an error").
//! The requests go through `crate::support` — the ONE harness every suite in this crate
//! drives the emulator with.

use serde_json::{json, Value};

use crate::automated_reasoning_policy_test_cases::{
    create_policy, create_test_case, get_test_case, CHECK_RESULTS,
};
use crate::support::{
    at_millis, date_time_seconds, emulator, epoch_millis, label, wall_clock_seconds, Emulator,
    Reply, ACCOUNT, REGION,
};

/// The most characters `guardContent` admits (`@length(max: 2048)`).
const MAX_GUARD_CONTENT: usize = 2048;

/// The most characters `queryContent` admits (`@length(max: 1024)`).
const MAX_QUERY_CONTENT: usize = 1024;

/// The `ResourceNotFoundException` message a request addressing a test the store does not
/// hold is answered with — Amazon Bedrock's wording for that error class, whose variable
/// parts are the id and the policy ARN the REQUEST carried. Asserted below against several
/// ids under several policies, so the template is pinned rather than one pair's spelling of
/// it.
fn not_found_message(test_case_id: &str, policy_arn: &str) -> String {
    format!(
        "The TestCase with testCaseId: {test_case_id} and policyArn: {policy_arn} does not exist"
    )
}

/// One `PATCH /automated-reasoning-policies/{policyArn}/test-cases/{testCaseId}`.
///
/// Both identifiers are path LABELS and the ARN carries `:` and `/`, which are
/// percent-encoded so the whole identifier stays one path segment, exactly as an SDK writes
/// it.
async fn update_test_case(
    emu: &Emulator,
    policy_arn: &str,
    test_case_id: &str,
    body: Value,
) -> Reply {
    emu.call(
        "PATCH",
        &format!(
            "/automated-reasoning-policies/{}/test-cases/{}",
            label(policy_arn),
            label(test_case_id)
        ),
        Some(body),
    )
    .await
}

/// The `updatedAt` a read of the test `test_case_id` reports, as the `date-time` text the
/// wire carries — the value a caller echoes back as `lastUpdatedAt`.
async fn stored_updated_at(emu: &Emulator, policy_arn: &str, test_case_id: &str) -> String {
    let read = get_test_case(emu, policy_arn, test_case_id).await;
    assert_eq!(
        read.status, 200,
        "case {test_case_id:?}: the test must read back before its updatedAt is taken, got \
         {} {:?}",
        read.status, read.body
    );
    let test_case = read.get("testCase");
    test_case
        .get("updatedAt")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!(
                "case {test_case_id:?}: updatedAt is modelled @timestampFormat(\"date-time\"), \
                 so the read must report it as an RFC 3339 string, got {test_case:?}"
            )
        })
        .to_string()
}

/// The body of an update carrying the two required content members, the token the test
/// currently holds, and whichever optional members the row names.
fn update_body(
    guard_content: &str,
    query_content: Option<&str>,
    expected: &str,
    confidence_threshold: Option<f64>,
    last_updated_at: &str,
) -> Value {
    let mut body = json!({
        "guardContent": guard_content,
        "expectedAggregatedFindingsResult": expected,
        "lastUpdatedAt": last_updated_at,
    });
    if let Some(query_content) = query_content {
        body["queryContent"] = json!(query_content);
    }
    if let Some(confidence_threshold) = confidence_threshold {
        body["confidenceThreshold"] = json!(confidence_threshold);
    }
    body
}

/// Create one test under `policy_arn` carrying every member, and hand back its id.
async fn minted_test(emu: &Emulator, policy_arn: &str, content: &str) -> String {
    let reply = create_test_case(
        emu,
        policy_arn,
        json!({
            "guardContent": content,
            "queryContent": format!("query for {content}"),
            "expectedAggregatedFindingsResult": "VALID",
            "confidenceThreshold": 0.5,
        }),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "case {content:?}: the create must answer 200, got {} {:?}",
        reply.status, reply.body
    );
    reply.text("testCaseId")
}

// ---------------------------------------------------------------------------------------
// 1. A mutation addressed to a test the store does not hold fails on its identity.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_test_the_store_does_not_hold_is_not_found() {
    let emu = emulator();
    let policy_arn = create_policy(&emu, "update identity policy").await;
    let other_policy_arn = create_policy(&emu, "other update identity policy").await;

    // The test this suite deletes, so the row below addresses an id that DID resolve and no
    // longer does — the lifecycle the issue reports.
    let deleted = minted_test(&emu, &policy_arn, "claim to be deleted").await;
    let before_delete = stored_updated_at(&emu, &policy_arn, &deleted).await;
    let removed = emu
        .call(
            "DELETE",
            &format!(
                "/automated-reasoning-policies/{}/test-cases/{}?updatedAt={}",
                label(&policy_arn),
                label(&deleted),
                label(&before_delete)
            ),
            None,
        )
        .await;
    assert_eq!(
        removed.status, 202,
        "case the deleted test: the delete must answer 202, got {} {:?}",
        removed.status, removed.body
    );

    // A test of ANOTHER policy, so its id is one this store minted but this policy holds no
    // test under.
    let elsewhere = minted_test(&emu, &other_policy_arn, "claim under another policy").await;

    // Rows: the pair the delete removed, an id nothing ever minted (both bounds of the
    // id's alphabet), and an id minted under another policy. Every one names a test this
    // policy holds none of, so every one is the resource's own 404.
    let missing_tests: Vec<(&str, String)> = vec![
        ("a test a delete removed", deleted.clone()),
        ("an id nothing minted", "AAAAAAAAAAAA".to_string()),
        ("another id nothing minted", "000000000000".to_string()),
        ("an id minted under another policy", elsewhere.clone()),
    ];

    // Spellings of `lastUpdatedAt`: the instant the removed test held, the same instant
    // truncated to whole milliseconds, the epoch and a far-future instant. None of them is
    // the write any addressed test holds, so a token-first implementation would answer the
    // conflict instead — the identity decides first.
    let held = epoch_millis("the removed test", &before_delete);
    let tokens: Vec<(&str, String)> = vec![
        ("the instant the removed test held", before_delete.clone()),
        (
            "that instant in whole milliseconds",
            at_millis("whole milliseconds", held),
        ),
        ("the epoch", at_millis("the epoch", 0)),
        // 2099-12-31T23:59:59Z, far past any instant this store can mint.
        (
            "far in the future",
            at_millis("far future", 4_102_444_799_000),
        ),
    ];

    for (test_label, test_case_id) in &missing_tests {
        for (token_label, token) in &tokens {
            let case = format!("{test_label} with {token_label}");
            let refused = update_test_case(
                &emu,
                &policy_arn,
                test_case_id,
                update_body("rewritten claim", None, "VALID", None, token),
            )
            .await;
            assert_eq!(
                (refused.status, refused.error.as_deref()),
                (404, Some("ResourceNotFoundException")),
                "case {case:?}: a request meeting every modelled constraint that names a test \
                 the store does not hold must answer 404 ResourceNotFoundException, got {} \
                 {:?}",
                refused.status,
                refused.body
            );
            assert_eq!(
                refused.get("message"),
                json!(not_found_message(test_case_id, &policy_arn)),
                "case {case:?}: the refusal must carry the service's wording for a test it \
                 does not hold, got {:?}",
                refused.body
            );

            // The read of the same pair is the same failure: the two operations resolve one
            // identity, so a teardown loop keying on the code sees one answer from both.
            let read = get_test_case(&emu, &policy_arn, test_case_id).await;
            assert_eq!(
                (
                    read.status,
                    read.error.as_deref(),
                    read.get("message")
                ),
                (
                    refused.status,
                    refused.error.as_deref(),
                    refused.get("message")
                ),
                "case {case:?}: the mutation and the read of one missing test must answer \
                 alike, got {:?} from the read and {:?} from the update",
                read.body,
                refused.body
            );
        }
    }

    // A `policyArn` this account holds no policy under: the test hangs off the policy, so
    // there is no policy to hold it and the answer is the same error class.
    let unknown_policies: Vec<(&str, String)> = vec![
        (
            "a policy id nothing minted",
            format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:automated-reasoning-policy/00000000000z"),
        ),
        ("a version nothing minted", format!("{policy_arn}:1")),
    ];
    for (case, arn) in &unknown_policies {
        let refused = update_test_case(
            &emu,
            arn,
            &elsewhere,
            update_body("rewritten claim", None, "VALID", None, &before_delete),
        )
        .await;
        assert_eq!(
            (refused.status, refused.error.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?}: {arn:?} names no policy of this account, so the update must answer \
             404, got {} {:?}",
            refused.status,
            refused.body
        );
    }

    // The test the other policy does hold is untouched by every row above.
    let read = get_test_case(&emu, &other_policy_arn, &elsewhere).await;
    assert_eq!(
        read.status, 200,
        "case the surviving test: a refused update must leave the test that does exist \
         readable, got {} {:?}",
        read.status, read.body
    );
}

// ---------------------------------------------------------------------------------------
// 2. An accepted update rewrites exactly the test's content members.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn an_accepted_update_rewrites_the_stored_test() {
    /// One rewrite: what the update carries, which the read must then report.
    struct Row {
        case: &'static str,
        guard_content: String,
        query_content: Option<String>,
        expected: &'static str,
        confidence_threshold: Option<f64>,
    }

    // Rows differ along the script and length of the content, the enum value expected, the
    // threshold at each published bound of `@range(min: 0, max: 1)`, and whether each
    // optional member arrives, goes away or stays away.
    let mut rows = vec![
        Row {
            case: "every member rewritten",
            guard_content: "the rewritten claim".to_string(),
            query_content: Some("the rewritten query".to_string()),
            expected: "INVALID",
            confidence_threshold: Some(0.25),
        },
        Row {
            case: "the optional members removed",
            guard_content: "a claim with no query and no threshold".to_string(),
            query_content: None,
            expected: "SATISFIABLE",
            confidence_threshold: None,
        },
        Row {
            case: "the threshold at its published minimum",
            guard_content: "a claim at zero confidence".to_string(),
            query_content: None,
            expected: "TOO_COMPLEX",
            confidence_threshold: Some(0.0),
        },
        Row {
            case: "the threshold at its published maximum",
            guard_content: "a claim at full confidence".to_string(),
            query_content: Some("the query beside it".to_string()),
            expected: "IMPOSSIBLE",
            confidence_threshold: Some(1.0),
        },
        Row {
            case: "content outside the ASCII range",
            guard_content: "Die Rückgabefrist beträgt 30 Tage — immer.".to_string(),
            query_content: Some("ما هي سياسة الإرجاع؟".to_string()),
            expected: "NO_TRANSLATION",
            confidence_threshold: Some(0.75),
        },
        Row {
            case: "content at its published maximum length",
            guard_content: "g".repeat(MAX_GUARD_CONTENT),
            query_content: Some("q".repeat(MAX_QUERY_CONTENT)),
            expected: "TRANSLATION_AMBIGUOUS",
            confidence_threshold: Some(0.5),
        },
    ];
    // One row per enum value no row above carries, so every `AutomatedReasoningCheckResult`
    // is written through the update rather than only the value a recording carried.
    for expected in CHECK_RESULTS {
        if !rows.iter().any(|row| row.expected == *expected) {
            rows.push(Row {
                case: "one more value of the whole enum",
                guard_content: format!("a claim expecting {expected}"),
                query_content: None,
                expected,
                confidence_threshold: None,
            });
        }
    }

    let emu = emulator();
    let policy_arn = create_policy(&emu, "accepted update policy").await;

    for (index, row) in rows.iter().enumerate() {
        let case = format!("{} ({index})", row.case);
        let id = minted_test(&emu, &policy_arn, &format!("claim {index}")).await;
        // A second test of the same policy, so the rewrite is shown to touch one record.
        let bystander = minted_test(&emu, &policy_arn, &format!("bystander {index}")).await;
        let bystander_before = get_test_case(&emu, &policy_arn, &bystander).await.body;

        let before = get_test_case(&emu, &policy_arn, &id).await;
        let created_at = before.get("testCase").get("createdAt").cloned();
        let token = stored_updated_at(&emu, &policy_arn, &id).await;

        let clock_before = wall_clock_seconds();
        let updated = update_test_case(
            &emu,
            &policy_arn,
            &id,
            update_body(
                &row.guard_content,
                row.query_content.as_deref(),
                row.expected,
                row.confidence_threshold,
                &token,
            ),
        )
        .await;
        let clock_after = wall_clock_seconds();
        assert_eq!(
            updated.status, 200,
            "case {case:?}: the update must answer 200, got {} {:?}",
            updated.status, updated.body
        );
        assert_eq!(
            (updated.text("policyArn"), updated.text("testCaseId")),
            (policy_arn.clone(), id.clone()),
            "case {case:?}: the answer must name the test the request addressed, got {:?}",
            updated.body
        );

        let read = get_test_case(&emu, &policy_arn, &id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the rewritten test must read back, got {} {:?}",
            read.status, read.body
        );
        let test_case = read.get("testCase");
        assert_eq!(
            test_case.get("guardContent").cloned(),
            Some(json!(row.guard_content)),
            "case {case:?}: the read must report the content the update carried, got {:?}",
            test_case
        );
        assert_eq!(
            test_case.get("queryContent").cloned(),
            row.query_content.as_ref().map(|value| json!(value)),
            "case {case:?}: an optional member the update omits is removed, got {:?}",
            test_case
        );
        assert_eq!(
            test_case
                .get("expectedAggregatedFindingsResult")
                .cloned(),
            Some(json!(row.expected)),
            "case {case:?}: the read must report the result the update expects, got {:?}",
            test_case
        );
        assert_eq!(
            test_case.get("confidenceThreshold").cloned(),
            row.confidence_threshold.map(|value| json!(value)),
            "case {case:?}: a threshold the update omits is removed, got {:?}",
            test_case
        );
        assert_eq!(
            test_case.get("testCaseId").cloned(),
            Some(json!(id)),
            "case {case:?}: the update must not move the test's identity, got {:?}",
            test_case
        );
        assert_eq!(
            test_case.get("createdAt").cloned(),
            created_at,
            "case {case:?}: the update must not move the instant the test was created, got \
             {test_case:?}"
        );

        // `updatedAt` names the instant of THIS write, so it falls inside the span the call
        // took rather than staying where the create left it.
        let written = date_time_seconds(&case, &stored_updated_at(&emu, &policy_arn, &id).await);
        assert!(
            written >= clock_before - 0.001 && written <= clock_after + 0.001,
            "case {case:?}: updatedAt must name the instant of the write, between \
             {clock_before} and {clock_after}, got {written}"
        );

        let bystander_after = get_test_case(&emu, &policy_arn, &bystander).await;
        assert_eq!(
            bystander_after.body, bystander_before,
            "case {case:?}: the update must leave every other test of the policy exactly as \
             it was"
        );
    }
}

// ---------------------------------------------------------------------------------------
// 3. `lastUpdatedAt` is the concurrency token, weighed once the record exists.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn the_token_must_name_the_write_the_test_holds() {
    let emu = emulator();
    let policy_arn = create_policy(&emu, "update token policy").await;
    let id = minted_test(&emu, &policy_arn, "the claim under test").await;

    let reported = stored_updated_at(&emu, &policy_arn, &id).await;
    let held = epoch_millis("the stored instant", &reported);
    let before = get_test_case(&emu, &policy_arn, &id).await.body;

    // Rows differ along the DISTANCE and the direction from the write the test holds, from
    // one millisecond either side out to the epoch and far into the future.
    let rows: Vec<(&str, String)> = vec![
        ("one millisecond earlier", at_millis("earlier", held - 1)),
        ("one millisecond later", at_millis("later", held + 1)),
        (
            "one second earlier",
            at_millis("a second earlier", held - 1_000),
        ),
        ("one second later", at_millis("a second later", held + 1_000)),
        ("a day earlier", at_millis("a day earlier", held - 86_400_000)),
        ("the epoch", at_millis("the epoch", 0)),
        // 2099-12-31T23:59:59Z, far past any instant this store can mint.
        (
            "far in the future",
            at_millis("far future", 4_102_444_799_000),
        ),
    ];

    for (case, token) in &rows {
        let refused = update_test_case(
            &emu,
            &policy_arn,
            &id,
            update_body("a rewrite that must not land", None, "INVALID", None, token),
        )
        .await;
        assert_eq!(
            (refused.status, refused.error.as_deref()),
            (400, Some("ConflictException")),
            "case {case:?}: {token:?} is not the write the test holds, so the update must be \
             refused, got {} {:?}",
            refused.status,
            refused.body
        );
        let read = get_test_case(&emu, &policy_arn, &id).await;
        assert_eq!(
            read.body, before,
            "case {case:?}: a refused update must change nothing about the test"
        );
    }

    // The token the test does hold rewrites it, so every row above was refused on the token
    // and not on the pair.
    let accepted = update_test_case(
        &emu,
        &policy_arn,
        &id,
        update_body("the rewrite that lands", None, "IMPOSSIBLE", None, &reported),
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "case the stored instant: the update must answer 200, got {} {:?}",
        accepted.status, accepted.body
    );
    assert_eq!(
        get_test_case(&emu, &policy_arn, &id)
            .await
            .get("testCase")
            .get("guardContent")
            .cloned(),
        Some(json!("the rewrite that lands")),
        "case the stored instant: the accepted update must be what the read reports"
    );
}

// ---------------------------------------------------------------------------------------
// 4. A replayed `clientRequestToken` is ignored.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_replayed_client_request_token_writes_nothing_twice() {
    // Tokens differing in the characters `IdempotencyToken`
    // (`^[a-zA-Z0-9]([-a-zA-Z0-9]{0,254}[a-zA-Z0-9])?$`) admits.
    let tokens = ["token1", "A-token-2", "0123456789abcdef"];

    let emu = emulator();
    let policy_arn = create_policy(&emu, "update replay policy").await;

    for (index, client_token) in tokens.iter().enumerate() {
        let case = format!("{client_token} ({index})");
        let id = minted_test(&emu, &policy_arn, &format!("replay claim {index}")).await;
        let stale = stored_updated_at(&emu, &policy_arn, &id).await;

        let mut body = update_body(
            &format!("the rewrite {index} carried"),
            None,
            CHECK_RESULTS[index % CHECK_RESULTS.len()],
            Some(0.5),
            &stale,
        );
        body["clientRequestToken"] = json!(client_token);

        let first = update_test_case(&emu, &policy_arn, &id, body.clone()).await;
        assert_eq!(
            first.status, 200,
            "case {case:?}: the first update must answer 200, got {} {:?}",
            first.status, first.body
        );
        let landed = get_test_case(&emu, &policy_arn, &id).await.body;

        // The retry is the SAME request: the same token beside the instant the caller read
        // before the write it is retrying. It must be ignored rather than refused on the
        // concurrency token, and must write nothing a second time.
        for attempt in 0..2 {
            let replay = update_test_case(&emu, &policy_arn, &id, body.clone()).await;
            assert_eq!(
                replay.status, 200,
                "case {case:?}: replay {attempt} of an update that landed must answer 200, got \
                 {} {:?}",
                replay.status, replay.body
            );
            assert_eq!(
                (replay.text("policyArn"), replay.text("testCaseId")),
                (policy_arn.clone(), id.clone()),
                "case {case:?}: replay {attempt} must name the test the request addressed, got \
                 {:?}",
                replay.body
            );
            assert_eq!(
                get_test_case(&emu, &policy_arn, &id).await.body,
                landed,
                "case {case:?}: replay {attempt} must leave the test exactly as the update \
                 that landed left it"
            );
        }

        // The same stale instant under a token nothing landed is the refusal of rule 3: it
        // is the TOKEN that makes the replay a replay, not the timestamp.
        let mut other = body.clone();
        other["clientRequestToken"] = json!(format!("{client_token}-other"));
        let refused = update_test_case(&emu, &policy_arn, &id, other).await;
        assert_eq!(
            (refused.status, refused.error.as_deref()),
            (400, Some("ConflictException")),
            "case {case:?}: a token nothing landed must not replay the write, got {} {:?}",
            refused.status,
            refused.body
        );
        assert_eq!(
            get_test_case(&emu, &policy_arn, &id).await.body,
            landed,
            "case {case:?}: the refused update must change nothing about the test"
        );
    }
}
