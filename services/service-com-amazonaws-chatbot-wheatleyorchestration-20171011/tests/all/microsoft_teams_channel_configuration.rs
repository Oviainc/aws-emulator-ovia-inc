// SPDX-License-Identifier: CC0-1.0

//! `CreateMicrosoftTeamsChannelConfiguration` and
//! `ListMicrosoftTeamsChannelConfigurations`, driven over the wire (restJson1) through the
//! same `MultiServiceRouter` the production router serves with.
//!
//! The rule every assertion here is about: a paginated listing of chat configurations
//! reports exactly the configurations the request's region holds, projected onto the
//! modelled shape, in one stable total order by `ChatConfigurationArn`, narrowed by the
//! request's optional filter member, in pages no larger than the caller's `MaxResults`
//! (and bounded even when it names none), resumable only from a `NextToken` the service
//! itself issued — and it changes nothing a later caller sees.
//!
//! The create's own rule, asserted here too: a channel configuration can only be created
//! for a Microsoft Teams team AUTHORIZED with the account, and a `TeamId` no authorization
//! names is refused with the modelled 400 `InvalidRequestException` carrying the fixed
//! sentence AWS Chatbot words this error class with (`state::TEAM_NOT_CONFIGURED_MESSAGE`,
//! which names neither the team nor the account), persisting nothing.
//!
//! The store is exercised as a CLASS of inputs rather than one recorded request: it is
//! read while EMPTY, and with configurations spanning the create's optional members
//! (`ChannelName`, `TeamName`, `SnsTopicArns`, `GuardrailPolicyArns`,
//! `UserAuthorizationRequired`, `Tags`) present and absent, every published
//! `LoggingLevel` (`smithy_query` shape=CustomerCwLogLevel: `pattern
//! ^(ERROR|INFO|NONE)$` — the whole catalogue), `TeamId`s spanning the published `UUID`
//! shape (`smithy_query` shape=UUID: `length 36..36`, `pattern
//! ^[0-9A-Fa-f]{8}(?:-[0-9A-Fa-f]{4}){3}-[0-9A-Fa-f]{12}$`) in lower, upper and mixed
//! case, and `ChannelId`s spanning the published `TeamsChannelId` pattern
//! (`smithy_query` shape=TeamsChannelId) at its shortest form and with either spelling of
//! its `%3a` escape. The page sizes span the published `MaxResults` range (`smithy_query`
//! operation=ListMicrosoftTeamsChannelConfigurations: `range 1..100`) at both bounds, one
//! past each bound, and in between.

use serde_json::{json, Map, Value};

use crate::wire::{Answer, Emulator, OTHER_REGION, REGION};

/// The canonical emulator account id (`emulator_server::EMULATOR_ACCOUNT_ID`), spelled
/// out here so the test pins the ARN a client actually sees.
const ACCOUNT: &str = "000000000000";

/// An `Arn` (`length 12..1224`) every create below carries as its required `IamRoleArn`.
const IAM_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/chatbot-role";
const OTHER_IAM_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/chatbot-other-role";

/// The operations' URIs (`smithy_query` operation=…: `http {method: POST, uri: …}`).
const CREATE_PATH: &str = "/create-ms-teams-channel-configuration";
const LIST_PATH: &str = "/list-ms-teams-channel-configurations";

/// `ResourceState` of a configuration AWS Chatbot reports as usable
/// (`crate::state::STATE_ENABLED`).
const STATE_ENABLED: &str = "ENABLED";

/// The modelled error a `NextToken` this service did not issue is refused with.
const INVALID_PARAMETER_EXCEPTION: &str = "InvalidParameterException";

/// The modelled error a violated input constraint is refused with (this service's only
/// client-input error; see `tests/all/input_constraint_refusals.rs`).
const INVALID_REQUEST_EXCEPTION: &str = "InvalidRequestException";

