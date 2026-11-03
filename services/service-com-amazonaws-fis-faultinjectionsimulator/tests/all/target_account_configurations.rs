// SPDX-License-Identifier: CC0-1.0

//! AWS FIS target account configurations, end to end over HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rule under test is the one FIS's target-account-configuration
//! surface states:
//!
//! * a target account configuration is a SUB-RESOURCE of an experiment template, named
//!   by the (`experimentTemplateId`, `accountId`) pair rather than by a minted id, so
//!   `CreateTargetAccountConfiguration` persists exactly the members the request carried
//!   under that name and every later `GetTargetAccountConfiguration` of that name reports
//!   them unchanged (an explicitly empty `description` is normalized like an omitted one,
//!   the rule `target_account_configuration_description.rs` owns);
//! * a create naming an account the template is already configured for answers
//!   `ConflictException` (HTTP 409), unless it REPEATS the request that created that
//!   configuration under the same `@idempotencyToken` — in which case it reports that
//!   configuration back instead of a second one (what that token is bound to is the rule
//!   `target_account_configuration_idempotency.rs` owns);
//! * `UpdateTargetAccountConfiguration` is a PARTIAL update of the configuration that
//!   name already holds: a member the request carries (`roleArn`, `description` — both
//!   optional, unlike the create's required `roleArn`) replaces the stored value, a
//!   member it omits leaves the stored value alone, the `accountId` naming the record
//!   never moves, and the record the update answers with is the one every later read
//!   reports;
//! * a create or a read naming a template the store does not hold, and a read or an
//!   update naming an account that template has no configuration for (including an
//!   account configured on a DIFFERENT template), answer `ResourceNotFoundException`
//!   (HTTP 404);
//! * the template's `targetAccountConfigurationsCount` is the number of configurations it
//!   holds, and an experiment carries the count its template had when it was started;
//! * a request outside a modelled constraint on `accountId`, `roleArn` or `description`,
//!   or carrying a `roleArn` that is not a well-formed IAM role ARN, is refused with HTTP
//!   400 before any of that (the ARN rule itself is asserted in
//!   `target_account_configuration_errors.rs`).
//!
//! The HTTP harness below is local to this suite: the one in `experiment_lifecycle.rs`
//! is private to that suite, and a test harness is not shared state.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use service_com_amazonaws_fis_faultinjectionsimulator::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// `TargetAccountId`'s published bounds (`@length(min: 12, max: 48)`), spelled out here
/// rather than imported from the crate so a wrong bound in the model or the handler fails
/// this suite.
const ACCOUNT_ID_FLOOR: usize = 12;
/// See [`ACCOUNT_ID_FLOOR`].
const ACCOUNT_ID_CEILING: usize = 48;

/// `RoleArn`'s published bounds (`@length(min: 20, max: 2048)`).
const ROLE_ARN_FLOOR: usize = 20;
/// See [`ROLE_ARN_FLOOR`].
const ROLE_ARN_CEILING: usize = 2048;

/// The length of the SHORTEST well-formed IAM role ARN:
/// `arn:aws:iam::<12 digits>:role/<one character>` — `"arn:aws:iam::"` (13) plus the
/// twelve-digit account id, `":role/"` (6) and a one-character role name. It is above
/// `RoleArn`'s modelled floor, so a `roleArn` at that floor is within the shape's bounds
/// and still not an ARN.
const SHORTEST_ROLE_ARN: usize = 32;

/// `TargetAccountConfigurationDescription`'s published bounds
/// (`@length(min: 0, max: 512)`).
const DESCRIPTION_FLOOR: usize = 0;
/// See [`DESCRIPTION_FLOOR`].
const DESCRIPTION_CEILING: usize = 512;

/// The region an ARN names when the request's credential scope offers none usable.
const FALLBACK_REGION: &str = "us-east-1";

/// A SigV4 credential scope naming this service, for `region`. The service field is what
/// routes the request to fis.
fn signed_for(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/fis/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    /// The member `name` of this answer's body.
    fn member(&self, name: &str) -> &Value {
        &self.body[name]
    }

    /// The `targetAccountConfiguration` this answer carries.
    fn configuration(&self) -> &Value {
        self.member("targetAccountConfiguration")
    }
}

async fn send(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body
        .map(|value| Bytes::from(serde_json::to_vec(&value).expect("the test body serializes")))
        .unwrap_or_default();
    let request = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "fis.us-east-1.amazonaws.com")
        .header("authorization", signed_for(FALLBACK_REGION))
        .header("content-type", "application/json")
        .body(payload)
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

/// Create an experiment template to hang target account configurations off, and report
/// its minted id. `label` only has to be unique within one test; whitespace in it is
/// collapsed because `ClientToken` is modelled `pattern ^[\S]+$`.
///
/// The template targets MULTIPLE accounts, which is the targeting a target account
/// configuration belongs to: `accountTargeting` and the configurations a template holds
/// state the same thing between them. A case that hangs NO configuration off its template
/// asks for the documented default instead ([`template_targeting`]).
async fn template(router: &MultiServiceRouter, label: &str) -> String {
    template_targeting(router, label, "multi-account").await
}

