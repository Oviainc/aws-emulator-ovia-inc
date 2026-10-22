// SPDX-License-Identifier: CC0-1.0

//! `ListMicrosoftTeamsConfiguredTeams`, over the wire (restJson1) through the same
//! `MultiServiceRouter` the production router serves with.
//!
//! The rule every assertion here is about: a paginated account-wide listing reports
//! exactly the authorizations the account's registry holds, projected onto the modelled
//! shape, in one stable total order by the resource's id, in pages no larger than the
//! caller's `MaxResults` (and bounded even when it names none), resumable only from a
//! `NextToken` the service itself issued — and it changes nothing a later caller sees.
//!
//! The registry is exercised as a CLASS of inputs rather than one recorded request: the
//! teams span the published `UUID` shape (`smithy_query` shape=UUID: `length 36..36`,
//! `pattern ^[0-9A-Fa-f]{8}(?:-[0-9A-Fa-f]{4}){3}-[0-9A-Fa-f]{12}$`) in lower case, upper
//! case and mixed case, with and without a display name, authorized out of `TeamId`
//! order; the page sizes span the published `MaxResults` range (`smithy_query`
//! operation=ListMicrosoftTeamsConfiguredTeams: `range 1..100`) at both bounds, one past
//! each bound, and in between.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_chatbot_wheatleyorchestration_20171011 as chatbot;
use smithy_server_runtime::MultiServiceRouter;

/// The region the tests sign for.
const REGION: &str = "us-east-1";

/// Another region, so the account-wide reach of an authorization can be checked.
const OTHER_REGION: &str = "us-west-2";

/// The operation's URI (`smithy_query` operation=ListMicrosoftTeamsConfiguredTeams:
/// `http {method: POST, uri: /list-ms-teams-configured-teams}`).
const LIST_PATH: &str = "/list-ms-teams-configured-teams";

/// `ResourceState` of a team AWS Chatbot reports as usable
/// (`crate::state::STATE_ENABLED`).
const STATE_ENABLED: &str = "ENABLED";

/// A SigV4 credential scope naming this service and `region`.
fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/chatbot/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=deadbeef"
    )
}

/// One answer from the emulator: the status, the `x-amzn-errortype` header a modelled
/// failure carries, and the parsed body.
struct Answer {
    status: u16,
    error_type: Option<String>,
    body: Value,
}