/// Three Microsoft Teams teams, in two tenants: the rows below put two configurations in
/// the first team and one in each of the others, so filtering on `TeamId` is not the
/// same as listing everything. The ids span the published `UUID` shape in lower, upper
/// and mixed case.
const TEAM_A: &str = "a0b1c2d3-4e5f-4061-8273-8495a6b7c8d9";
const TEAM_B: &str = "FFFFFFFF-FFFF-4FFF-8FFF-FFFFFFFFFFFF";
const TEAM_C: &str = "AbCdEf01-2345-4678-9aBc-DeF012345678";
const TENANT_ONE: &str = "22222222-2222-2222-2222-222222222222";
const TENANT_TWO: &str = "33333333-3333-3333-3333-333333333333";

/// The display name each team is AUTHORIZED under, which a create that names no
/// `TeamName` falls back to. `TEAM_A` is authorized without one, so a create for it that
/// names none reports none.
const TEAM_B_AUTHORIZED_NAME: &str = "Authorized Team B";
const TEAM_C_AUTHORIZED_NAME: &str = "Release Team";

/// The teams the account has authorized before any create below runs: on AWS a team is
/// authorized by the one-time Microsoft Teams client handshake in the AWS Chatbot
/// console, which no operation in this model performs, so the state seam stands in for it.
///
/// (tenant id, team id, display name)
const AUTHORIZED_TEAMS: [(&str, &str, Option<&str>); 3] = [
    (TENANT_ONE, TEAM_A, None),
    (TENANT_TWO, TEAM_B, Some(TEAM_B_AUTHORIZED_NAME)),
    (TENANT_TWO, TEAM_C, Some(TEAM_C_AUTHORIZED_NAME)),
];

/// A team the account HAS authorized but no configuration below is created for: well
/// inside the published `UUID` shape, so it reaches the handler as a filter rather than
/// being refused.
const TEAM_WITH_NO_CONFIGURATION: &str = "00000000-0000-4000-8000-000000000000";

/// The `ChatConfigurationArn` the service must mint for `configuration_name` in `region`.
fn arn_in_region(region: &str, configuration_name: &str) -> String {
    format!(
        "arn:aws:chatbot:{region}:{ACCOUNT}:chat-configuration/microsoft-teams-channel/\
         {configuration_name}"
    )
}

fn expected_arn(configuration_name: &str) -> String {
    arn_in_region(REGION, configuration_name)
}

/// One row of the create table: the request body, and the configuration it must leave in
/// the store.
struct CreateCase {
    label: &'static str,
    request: Value,
    expected: Value,
}

