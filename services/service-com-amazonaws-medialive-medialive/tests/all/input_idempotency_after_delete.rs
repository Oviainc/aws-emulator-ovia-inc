// SPDX-License-Identifier: CC0-1.0

//! How long an input `RequestId` stays a replay: a MediaLive input create
//! answers a retry with the input its `@idempotencyToken` minted only while that
//! input is still one the account may use, and a partner create still has to be
//! aimed at an input the account may still change.
//!
//! `DeleteInput` is a tombstone delete — the record stays readable, in the
//! transient `DELETING` state and then in the terminal `DELETED` one
//! (`smithy_query(query="shape", shape="InputState")`) — so a retry carrying the
//! token of an input whose delete has been accepted cannot be answered with that
//! record: it names nothing a caller can use or change. The retried create makes
//! an input of its own instead, and a retry aimed at a primary whose delete has
//! been accepted is refused exactly as a first attempt would be.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.
//!
//! The suites are TABLES: the recorded partner-input retry is ONE row.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::wait_for_the_clock;
use crate::support::resolved_input_body;
use crate::wire::{call, error_code, error_message, medialive};

/// The collection `CreateInput` and `ListInputs` are bound to, and the prefix
/// `CreatePartnerInput`'s `/prod/inputs/{InputId}/partners` extends
/// (`smithy_query(query="operation",
/// operation="CreateInput,CreatePartnerInput,ListInputs")`).
const INPUTS: &str = "/prod/inputs";

/// `CreateInput`, pointed at resources this account really holds, which has to
/// succeed.
async fn created_input(router: &MultiServiceRouter, case: &str, body: Value) -> Value {
    let body = resolved_input_body(router, body).await;
    let (status, response) = call(router, "POST", INPUTS, Some(body)).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected CreateInput to answer 201, got {status}: {response}"
    );
    response["input"].clone()
}

/// `CreatePartnerInput`: `POST /prod/inputs/{InputId}/partners`.
async fn create_partner(
    router: &MultiServiceRouter,
    input_id: &str,
    body: Option<Value>,
) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("{INPUTS}/{input_id}/partners"),
        body,
    )
    .await
}

/// `CreatePartnerInput` that has to succeed, answering the partner it created.
async fn created_partner(
    router: &MultiServiceRouter,
    case: &str,
    input_id: &str,
    body: Option<Value>,
) -> Value {
    let (status, response) = create_partner(router, input_id, body).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected CreatePartnerInput to answer 201, got {status}: {response}"
    );
    response["input"].clone()
}

async fn describe_input(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(router, "GET", &format!("{INPUTS}/{id}"), None).await
}