async fn template_targeting(
    router: &MultiServiceRouter,
    label: &str,
    targeting: &str,
) -> String {
    let token: String = label
        .chars()
        .map(|c| if c.is_whitespace() { '-' } else { c })
        .collect();
    let answer = send(
        router,
        "POST",
        "/experimentTemplates",
        Some(json!({
            "clientToken": format!("template-{token}"),
            "description": format!("target account configuration suite: {label}"),
            "roleArn": "arn:aws:iam::000000000000:role/fis-target-account-configurations",
            "stopConditions": [{ "source": "none" }],
            "targets": {},
            "experimentOptions": { "accountTargeting": targeting },
            // `aws:fis:wait` takes a required `duration`, an ISO 8601 duration within the
            // documented 1 to 720 minutes.
            "actions": {
                "wait": { "actionId": "aws:fis:wait", "parameters": { "duration": "PT5M" } }
            }
        })),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {label:?}: expected HTTP 200 from CreateExperimentTemplate, got {} with {:?}",
        answer.status, answer.body
    );
    answer.member("experimentTemplate")["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {label:?}: no experimentTemplate.id in {:?}", answer.body))
        .to_string()
}

async fn create_configuration(
    router: &MultiServiceRouter,
    template_id: &str,
    account_id: &str,
    body: Value,
) -> Answer {
    send(
        router,
        "POST",
        &format!("/experimentTemplates/{template_id}/targetAccountConfigurations/{account_id}"),
        Some(body),
    )
    .await
}

async fn get_configuration(
    router: &MultiServiceRouter,
    template_id: &str,
    account_id: &str,
) -> Answer {
    send(
        router,
        "GET",
        &format!("/experimentTemplates/{template_id}/targetAccountConfigurations/{account_id}"),
        None,
    )
    .await
}

async fn update_configuration(
    router: &MultiServiceRouter,
    template_id: &str,
    account_id: &str,
    body: Value,
) -> Answer {
    send(
        router,
        "PATCH",
        &format!("/experimentTemplates/{template_id}/targetAccountConfigurations/{account_id}"),
        Some(body),
    )
    .await
}

async fn get_template(router: &MultiServiceRouter, template_id: &str) -> Answer {
    send(
        router,
        "GET",
        &format!("/experimentTemplates/{template_id}"),
        None,
    )
    .await
}

/// A create body carrying `roleArn`, plus `description` and `clientToken` only when the
/// case names them — an absent member has to stay absent on the wire.
fn create_body(role_arn: &str, description: Option<&str>, client_token: Option<&str>) -> Value {
    let mut body = Map::new();
    body.insert("roleArn".to_string(), json!(role_arn));
    if let Some(description) = description {
        body.insert("description".to_string(), json!(description));
    }
    if let Some(client_token) = client_token {
        body.insert("clientToken".to_string(), json!(client_token));
    }
    Value::Object(body)
}

/// An update body carrying only the members the case names: `roleArn` and `description`
/// are both optional on `UpdateTargetAccountConfiguration`, and an omitted member has to
/// stay off the wire for the "omitted keeps the stored value" rule to be under test at
/// all. An empty body is a legal update that carries nothing.
fn update_body(role_arn: Option<&str>, description: Option<&str>) -> Value {
    let mut body = Map::new();
    if let Some(role_arn) = role_arn {
        body.insert("roleArn".to_string(), json!(role_arn));
    }
    if let Some(description) = description {
        body.insert("description".to_string(), json!(description));
    }
    Value::Object(body)
}

/// An account id of exactly `len` characters: `TargetAccountId` is modelled
/// `@length(12..48)`, `pattern ^[\S]+$`, and AWS account ids are digit strings.
fn account_id_of(len: usize) -> String {
    let base = "000000000001".to_string();
    if len <= base.len() {
        base[..len].to_string()
    } else {
        format!("{base}{}", "9".repeat(len - base.len()))
    }
}