/// The create rows, each differing from the others along what the rule generalises over:
/// which optional members the request carries, which `LoggingLevel` value, how many ARNs
/// and tags the lists carry, the case of the `TeamId`, and the form of the `ChannelId`.
fn create_cases() -> Vec<CreateCase> {
    vec![
        CreateCase {
            label: "only the required members",
            request: json!({
                "ConfigurationName": "minimal",
                "ChannelId": "19%3aminimal%40thread.tacv2",
                "TeamId": TEAM_A,
                "TenantId": TENANT_ONE,
                "IamRoleArn": IAM_ROLE_ARN,
            }),
            // `ChannelName`, `TeamName`, `LoggingLevel` and `UserAuthorizationRequired`
            // are optional on BOTH the request and the modelled configuration, so one the
            // request left out stays absent rather than being invented.
            expected: json!({
                "ChatConfigurationArn": expected_arn("minimal"),
                "ConfigurationName": "minimal",
                "ChannelId": "19%3aminimal%40thread.tacv2",
                "TeamId": TEAM_A,
                "TenantId": TENANT_ONE,
                "IamRoleArn": IAM_ROLE_ARN,
                "SnsTopicArns": [],
                "GuardrailPolicyArns": [],
                "Tags": [],
                "State": STATE_ENABLED,
            }),
        },
        CreateCase {
            label: "every member, two SNS topics and two tags, upper-case TeamId",
            request: json!({
                "ConfigurationName": "full-config_1",
                "ChannelId": "19%3Afull%40thread.tacv2",
                "ChannelName": "ops-alerts",
                "TeamId": TEAM_B,
                "TeamName": "Operations",
                "TenantId": TENANT_TWO,
                "IamRoleArn": IAM_ROLE_ARN,
                "SnsTopicArns": [
                    "arn:aws:sns:us-east-1:000000000000:alerts",
                    "arn:aws:sns:us-west-2:000000000000:pages",
                ],
                "LoggingLevel": "INFO",
                "GuardrailPolicyArns": ["arn:aws:iam::000000000000:policy/ReadOnly"],
                "UserAuthorizationRequired": true,
                "Tags": [
                    {"TagKey": "env", "TagValue": "dev"},
                    {"TagKey": "team", "TagValue": "sre"},
                ],
            }),
            expected: json!({
                "ChatConfigurationArn": expected_arn("full-config_1"),
                "ConfigurationName": "full-config_1",
                "ChannelId": "19%3Afull%40thread.tacv2",
                "ChannelName": "ops-alerts",
                "TeamId": TEAM_B,
                "TeamName": "Operations",
                "TenantId": TENANT_TWO,
                "IamRoleArn": IAM_ROLE_ARN,
                "SnsTopicArns": [
                    "arn:aws:sns:us-east-1:000000000000:alerts",
                    "arn:aws:sns:us-west-2:000000000000:pages",
                ],
                "LoggingLevel": "INFO",
                "GuardrailPolicyArns": ["arn:aws:iam::000000000000:policy/ReadOnly"],
                "UserAuthorizationRequired": true,
                "Tags": [
                    {"TagKey": "env", "TagValue": "dev"},
                    {"TagKey": "team", "TagValue": "sre"},
                ],
                "State": STATE_ENABLED,
            }),
        },
        CreateCase {
            // The second configuration of `TEAM_A`, so the `TeamId` filter has more than
            // one entry to answer with. The `ChannelId` is the shortest value the
            // published `TeamsChannelId` pattern admits, and the tag value is empty
            // (`TagValue` is `length 0..256`).
            label: "a second configuration for the same team, logging ERROR",
            request: json!({
                "ConfigurationName": "logging-error",
                "ChannelId": "%3a%40",
                "ChannelName": "a",
                "TeamId": TEAM_A,
                "TeamName": "Platform",
                "TenantId": TENANT_ONE,
                "IamRoleArn": IAM_ROLE_ARN,
                "SnsTopicArns": ["arn:aws:sns:us-east-1:000000000000:one"],
                "LoggingLevel": "ERROR",
                "UserAuthorizationRequired": false,
                "Tags": [{"TagKey": "only", "TagValue": ""}],
            }),
            expected: json!({
                "ChatConfigurationArn": expected_arn("logging-error"),
                "ConfigurationName": "logging-error",
                "ChannelId": "%3a%40",
                "ChannelName": "a",
                "TeamId": TEAM_A,
                "TeamName": "Platform",
                "TenantId": TENANT_ONE,
                "IamRoleArn": IAM_ROLE_ARN,
                "SnsTopicArns": ["arn:aws:sns:us-east-1:000000000000:one"],
                "LoggingLevel": "ERROR",
                "GuardrailPolicyArns": [],
                "UserAuthorizationRequired": false,
                "Tags": [{"TagKey": "only", "TagValue": ""}],
                "State": STATE_ENABLED,
            }),
        },
        CreateCase {
            label: "logging NONE, no tags, mixed-case TeamId and no TeamName",
            request: json!({
                "ConfigurationName": "logging-none",
                "ChannelId": "19%3amixed%40thread.tacv2",
                "ChannelName": "release-train",
                "TeamId": TEAM_C,
                "TenantId": TENANT_TWO,
                "IamRoleArn": OTHER_IAM_ROLE_ARN,
                "LoggingLevel": "NONE",
                "Tags": [],
            }),
            expected: json!({
                "ChatConfigurationArn": expected_arn("logging-none"),
                "ConfigurationName": "logging-none",
                "ChannelId": "19%3amixed%40thread.tacv2",
                "ChannelName": "release-train",
                "TeamId": TEAM_C,
                // The request named no `TeamName`, so the configuration reports the one
                // the account's authorization for this team holds.
                "TeamName": TEAM_C_AUTHORIZED_NAME,
                "TenantId": TENANT_TWO,
                "IamRoleArn": OTHER_IAM_ROLE_ARN,
                "SnsTopicArns": [],
                "LoggingLevel": "NONE",
                "GuardrailPolicyArns": [],
                "Tags": [],
                "State": STATE_ENABLED,
            }),
        },
    ]
}

