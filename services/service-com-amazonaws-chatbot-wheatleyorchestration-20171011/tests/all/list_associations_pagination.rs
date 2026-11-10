// SPDX-License-Identifier: CC0-1.0

//! How `ListAssociations` PAGES the resources linked to a channel configuration, driven
//! over the wire (restJson1) through the same `MultiServiceRouter` the production router
//! serves with.
//!
//! The rules every assertion here is about:
//!
//! * **The page size is the modelled one.** `MaxResults` is `@range min 1 max 100`
//!   (`smithy_query(query="operation", operation="ListAssociations")`), so a value outside
//!   that range is refused as this service's modelled client-input failure
//!   `InvalidRequestException` (HTTP 400) and nothing is reported, while each published
//!   bound is served.
//! * **A page never carries more than it was asked for, and a token is issued exactly when
//!   the scan has more to report.** Following the tokens visits every resource linked to the
//!   configuration exactly once, in one stable total order, whatever the page size; the last
//!   page of a scan carries no token, and a caller that names no page size still gets the
//!   whole answer while the total is within the page the service bounds a response to.
//! * **A listing reports and does not consume.** `ListAssociations` is `@readonly`, so
//!   repeating a listing — the whole walk, or one page by its own token — answers exactly
//!   what it answered before, and the links are all still there afterwards.
//!
//! The tables span what those rules generalise over: the value `MaxResults` carries, how
//! many resources the configuration holds (none, one, several), and the page size a walk
//! asks for (every size from one to past the total, plus the published maximum) — rather
//! than one recorded request.

use serde_json::{json, Value};
use service_com_amazonaws_chatbot_wheatleyorchestration_20171011 as chatbot;

use crate::wire::{Answer, Emulator, REGION};

/// The canonical emulator account id (`emulator_server::EMULATOR_ACCOUNT_ID`), spelled out
/// so the test pins the ARN a client actually sees.
const ACCOUNT: &str = "000000000000";

/// The Slack workspace the channel configurations below are created in: one of the
/// authorizations the store seeds (`state::seed_slack_workspaces`), standing in for the
/// one-time Slack client handshake no operation of this model performs.
const SEEDED_SLACK_TEAM_ID: &str = "TEMULATOR001";

/// An `Arn` every create below carries.
const IAM_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/chatbot-role";

/// The modelled client-input error class a constraint violation is refused with.
const INVALID_REQUEST_EXCEPTION: &str = "InvalidRequestException";

/// The page a response is bounded to when the caller names no `MaxResults`.
const DEFAULT_PAGE_SIZE: i64 = chatbot::state::MAX_PAGE_SIZE as i64;