/// This service mounted alone over one shared engine, with the engine kept so a test can
/// reach the authorization seam.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(chatbot::EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// Authorize the Microsoft Teams team `team_id` in tenant `tenant_id`, under the
    /// display name `team_name`. This is the out-of-band seam standing in for the
    /// one-time Microsoft Teams client handshake: no operation in this model performs it.
    fn authorize(&self, tenant_id: &str, team_id: &str, team_name: Option<&str>) {
        chatbot::state::authorize_microsoft_teams_team(
            &*self.engine,
            tenant_id,
            team_id,
            team_name,
        )
        .unwrap_or_else(|| panic!("the shared engine must hold the chatbot store"));
    }

    async fn call_in_region(&self, region: &str, path: &str, body: &Value) -> Answer {
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://chatbot.{region}.amazonaws.com{path}"))
            .header("host", format!("chatbot.{region}.amazonaws.com"))
            .header("content-type", "application/json")
            .header("authorization", authorization(region))
            .body(bytes::Bytes::from(body.to_string()))
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                panic!(
                    "{path}: response body is not JSON ({e}): {}",
                    String::from_utf8_lossy(&bytes)
                )
            })
        };
        Answer {
            status,
            error_type,
            body,
        }
    }

    /// One page of `ListMicrosoftTeamsConfiguredTeams` in `region`: the teams it carries
    /// and its token.
    async fn teams_page_in_region(
        &self,
        region: &str,
        request: &Value,
    ) -> (Vec<Value>, Option<String>) {
        let answer = self.call_in_region(region, LIST_PATH, request).await;
        assert_eq!(
            answer.status, 200,
            "case {request}: expected 200, got {} with {}",
            answer.status, answer.body
        );
        let teams = answer
            .body
            .get("ConfiguredTeams")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_else(|| {
                panic!("case {request}: no ConfiguredTeams list in {}", answer.body)
            });
        let next_token = answer
            .body
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        (teams, next_token)
    }

    /// Every configured team `region` reports, walking the pages `max_results` splits
    /// them into.
    async fn all_teams_in_region(&self, region: &str, max_results: Option<usize>) -> Vec<Value> {
        let mut all: Vec<Value> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut request = serde_json::Map::new();
            if let Some(max) = max_results {
                request.insert("MaxResults".to_string(), json!(max));
            }
            if let Some(cursor) = &token {
                request.insert("NextToken".to_string(), json!(cursor));
            }
            let request = Value::Object(request);
            let (page, next) = self.teams_page_in_region(region, &request).await;
            if let Some(max) = max_results {
                assert!(
                    page.len() <= max,
                    "case MaxResults {max}: a page carried {} teams: {page:?}",
                    page.len()
                );
            }
            for team in &page {
                assert!(
                    !all.contains(team),
                    "case MaxResults {max_results:?}: {team} was served on two pages"
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

    /// Every configured team, as [`REGION`] reports it.
    async fn all_teams(&self, max_results: Option<usize>) -> Vec<Value> {
        self.all_teams_in_region(REGION, max_results).await
    }
}

/// The `TeamId` of a listed team.
fn team_id_of(team: &Value) -> &str {
    team["TeamId"]
        .as_str()
        .unwrap_or_else(|| panic!("a listed team must carry a TeamId: {team}"))
}

/// The team authorizations every test below adds: the published `UUID` shape in lower
/// case, upper case, mixed case and digits only, with a display name and without one,
/// in two tenants — authorized in an order that is NOT `TeamId` order, so the listing's
/// order can only come from the registry.
///
/// (case, tenant id, team id, team name)
#[allow(clippy::type_complexity)]
const AUTHORIZED: [(&str, &str, &str, Option<&str>); 5] = [
    (
        "lower-case hex, named",
        "22222222-2222-2222-2222-222222222222",
        "a0b1c2d3-4e5f-4061-8273-8495a6b7c8d9",
        Some("deadbeef-0001-4000-8000-000000000001"),
    ),
    (
        "upper-case hex, named",
        "22222222-2222-2222-2222-222222222222",
        "FFFFFFFF-FFFF-4FFF-8FFF-FFFFFFFFFFFF",
        Some("DEADBEEF-0002-4000-8000-000000000002"),
    ),
    (
        "mixed-case hex, no display name",
        "33333333-3333-3333-3333-333333333333",
        "AbCdEf01-2345-4678-9aBc-DeF012345678",
        None,
    ),
    (
        "digits only, named",
        "33333333-3333-3333-3333-333333333333",
        "12345678-1234-4234-8234-123456789012",
        Some("00000000-0003-4000-8000-000000000003"),
    ),
    (
        "all zeroes, sorts first",
        "22222222-2222-2222-2222-222222222222",
        "00000000-0000-4000-8000-000000000000",
        None,
    ),
];

/// `AUTHORIZED` as the modelled `ConfiguredTeam` the listing must report for it: the
/// tenant and name come from the authorization, the state from the store's own
/// `ENABLED`, and an authorization with no display name reports no `TeamName`.
fn expected_view(tenant_id: &str, team_id: &str, team_name: Option<&str>) -> Value {
    let mut view = serde_json::Map::new();
    view.insert("TenantId".to_string(), json!(tenant_id));
    view.insert("TeamId".to_string(), json!(team_id));
    if let Some(name) = team_name {
        view.insert("TeamName".to_string(), json!(name));
    }
    view.insert("State".to_string(), json!(STATE_ENABLED));
    Value::Object(view)
}

/// An emulator holding the seeded registry plus every row of [`AUTHORIZED`].
fn emulator_with_authorized_teams() -> Emulator {
    let emulator = Emulator::new();
    for (_, tenant_id, team_id, team_name) in AUTHORIZED {
        emulator.authorize(tenant_id, team_id, team_name);
    }
    emulator
}

#[tokio::test]
async fn the_listing_reports_every_authorization_the_registry_holds_and_nothing_else() {
    let emulator = emulator_with_authorized_teams();

    let mut expected: Vec<Value> = chatbot::state::seed_microsoft_teams_configured_teams()
        .iter()
        .map(|team| expected_view(&team.tenant_id, &team.team_id, team.team_name.as_deref()))
        .collect();
    assert!(
        !expected.is_empty(),
        "the store must start with at least one configured team, or no client could ever \
         discover one"
    );
    for (_, tenant_id, team_id, team_name) in AUTHORIZED {
        expected.push(expected_view(tenant_id, team_id, team_name));
    }
    // The listing's order is the registry's: `TeamId` ascending, whatever order the
    // authorizations arrived in.
    expected.sort_by(|left, right| team_id_of(left).cmp(team_id_of(right)));

    let listed = emulator.all_teams(None).await;
    assert_eq!(
        listed, expected,
        "the listing must report exactly the authorizations the registry holds, in TeamId \
         order, got {listed:?}"
    );
}

#[tokio::test]
async fn each_authorization_is_reported_as_the_registry_holds_it() {
    let emulator = emulator_with_authorized_teams();
    let listed = emulator.all_teams(None).await;

    for (case, tenant_id, team_id, team_name) in AUTHORIZED {
        let found = listed
            .iter()
            .find(|team| team_id_of(team) == team_id)
            .unwrap_or_else(|| panic!("case {case:?}: {team_id} is not listed in {listed:?}"));
        assert_eq!(
            *found,
            expected_view(tenant_id, team_id, team_name),
            "case {case:?}: expected {}, got {found}",
            expected_view(tenant_id, team_id, team_name)
        );
    }

    // Re-authorizing one team replaces its tenant and display name rather than adding a
    // second entry: one team id, one authorization.
    let (case, _, team_id, _) = AUTHORIZED[0];
    let replacement_tenant = "44444444-4444-4444-4444-444444444444";
    let replacement_name = "feedface-0009-4000-8000-000000000009";
    emulator.authorize(replacement_tenant, team_id, Some(replacement_name));
    let relisted = emulator.all_teams(None).await;
    assert_eq!(
        relisted.len(),
        listed.len(),
        "case {case:?}: re-authorizing must not add a second entry, got {relisted:?}"
    );
    let found = relisted
        .iter()
        .find(|team| team_id_of(team) == team_id)
        .unwrap_or_else(|| panic!("case {case:?}: {team_id} is not listed in {relisted:?}"));
    assert_eq!(
        *found,
        expected_view(replacement_tenant, team_id, Some(replacement_name)),
        "case {case:?}: re-authorizing must replace the authorization, got {found}"
    );
}

#[tokio::test]
async fn max_results_pages_the_same_listing_whatever_the_page_size() {
    let emulator = emulator_with_authorized_teams();

    let whole = emulator.all_teams(None).await;
    assert!(
        whole.len() >= AUTHORIZED.len(),
        "expected at least the {} authorized teams, got {whole:?}",
        AUTHORIZED.len()
    );

    // The model's `MaxResults` is `range 1..100`, so both published bounds are covered
    // here, along with page sizes that divide the total evenly, unevenly, and not at all.
    for page_size in [1usize, 2, 3, 7, 100] {
        let paged = emulator.all_teams(Some(page_size)).await;
        assert_eq!(
            paged, whole,
            "case MaxResults {page_size}: paging must not change the total answer, got \
             {paged:?}"
        );
    }
}

#[tokio::test]
async fn a_max_results_outside_the_published_range_is_refused() {
    let emulator = emulator_with_authorized_teams();

    // One past each published bound of `MaxResults` (`range 1..100`).
    for max_results in [0i64, 101] {
        let answer = emulator
            .call_in_region(REGION, LIST_PATH, &json!({"MaxResults": max_results}))
            .await;
        assert_eq!(
            answer.status, 400,
            "case MaxResults {max_results}: expected 400, got {} with {}",
            answer.status, answer.body
        );
    }
}

#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let emulator = emulator_with_authorized_teams();

    // Every token below satisfies the modelled `PaginationToken`
    // (`pattern ^[a-zA-Z0-9=\/+_.\-,#:\\"{}]{4,1276}$`), so it reaches the handler, and
    // none of them is a cursor this service issued: not base64 at all, base64 of a
    // string that is no `TeamId`, base64 of a `TeamId` one character short, and base64 of
    // a `TeamId` with a non-hex digit.
    let tokens = [
        ("not base64 at all", "not-a-token".to_string()),
        (
            "base64 of a non-UUID string",
            aws_smithy_types::base64::encode("the-first-team-please"),
        ),
        (
            "base64 of a 35-character UUID",
            aws_smithy_types::base64::encode("00000000-0000-4000-8000-00000000000"),
        ),
        (
            "base64 of a UUID with a non-hex digit",
            aws_smithy_types::base64::encode("0000000g-0000-4000-8000-000000000000"),
        ),
    ];

    for (case, token) in &tokens {
        let answer = emulator
            .call_in_region(REGION, LIST_PATH, &json!({"NextToken": token}))
            .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {} with {}",
            answer.status, answer.body
        );
        let error_type = answer.error_type.clone().unwrap_or_default();
        assert!(
            error_type.ends_with("InvalidParameterException"),
            "case {case:?}: expected InvalidParameterException, got {:?}",
            answer.error_type
        );
    }

    // A token the service DID issue resumes the scan: the pages it splits the registry
    // into carry every team exactly once.
    let (first_page, token) = emulator
        .teams_page_in_region(REGION, &json!({"MaxResults": 1}))
        .await;
    let token = token.unwrap_or_else(|| {
        panic!("a one-team page of a multi-team registry must carry a NextToken")
    });
    let (second_page, _) = emulator
        .teams_page_in_region(REGION, &json!({"MaxResults": 1, "NextToken": token}))
        .await;
    assert_eq!(
        first_page.len(),
        1,
        "case MaxResults 1: expected one team, got {first_page:?}"
    );
    assert_ne!(
        second_page, first_page,
        "case resumed page: a resumed scan must not repeat the previous page, got \
         {second_page:?}"
    );
}

