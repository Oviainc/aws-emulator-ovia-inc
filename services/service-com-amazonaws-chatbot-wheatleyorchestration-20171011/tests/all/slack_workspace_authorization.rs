// SPDX-License-Identifier: CC0-1.0

//! The Slack workspace AUTHORIZATION rule, over the wire (restJson1) through the same
//! `MultiServiceRouter` the production router serves with.
//!
//! The rule every assertion here is about: a chat channel configuration can only be
//! created for a chat workspace the calling account holds an authorization for, so a
//! create naming a workspace the account's authorization registry does not hold is
//! refused with a modelled 400 client error whose `message` names the workspace and the
//! account, and persists nothing — while a create naming an authorized one succeeds and
//! takes the workspace's display name from that authorization.
//!
//! The registry is exercised as a CLASS of inputs rather than one recorded request: the
//! refused table spans the published `SlackTeamId` shape (`smithy_query`:
//! `length 1..255`, `pattern ^[0-9A-Z]{1,255}$`) at both bounds and in between, and the
//! accepted table spans the same shape, so the refusal follows from the registry alone
//! and never from the spelling, length or charset of an id.

use serde_json::{json, Value};
use service_com_amazonaws_chatbot_wheatleyorchestration_20171011 as chatbot;

use crate::wire::{Answer, Emulator, OTHER_REGION, REGION};

/// The canonical emulator account id (`emulator_server::EMULATOR_ACCOUNT_ID`), spelled
/// out here so the test pins what a client actually reads in the refusal.
const ACCOUNT: &str = "000000000000";

const IAM_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/chatbot-role";

/// The modelled 400 client errors `CreateSlackChannelConfiguration` declares
/// (`smithy_query` operation=CreateSlackChannelConfiguration: `InvalidRequestException`
/// and `InvalidParameterException`, both `httpError 400` with a lowercase `message`).
const MODELLED_400_ERRORS: [&str; 2] = ["InvalidRequestException", "InvalidParameterException"];

/// The workspace-authorization half of the shared [`Emulator`] harness: the state seam
/// and the Slack-channel calls this suite drives it through.
impl Emulator {
    /// Authorize `slack_team_id` with the account under the display name
    /// `slack_team_name`. This is the out-of-band seam standing in for the one-time
    /// Slack client (OAuth) handshake: no operation in this model performs it.
    fn authorize(&self, slack_team_id: &str, slack_team_name: &str) {
        chatbot::state::authorize_slack_workspace(&*self.engine, slack_team_id, slack_team_name)
            .unwrap_or_else(|| panic!("the shared engine must hold the chatbot store"));
    }

    async fn create(&self, configuration_name: &str, slack_team_id: &str) -> Answer {
        self.create_in_region(REGION, configuration_name, slack_team_id)
            .await
    }

    async fn create_in_region(
        &self,
        region: &str,
        configuration_name: &str,
        slack_team_id: &str,
    ) -> Answer {
        self.call_in_region(
            region,
            "/create-slack-channel-configuration",
            &json!({
                "ConfigurationName": configuration_name,
                "SlackTeamId": slack_team_id,
                "SlackChannelId": "C0123ABCD",
                "IamRoleArn": IAM_ROLE_ARN,
            }),
        )
        .await
    }

    /// Every Slack channel configuration [`REGION`] holds, as the listing reports them.
    async fn configurations(&self) -> Vec<Value> {
        let answer = self
            .call("/describe-slack-channel-configurations", &json!({}))
            .await;
        assert_eq!(
            answer.status, 200,
            "describe configurations: expected 200, got {} with {}",
            answer.status, answer.body
        );
        answer
            .body
            .get("SlackChannelConfigurations")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_else(|| panic!("no SlackChannelConfigurations list in {}", answer.body))
    }

    /// One page of `DescribeSlackWorkspaces`: the workspaces it carries and its token.
    async fn workspaces_page(&self, request: &Value) -> (Vec<Value>, Option<String>) {
        let answer = self.call("/describe-slack-workspaces", request).await;
        assert_eq!(
            answer.status, 200,
            "describe workspaces {request}: expected 200, got {} with {}",
            answer.status, answer.body
        );
        let workspaces = answer
            .body
            .get("SlackWorkspaces")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_else(|| panic!("no SlackWorkspaces list in {}", answer.body));
        let next_token = answer
            .body
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        (workspaces, next_token)
    }

    /// Every authorized workspace, walking the pages `max_results` splits them into.
    async fn all_workspaces(&self, max_results: Option<usize>) -> Vec<Value> {
        let mut all = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut request = serde_json::Map::new();
            if let Some(max) = max_results {
                request.insert("MaxResults".to_string(), json!(max));
            }
            if let Some(cursor) = &token {
                request.insert("NextToken".to_string(), json!(cursor));
            }
            let (page, next) = self.workspaces_page(&Value::Object(request)).await;
            if let Some(max) = max_results {
                assert!(
                    page.len() <= max,
                    "MaxResults {max}: a page carried {} workspaces",
                    page.len()
                );
            }
            all.extend(page);
            match next {
                Some(cursor) => token = Some(cursor),
                None => break,
            }
        }
        all
    }
}