/// Create the Slack channel configuration `name` and answer its `ChatConfigurationArn`.
async fn given_slack_channel_configuration(emulator: &Emulator, name: &str) -> String {
    let created = emulator
        .call(
            "/create-slack-channel-configuration",
            &json!({
                "ConfigurationName": name,
                "SlackTeamId": SEEDED_SLACK_TEAM_ID,
                "SlackChannelId": "C0123ABCD",
                "IamRoleArn": IAM_ROLE_ARN,
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "creating the channel configuration {name:?}: expected 200, got {} with {}",
        created.status, created.body
    );
    created.body["ChannelConfiguration"]["ChatConfigurationArn"]
        .as_str()
        .unwrap_or_else(|| panic!("creating {name:?}: no ChatConfigurationArn in {}", created.body))
        .to_string()
}

/// Create the custom action `action_name` and answer the ARN it was minted under.
async fn given_custom_action(emulator: &Emulator, action_name: &str) -> String {
    let created = emulator
        .call(
            "/create-custom-action",
            &json!({
                "ActionName": action_name,
                "Definition": {"CommandText": "ssm list-documents"},
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "creating the custom action {action_name:?}: expected 200, got {} with {}",
        created.status, created.body
    );
    created.body["CustomActionArn"]
        .as_str()
        .unwrap_or_else(|| panic!("creating {action_name:?}: no CustomActionArn in {}", created.body))
        .to_string()
}

/// Link `resource` to `chat_configuration` and assert the associate was served.
async fn given_association(emulator: &Emulator, chat_configuration: &str, resource: &str) {
    let associated = emulator
        .call(
            "/associate-to-configuration",
            &json!({"ChatConfiguration": chat_configuration, "Resource": resource}),
        )
        .await;
    assert_eq!(
        associated.status, 201,
        "linking {resource:?} to {chat_configuration:?}: expected 201, got {} with {}",
        associated.status, associated.body
    );
}

/// One `ListAssociations` call: `max_results` as the page size, `next_token` as the cursor.
async fn list_associations(
    emulator: &Emulator,
    chat_configuration: &str,
    max_results: Option<i64>,
    next_token: Option<&str>,
) -> Answer {
    let mut request = json!({"ChatConfiguration": chat_configuration});
    if let Some(max) = max_results {
        request["MaxResults"] = json!(max);
    }
    if let Some(token) = next_token {
        request["NextToken"] = json!(token);
    }
    emulator.call("/list-associations", &request).await
}

/// The `Resource` members of an `Associations` list.
fn resources_of(body: &Value, case: &str) -> Vec<String> {
    body["Associations"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no Associations list in {body}"))
        .iter()
        .map(|association| {
            association["Resource"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no Resource in {association}"))
                .to_string()
        })
        .collect()
}

/// Walk every page of the associations of `chat_configuration` with `max_results` as the
/// page size, asserting the per-page invariants, and answer the pages in order.
async fn walk_pages(
    emulator: &Emulator,
    chat_configuration: &str,
    max_results: Option<i64>,
    case: &str,
) -> Vec<Vec<String>> {
    let page_size = max_results.unwrap_or(DEFAULT_PAGE_SIZE);
    let mut pages: Vec<Vec<String>> = Vec::new();
    let mut next_token: Option<String> = None;
    loop {
        let answer =
            list_associations(emulator, chat_configuration, max_results, next_token.as_deref())
                .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?} page {}: expected HTTP 200, got {} with {}",
            pages.len(),
            answer.status,
            answer.body
        );
        let page = resources_of(&answer.body, case);
        assert!(
            page.len() as i64 <= page_size,
            "case {case:?} page {}: a page carries at most {page_size} resources, got {}",
            pages.len(),
            page.len()
        );
        let token = answer.body["NextToken"].as_str().map(str::to_string);
        if token.is_some() {
            // A token means the scan has more to report, which it only can once the page it
            // just filled was as large as the caller allowed.
            assert_eq!(
                page.len() as i64,
                page_size,
                "case {case:?} page {}: a token is issued only on a full page, got {} \
                 resources in {}",
                pages.len(),
                page.len(),
                answer.body
            );
        }
        pages.push(page);
        match token {
            None => return pages,
            Some(token) => next_token = Some(token),
        }
        assert!(
            pages.len() < 32,
            "case {case:?}: the walk issued {} pages without ending",
            pages.len()
        );
    }
}

/// The resources of every page of a walk, concatenated.
fn flattened(pages: &[Vec<String>]) -> Vec<String> {
    pages.iter().flatten().cloned().collect()
}

#[tokio::test]
async fn a_max_results_outside_the_modelled_range_is_refused_and_each_bound_is_served() {
    let emulator = Emulator::new();
    let chat_configuration = given_slack_channel_configuration(&emulator, "assoc-bounds").await;
    let resource = given_custom_action(&emulator, "bounds-probe").await;
    given_association(&emulator, &chat_configuration, &resource).await;

    // `MaxResults` is `@range min 1 max 100`: each row is a value outside it — both
    // published bounds crossed by one, and further out in each direction.
    let refused: [(&str, i64); 5] = [
        ("one below the range minimum", 0),
        ("negative", -1),
        ("far below the range minimum", -100),
        ("one above the range maximum", 101),
        ("far above the range maximum", 100_000),
    ];
    for (case, max_results) in refused {
        let answer = list_associations(&emulator, &chat_configuration, Some(max_results), None).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?} (MaxResults={max_results}): expected HTTP 400, got {} with {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.code(),
            INVALID_REQUEST_EXCEPTION,
            "case {case:?} (MaxResults={max_results}): expected the modelled \
             {INVALID_REQUEST_EXCEPTION}, got {:?} in {}",
            answer.code(),
            answer.body
        );
        assert_eq!(
            answer.error_type.as_deref(),
            Some(INVALID_REQUEST_EXCEPTION),
            "case {case:?} (MaxResults={max_results}): expected x-amzn-errortype \
             {INVALID_REQUEST_EXCEPTION}, got {:?}",
            answer.error_type
        );
    }

    // Each row is a value INSIDE the range — the two published bounds and one between them
    // — and is served the one resource linked, with no token, since nothing is left over.
    let served: [(&str, Option<i64>); 4] = [
        ("the range minimum", Some(1)),
        ("between the bounds", Some(7)),
        ("the range maximum", Some(100)),
        ("no page size at all", None),
    ];
    for (case, max_results) in served {
        let answer = list_associations(&emulator, &chat_configuration, max_results, None).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {} with {}",
            answer.status, answer.body
        );
        assert_eq!(
            resources_of(&answer.body, case),
            vec![resource.clone()],
            "case {case:?}: expected the one linked resource, got {}",
            answer.body
        );
        assert!(
            answer.body["NextToken"].is_null(),
            "case {case:?}: a page that exhausted the scan carries no token, got {}",
            answer.body
        );
    }
}

#[tokio::test]
async fn every_page_size_reports_the_same_links_exactly_once_and_ends_without_a_token() {
    let emulator = Emulator::new();

    // Each row is a configuration holding one more resource than the row before, so the
    // claim below is made over a scan that fits in a page, one that is exactly a page and
    // ones that need several — and the action names differ in length and charset, so what
    // is reported cannot follow from the spelling of one ARN.
    let action_names = ["a", "paged-action", "Mixed_Case-9", &"z".repeat(64), "b-2"];
    let mut resources: Vec<String> = Vec::new();
    for (count, action_name) in action_names.iter().enumerate() {
        resources.push(given_custom_action(&emulator, action_name).await);
        let chat_configuration =
            given_slack_channel_configuration(&emulator, &format!("assoc-paged-{count}")).await;
        for resource in &resources {
            given_association(&emulator, &chat_configuration, resource).await;
        }
        let mut expected = resources.clone();
        expected.sort();

        // Every page size from one resource to past the total, plus the published maximum
        // and no page size at all: the SAME total answer, split differently.
        let page_sizes: Vec<Option<i64>> = (1..=(resources.len() as i64 + 2))
            .map(Some)
            .chain([Some(100), None])
            .collect();
        for max_results in page_sizes {
            let case = format!("{} linked, MaxResults={max_results:?}", resources.len());
            let pages = walk_pages(&emulator, &chat_configuration, max_results, &case).await;
            assert_eq!(
                flattened(&pages),
                expected,
                "case {case:?}: expected every link exactly once in one stable order, got {} \
                 page(s): {pages:?}",
                pages.len()
            );
            // The walk ends because the last page carried no token, so it is the only page
            // that may be short — and a scan is never split into more pages than its total
            // needs: a token is issued only while something is left over, so a total that
            // divides evenly into the page size needs no extra, empty page.
            let page_size = max_results.unwrap_or(DEFAULT_PAGE_SIZE);
            let total = expected.len() as i64;
            let expected_pages = if total == 0 {
                1
            } else {
                total.div_euclid(page_size) + i64::from(total % page_size != 0)
            };
            assert_eq!(
                pages.len() as i64,
                expected_pages,
                "case {case:?}: expected {expected_pages} page(s), got {}: {pages:?}",
                pages.len()
            );
        }
    }

    // A configuration this account holds nothing under reports an empty list, at any page
    // size, rather than a refusal.
    let absent =
        format!("arn:aws:chatbot:{REGION}:{ACCOUNT}:chat-configuration/slack-channel/absent");
    for max_results in [Some(1), Some(100), None] {
        let case = format!("no configuration stored, MaxResults={max_results:?}");
        let pages = walk_pages(&emulator, &absent, max_results, &case).await;
        assert_eq!(
            pages,
            vec![Vec::<String>::new()],
            "case {case:?}: expected one empty page, got {pages:?}"
        );
    }
}

#[tokio::test]
async fn a_listing_reports_without_consuming_what_a_later_caller_sees() {
    let emulator = Emulator::new();
    let chat_configuration = given_slack_channel_configuration(&emulator, "assoc-readonly").await;
    let mut expected = Vec::new();
    for action_name in ["read-1", "read-2", "read-3"] {
        let resource = given_custom_action(&emulator, action_name).await;
        given_association(&emulator, &chat_configuration, &resource).await;
        expected.push(resource);
    }
    expected.sort();

    // The first page of a two-at-a-time walk, and the token it ends on.
    let first = list_associations(&emulator, &chat_configuration, Some(2), None).await;
    assert_eq!(
        first.status, 200,
        "the first page is served, got {} with {}",
        first.status, first.body
    );
    let token = first.body["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("a scan with more to report issues a token, got {}", first.body))
        .to_string();

    // Reading the same position again — the first page, and the page the token names —
    // answers exactly what it answered before, however many times it is asked for.
    for attempt in 0..3 {
        let repeated = list_associations(&emulator, &chat_configuration, Some(2), None).await;
        assert_eq!(
            resources_of(&repeated.body, "the first page, repeated"),
            resources_of(&first.body, "the first page"),
            "attempt {attempt}: re-reading the first page must answer the same resources, got {}",
            repeated.body
        );
        assert_eq!(
            repeated.body["NextToken"].as_str(),
            Some(token.as_str()),
            "attempt {attempt}: re-reading the first page must answer the same token, got {}",
            repeated.body
        );

        let resumed =
            list_associations(&emulator, &chat_configuration, Some(2), Some(&token)).await;
        assert_eq!(
            resumed.status, 200,
            "attempt {attempt}: resuming from the token is served, got {} with {}",
            resumed.status, resumed.body
        );
        assert_eq!(
            resources_of(&resumed.body, "the page the token names"),
            expected[2..].to_vec(),
            "attempt {attempt}: expected the links after the cursor, got {}",
            resumed.body
        );
    }

    // And every link is still there afterwards: a read consumed none of them.
    let after = walk_pages(&emulator, &chat_configuration, None, "after the repeated reads").await;
    assert_eq!(
        flattened(&after),
        expected,
        "the links a read reported must still be linked, got {after:?}"
    );
}
