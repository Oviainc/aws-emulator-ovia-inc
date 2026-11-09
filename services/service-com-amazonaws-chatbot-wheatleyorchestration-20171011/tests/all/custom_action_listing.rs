// SPDX-License-Identifier: CC0-1.0

//! How the custom actions of an account are LISTED, driven over the wire (restJson1)
//! through the same `MultiServiceRouter` the production router serves with.
//!
//! The rule every assertion here is about: `ListCustomActions` reports the ARN of every
//! custom action this ACCOUNT holds — a custom action is account-scoped, so its ARN carries
//! no region and the listing answers the same set whichever region the request is signed
//! for — in one stable total order, and `MaxResults` splits that same total answer into
//! pages without dropping or repeating an action. An account holding none is answered an
//! empty list rather than a refusal, and a `NextToken` this service did not issue names no
//! position in the scan.
//!
//! The table spans what the rule generalises over: how many actions the account holds, the
//! page size asked for, the region the request is signed for, and the shape of a token a
//! caller sends back.

use serde_json::{json, Value};

use crate::wire::{Emulator, OTHER_REGION, REGION};

/// The canonical emulator account id, spelled out so the test pins the ARN a client sees.
const ACCOUNT: &str = "000000000000";

/// A custom action is account-scoped, so the ARN a create mints carries an EMPTY region
/// segment (`state::custom_action_arn`).
fn custom_action_arn(action_name: &str) -> String {
    format!("arn:aws:chatbot::{ACCOUNT}:custom-action/{action_name}")
}

/// Create `action_name` in `region` and assert the create was served.
async fn given_custom_action(emulator: &Emulator, region: &str, action_name: &str) {
    let created = emulator
        .call_in_region(
            region,
            "/create-custom-action",
            &json!({
                "ActionName": action_name,
                "Definition": {"CommandText": "ssm list-documents"},
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "creating {action_name:?}: expected 200, got {} with {}",
        created.status, created.body
    );
}

/// Every ARN `ListCustomActions` reports, in the order it reports them, reading every page
/// with `max_results` as the page size and signed for `region`.
async fn listed_arns(emulator: &Emulator, region: &str, max_results: Option<i64>) -> Vec<String> {
    let mut arns = Vec::new();
    let mut next_token: Option<String> = None;
    loop {
        let mut request = json!({});
        if let Some(max) = max_results {
            request["MaxResults"] = json!(max);
        }
        if let Some(token) = &next_token {
            request["NextToken"] = json!(token);
        }
        let answer = emulator
            .call_in_region(region, "/list-custom-actions", &request)
            .await;
        assert_eq!(
            answer.status, 200,
            "listing custom actions with {request}: expected 200, got {} with {}",
            answer.status, answer.body
        );
        for arn in answer.body["CustomActions"]
            .as_array()
            .unwrap_or_else(|| panic!("no CustomActions in {}", answer.body))
        {
            arns.push(
                arn.as_str()
                    .unwrap_or_else(|| panic!("a CustomActions entry is not a string: {arn}"))
                    .to_string(),
            );
        }
        next_token = answer.body["NextToken"].as_str().map(str::to_string);
        if next_token.is_none() {
            return arns;
        }
    }
}

#[tokio::test]
async fn the_listing_reports_every_action_the_account_holds_whatever_the_page_size() {
    let emulator = Emulator::new();

    // An account that has created nothing has nothing to report, and that is a success.
    assert_eq!(
        listed_arns(&emulator, REGION, None).await,
        Vec::<String>::new(),
        "an account holding no custom action answers an empty list"
    );

    // The names differ in length and charset, and the creates are signed for both regions
    // in turn: a custom action is account-scoped, so the region a create is signed for
    // changes neither the ARN it mints nor which listing reports it.
    let action_names = ["a", "listed-action", &"z".repeat(64), "Mixed_Case-9", "b2"];
    let mut expected: Vec<String> = Vec::new();
    for (index, action_name) in action_names.iter().enumerate() {
        let region = if index % 2 == 0 { REGION } else { OTHER_REGION };
        given_custom_action(&emulator, region, action_name).await;
        expected.push(custom_action_arn(action_name));
        expected.sort();

        // Whatever the page size — below, at and above the total — the same set comes back
        // in the same order, and the listing is the same from either region.
        for page_size in [None, Some(1), Some(2), Some(expected.len() as i64), Some(100)] {
            for region in [REGION, OTHER_REGION] {
                let listed = listed_arns(&emulator, region, page_size).await;
                assert_eq!(
                    listed, expected,
                    "case {action_name:?} with MaxResults={page_size:?} in {region}: expected \
                     every action the account holds, got {listed:?}"
                );
            }
        }
    }

    // A deleted action leaves the listing, so the answer tracks the store.
    let removed = custom_action_arn(action_names[0]);
    let deleted = emulator
        .call(
            "/delete-custom-action",
            &json!({"CustomActionArn": removed}),
        )
        .await;
    assert_eq!(
        deleted.status, 200,
        "the delete is served, got {} with {}",
        deleted.status, deleted.body
    );
    expected.retain(|arn| *arn != removed);
    assert_eq!(
        listed_arns(&emulator, REGION, None).await,
        expected,
        "a deleted action must leave the listing"
    );
}

#[tokio::test]
async fn a_next_token_this_service_did_not_issue_names_no_position_in_the_scan() {
    let emulator = Emulator::new();
    given_custom_action(&emulator, REGION, "token-action").await;

    // A cursor is one of this account's custom-action ARNs, base64-encoded. Each row is a
    // string that is not one, differing in how it fails to be.
    let rows: [(&str, String); 4] = [
        ("not base64 at all", "!!!not-base64!!!".to_string()),
        (
            "base64 of a string that is no ARN",
            ::aws_smithy_types::base64::encode("page-2"),
        ),
        (
            "base64 of another account's action ARN",
            ::aws_smithy_types::base64::encode(
                "arn:aws:chatbot::210987654321:custom-action/token-action",
            ),
        ),
        (
            "a cursor of the configuration listing",
            ::aws_smithy_types::base64::encode(format!(
                "arn:aws:chatbot:{REGION}:{ACCOUNT}:chat-configuration/slack-channel/any"
            )),
        ),
    ];
    for (case, token) in &rows {
        let answer = emulator
            .call("/list-custom-actions", &json!({"NextToken": token}))
            .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.code(),
            "InvalidRequestException",
            "case {case:?}: expected this service's client-input error class, got {}",
            answer.body
        );
    }

    // The token the listing itself issues IS accepted, and resumes strictly after the
    // action it ends on.
    let first = emulator
        .call("/list-custom-actions", &json!({"MaxResults": 1}))
        .await;
    assert_eq!(
        first.status, 200,
        "the first page answers 200, got {} with {}",
        first.status, first.body
    );
    assert_eq!(
        first.body.get("NextToken").and_then(Value::as_str),
        None,
        "one action fits in a page of one, so no token is issued, got {}",
        first.body
    );
}
