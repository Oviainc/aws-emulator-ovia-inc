// SPDX-License-Identifier: CC0-1.0

//! `DeleteAutomatedReasoningPolicyTestCase` end-to-end: the stored test
//! `CreateAutomatedReasoningPolicyTestCase` minted is removed from the store, over the same
//! `MultiServiceRouter` the emulator boots.
//!
//! The rules under test, each asserted over a SET of inputs rather than over one recorded
//! request:
//!
//! 1. **A deleted test is gone from every read**: the delete answers `202` with no members,
//!    a later `GetAutomatedReasoningPolicyTestCase` is `404` and
//!    `ListAutomatedReasoningPolicyTestCases` reports exactly the policy's remaining tests,
//!    unchanged — for policies holding one and several tests, for the first, a middle and
//!    the last test of a policy, and for a `lastUpdatedAt` spelled as the read reported it
//!    and truncated to whole milliseconds. Deleting again succeeds: the operation is
//!    documented idempotent.
//! 2. **`lastUpdatedAt` is a concurrency token**: a delete naming any instant other than the
//!    write the test holds is refused with `ConflictException` and leaves the test readable,
//!    over instants on both sides of the stored one, at millisecond and second distance,
//!    and at the epoch and far in the future.
//! 3. **A test is addressed by its policy AND its id**: a `policyArn` this account holds no
//!    policy under is `404`, while a `testCaseId` the addressed policy holds no test under
//!    is the idempotent success of rule 1 — including an id minted under ANOTHER policy,
//!    which still reads back under the policy that does hold it.
//! 4. **A deleted test releases its `clientRequestToken`**: a create replaying the token of
//!    a deleted test mints a NEW test rather than naming the one that is gone.
//!
//! Every expectation is derived from the model, cited per row
//! (`smithy_query(query="operation",operation="DeleteAutomatedReasoningPolicyTestCase")`:
//! `@http(code: 202, method: "DELETE")`, `lastUpdatedAt` bound to the `updatedAt` query
//! parameter as a `date-time` timestamp, `ConflictException` 400, `ResourceNotFoundException`
//! 404), and from its documentation ("This operation is idempotent; if you delete a test
//! more than once, each call succeeds"; `lastUpdatedAt` "is used as a concurrency token to
//! prevent conflicting modifications"). The requests go through `crate::support` — the ONE
//! harness every suite in this crate drives the emulator with.

use aws_smithy_types::{date_time::Format, DateTime};
use serde_json::{json, Value};

use crate::automated_reasoning_policy_test_cases::{
    create_policy, create_test_case, get_test_case, list_test_cases, CHECK_RESULTS,
};
use crate::support::{emulator, label, Emulator, Reply, ACCOUNT, REGION};

/// The status `DeleteAutomatedReasoningPolicyTestCase` answers a delete it accepted with
/// (`@http(code: 202)`).
const DELETE_ACCEPTED: u16 = 202;

/// The `ResourceNotFoundException` message a request addressing a test the store does not
/// hold is answered with — Amazon Bedrock's wording for that error class, whose variable
/// parts are the id and the policy ARN the request carried. Asserted here against the ids
/// of SEVERAL deleted tests under several policies, so the template is pinned rather than
/// one pair's spelling of it.
fn not_found_message(test_case_id: &str, policy_arn: &str) -> String {
    format!(
        "The TestCase with testCaseId: {test_case_id} and policyArn: {policy_arn} does not exist"
    )
}