#[tokio::test]
async fn the_listing_is_account_wide_and_leaves_the_registry_unchanged() {
    let emulator = emulator_with_authorized_teams();

    let from_region = emulator.all_teams_in_region(REGION, None).await;
    let from_other_region = emulator.all_teams_in_region(OTHER_REGION, None).await;
    assert_eq!(
        from_other_region, from_region,
        "case {OTHER_REGION}: a team authorization is account-wide, so every region must \
         report the same teams, got {from_other_region:?}"
    );

    // A read changes nothing a later caller sees: the same listing, after reading every
    // page in two regions.
    let again = emulator.all_teams(None).await;
    assert_eq!(
        again, from_region,
        "reading the listing must not change what a later caller sees, got {again:?}"
    );
}

#[tokio::test]
async fn every_listed_team_carries_the_modelled_uuid_shape() {
    let emulator = emulator_with_authorized_teams();
    let listed = emulator.all_teams(None).await;

    // `ConfiguredTeam`'s `TenantId`, `TeamId` and `TeamName` all target the model's
    // `UUID` (`smithy_query` shape=ConfiguredTeam): `length 36..36`,
    // `pattern ^[0-9A-Fa-f]{8}(?:-[0-9A-Fa-f]{4}){3}-[0-9A-Fa-f]{12}$`.
    for team in &listed {
        for member in ["TenantId", "TeamId", "TeamName"] {
            let Some(value) = team.get(member).and_then(Value::as_str) else {
                continue;
            };
            assert!(
                is_modelled_uuid(value),
                "case {member} of {team}: expected the modelled UUID shape, got {value:?}"
            );
        }
        assert_eq!(
            team["State"],
            json!(STATE_ENABLED),
            "case {team}: expected State {STATE_ENABLED}, got {}",
            team["State"]
        );
    }
    // `TenantId` and `TeamId` are required members, so every listed team carries them.
    for team in &listed {
        for member in ["TenantId", "TeamId"] {
            assert!(
                team.get(member).and_then(Value::as_str).is_some(),
                "case {member} of {team}: a required member must be present"
            );
        }
    }
}

/// Whether `value` satisfies the model's `UUID` shape: five `-`-separated runs of
/// 8/4/4/4/12 hex digits.
fn is_modelled_uuid(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == 5
        && groups
            .iter()
            .zip([8usize, 4, 4, 4, 12])
            .all(|(group, run)| {
                group.len() == run && group.chars().all(|c| c.is_ascii_hexdigit())
            })
}