/// The `ChatConfigurationArn` of a listed configuration.
fn arn_of(configuration: &Value) -> &str {
    configuration["ChatConfigurationArn"]
        .as_str()
        .unwrap_or_else(|| panic!("a listed configuration must carry a ChatConfigurationArn: {configuration}"))
}

/// An emulator whose account has authorized every team in [`AUTHORIZED_TEAMS`], plus the
/// one no configuration is created for — and no chat configuration at all.
fn emulator_with_authorized_teams() -> Emulator {
    let emulator = Emulator::new();
    for (tenant_id, team_id, team_name) in AUTHORIZED_TEAMS {
        authorize(&emulator, tenant_id, team_id, team_name);
    }
    authorize(&emulator, TENANT_ONE, TEAM_WITH_NO_CONFIGURATION, None);
    emulator
}

/// Authorize `team_id` with the account through the state seam standing in for the
/// one-time Microsoft Teams client handshake.
fn authorize(emulator: &Emulator, tenant_id: &str, team_id: &str, team_name: Option<&str>) {
    service_com_amazonaws_chatbot_wheatleyorchestration_20171011::state::authorize_microsoft_teams_team(
        &*emulator.engine,
        tenant_id,
        team_id,
        team_name,
    )
    .unwrap_or_else(|| panic!("the shared engine must hold the chatbot store"));
}

/// Create the configuration `request` describes in `region`, and hand back the
/// configuration the create answered with.
async fn create_in_region(emulator: &Emulator, region: &str, label: &str, request: &Value) -> Value {
    let answer = emulator.call_in_region(region, CREATE_PATH, request).await;
    assert_eq!(
        answer.status, 200,
        "case {label:?}: expected 200 from the create, got {} with {}",
        answer.status, answer.body
    );
    answer
        .body
        .get("ChannelConfiguration")
        .cloned()
        .unwrap_or_else(|| panic!("case {label:?}: no ChannelConfiguration in {}", answer.body))
}

/// One page of `ListMicrosoftTeamsChannelConfigurations` in `region`: the configurations
/// it carries and its token.
async fn page_in_region(
    emulator: &Emulator,
    region: &str,
    request: &Value,
) -> (Vec<Value>, Option<String>) {
    let answer = emulator.call_in_region(region, LIST_PATH, request).await;
    assert_eq!(
        answer.status, 200,
        "case {request}: expected 200, got {} with {}",
        answer.status, answer.body
    );
    let configurations = answer
        .body
        .get("TeamChannelConfigurations")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "case {request}: no TeamChannelConfigurations list in {}",
                answer.body
            )
        });
    let next_token = answer
        .body
        .get("NextToken")
        .and_then(Value::as_str)
        .map(str::to_string);
    (configurations, next_token)
}