/// One `DELETE /automated-reasoning-policies/{policyArn}/test-cases/{testCaseId}`, carrying
/// `last_updated_at` in the `updatedAt` query parameter the model binds it to.
///
/// The ARN and the id are path LABELS and the token is a query VALUE; all three carry `:`,
/// which is percent-encoded so each stays one path segment or one parameter, exactly as an
/// SDK writes them.
async fn delete_test_case(
    emu: &Emulator,
    policy_arn: &str,
    test_case_id: &str,
    last_updated_at: &str,
) -> Reply {
    emu.call(
        "DELETE",
        &format!(
            "/automated-reasoning-policies/{}/test-cases/{}?updatedAt={}",
            label(policy_arn),
            label(test_case_id),
            label(last_updated_at)
        ),
        None,
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

/// An RFC 3339 `date-time` as whole milliseconds since the epoch.
fn epoch_millis(case: &str, text: &str) -> i64 {
    let parsed = DateTime::from_str(text, Format::DateTime).unwrap_or_else(|error| {
        panic!("case {case:?}: {text:?} does not parse as an RFC 3339 date-time: {error}")
    });
    parsed.secs() * 1_000 + i64::from(parsed.subsec_nanos() / 1_000_000)
}

/// `millis` since the epoch as the RFC 3339 `date-time` text a client sends.
fn at_millis(case: &str, millis: i64) -> String {
    DateTime::from_millis(millis)
        .fmt(Format::DateTime)
        .unwrap_or_else(|error| panic!("case {case:?}: {millis} does not render: {error}"))
}

/// Assert `reply` is the answer the delete gives a request it accepted: the modelled status
/// and a body carrying no members (`DeleteAutomatedReasoningPolicyTestCaseResponse` declares
/// none).
fn assert_accepted(case: &str, reply: &Reply) {
    assert_eq!(
        reply.status, DELETE_ACCEPTED,
        "case {case:?}: the delete must answer {DELETE_ACCEPTED}, got {} {:?}",
        reply.status, reply.body
    );
    assert!(
        reply.body == Value::Null || reply.body == json!({}),
        "case {case:?}: the delete's answer carries no members, got {:?}",
        reply.body
    );
    assert!(
        reply.error.is_none(),
        "case {case:?}: an accepted delete carries no error, got {:?}",
        reply.error
    );
}

/// The `testCaseId` of every test a listing of `policy_arn` reports, in the order it reports
/// them.
async fn listed_ids(case: &str, emu: &Emulator, policy_arn: &str) -> Vec<String> {
    let listed = list_test_cases(emu, policy_arn, "maxResults=1000").await;
    assert_eq!(
        listed.status, 200,
        "case {case:?}: the listing must answer 200, got {} {:?}",
        listed.status, listed.body
    );
    listed
        .get("testCases")
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: testCases is a required list, got {:?}", listed.body))
        .iter()
        .map(|item| {
            item.get("testCaseId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// Create `count` tests under `policy_arn`, each with its own content and expected result,
/// and hand back their ids in ASCENDING order — the order a listing walks, so a row can
/// address the first, a middle or the last test of its policy.
async fn minted_tests(emu: &Emulator, policy_arn: &str, count: usize) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    for index in 0..count {
        let mut body = json!({
            "guardContent": format!("claim {index} of {count}"),
            "expectedAggregatedFindingsResult": CHECK_RESULTS[index % CHECK_RESULTS.len()],
        });
        // Rows differ in whether the optional members were stored, so the delete is driven
        // against tests of both forms.
        if index % 2 == 0 {
            body["queryContent"] = json!(format!("query {index}"));
            body["confidenceThreshold"] = json!(0.5);
        }
        let reply = create_test_case(emu, policy_arn, body).await;
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

// ---------------------------------------------------------------------------------------
// 1. A deleted test is gone from every read, and deleting it again succeeds.
// ---------------------------------------------------------------------------------------

/// How the row spells the `lastUpdatedAt` it sends.
#[derive(Debug, Clone, Copy)]
enum Token {
    /// Exactly the text the read reported.
    AsRead,
    /// The same instant truncated to whole milliseconds — what a client carrying
    /// millisecond instants echoes back after reading a finer-grained one.
    WholeMilliseconds,
}

#[tokio::test]
async fn a_deleted_test_is_gone_from_every_read() {
    // Rows differ along how many tests the policy holds, which of them (first, middle,
    // last in listing order) the delete addresses, and how the token is spelled.
    struct Row {
        case: &'static str,
        held: usize,
        victim: usize,
        token: Token,
    }

    let rows = [
        Row {
            case: "the only test of its policy",
            held: 1,
            victim: 0,
            token: Token::AsRead,
        },
        Row {
            case: "the first of three",
            held: 3,
            victim: 0,
            token: Token::AsRead,
        },
        Row {
            case: "a middle test of three, token truncated to milliseconds",
            held: 3,
            victim: 1,
            token: Token::WholeMilliseconds,
        },
        Row {
            case: "the last of three",
            held: 3,
            victim: 2,
            token: Token::WholeMilliseconds,
        },
        Row {
            case: "one of two",
            held: 2,
            victim: 1,
            token: Token::AsRead,
        },
    ];

    let emu = emulator();

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        // `AutomatedReasoningPolicyName` is `[0-9a-zA-Z-_ ]+`, so the row's index names its
        // policy rather than the case text.
        let policy_arn = create_policy(&emu, &format!("deletion policy {index}")).await;
        let ids = minted_tests(&emu, &policy_arn, row.held).await;
        let victim = ids[row.victim].clone();

        // The bodies of the tests that must survive, read BEFORE the delete, so the
        // assertion afterwards is against what the store held rather than against a
        // hand-written expectation.
        let mut survivors: Vec<(String, Value)> = Vec::new();
        for id in ids.iter().filter(|id| **id != victim) {
            survivors.push((
                id.clone(),
                get_test_case(&emu, &policy_arn, id).await.body,
            ));
        }

        let reported = stored_updated_at(&emu, &policy_arn, &victim).await;
        let token = match row.token {
            Token::AsRead => reported.clone(),
            Token::WholeMilliseconds => at_millis(case, epoch_millis(case, &reported)),
        };

        let deleted = delete_test_case(&emu, &policy_arn, &victim, &token).await;
        assert_accepted(case, &deleted);

        let read = get_test_case(&emu, &policy_arn, &victim).await;
        assert_eq!(
            (read.status, read.error.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?}: the deleted test must be gone from the read, got {} {:?}",
            read.status,
            read.body
        );
        assert_eq!(
            read.get("message"),
            json!(not_found_message(&victim, &policy_arn)),
            "case {case:?}: the refusal must carry the service's wording for a test it does \
             not hold, got {:?}",
            read.body
        );

        let mut expected: Vec<String> = ids.iter().filter(|id| **id != victim).cloned().collect();
        expected.sort();
        assert_eq!(
            listed_ids(case, &emu, &policy_arn).await,
            expected,
            "case {case:?}: the listing must report exactly the policy's remaining tests"
        );
        for (id, before) in &survivors {
            let after = get_test_case(&emu, &policy_arn, id).await;
            assert_eq!(
                after.body, *before,
                "case {case:?}: the delete must leave test {id:?} exactly as it was"
            );
        }

        // "This operation is idempotent; if you delete a test more than once, each call
        // succeeds" — with the token the deleted test carried, and with one it never did.
        for (spelling, repeated_token) in [
            ("the same token", token.clone()),
            ("the epoch", at_millis(case, 0)),
        ] {
            let again = delete_test_case(&emu, &policy_arn, &victim, &repeated_token).await;
            assert_accepted(&format!("{case} deleted again with {spelling}"), &again);
        }
        assert_eq!(
            listed_ids(case, &emu, &policy_arn).await,
            expected,
            "case {case:?}: a repeated delete must change no other test"
        );
    }
}

// ---------------------------------------------------------------------------------------
// 2. `lastUpdatedAt` is the concurrency token.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn the_token_must_name_the_write_the_test_holds() {
    let emu = emulator();
    let policy_arn = create_policy(&emu, "concurrency token policy").await;
    let ids = minted_tests(&emu, &policy_arn, 2).await;
    let victim = ids[0].clone();
    let survivor = ids[1].clone();

    let reported = stored_updated_at(&emu, &policy_arn, &victim).await;
    let held = epoch_millis("the stored instant", &reported);
    let before = get_test_case(&emu, &policy_arn, &victim).await.body;

    // Rows differ along the DISTANCE and the direction from the write the test holds, from
    // one millisecond either side out to the epoch and far into the future.
    let rows: Vec<(&str, String)> = vec![
        ("one millisecond earlier", at_millis("earlier", held - 1)),
        ("one millisecond later", at_millis("later", held + 1)),
        ("one second earlier", at_millis("a second earlier", held - 1_000)),
        ("one second later", at_millis("a second later", held + 1_000)),
        ("a day earlier", at_millis("a day earlier", held - 86_400_000)),
        ("the epoch", at_millis("the epoch", 0)),
        // 2099-12-31T23:59:59Z, far past any instant this store can mint.
        ("far in the future", at_millis("far future", 4_102_444_799_000)),
    ];

    for (case, token) in &rows {
        let refused = delete_test_case(&emu, &policy_arn, &victim, token).await;
        assert_eq!(
            (refused.status, refused.error.as_deref()),
            (400, Some("ConflictException")),
            "case {case:?}: {token:?} is not the write the test holds, so the delete must be \
             refused, got {} {:?}",
            refused.status,
            refused.body
        );
        let read = get_test_case(&emu, &policy_arn, &victim).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: a refused delete must leave the test readable, got {} {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.body, before,
            "case {case:?}: a refused delete must change nothing about the test"
        );
    }

    // The token the test does hold deletes it, so every row above was refused on the token
    // and not on the pair.
    let accepted = delete_test_case(&emu, &policy_arn, &victim, &reported).await;
    assert_accepted("the stored instant", &accepted);
    assert_eq!(
        listed_ids("after the accepted delete", &emu, &policy_arn).await,
        vec![survivor],
        "case after the accepted delete: only the test the delete did not address is left"
    );
}

// ---------------------------------------------------------------------------------------
// 3. A test is addressed by its policy AND its id.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_delete_is_addressed_by_its_policy_and_its_id() {
    let emu = emulator();
    let policy_arn = create_policy(&emu, "owning policy").await;
    let other_policy_arn = create_policy(&emu, "other policy").await;
    let ids = minted_tests(&emu, &policy_arn, 1).await;
    let id = ids[0].clone();
    let token = stored_updated_at(&emu, &policy_arn, &id).await;

    // Rows: an ARN of a policy nothing minted, a well-formed ARN of another region, and the
    // policy's own versioned ARN — `AutomatedReasoningPolicyArn` admits a `:<version>`
    // suffix and this policy has no versions. None names a policy of this account, so there
    // is no policy to delete a test from.
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
        let refused = delete_test_case(&emu, arn, &id, &token).await;
        assert_eq!(
            (refused.status, refused.error.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?}: {arn:?} names no policy of this account, so the delete must answer \
             404, got {} {:?}",
            refused.status,
            refused.body
        );
    }

    // Rows: an id nothing minted, and the minted id under ANOTHER policy this account does
    // hold. Neither names a test of the policy addressed, so each is the idempotent success
    // of a test that is already not there.
    let unknown_tests: Vec<(&str, &str, String)> = vec![
        (
            "an id nothing minted",
            policy_arn.as_str(),
            "AAAAAAAAAAAA".to_string(),
        ),
        (
            "the minted id under another policy",
            other_policy_arn.as_str(),
            id.clone(),
        ),
    ];
    for (case, arn, test_case_id) in &unknown_tests {
        let reply = delete_test_case(&emu, arn, test_case_id, &token).await;
        assert_accepted(case, &reply);
    }

    // The pair the create minted is untouched by every row above, and deleting THAT pair
    // removes it.
    let read = get_test_case(&emu, &policy_arn, &id).await;
    assert_eq!(
        read.status, 200,
        "case the minted pair: the test must still read back, got {} {:?}",
        read.status, read.body
    );
    assert_accepted(
        "the minted pair",
        &delete_test_case(&emu, &policy_arn, &id, &token).await,
    );
    assert!(
        listed_ids("the minted pair", &emu, &policy_arn)
            .await
            .is_empty(),
        "case the minted pair: the policy holds no test once its only test is deleted"
    );
}

// ---------------------------------------------------------------------------------------
// 4. A deleted test releases the idempotency token that minted it.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_deleted_test_releases_its_client_request_token() {
    // Tokens differing in the characters `IdempotencyToken`
    // (`^[a-zA-Z0-9]([-a-zA-Z0-9]{0,254}[a-zA-Z0-9])?$`) admits.
    let tokens = ["token1", "A-token-2", "0123456789abcdef"];

    let emu = emulator();
    let policy_arn = create_policy(&emu, "token release policy").await;

    for (index, client_token) in tokens.iter().enumerate() {
        let body = json!({
            "guardContent": format!("claim {index}"),
            "expectedAggregatedFindingsResult": CHECK_RESULTS[index % CHECK_RESULTS.len()],
            "clientRequestToken": client_token,
        });

        let first = create_test_case(&emu, &policy_arn, body.clone()).await;
        assert_eq!(
            first.status, 200,
            "case {client_token:?}: the first create must answer 200, got {} {:?}",
            first.status, first.body
        );
        let minted = first.text("testCaseId");
        let last_updated_at = stored_updated_at(&emu, &policy_arn, &minted).await;
        assert_accepted(
            client_token,
            &delete_test_case(&emu, &policy_arn, &minted, &last_updated_at).await,
        );

        // The token no longer names a stored test, so replaying it mints a NEW one rather
        // than answering with the test that is gone.
        let replay = create_test_case(&emu, &policy_arn, body).await;
        assert_eq!(
            replay.status, 200,
            "case {client_token:?}: a create replaying the token of a deleted test must answer \
             200, got {} {:?}",
            replay.status, replay.body
        );
        let reminted = replay.text("testCaseId");
        assert_ne!(
            reminted, minted,
            "case {client_token:?}: the replay must mint a new test, not name the deleted one"
        );
        let read = get_test_case(&emu, &policy_arn, &reminted).await;
        assert_eq!(
            read.status, 200,
            "case {client_token:?}: the re-minted test must read back, got {} {:?}",
            read.status, read.body
        );
        assert_eq!(
            listed_ids(client_token, &emu, &policy_arn).await,
            vec![reminted.clone()],
            "case {client_token:?}: the policy holds exactly the test the replay minted"
        );

        // Leave the policy empty for the next row, so each token is driven against a store
        // holding only what that row created.
        let last_updated_at = stored_updated_at(&emu, &policy_arn, &reminted).await;
        assert_accepted(
            client_token,
            &delete_test_case(&emu, &policy_arn, &reminted, &last_updated_at).await,
        );
    }
}