/// One member-bounds row of the update suite: the case, the account it configures, the
/// `roleArn` the update carries, the `description` it carries, and the HTTP status
/// expected of it.
type UpdateBoundRow = (String, &'static str, Option<String>, Option<String>, u16);

/// A `roleArn` the account `account_id` may be configured with: a role of THAT account
/// when `account_id` is an account number, and a role of this emulator's own account when
/// it is not.
///
/// The account-ownership rule compares the ARN's account field with the `accountId` being
/// configured, and an `accountId` that is no account number at all (a 48-character value,
/// say, which `TargetAccountId`'s `@length(12..48)` still admits) names no account for a
/// role to belong to — so such a row carries a well-formed role ARN of any account, and the
/// bound it is really about is the one it is testing.
fn owned_role_arn(account_id: &str, role_name: &str) -> String {
    let account = if account_id.len() == 12 && account_id.bytes().all(|byte| byte.is_ascii_digit())
    {
        account_id
    } else {
        "000000000000"
    };
    format!("arn:aws:iam::{account}:role/{role_name}")
}

/// An IAM role ARN of exactly `len` characters, in the account `account_id`, padded in the
/// role name.
///
/// The account is the CONFIGURED one because a target account configuration's `roleArn`
/// has to name a role of the account it configures — which is the rule
/// `target_account_role_ownership.rs` asserts — so every well-formed value this builds
/// belongs to the row's own account.
///
/// WELL-FORMED only from [`SHORTEST_ROLE_ARN`] characters up: under that length the value
/// is still `arn:`-shaped (and still satisfies `pattern ^[\S]+$`) but is not an IAM role
/// ARN, which is exactly what the rows at and below `RoleArn`'s modelled floor need.
fn role_arn_of(account_id: &str, len: usize) -> String {
    let prefix = format!("arn:aws:iam::{account_id}:role/");
    if len <= prefix.len() {
        // Still an `arn:`-shaped value, just a shorter one: used for the published floor
        // and the value one character under it.
        format!("arn:aws:iam::1:role/{}", "x".repeat(len.saturating_sub(20)))[..len].to_string()
    } else {
        format!("{prefix}{}", "r".repeat(len - prefix.len()))
    }
}

/// One round-trip row: what the create request carries for one account.
struct Row {
    case: &'static str,
    account_id: String,
    role_arn: String,
    description: Option<String>,
    client_token: Option<String>,
}

/// The rows the round-trip rule is asserted over.
///
/// They differ along everything that rule generalises over: the length and shape of the
/// `accountId` naming the configuration (both published bounds of `TargetAccountId`),
/// the length and shape of the `roleArn` (the shortest WELL-FORMED ARN, a role with a
/// path, a role at the published ceiling), whether a `description` is absent, empty, plain
/// text, non-ASCII, or at `TargetAccountConfigurationDescription`'s published ceiling,
/// and whether the request carries a `clientToken` at all.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "no-description-no-token",
            account_id: account_id_of(ACCOUNT_ID_FLOOR),
            role_arn: format!(
                "arn:aws:iam::{}:role/FisTargetAccount",
                account_id_of(ACCOUNT_ID_FLOOR)
            ),
            description: None,
            client_token: None,
        },
        Row {
            case: "empty-description",
            account_id: "210987654321".to_string(),
            role_arn: "arn:aws:iam::210987654321:role/service-role/fis-target".to_string(),
            description: Some(String::new()),
            client_token: Some("token-empty-description".to_string()),
        },
        Row {
            case: "plain-description-role-arn-at-the-shortest-well-formed-length",
            account_id: "333333333333".to_string(),
            role_arn: role_arn_of("333333333333", SHORTEST_ROLE_ARN),
            description: Some("the staging account".to_string()),
            client_token: Some("token-floor-role".to_string()),
        },
        Row {
            case: "non-ascii-description",
            account_id: "444444444444".to_string(),
            role_arn: "arn:aws-cn:iam::444444444444:role/fis-目标账户".to_string(),
            description: Some("説明 — ünïcødé ✅ описание".to_string()),
            client_token: Some("token-非ascii".to_string()),
        },
        Row {
            case: "description-at-the-ceiling-account-id-at-the-ceiling",
            account_id: account_id_of(ACCOUNT_ID_CEILING),
            role_arn: role_arn_of("000000000000", ROLE_ARN_CEILING),
            description: Some("d".repeat(DESCRIPTION_CEILING)),
            client_token: Some("token-ceiling".to_string()),
        },
    ]
}

/// The `description` a configuration REPORTS once a write carried `sent`.
///
/// A description with characters in it round-trips exactly, while an explicitly empty one
/// is normalized like an omitted one — the configuration then has no description at all.
/// `target_account_configuration_description.rs` asserts that rule on its own; here it
/// only says what these rows expect to read back.
fn reported_description(sent: Option<&str>) -> Option<&str> {
    sent.filter(|description| !description.is_empty())
}

/// What the configuration for one row has to look like, whoever reports it.
fn assert_matches_row(case: &str, source: &str, row: &Row, configuration: &Value) {
    assert_configuration(
        case,
        source,
        &row.account_id,
        &row.role_arn,
        reported_description(row.description.as_deref()),
        configuration,
    );
}

/// What a reported `targetAccountConfiguration` has to carry: the name it is stored
/// under, the stored `roleArn`, and the stored `description` — absent when the record
/// holds none.
fn assert_configuration(
    case: &str,
    source: &str,
    account_id: &str,
    role_arn: &str,
    description: Option<&str>,
    configuration: &Value,
) {
    assert_eq!(
        configuration["accountId"],
        json!(account_id),
        "case {case:?}: expected {source} to name accountId {account_id:?}, \
         got {configuration:?}"
    );
    assert_eq!(
        configuration["roleArn"],
        json!(role_arn),
        "case {case:?}: expected {source} to report roleArn {role_arn:?}, \
         got {configuration:?}"
    );
    match description {
        Some(description) => assert_eq!(
            configuration["description"],
            json!(description),
            "case {case:?}: expected {source} to report a description of {} characters, \
             got {configuration:?}",
            description.chars().count()
        ),
        None => assert!(
            configuration.get("description").is_none_or(Value::is_null),
            "case {case:?}: expected {source} to report NO description, got {configuration:?}"
        ),
    }
}