/// Every configuration `region` reports for `team_id` (every team when it is `None`),
/// walking the pages `max_results` splits them into.
async fn all_in_region(
    emulator: &Emulator,
    region: &str,
    max_results: Option<usize>,
    team_id: Option<&str>,
) -> Vec<Value> {
    let mut all: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut request = Map::new();
        if let Some(max) = max_results {
            request.insert("MaxResults".to_string(), json!(max));
        }
        if let Some(team) = team_id {
            request.insert("TeamId".to_string(), json!(team));
        }
        if let Some(cursor) = &token {
            request.insert("NextToken".to_string(), json!(cursor));
        }
        let request = Value::Object(request);
        let (page, next) = page_in_region(emulator, region, &request).await;
        if let Some(max) = max_results {
            assert!(
                page.len() <= max,
                "case MaxResults {max}: a page carried {} configurations: {page:?}",
                page.len()
            );
        }
        for configuration in &page {
            assert!(
                !all.contains(configuration),
                "case MaxResults {max_results:?} / TeamId {team_id:?}: {configuration} was \
                 served on two pages"
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

/// Every configuration [`REGION`] reports.
async fn all_configurations(emulator: &Emulator, max_results: Option<usize>) -> Vec<Value> {
    all_in_region(emulator, REGION, max_results, None).await
}

/// An emulator holding every row of [`create_cases`] in [`REGION`] — and nothing else.
///
/// The rows are created in an order that is NOT `ChatConfigurationArn` order, so the
/// listing's order can only come from the store.
async fn emulator_with_configurations() -> Emulator {
    let emulator = emulator_with_authorized_teams();
    for case in create_cases() {
        create_in_region(&emulator, REGION, case.label, &case.request).await;
    }
    emulator
}

/// [`create_cases`]'s expected configurations, in the `ChatConfigurationArn` order the
/// listing must report them in.
fn expected_listing() -> Vec<Value> {
    let mut expected: Vec<Value> = create_cases().into_iter().map(|case| case.expected).collect();
    expected.sort_by(|left, right| arn_of(left).cmp(arn_of(right)));
    expected
}

#[tokio::test]
async fn an_account_that_created_no_configuration_is_listed_none() {
    // Nothing seeds a chat configuration, so a fresh store reports none: a client must
    // never be handed a configuration it never created. Asserted in both regions and over
    // every page size, so no path fabricates an entry.
    let emulator = emulator_with_authorized_teams();

    for region in [REGION, OTHER_REGION] {
        for max_results in [None, Some(1usize), Some(100)] {
            let listed = all_in_region(&emulator, region, max_results, None).await;
            assert!(
                listed.is_empty(),
                "case {region} / MaxResults {max_results:?}: an account that created no \
                 configuration must be listed none, got {listed:?}"
            );
        }

        // The member itself is still served, as an empty list rather than omitted.
        let (page, token) = page_in_region(&emulator, region, &json!({})).await;
        assert!(
            page.is_empty() && token.is_none(),
            "case {region}: an empty store is one empty page with no token, got {page:?} and \
             {token:?}"
        );
    }
}

#[tokio::test]
async fn the_listing_reports_every_created_configuration_and_nothing_else() {
    let emulator = emulator_with_configurations().await;

    let listed = all_configurations(&emulator, None).await;
    assert_eq!(
        listed,
        expected_listing(),
        "the listing must report exactly the configurations the store holds, in \
         ChatConfigurationArn order, got {listed:?}"
    );
}

#[tokio::test]
async fn each_created_configuration_is_listed_exactly_as_the_create_answered_it() {
    // The round trip: the create's own answer and the listing's entry are two
    // projections of ONE stored record, so they cannot disagree.
    let emulator = emulator_with_authorized_teams();

    for case in create_cases() {
        let created = create_in_region(&emulator, REGION, case.label, &case.request).await;
        assert_eq!(
            created, case.expected,
            "case {:?}: expected the create to answer {}, got {created}",
            case.label, case.expected
        );

        let listed = all_configurations(&emulator, None).await;
        let found = listed
            .iter()
            .find(|configuration| arn_of(configuration) == arn_of(&case.expected))
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: {} is not listed in {listed:?}",
                    case.label,
                    arn_of(&case.expected)
                )
            });
        assert_eq!(
            *found, case.expected,
            "case {:?}: expected the listing to report {}, got {found}",
            case.label, case.expected
        );
    }
}