/// The `State` an input reports, as a client reads it.
async fn state_of(router: &MultiServiceRouter, case: &str, id: &str) -> String {
    let (status, described) = describe_input(router, id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DescribeInput({id}) to answer 200, got {status}: {described}"
    );
    described["state"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// `DeleteInput`, which has to be accepted.
async fn delete_input(router: &MultiServiceRouter, case: &str, id: &str) {
    let (status, deleted) = call(router, "DELETE", &format!("{INPUTS}/{id}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DeleteInput to answer 200, got {status}: {deleted}"
    );
}

/// Every input id this account holds, sorted, as `ListInputs` reports them.
async fn held_ids(router: &MultiServiceRouter, case: &str) -> Vec<String> {
    let (status, response) = call(router, "GET", INPUTS, None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListInputs to answer 200, got {status}: {response}"
    );
    let mut ids: Vec<String> = response["inputs"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|input| input["id"].as_str().unwrap_or_default().to_string())
        .collect();
    ids.sort();
    ids
}

/// The `inputPartnerIds` an input reports, sorted.
fn partner_ids(input: &Value) -> Vec<String> {
    let mut ids: Vec<String> = input["inputPartnerIds"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|id| id.as_str().unwrap_or_default().to_string())
        .collect();
    ids.sort();
    ids
}

/// The primary every row creates first: the input a partner is made FOR, and
/// (for the standalone rows) just another input of the account.
fn primary_body(name: &str) -> Value {
    json!({
        "name": name,
        "type": "AWS_CDI",
        "destinations": [{ "streamName": "cdi-a" }],
    })
}

/// Which create a row's token is spent on. Both creates of an input carry a
/// `RequestId` (`smithy_query(query="operation",
/// operation="CreateInput,CreatePartnerInput")`), so the rule is asserted over
/// both.
#[derive(Clone, Copy, Debug)]
enum Spend {
    /// `CreateInput` of an input of this `InputType`.
    Standalone(&'static str),
    /// `CreatePartnerInput` for the row's primary.
    Partner,
}

/// The `CreateInput` body for one of the types the rows name. The per-type
/// member each body fills comes from the `CreateInput` input signature and the
/// `InputType` enum (`smithy_query(query="shape", shape="InputType")`); a
/// pulled-from type is given its redundant source pair by the shared harness.
fn body_for(input_type: &str, name: &str) -> Value {
    match input_type {
        "UDP_PUSH" | "RTP_PUSH" => json!({
            "name": name,
            "type": input_type,
            "destinations": [{}],
        }),
        "AWS_CDI" => json!({
            "name": name,
            "type": input_type,
            "destinations": [{ "streamName": "cdi-a" }],
        }),
        _ => json!({ "name": name, "type": input_type }),
    }
}

/// Spend `token` on the create `spend` names and answer the input it created.
async fn spend(
    router: &MultiServiceRouter,
    case: &str,
    primary_id: &str,
    spend: Spend,
    name: &str,
    token: &str,
) -> Value {
    match spend {
        Spend::Standalone(input_type) => {
            let mut body = body_for(input_type, name);
            body["requestId"] = json!(token);
            created_input(router, case, body).await
        }
        Spend::Partner => {
            created_partner(router, case, primary_id, Some(json!({ "requestId": token }))).await
        }
    }
}

/// Rule: a create's `@idempotencyToken` answers a retry with the input it minted
/// only while that input is still one a caller may use — accepting that input's
/// delete ends the token's window, in the transient `DELETING` half as much as in
/// the terminal `DELETED` one — so the retry mints a NEW input and resurrects
/// nothing.
///
/// The rows differ along what the rule generalises over: WHICH create spent the
/// token (the standalone `CreateInput` and `CreatePartnerInput`), the input TYPE
/// the standalone rows create, and WHICH half of the delete lifecycle the retry
/// lands in (before the clock has made the deletion due, and after). The issue's
/// recorded case is the partner row retried after the clock.
#[tokio::test]
async fn a_deleted_input_is_not_resurrected_by_its_idempotency_token() {
    // (case, the create that spends the token, whether the retry waits for the
    //  clock to make the deletion due)
    let table: [(&str, Spend, bool); 6] = [
        (
            "a url pull create retried inside the DELETING window",
            Spend::Standalone("URL_PULL"),
            false,
        ),
        (
            "a url pull create retried once the delete settled",
            Spend::Standalone("URL_PULL"),
            true,
        ),
        (
            "a udp push create retried once the delete settled",
            Spend::Standalone("UDP_PUSH"),
            true,
        ),
        (
            "an mp4 file create retried inside the DELETING window",
            Spend::Standalone("MP4_FILE"),
            false,
        ),
        (
            "a partner create retried inside the DELETING window",
            Spend::Partner,
            false,
        ),
        (
            "a partner create retried once the delete settled",
            Spend::Partner,
            true,
        ),
    ];

    for (case, spent_on, wait) in table {
        let router = medialive();
        let primary = created_input(&router, case, primary_body("retry-primary")).await;
        let primary_id = primary["id"].as_str().unwrap_or_default().to_string();

        let token = "retry-token";
        let first = spend(&router, case, &primary_id, spent_on, "first", token).await;
        let first_id = first["id"].as_str().unwrap_or_default().to_string();

        // While the input is live the token IS a replay: the same create carrying
        // it answers with that input and creates nothing new.
        let replayed = spend(&router, case, &primary_id, spent_on, "first", token).await;
        assert_eq!(
            replayed["id"],
            json!(first_id),
            "case {case:?}: expected a live input's token to replay {first_id:?}, got {:?}",
            replayed["id"]
        );
        assert_eq!(
            held_ids(&router, case).await.len(),
            2,
            "case {case:?}: expected the replay to have created nothing, got {:?}",
            held_ids(&router, case).await
        );

        delete_input(&router, case, &first_id).await;
        if wait {
            wait_for_the_clock();
        }
        let dead_state = state_of(&router, case, &first_id).await;
        let expected_state = if wait { "DELETED" } else { "DELETING" };
        assert_eq!(
            dead_state, expected_state,
            "case {case:?}: expected the deleted input to report {expected_state}, got {dead_state:?}"
        );

        // The window is over: the same create carrying the same token mints an
        // input of its own.
        let retried = spend(&router, case, &primary_id, spent_on, "second", token).await;
        let retried_id = retried["id"].as_str().unwrap_or_default().to_string();
        assert_ne!(
            retried_id, first_id,
            "case {case:?}: expected a freshly minted input, got the deleted {first_id:?}"
        );
        assert!(
            !["DELETING", "DELETED"].contains(&retried["state"].as_str().unwrap_or_default()),
            "case {case:?}: expected the retry to answer with a live input, got {retried}"
        );

        // The retry really stored that input, and left the deleted record exactly
        // as the clock had it.
        let retried_state = state_of(&router, case, &retried_id).await;
        assert!(
            !["DELETING", "DELETED"].contains(&retried_state.as_str()),
            "case {case:?}: expected the stored retry to be live, got {retried_state:?}"
        );
        assert_eq!(
            state_of(&router, case, &first_id).await,
            dead_state,
            "case {case:?}: expected the deleted input to stay {dead_state:?}"
        );
        let mut expected_held = vec![primary_id.clone(), first_id.clone(), retried_id.clone()];
        expected_held.sort();
        assert_eq!(
            held_ids(&router, case).await,
            expected_held,
            "case {case:?}: expected the account to hold the primary, the deleted input and the \
             retry's own, got {:?}",
            held_ids(&router, case).await
        );

        // A partner the retry minted is a partner of the input the request named.
        if matches!(spent_on, Spend::Partner) {
            let (_, described_primary) = describe_input(&router, &primary_id).await;
            assert!(
                partner_ids(&described_primary).contains(&retried_id),
                "case {case:?}: expected the primary to name its new partner {retried_id:?}, got {:?}",
                partner_ids(&described_primary)
            );
            assert_eq!(
                partner_ids(&retried),
                vec![primary_id.clone()],
                "case {case:?}: expected the new partner to name its primary, got {:?}",
                partner_ids(&retried)
            );
        }
    }
}

/// Rule: a partner is created FOR the input the request names, so that input must
/// be one the account still holds and may still change — a `RequestId` cannot
/// turn a request this operation refuses into one it serves, so a retry aimed at
/// an input whose delete has been accepted is refused exactly as a first attempt
/// is, with this operation's modelled `BadRequestException` (400, the only client
/// refusal besides the authorization and throttling ones that
/// `smithy_query(query="operation", operation="CreatePartnerInput")` declares),
/// and creates nothing.
///
/// The rows differ along what the rule generalises over: whether the retry's
/// token was already spent on a partner of this very input, was never spent at
/// all, or is absent, and which half of the delete lifecycle the call lands in.
#[tokio::test]
async fn a_partner_create_for_a_deleted_input_is_refused_whatever_token_it_carries() {
    // (case, the token the refused call carries, whether that token was already
    //  spent on a partner of this input, whether the call waits for the clock)
    let table: [(&str, Option<&str>, bool, bool); 5] = [
        (
            "a token already spent on a partner of this input, inside the DELETING window",
            Some("token-a"),
            true,
            false,
        ),
        (
            "a token already spent on a partner of this input, once the delete settled",
            Some("token-a"),
            true,
            true,
        ),
        (
            "a token this account has never spent",
            Some("never-spent"),
            false,
            true,
        ),
        ("no token at all, inside the DELETING window", None, false, false),
        ("no token at all, once the delete settled", None, false, true),
    ];

    for (case, token, spent_first, wait) in table {
        let router = medialive();
        let primary = created_input(&router, case, primary_body("refused-primary")).await;
        let primary_id = primary["id"].as_str().unwrap_or_default().to_string();

        if spent_first {
            let spent = token.expect("a row that spends a token carries one");
            created_partner(
                &router,
                case,
                &primary_id,
                Some(json!({ "requestId": spent })),
            )
            .await;
        }

        delete_input(&router, case, &primary_id).await;
        if wait {
            wait_for_the_clock();
        }
        let before = held_ids(&router, case).await;

        let body = token.map(|token| json!({ "requestId": token }));
        let (status, refused) = create_partner(&router, &primary_id, body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the partner create to be refused, got {status}: {refused}"
        );
        assert_eq!(
            error_code(status, &refused),
            "BadRequestException",
            "case {case:?}: expected the operation's own BadRequestException, got {refused}"
        );
        assert_eq!(
            error_message(&refused),
            format!("inputId={primary_id} already deleted"),
            "case {case:?}: expected the refusal to name the member and value the request carried, \
             got {:?}",
            error_message(&refused)
        );

        // A refused create stored nothing.
        assert_eq!(
            held_ids(&router, case).await,
            before,
            "case {case:?}: expected the refusal to create nothing, got {:?}",
            held_ids(&router, case).await
        );
    }
}