#[tokio::test]
async fn a_created_target_account_configuration_reads_back_unchanged() {
    let router = emulator();
    let template_id = template(&router, "round-trip").await;

    for row in rows() {
        let created = create_configuration(
            &router,
            &template_id,
            &row.account_id,
            create_body(
                &row.role_arn,
                row.description.as_deref(),
                row.client_token.as_deref(),
            ),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {:?}: expected HTTP 200 from CreateTargetAccountConfiguration, got {} \
             with {:?}",
            row.case, created.status, created.body
        );
        assert_matches_row(
            row.case,
            "the create answer",
            &row,
            created.configuration(),
        );

        // The round trip: the read reports the record the create returned.
        let fetched = get_configuration(&router, &template_id, &row.account_id).await;
        assert_eq!(
            fetched.status, 200,
            "case {:?}: expected HTTP 200 from GetTargetAccountConfiguration, got {} with {:?}",
            row.case, fetched.status, fetched.body
        );
        assert_eq!(
            fetched.configuration(),
            created.configuration(),
            "case {:?}: the read disagreed with the create: {:?}",
            row.case,
            fetched.body
        );
        assert_matches_row(row.case, "the read", &row, fetched.configuration());

        // Reading twice reports the same record: a read changes nothing.
        let again = get_configuration(&router, &template_id, &row.account_id).await;
        assert_eq!(
            again.configuration(),
            fetched.configuration(),
            "case {:?}: two reads of one configuration disagreed: {:?}",
            row.case,
            again.body
        );
    }

    // Every row landed on the same template, under its own name.
    let fetched = get_template(&router, &template_id).await;
    assert_eq!(
        fetched.member("experimentTemplate")["targetAccountConfigurationsCount"],
        json!(rows().len()),
        "case round-trip: expected the template to count every configuration created on \
         it, got {:?}",
        fetched.body
    );
}

#[tokio::test]
async fn a_template_counts_the_configurations_it_holds_and_an_experiment_snapshots_that_count() {
    let router = emulator();

    // The rows differ along how many accounts the template is configured for: none, one,
    // several.
    for configured in [0usize, 1, 3] {
        let case = format!("{configured}-configurations");
        // A template that is configured for no account at all targets a SINGLE account:
        // a multi-account template is only one FIS runs once it names a target account,
        // which `account_targeting_agreement.rs` owns.
        let targeting = if configured == 0 {
            "single-account"
        } else {
            "multi-account"
        };
        let template_id = template_targeting(&router, &case, targeting).await;
        for index in 0..configured {
            let account_id = format!("55555555{index:04}");
            let created = create_configuration(
                &router,
                &template_id,
                &account_id,
                create_body(
                    &format!("arn:aws:iam::{account_id}:role/fis-counted"),
                    Some(&format!("account {index}")),
                    Some(&format!("token-count-{case}-{index}")),
                ),
            )
            .await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: expected HTTP 200 creating configuration {index}, got {} \
                 with {:?}",
                created.status, created.body
            );
        }

        let fetched = get_template(&router, &template_id).await;
        assert_eq!(
            fetched.member("experimentTemplate")["targetAccountConfigurationsCount"],
            json!(configured),
            "case {case:?}: expected the template to report {configured} target account \
             configurations, got {:?}",
            fetched.body
        );

        // An experiment carries the count its template had when it was started.
        let started = send(
            &router,
            "POST",
            "/experiments",
            Some(json!({
                "clientToken": format!("start-{case}"),
                "experimentTemplateId": template_id
            })),
        )
        .await;
        assert_eq!(
            started.status, 200,
            "case {case:?}: expected HTTP 200 from StartExperiment, got {} with {:?}",
            started.status, started.body
        );
        assert_eq!(
            started.member("experiment")["targetAccountConfigurationsCount"],
            json!(configured),
            "case {case:?}: expected the experiment to carry its template's count of \
             {configured}, got {:?}",
            started.body
        );
    }
}

#[tokio::test]
async fn a_second_create_for_one_account_conflicts_unless_it_replays_the_token() {
    let router = emulator();

    // The rows differ along what the second create carries: the same token (a retry),
    // another token, or no token at all.
    for row in rows().into_iter().filter(|row| row.client_token.is_some()) {
        let template_id = template(&router, row.case).await;
        let first = create_configuration(
            &router,
            &template_id,
            &row.account_id,
            create_body(
                &row.role_arn,
                row.description.as_deref(),
                row.client_token.as_deref(),
            ),
        )
        .await;
        assert_eq!(
            first.status, 200,
            "case {:?}: expected HTTP 200 from the first create, got {} with {:?}",
            row.case, first.status, first.body
        );

        // A retry of the request that created it: the `@idempotencyToken` is replayed, so
        // the configuration it already created comes back.
        let retried = create_configuration(
            &router,
            &template_id,
            &row.account_id,
            create_body(
                &row.role_arn,
                row.description.as_deref(),
                row.client_token.as_deref(),
            ),
        )
        .await;
        assert_eq!(
            retried.status, 200,
            "case {:?}: expected a replayed clientToken to answer HTTP 200, got {} with {:?}",
            row.case, retried.status, retried.body
        );
        assert_eq!(
            retried.configuration(),
            first.configuration(),
            "case {:?}: a replayed clientToken must report the first configuration, got {:?}",
            row.case,
            retried.body
        );

        // Any other create for the same account is a conflict, and leaves the stored
        // record alone.
        for (label, token) in [
            ("another-token", Some("token-second-attempt")),
            ("no-token", None),
        ] {
            let case = format!("{}/{label}", row.case);
            let conflicting = create_configuration(
                &router,
                &template_id,
                &row.account_id,
                create_body(
                    // The row's own `roleArn`: a configuration's role has to belong to the
                    // account it configures, so a second create for that account carries a
                    // role of it too. What makes this a second create rather than a replay
                    // is the token, and the description is what would be overwritten.
                    &row.role_arn,
                    Some("a second create for the same account"),
                    token,
                ),
            )
            .await;
            assert_eq!(
                conflicting.status, 409,
                "case {case:?}: expected HTTP 409 for a second create, got {} with {:?}",
                conflicting.status, conflicting.body
            );
            assert_eq!(
                conflicting.error_code(),
                "ConflictException",
                "case {case:?}: expected ConflictException, got {:?}",
                conflicting.body
            );

            let fetched = get_configuration(&router, &template_id, &row.account_id).await;
            assert_eq!(
                fetched.configuration(),
                first.configuration(),
                "case {case:?}: a refused create must not change the stored \
                 configuration, got {:?}",
                fetched.body
            );
        }

        // One account, one configuration: nothing above created a second.
        let fetched = get_template(&router, &template_id).await;
        assert_eq!(
            fetched.member("experimentTemplate")["targetAccountConfigurationsCount"],
            json!(1),
            "case {:?}: expected the template to hold exactly one configuration, got {:?}",
            row.case,
            fetched.body
        );
    }
}