#[tokio::test]
async fn team_id_narrows_the_listing_to_that_team_alone() {
    let emulator = emulator_with_configurations().await;
    let whole = all_configurations(&emulator, None).await;

    // Every team the rows name, plus one the rows name no configuration for: a filter is
    // not a lookup, so a team with no configuration is answered an empty listing rather
    // than a not-found.
    for team_id in [TEAM_A, TEAM_B, TEAM_C, TEAM_WITH_NO_CONFIGURATION] {
        let mut expected: Vec<Value> = whole
            .iter()
            .filter(|configuration| configuration["TeamId"] == json!(team_id))
            .cloned()
            .collect();
        expected.sort_by(|left, right| arn_of(left).cmp(arn_of(right)));

        let filtered = all_in_region(&emulator, REGION, None, Some(team_id)).await;
        assert_eq!(
            filtered, expected,
            "case TeamId {team_id}: expected {} configuration(s), got {filtered:?}",
            expected.len()
        );

        // Paging the filtered listing answers the same set, one entry at a time.
        let paged = all_in_region(&emulator, REGION, Some(1), Some(team_id)).await;
        assert_eq!(
            paged, expected,
            "case TeamId {team_id} / MaxResults 1: paging must not change the filtered \
             answer, got {paged:?}"
        );
    }

    // `TEAM_A` carries two of the rows, so the filter really is narrower than the whole
    // listing and wider than one entry.
    let team_a = all_in_region(&emulator, REGION, None, Some(TEAM_A)).await;
    assert_eq!(
        team_a.len(),
        2,
        "case TeamId {TEAM_A}: expected the two configurations created for it, got {team_a:?}"
    );
    assert!(
        team_a.len() < whole.len(),
        "case TeamId {TEAM_A}: a filtered listing must be narrower than the whole one, got \
         {team_a:?} of {whole:?}"
    );
}

#[tokio::test]
async fn max_results_pages_the_same_listing_whatever_the_page_size() {
    let emulator = emulator_with_configurations().await;

    let whole = all_configurations(&emulator, None).await;
    assert_eq!(
        whole.len(),
        create_cases().len(),
        "expected exactly the {} created configurations, got {whole:?}",
        create_cases().len()
    );

    // The model's `MaxResults` is `range 1..100`, so both published bounds are covered
    // here, along with page sizes that divide the total evenly, unevenly and not at all.
    for page_size in [1usize, 2, 3, 100] {
        let paged = all_configurations(&emulator, Some(page_size)).await;
        assert_eq!(
            paged, whole,
            "case MaxResults {page_size}: paging must not change the total answer, got \
             {paged:?}"
        );
    }
}

#[tokio::test]
async fn a_max_results_outside_the_published_range_is_refused() {
    let emulator = emulator_with_configurations().await;

    // One past each published bound of `MaxResults` (`range 1..100`).
    for max_results in [0i64, 101] {
        let answer: Answer = emulator
            .call(LIST_PATH, &json!({"MaxResults": max_results}))
            .await;
        assert_eq!(
            answer.status, 400,
            "case MaxResults {max_results}: expected 400, got {} with {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.code(),
            INVALID_REQUEST_EXCEPTION,
            "case MaxResults {max_results}: expected {INVALID_REQUEST_EXCEPTION}, got {} in {}",
            answer.code(),
            answer.body
        );
    }
}

#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let emulator = emulator_with_configurations().await;

    // Every token below satisfies the modelled `PaginationToken`
    // (`pattern ^[a-zA-Z0-9=\/+_.\-,#:\\"{}]{4,1276}$`), so it reaches the handler, and
    // none of them is a cursor this service issued for this region: not base64 at all,
    // base64 of a configuration name rather than an ARN, base64 of another chat client's
    // ARN, and base64 of this region's ARN form as another region would carry it.
    let tokens = [
        ("not base64 at all", "not-a-token".to_string()),
        (
            "base64 of a configuration name",
            aws_smithy_types::base64::encode("minimal"),
        ),
        (
            "base64 of a Slack channel configuration ARN",
            aws_smithy_types::base64::encode(format!(
                "arn:aws:chatbot:{REGION}:{ACCOUNT}:chat-configuration/slack-channel/minimal"
            )),
        ),
        (
            "base64 of another region's ARN",
            aws_smithy_types::base64::encode(arn_in_region(OTHER_REGION, "minimal")),
        ),
    ];

    for (case, token) in &tokens {
        let answer = emulator.call(LIST_PATH, &json!({"NextToken": token})).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {} with {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_type.as_deref(),
            Some(INVALID_PARAMETER_EXCEPTION),
            "case {case:?}: expected {INVALID_PARAMETER_EXCEPTION}, got {:?}",
            answer.error_type
        );
    }

    // A token the service DID issue resumes the scan: the pages it splits the store into
    // carry every configuration exactly once, in the same order.
    let (first_page, token) = page_in_region(&emulator, REGION, &json!({"MaxResults": 1})).await;
    let token = token.unwrap_or_else(|| {
        panic!("a one-configuration page of a multi-configuration store must carry a NextToken")
    });
    let (second_page, _) = page_in_region(
        &emulator,
        REGION,
        &json!({"MaxResults": 1, "NextToken": token}),
    )
    .await;
    assert_eq!(
        first_page.len(),
        1,
        "case MaxResults 1: expected one configuration, got {first_page:?}"
    );
    assert_ne!(
        second_page, first_page,
        "case resumed page: a resumed scan must not repeat the previous page, got {second_page:?}"
    );
}