/// A `SlackTeamId` of `length` characters, all `fill` — for the published length bounds.
fn team_id(fill: char, length: usize) -> String {
    std::iter::repeat_n(fill, length).collect()
}

#[tokio::test]
async fn a_create_for_a_workspace_the_account_never_authorized_is_refused_and_persists_nothing() {
    let emulator = Emulator::new();
    // The one workspace this account DOES hold an authorization for; every id below
    // differs from it, so only the registry can tell them apart.
    emulator.authorize("T0123ABCD", "Primary Workspace");

    // Spans the published `SlackTeamId` shape (`length 1..255`,
    // `pattern ^[0-9A-Z]{1,255}$`): both length bounds, letters only, digits only, and
    // an id one character from the authorized one.
    let unauthorized = [
        ("a Slack workspace id's usual shape", "T47ACFD6FF1".to_string()),
        ("the minimum length, one letter", team_id('A', 1)),
        ("the maximum length, 255 letters", team_id('Z', 255)),
        ("digits only", "0123456789".to_string()),
        ("one character from an authorized id", "T0123ABCE".to_string()),
    ];

    for (index, (case, slack_team_id)) in unauthorized.iter().enumerate() {
        let answer = emulator.create(&format!("refused-{index}"), slack_team_id).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {} with {}",
            answer.status, answer.body
        );
        let error_type = answer.error_type.clone().unwrap_or_default();
        assert!(
            MODELLED_400_ERRORS
                .iter()
                .any(|modelled| error_type.ends_with(modelled)),
            "case {case:?}: expected one of {MODELLED_400_ERRORS:?} in x-amzn-errortype, got {:?}",
            answer.error_type
        );
        let message = answer.message();
        assert!(
            message.contains(slack_team_id.as_str()),
            "case {case:?}: the message must name the unauthorized workspace, got {message:?}"
        );
        assert!(
            message.contains(ACCOUNT),
            "case {case:?}: the message must name the account, got {message:?}"
        );
        assert!(
            answer.body.get("ChannelConfiguration").is_none(),
            "case {case:?}: a refused create must carry no ChannelConfiguration, got {}",
            answer.body
        );
    }

    let stored = emulator.configurations().await;
    assert!(
        stored.is_empty(),
        "a refused create must persist nothing, got {stored:?}"
    );
}

#[tokio::test]
async fn a_create_for_an_authorized_workspace_succeeds_and_names_it_from_the_authorization() {
    let emulator = Emulator::new();

    // The same `SlackTeamId` class as the refused table above — the only difference is
    // that these ids are in the authorization registry.
    let authorized = [
        ("a Slack workspace id's usual shape", "T47ACFD6FF2".to_string(), "Recorded Shape"),
        ("the minimum length, one letter", team_id('B', 1), "One Letter"),
        ("the maximum length, 255 letters", team_id('Y', 255), "Longest Id"),
        ("digits only", "9876543210".to_string(), "Digits Only"),
    ];

    for (index, (case, slack_team_id, slack_team_name)) in authorized.iter().enumerate() {
        emulator.authorize(slack_team_id, slack_team_name);
        let configuration_name = format!("authorized-{index}");
        let answer = emulator.create(&configuration_name, slack_team_id).await;
        assert_eq!(
            answer.status, 201,
            "case {case:?}: expected 201, got {} with {}",
            answer.status, answer.body
        );
        let created = answer
            .body
            .get("ChannelConfiguration")
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: no ChannelConfiguration in {}", answer.body));
        assert_eq!(
            created["SlackTeamId"],
            json!(slack_team_id),
            "case {case:?}: expected SlackTeamId {slack_team_id:?}, got {created}"
        );
        assert_eq!(
            created["SlackTeamName"],
            json!(slack_team_name),
            "case {case:?}: the configuration must take its SlackTeamName from the \
             authorization ({slack_team_name:?}), got {created}"
        );

        // Read it back out of the store: the name must come from the authorization, not
        // from the create response.
        let stored = emulator.configurations().await;
        let found = stored
            .iter()
            .find(|configuration| configuration["ConfigurationName"] == json!(configuration_name))
            .unwrap_or_else(|| panic!("case {case:?}: {configuration_name} is not stored"));
        assert_eq!(
            found["SlackTeamName"],
            json!(slack_team_name),
            "case {case:?}: the stored configuration must name the workspace \
             {slack_team_name:?}, got {found}"
        );
    }
}