#[tokio::test]
async fn a_name_the_store_does_not_hold_answers_resource_not_found() {
    let router = emulator();
    let configured = template(&router, "configured").await;
    let other = template(&router, "other").await;
    let account_id = "777777777777";
    let created = create_configuration(
        &router,
        &configured,
        account_id,
        create_body(
            "arn:aws:iam::777777777777:role/fis-isolated",
            Some("configured on one template only"),
            Some("token-isolated"),
        ),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case setup: expected HTTP 200 from CreateTargetAccountConfiguration, got {} with {:?}",
        created.status, created.body
    );

    // An experiment-template id no record carries, within the modelled `^[\S]+$` and
    // `@length(0..64)`.
    let unknown_template = "EXT000000000099";

    // The rows differ along WHICH half of the name misses: the template, the account, or
    // both — and a configured account read on another template is a miss too.
    let cases: [(&str, &str, &str); 4] = [
        ("unknown-template", unknown_template, account_id),
        ("unknown-account", &configured, "888888888888"),
        ("unknown-both", unknown_template, "888888888888"),
        ("configured-on-another-template", &other, account_id),
    ];
    for (case, template_id, account) in cases {
        let fetched = get_configuration(&router, template_id, account).await;
        assert_eq!(
            fetched.status, 404,
            "case {case:?}: expected HTTP 404 from GetTargetAccountConfiguration, got {} \
             with {:?}",
            fetched.status, fetched.body
        );
        assert_eq!(
            fetched.error_code(),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            fetched.body
        );
    }

    // A create for a template the store does not hold has nothing to configure.
    let created = create_configuration(
        &router,
        unknown_template,
        account_id,
        create_body(
            "arn:aws:iam::777777777777:role/fis-no-template",
            None,
            Some("token-no-template"),
        ),
    )
    .await;
    assert_eq!(
        created.status, 404,
        "case create-unknown-template: expected HTTP 404, got {} with {:?}",
        created.status, created.body
    );
    assert_eq!(
        created.error_code(),
        "ResourceNotFoundException",
        "case create-unknown-template: expected ResourceNotFoundException, got {:?}",
        created.body
    );

    // The refused create persisted nothing.
    let fetched = get_template(&router, &other).await;
    assert_eq!(
        fetched.member("experimentTemplate")["targetAccountConfigurationsCount"],
        json!(0),
        "case create-unknown-template: a refused create must persist nothing, got {:?}",
        fetched.body
    );
}

#[tokio::test]
async fn a_request_outside_a_modelled_bound_is_refused() {
    let router = emulator();

    // Each published bound of each constrained member, and one past it.
    let cases: Vec<(String, String, Option<String>, u16)> = vec![
        (
            format!("accountId at the floor ({ACCOUNT_ID_FLOOR})"),
            account_id_of(ACCOUNT_ID_FLOOR),
            None,
            200,
        ),
        (
            format!("accountId at the ceiling ({ACCOUNT_ID_CEILING})"),
            account_id_of(ACCOUNT_ID_CEILING),
            None,
            200,
        ),
        (
            "accountId below the floor".to_string(),
            account_id_of(ACCOUNT_ID_FLOOR - 1),
            None,
            400,
        ),
        (
            "accountId above the ceiling".to_string(),
            account_id_of(ACCOUNT_ID_CEILING + 1),
            None,
            400,
        ),
        (
            format!("description at the floor ({DESCRIPTION_FLOOR})"),
            "121212121212".to_string(),
            Some("d".repeat(DESCRIPTION_FLOOR)),
            200,
        ),
        (
            format!("description at the ceiling ({DESCRIPTION_CEILING})"),
            "131313131313".to_string(),
            Some("d".repeat(DESCRIPTION_CEILING)),
            200,
        ),
        (
            "description above the ceiling".to_string(),
            "141414141414".to_string(),
            Some("d".repeat(DESCRIPTION_CEILING + 1)),
            400,
        ),
    ];
    for (case, account_id, description, expected) in cases {
        let template_id = template(&router, &case).await;
        let answer = create_configuration(
            &router,
            &template_id,
            &account_id,
            create_body(
                // A role the account being configured may carry: the two members have to
                // agree about which account the role belongs to.
                &owned_role_arn(&account_id, "fis-bounds"),
                description.as_deref(),
                // Its OWN token: an `@idempotencyToken` is bound to the request that
                // first carried it, so each row — a different request — carries a
                // different one and what is under test is the bound and not the token.
                Some(&crate::resource_tagging::client_token("bounds", &case)),
            ),
        )
        .await;
        assert_eq!(
            answer.status, expected,
            "case {case:?}: expected HTTP {expected}, got {} with {:?}",
            answer.status, answer.body
        );
    }

    // `roleArn` on each of its published bounds, and one past each. A value at the
    // modelled floor is INSIDE the shape's bounds and still shorter than any IAM role
    // ARN, so it is refused as a malformed ARN rather than accepted.
    for (case, len, expected) in [
        (
            "roleArn at the shortest well-formed ARN",
            SHORTEST_ROLE_ARN,
            200,
        ),
        ("roleArn at the ceiling", ROLE_ARN_CEILING, 200),
        (
            "roleArn at the floor, too short to be an ARN",
            ROLE_ARN_FLOOR,
            400,
        ),
        ("roleArn below the floor", ROLE_ARN_FLOOR - 1, 400),
        ("roleArn above the ceiling", ROLE_ARN_CEILING + 1, 400),
    ] {
        let template_id = template(&router, case).await;
        let role_arn = role_arn_of("151515151515", len);
        assert_eq!(
            role_arn.chars().count(),
            len,
            "case {case:?}: the test built a roleArn of the wrong length: {role_arn:?}"
        );
        let answer = create_configuration(
            &router,
            &template_id,
            "151515151515",
            create_body(
                &role_arn,
                None,
                // Per row, for the reason the rows above carry their own token.
                Some(&crate::resource_tagging::client_token("role-bounds", case)),
            ),
        )
        .await;
        assert_eq!(
            answer.status, expected,
            "case {case:?}: expected HTTP {expected}, got {} with {:?}",
            answer.status, answer.body
        );
    }
}