#[tokio::test]
async fn the_listing_is_regional_and_leaves_the_store_unchanged() {
    let emulator = emulator_with_configurations().await;

    let from_region = all_configurations(&emulator, None).await;
    let from_other_region = all_in_region(&emulator, OTHER_REGION, None, None).await;
    assert!(
        from_other_region.is_empty(),
        "case {OTHER_REGION}: a chat configuration belongs to the region its ARN names, so \
         another region must report none of them, got {from_other_region:?}"
    );

    // A configuration created in the other region is reported there — and only there,
    // even though it carries the same name as one of this region's.
    let case = &create_cases()[0];
    let created = create_in_region(&emulator, OTHER_REGION, case.label, &case.request).await;
    assert_eq!(
        created["ChatConfigurationArn"],
        json!(arn_in_region(OTHER_REGION, "minimal")),
        "case {OTHER_REGION}: the minted ARN must name the request's region, got {created}"
    );
    let from_other_region = all_in_region(&emulator, OTHER_REGION, None, None).await;
    assert_eq!(
        from_other_region,
        vec![created],
        "case {OTHER_REGION}: expected only the configuration created there, got \
         {from_other_region:?}"
    );

    // A read changes nothing a later caller sees: the same listing, after reading every
    // page in two regions and filtering on every team.
    let again = all_configurations(&emulator, None).await;
    assert_eq!(
        again, from_region,
        "reading the listing must not change what a later caller sees, got {again:?}"
    );
}

#[tokio::test]
async fn a_create_for_a_team_the_account_has_not_authorized_is_refused_and_persists_nothing() {
    // The create rule: a channel configuration can only be created for a team the account
    // has authorized, and the registry — not the spelling or shape of the id — is what
    // decides that. The rows span the published `UUID` shape (lower case, upper case,
    // mixed case, all zeroes) so no row is accepted because of how its id is written.
    let emulator = emulator_with_authorized_teams();
    let before = all_configurations(&emulator, None).await;

    let unauthorized = [
        ("lower-case hex", "deadbeef-0001-4000-8000-0000000000ff"),
        ("upper-case hex", "DEADBEEF-0002-4000-8000-0000000000FF"),
        ("mixed-case hex", "DeAdBeEf-0003-4000-8000-0000000000Ff"),
        ("all zeroes", "00000000-0000-0000-0000-000000000000"),
    ];

    for (index, (case, team_id)) in unauthorized.iter().enumerate() {
        let request = json!({
            "ConfigurationName": format!("unauthorized-{index}"),
            "ChannelId": "19%3aunauthorized%40thread.tacv2",
            "TeamId": team_id,
            "TenantId": TENANT_ONE,
            "IamRoleArn": IAM_ROLE_ARN,
        });
        let answer = emulator.call(CREATE_PATH, &request).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {} with {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_type.as_deref(),
            Some(INVALID_REQUEST_EXCEPTION),
            "case {case:?}: expected {INVALID_REQUEST_EXCEPTION}, got {:?}",
            answer.error_type
        );
        // AWS Chatbot words this error class as one fixed sentence about the team id,
        // naming neither the team nor the account.
        assert_eq!(
            answer.message(),
            service_com_amazonaws_chatbot_wheatleyorchestration_20171011::state::TEAM_NOT_CONFIGURED_MESSAGE,
            "case {case:?}: expected the team-not-configured refusal, got {:?}",
            answer.message()
        );
    }

    let after = all_configurations(&emulator, None).await;
    assert_eq!(
        after, before,
        "a refused create must persist nothing, got {after:?}"
    );

    // The same request for an AUTHORIZED team is accepted, so the refusal is about the
    // authorization and nothing else.
    let accepted = json!({
        "ConfigurationName": "authorized-team",
        "ChannelId": "19%3aunauthorized%40thread.tacv2",
        "TeamId": TEAM_WITH_NO_CONFIGURATION,
        "TenantId": TENANT_ONE,
        "IamRoleArn": IAM_ROLE_ARN,
    });
    let created = create_in_region(&emulator, REGION, "authorized team", &accepted).await;
    assert_eq!(
        created["TeamId"],
        json!(TEAM_WITH_NO_CONFIGURATION),
        "case authorized team: expected the created configuration to carry the requested \
         team, got {created}"
    );
}