#[tokio::test]
async fn an_authorization_reaches_every_region() {
    let emulator = Emulator::new();
    emulator.authorize("TREGIONWIDE", "Region Wide Workspace");

    // A workspace authorization is account-wide, so the SAME workspace backs a
    // configuration whichever region the create is signed for, and an unauthorized one
    // is refused in every region.
    for region in [REGION, OTHER_REGION] {
        let answer = emulator
            .create_in_region(region, "regional", "TREGIONWIDE")
            .await;
        assert_eq!(
            answer.status, 201,
            "region {region}: expected 201 for an authorized workspace, got {} with {}",
            answer.status, answer.body
        );

        let refused = emulator
            .create_in_region(region, "regional-refused", "TNEVERAUTHORIZED")
            .await;
        assert_eq!(
            refused.status, 400,
            "region {region}: expected 400 for an unauthorized workspace, got {} with {}",
            refused.status, refused.body
        );
    }
}

#[tokio::test]
async fn the_authorizations_the_store_starts_with_are_listed_and_back_a_configuration() {
    let emulator = Emulator::new();

    let expected: Vec<Value> = chatbot::state::seed_slack_workspaces()
        .iter()
        .map(|workspace| {
            json!({
                "SlackTeamId": workspace.slack_team_id,
                "SlackTeamName": workspace.slack_team_name,
                "State": workspace.state,
            })
        })
        .collect();
    assert!(
        !expected.is_empty(),
        "the store must start with at least one authorized workspace, or no client could \
         ever configure a channel"
    );

    let listed = emulator.all_workspaces(None).await;
    assert_eq!(
        listed, expected,
        "DescribeSlackWorkspaces must report the authorizations the store holds, got {listed:?}"
    );

    // A workspace the listing reports as authorized really does back a configuration.
    for workspace in &listed {
        let slack_team_id = workspace["SlackTeamId"].as_str().expect("a SlackTeamId");
        let answer = emulator
            .create(&format!("seeded-{slack_team_id}"), slack_team_id)
            .await;
        assert_eq!(
            answer.status, 201,
            "workspace {slack_team_id}: expected 201, got {} with {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.body["ChannelConfiguration"]["SlackTeamName"], workspace["SlackTeamName"],
            "workspace {slack_team_id}: the configuration must carry the listed display name, \
             got {}",
            answer.body
        );
    }
}

#[tokio::test]
async fn authorizing_a_workspace_twice_leaves_one_authorization_under_the_newer_name() {
    let emulator = Emulator::new();
    let before = emulator.all_workspaces(None).await.len();

    emulator.authorize("TIDEMPOTENT", "First Name");
    let after_first = emulator.all_workspaces(None).await;
    assert_eq!(
        after_first.len(),
        before + 1,
        "authorizing one workspace must add exactly one authorization, got {after_first:?}"
    );

    emulator.authorize("TIDEMPOTENT", "Second Name");
    let after_second = emulator.all_workspaces(None).await;
    assert_eq!(
        after_second.len(),
        before + 1,
        "re-authorizing one workspace must not add a second authorization, got {after_second:?}"
    );
    let entry = after_second
        .iter()
        .find(|workspace| workspace["SlackTeamId"] == json!("TIDEMPOTENT"))
        .expect("the re-authorized workspace is listed");
    assert_eq!(
        entry["SlackTeamName"],
        json!("Second Name"),
        "re-authorizing must replace the display name, got {entry}"
    );
}

#[tokio::test]
async fn max_results_pages_the_same_authorization_list_whatever_the_page_size() {
    let emulator = Emulator::new();
    // Seven more than the store starts with, so the page sizes below divide the total
    // evenly, unevenly, and not at all.
    for n in 0..7u32 {
        emulator.authorize(&format!("TPAGED{n}"), &format!("Paged Workspace {n}"));
    }

    let whole = emulator.all_workspaces(None).await;
    assert!(
        whole.len() >= 7,
        "expected at least the seven authorized workspaces, got {whole:?}"
    );

    // The model's `MaxResults` is `range 1..100` (`smithy_query`:
    // DescribeSlackWorkspaces), so both bounds are covered here.
    for page_size in [1usize, 2, 3, 7, 100] {
        let paged = emulator.all_workspaces(Some(page_size)).await;
        assert_eq!(
            paged, whole,
            "MaxResults {page_size}: paging must not change the total answer, got {paged:?}"
        );
    }
}

#[tokio::test]
async fn a_workspace_next_token_this_service_did_not_issue_is_refused() {
    let emulator = Emulator::new();

    for token in ["not-a-token", "VGhpcyBpcyBub3QgYSB0ZWFtIGlk"] {
        let answer = emulator
            .call("/describe-slack-workspaces", &json!({"NextToken": token}))
            .await;
        assert_eq!(
            answer.status, 400,
            "token {token:?}: expected 400, got {} with {}",
            answer.status, answer.body
        );
        let error_type = answer.error_type.clone().unwrap_or_default();
        assert!(
            error_type.ends_with("InvalidParameterException"),
            "token {token:?}: expected InvalidParameterException, got {:?}",
            answer.error_type
        );
    }
}