/// The `roleArn` a configuration the update suite touches is CREATED with: a role of the
/// account it configures, so what an update left alone is distinguishable from what it
/// replaced.
fn baseline_role_arn(account_id: &str) -> String {
    owned_role_arn(account_id, "fis-update-baseline")
}

/// One partial-update row: what the configuration was created with, and what the update
/// request then carries for it.
struct UpdateRow {
    case: &'static str,
    /// The account this row's configuration is created under — one per row, so no row's
    /// update can touch another's record. Twelve characters, `TargetAccountId`'s published
    /// floor, and an account number, so the row's `roleArn` can belong to it.
    account_id: &'static str,
    /// The `description` the CREATE carried, so a row can start from a record that has
    /// one and a row can start from a record that has none.
    baseline_description: Option<String>,
    /// The `roleArn` the UPDATE carries, or `None` when it omits it.
    role_arn: Option<String>,
    /// The `description` the UPDATE carries, or `None` when it omits it.
    description: Option<String>,
}

impl UpdateRow {
    /// The rule, spelled out once: a member the update carries replaces the stored value,
    /// a member it omits keeps the stored value.
    fn expected_role_arn(&self) -> String {
        self.role_arn
            .clone()
            .unwrap_or_else(|| baseline_role_arn(self.account_id))
    }

    /// See [`UpdateRow::expected_role_arn`] — and an absent `description` stays absent
    /// only while no update carries one. A carried description is reported as
    /// [`reported_description`] says, so an explicitly empty one clears what was stored.
    fn expected_description(&self) -> Option<String> {
        let stored = match self.description.as_deref() {
            Some(carried) => Some(carried),
            None => self.baseline_description.as_deref(),
        };
        reported_description(stored).map(str::to_string)
    }
}

/// The rows the partial-update rule is asserted over.
///
/// They differ along everything that rule generalises over: WHICH members the update
/// carries (neither, `roleArn` only, `description` only, both), what the record already
/// held (a description, or none at all), and the value and shape of each carried member —
/// a role with a path, a role at the shortest WELL-FORMED ARN length and at `RoleArn`'s
/// published ceiling, a description that is empty, non-ASCII, or at
/// `TargetAccountConfigurationDescription`'s published ceiling.
fn update_rows() -> Vec<UpdateRow> {
    vec![
        UpdateRow {
            case: "carries-nothing",
            account_id: "900000000000",
            baseline_description: Some("the record the update leaves alone".to_string()),
            role_arn: None,
            description: None,
        },
        UpdateRow {
            case: "carries-nothing-onto-a-record-with-no-description",
            account_id: "900000000001",
            baseline_description: None,
            role_arn: None,
            description: None,
        },
        UpdateRow {
            case: "role-arn-only",
            account_id: "900000000002",
            baseline_description: Some("kept while only the roleArn moves".to_string()),
            role_arn: Some("arn:aws:iam::900000000002:role/service-role/fis-updated".to_string()),
            description: None,
        },
        UpdateRow {
            case: "role-arn-only-at-the-shortest-well-formed-length",
            account_id: "900000000003",
            baseline_description: None,
            role_arn: Some(role_arn_of("900000000003", SHORTEST_ROLE_ARN)),
            description: None,
        },
        UpdateRow {
            case: "description-only",
            account_id: "900000000004",
            baseline_description: Some("the description before the update".to_string()),
            role_arn: None,
            description: Some("the description after the update".to_string()),
        },
        UpdateRow {
            case: "description-only-onto-a-record-that-had-none",
            account_id: "900000000005",
            baseline_description: None,
            role_arn: None,
            description: Some("a description the create never carried".to_string()),
        },
        UpdateRow {
            case: "empty-description",
            account_id: "900000000006",
            baseline_description: Some("about to become empty".to_string()),
            role_arn: None,
            description: Some(String::new()),
        },
        UpdateRow {
            case: "both-members-non-ascii-description",
            account_id: "900000000007",
            baseline_description: Some("説明の前".to_string()),
            role_arn: Some("arn:aws-cn:iam::900000000007:role/fis-目标账户-updated".to_string()),
            description: Some("説明 — ünïcødé ✅ описание".to_string()),
        },
        UpdateRow {
            case: "both-members-at-their-ceilings",
            account_id: "900000000008",
            baseline_description: None,
            role_arn: Some(role_arn_of("900000000008", ROLE_ARN_CEILING)),
            description: Some("u".repeat(DESCRIPTION_CEILING)),
        },
    ]
}