#[tokio::test]
async fn a_create_past_the_stores_cap_is_refused_and_the_listing_stays_bounded() {
    // The store is bounded: the cap is the emulator's own
    // (`state::MAX_MICROSOFT_TEAMS_CHANNEL_CONFIGURATIONS`), and past it a create is
    // refused with the modelled `LimitExceededException` (HTTP 403) rather than growing
    // the store. The cap is counted over the WHOLE store, so signing for another region
    // hands the caller no fresh budget.
    const CAP: usize = service_com_amazonaws_chatbot_wheatleyorchestration_20171011::state::MAX_MICROSOFT_TEAMS_CHANNEL_CONFIGURATIONS;

    let emulator = emulator_with_authorized_teams();
    for index in 0..CAP {
        let request = json!({
            "ConfigurationName": format!("capped-{index}"),
            "ChannelId": "19%3acapped%40thread.tacv2",
            "TeamId": TEAM_A,
            "TenantId": TENANT_ONE,
            "IamRoleArn": IAM_ROLE_ARN,
        });
        create_in_region(&emulator, REGION, "at the cap", &request).await;
    }

    for (case, region) in [("same region", REGION), ("another region", OTHER_REGION)] {
        let request = json!({
            "ConfigurationName": "one-too-many",
            "ChannelId": "19%3acapped%40thread.tacv2",
            "TeamId": TEAM_A,
            "TenantId": TENANT_ONE,
            "IamRoleArn": IAM_ROLE_ARN,
        });
        let answer = emulator.call_in_region(region, CREATE_PATH, &request).await;
        assert_eq!(
            answer.status, 403,
            "case {case:?}: expected 403 past the cap, got {} with {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_type.as_deref(),
            Some("LimitExceededException"),
            "case {case:?}: expected LimitExceededException, got {:?}",
            answer.error_type
        );
    }

    // The listing still answers one bounded page per request, and the whole store.
    let listed = all_configurations(&emulator, None).await;
    assert_eq!(
        listed.len(),
        CAP,
        "expected the cap's worth of configurations, got {}",
        listed.len()
    );
}

#[tokio::test]
async fn a_duplicate_configuration_name_in_the_same_region_is_refused_and_persists_nothing() {
    let emulator = emulator_with_configurations().await;
    let before = all_configurations(&emulator, None).await;

    let case = &create_cases()[0];
    let answer = emulator.call(CREATE_PATH, &case.request).await;
    assert_eq!(
        answer.status, 409,
        "case duplicate {:?}: expected 409, got {} with {}",
        case.label, answer.status, answer.body
    );
    assert_eq!(
        answer.message(),
        format!(
            "Resource with arn {} already exists.",
            expected_arn("minimal")
        ),
        "case duplicate {:?}: expected the conflict to name the stored ARN, got {:?}",
        case.label,
        answer.message()
    );

    let after = all_configurations(&emulator, None).await;
    assert_eq!(
        after, before,
        "case duplicate {:?}: a refused create must persist nothing, got {after:?}",
        case.label
    );
}