#[tokio::test]
async fn an_update_replaces_the_members_it_carries_and_keeps_the_rest() {
    let router = emulator();
    let template_id = template(&router, "partial-update").await;
    let rows = update_rows();

    for row in &rows {
        let account_id = row.account_id;
        let created = create_configuration(
            &router,
            &template_id,
            account_id,
            create_body(
                &baseline_role_arn(account_id),
                row.baseline_description.as_deref(),
                Some(&format!("token-update-{account_id}")),
            ),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {:?}: expected HTTP 200 creating the record to update, got {} with {:?}",
            row.case, created.status, created.body
        );

        let expected_role_arn = row.expected_role_arn();
        let expected_description = row.expected_description();
        let body = update_body(row.role_arn.as_deref(), row.description.as_deref());

        let updated = update_configuration(&router, &template_id, account_id, body.clone()).await;
        assert_eq!(
            updated.status, 200,
            "case {:?}: expected HTTP 200 from UpdateTargetAccountConfiguration, got {} \
             with {:?}",
            row.case, updated.status, updated.body
        );
        assert_configuration(
            row.case,
            "the update answer",
            account_id,
            &expected_role_arn,
            expected_description.as_deref(),
            updated.configuration(),
        );

        // The round trip: the read reports the record the update returned, so the update
        // persisted what it answered.
        let fetched = get_configuration(&router, &template_id, account_id).await;
        assert_eq!(
            fetched.status, 200,
            "case {:?}: expected HTTP 200 from GetTargetAccountConfiguration, got {} with {:?}",
            row.case, fetched.status, fetched.body
        );
        assert_eq!(
            fetched.configuration(),
            updated.configuration(),
            "case {:?}: the read disagreed with the update: {:?}",
            row.case,
            fetched.body
        );
        assert_configuration(
            row.case,
            "the read after the update",
            account_id,
            &expected_role_arn,
            expected_description.as_deref(),
            fetched.configuration(),
        );

        // Idempotent: the same update applied again reports the same record.
        let again = update_configuration(&router, &template_id, account_id, body).await;
        assert_eq!(
            again.configuration(),
            updated.configuration(),
            "case {:?}: applying one update twice reported two different records: {:?}",
            row.case,
            again.body
        );
    }

    // Every row updated the record its name already held: no update created a second one.
    let fetched = get_template(&router, &template_id).await;
    assert_eq!(
        fetched.member("experimentTemplate")["targetAccountConfigurationsCount"],
        json!(rows.len()),
        "case partial-update: expected the template to still hold exactly the {} \
         configurations the creates made, got {:?}",
        rows.len(),
        fetched.body
    );

    // One row's update left every other row's record alone.
    for row in &rows {
        let fetched = get_configuration(&router, &template_id, row.account_id).await;
        assert_configuration(
            row.case,
            "the read after every other row was updated",
            row.account_id,
            &row.expected_role_arn(),
            row.expected_description().as_deref(),
            fetched.configuration(),
        );
    }
}

#[tokio::test]
async fn an_update_naming_a_configuration_the_store_does_not_hold_answers_resource_not_found() {
    let router = emulator();
    let configured = template(&router, "update-configured").await;
    let other = template(&router, "update-other").await;
    let account_id = "787878787878";
    let created = create_configuration(
        &router,
        &configured,
        account_id,
        create_body(
            &baseline_role_arn(account_id),
            Some("configured on one template only"),
            Some("token-update-isolated"),
        ),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case setup: expected HTTP 200 from CreateTargetAccountConfiguration, got {} with {:?}",
        created.status, created.body
    );

    // An experiment-template id no record carries, within the modelled `^[\S]+$` and
    // `@length(0..64)`.
    let unknown_template = "EXT000000000099";

    // The rows differ along WHICH half of the name misses: the template, the account, or
    // both — and an account configured on another template is a miss too.
    let cases: [(&str, &str, &str); 4] = [
        ("unknown-template", unknown_template, account_id),
        ("unknown-account", &configured, "898989898989"),
        ("unknown-both", unknown_template, "898989898989"),
        ("configured-on-another-template", &other, account_id),
    ];
    for (case, template_id, account) in cases {
        let answer = update_configuration(
            &router,
            template_id,
            account,
            update_body(
                // A role of the account the request names, so what the refusal is about is
                // the record that does not exist and not the role.
                Some(&owned_role_arn(account, "fis-no-such-record")),
                Some("an update of a record that does not exist"),
            ),
        )
        .await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected HTTP 404 from UpdateTargetAccountConfiguration, got {} \
             with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            answer.body
        );
    }

    // None of those refusals created or changed anything.
    let fetched = get_configuration(&router, &configured, account_id).await;
    assert_configuration(
        "configured",
        "the read after four refused updates",
        account_id,
        &baseline_role_arn(account_id),
        Some("configured on one template only"),
        fetched.configuration(),
    );
    for (case, template_id) in [("configured", &configured), ("other", &other)] {
        let expected = if case == "configured" { 1 } else { 0 };
        let fetched = get_template(&router, template_id).await;
        assert_eq!(
            fetched.member("experimentTemplate")["targetAccountConfigurationsCount"],
            json!(expected),
            "case {case:?}: expected the template to still hold {expected} configurations \
             after the refused updates, got {:?}",
            fetched.body
        );
    }
}

#[tokio::test]
async fn an_update_outside_a_modelled_bound_is_refused_and_persists_nothing() {
    let router = emulator();

    // `accountId` on each of its published bounds, and one past each. A value within the
    // bounds names a record that exists, so it succeeds; a value outside them is refused
    // before the lookup, so it never reaches one.
    for (case, len, expected) in [
        ("accountId at the floor", ACCOUNT_ID_FLOOR, 200),
        ("accountId at the ceiling", ACCOUNT_ID_CEILING, 200),
        ("accountId below the floor", ACCOUNT_ID_FLOOR - 1, 400),
        ("accountId above the ceiling", ACCOUNT_ID_CEILING + 1, 400),
    ] {
        let template_id = template(&router, case).await;
        let account_id = account_id_of(len);
        if expected == 200 {
            let created = create_configuration(
                &router,
                &template_id,
                &account_id,
                create_body(
                    &baseline_role_arn(&account_id),
                    None,
                    // Per row: each row creates a DIFFERENT configuration, so each one
                    // is a different request and carries its own token.
                    Some(&crate::resource_tagging::client_token(
                        "update-account-bounds",
                        case,
                    )),
                ),
            )
            .await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: expected HTTP 200 creating the record to update, got {} \
                 with {:?}",
                created.status, created.body
            );
        }
        let answer = update_configuration(
            &router,
            &template_id,
            &account_id,
            update_body(Some(&owned_role_arn(&account_id, "fis-bounds")), None),
        )
        .await;
        assert_eq!(
            answer.status, expected,
            "case {case:?}: expected HTTP {expected}, got {} with {:?}",
            answer.status, answer.body
        );
    }

    // `roleArn` and `description` on each of their published bounds, and one past each,
    // carried by the UPDATE. A refused update must leave the stored record exactly as the
    // create left it. Each row states the account it configures, because a `roleArn` the
    // update carries has to name a role of that account.
    let template_id = template(&router, "update-member-bounds").await;
    let cases: Vec<UpdateBoundRow> = vec![
        (
            format!("roleArn at the shortest well-formed ARN ({SHORTEST_ROLE_ARN})"),
            "910000000000",
            Some(role_arn_of("910000000000", SHORTEST_ROLE_ARN)),
            None,
            200,
        ),
        (
            format!("roleArn at the ceiling ({ROLE_ARN_CEILING})"),
            "910000000001",
            Some(role_arn_of("910000000001", ROLE_ARN_CEILING)),
            None,
            200,
        ),
        (
            format!("roleArn at the floor ({ROLE_ARN_FLOOR}), too short to be an ARN"),
            "910000000002",
            Some(role_arn_of("910000000002", ROLE_ARN_FLOOR)),
            None,
            400,
        ),
        (
            "roleArn below the floor".to_string(),
            "910000000003",
            Some(role_arn_of("910000000003", ROLE_ARN_FLOOR - 1)),
            None,
            400,
        ),
        (
            "roleArn above the ceiling".to_string(),
            "910000000004",
            Some(role_arn_of("910000000004", ROLE_ARN_CEILING + 1)),
            None,
            400,
        ),
        (
            format!("description at the floor ({DESCRIPTION_FLOOR})"),
            "910000000005",
            None,
            Some("d".repeat(DESCRIPTION_FLOOR)),
            200,
        ),
        (
            format!("description at the ceiling ({DESCRIPTION_CEILING})"),
            "910000000006",
            None,
            Some("d".repeat(DESCRIPTION_CEILING)),
            200,
        ),
        (
            "description above the ceiling".to_string(),
            "910000000007",
            None,
            Some("d".repeat(DESCRIPTION_CEILING + 1)),
            400,
        ),
    ];
    for (case, account_id, role_arn, description, expected) in cases {
        let created = create_configuration(
            &router,
            &template_id,
            account_id,
            create_body(
                &baseline_role_arn(account_id),
                Some("the record a refused update must not touch"),
                Some(&format!("token-member-bounds-{account_id}")),
            ),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected HTTP 200 creating the record to update, got {} with {:?}",
            created.status, created.body
        );

        let answer = update_configuration(
            &router,
            &template_id,
            account_id,
            update_body(role_arn.as_deref(), description.as_deref()),
        )
        .await;
        assert_eq!(
            answer.status, expected,
            "case {case:?}: expected HTTP {expected}, got {} with {:?}",
            answer.status, answer.body
        );

        if expected == 400 {
            let fetched = get_configuration(&router, &template_id, account_id).await;
            assert_configuration(
                &case,
                "the read after a refused update",
                account_id,
                &baseline_role_arn(account_id),
                Some("the record a refused update must not touch"),
                fetched.configuration(),
            );
        }
    }
}
