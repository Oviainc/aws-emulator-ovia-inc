// SPDX-License-Identifier: CC0-1.0

//! Cloud connectors end to end: `CreateCloudConnector` mints one,
//! `UpdateCloudConnector` changes it, `GetCloudConnector` reports what is held.
//!
//! Every case goes through the front door a client uses — the shared harness in
//! `all/parameter_store.rs` ([`router`], [`call`], [`call_in_region`]) dispatches
//! `POST /` with `X-Amz-Target: AmazonSSM.<Operation>` through the multi-service
//! router. Nothing reaches into the state struct, so a response that is not computed
//! from persisted state cannot pass: every read follows a write.
//!
//! The suites are tables. The rule under test is that an update replaces the members it
//! CARRIES and leaves every member it omits as it was, and that an update of a
//! `CloudConnectorId` the account holds no connector for at the endpoint's Region is
//! refused with `ResourceNotFoundException` and writes nothing. Each row is one input
//! along the axis that generalises over — which members the update carries, how many,
//! their size at the modelled bounds, the form and the subscription count of a
//! configuration, the Region the connector is held at, and how many callers ask at once
//! — rather than the single update a recorded unit happened to send.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::parameter_store::{call, call_in_region, error_code, router, ACCOUNT, REGION};

/// The sentence `ResourceNotFoundException` carries for a `CloudConnectorId` the account
/// holds no cloud connector for
/// (`crate::state::cloud_connectors::CLOUD_CONNECTOR_NOT_FOUND`): fixed text, naming
/// the resource class and interpolating nothing from the request.
const NOT_FOUND_MESSAGE: &str =
    service_com_amazonaws_ssm_amazonssm::state::cloud_connectors::CLOUD_CONNECTOR_NOT_FOUND;

/// How many cloud connectors the store holds at once
/// (`crate::state::cloud_connectors::MAX_CLOUD_CONNECTORS`): the emulator's own ceiling
/// on the one map a create grows, not a quota AWS publishes.
const MAX_CLOUD_CONNECTORS: usize =
    service_com_amazonaws_ssm_amazonssm::state::cloud_connectors::MAX_CLOUD_CONNECTORS;

/// The published ceiling of `DisplayName`
/// (`smithy_query(query="shape", shape="DisplayName")`: `length 1..256`).
const DISPLAY_NAME_MAX: usize = 256;

/// The published floor of the same member.
const DISPLAY_NAME_MIN: usize = 1;

/// The published ceiling of `CloudConnectorDescription`
/// (`smithy_query(query="shape", shape="CloudConnectorDescription")`: `length 0..1024`).
const DESCRIPTION_MAX: usize = 1024;

/// An Azure tenant id: `AzureConfiguration$TenantId` is a hex UUID
/// (`^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$`).
const TENANT_ID: &str = "11111111-2222-3333-4444-555555555555";

/// See [`TENANT_ID`]: `AzureConfiguration$ApplicationId` has the same shape.
const APPLICATION_ID: &str = "66666666-7777-8888-9999-aaaaaaaaaaaa";

/// The tenant an update's replacement configuration names instead.
const OTHER_TENANT_ID: &str = "bbbbbbbb-cccc-dddd-eeee-ffffffffffff";

/// See [`OTHER_TENANT_ID`].
const OTHER_APPLICATION_ID: &str = "01234567-89ab-cdef-0123-456789abcdef";

/// `AzureSubscription$Id` values, same UUID shape again.
const SUBSCRIPTION_IDS: &[&str] = &[
    "aaaaaaa1-0000-0000-0000-000000000001",
    "aaaaaaa2-0000-0000-0000-000000000002",
    "aaaaaaa3-0000-0000-0000-000000000003",
];

/// A `CloudConnectorConfiguration`: the `AzureConfiguration` member of the modelled
/// union, targeting `subscriptions` of them (none at all when the list is empty, which
/// is a configuration that carries no `Targets` member).
fn azure_configuration(tenant: &str, application: &str, subscriptions: &[&str]) -> Value {
    let mut azure = json!({
        "TenantId": tenant,
        "TenantDisplayName": "Contoso",
        "ApplicationId": application,
        "ApplicationDisplayName": "Systems Manager connector",
    });
    if !subscriptions.is_empty() {
        azure["Targets"] = json!({
            "Subscriptions": subscriptions
                .iter()
                .map(|id| json!({ "Id": id, "DisplayName": format!("subscription {id}") }))
                .collect::<Vec<Value>>(),
        });
    }
    json!({ "AzureConfiguration": azure })
}

/// The IAM role a connector "uses to communicate with the third-party cloud
/// environment", in the form `CloudConnectorIamRoleArn` models.
fn role_arn(name: &str) -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/{name}")
}

/// The Config connector ARN a create associates the cloud connector with, in the form
/// `ConfigConnectorArn` models.
fn config_connector_arn(region: &str, name: &str) -> String {
    format!("arn:aws:config:{region}:{ACCOUNT}:connector/{name}")
}

/// The ARN the service must mint for the connector `id` names in `region`
/// (`CloudConnectorArn`).
fn cloud_connector_arn(region: &str, id: &str) -> String {
    format!("arn:aws:ssm:{region}:{ACCOUNT}:cloud-connector/{id}")
}

/// The create body every case starts from, at `region`'s endpoint.
fn create_body(region: &str) -> Value {
    json!({
        "DisplayName": "Azure production tenant",
        "Description": "the connector every case starts from",
        "RoleArn": role_arn("SsmCloudConnectorRole"),
        "ConfigConnectorArn": config_connector_arn(region, "production"),
        "Configuration": azure_configuration(TENANT_ID, APPLICATION_ID, &SUBSCRIPTION_IDS[..1]),
        "Tags": [{ "Key": "Team", "Value": "platform" }],
    })
}

/// `CreateCloudConnector` at `region`'s endpoint, asserting it succeeded, and returning
/// the `CloudConnectorId` it minted.
async fn create_ok(router: &MultiServiceRouter, case: &str, region: &str, body: &Value) -> String {
    let (status, response) = call_in_region(router, region, "CreateCloudConnector", body).await;
    assert_eq!(
        status, 200,
        "case {case}: CreateCloudConnector must succeed, got {status} {response}"
    );
    let id = response["CloudConnectorId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: a create reports a CloudConnectorId, got {response}"))
        .to_string();
    assert_eq!(
        id.len(),
        36,
        "case {case}: a CloudConnectorId is 36 characters, got {id:?}"
    );
    id
}

/// `UpdateCloudConnector` with `body`, returning `(status, body)`.
async fn update(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "UpdateCloudConnector", body).await
}

/// `GetCloudConnector` of `id` at `region`'s endpoint, returning `(status, body)`.
async fn get_in_region(router: &MultiServiceRouter, region: &str, id: &str) -> (u16, Value) {
    call_in_region(
        router,
        region,
        "GetCloudConnector",
        &json!({ "CloudConnectorId": id }),
    )
    .await
}

/// `GetCloudConnector` of `id`, asserting the read succeeded, and returning its body.
async fn get_ok(router: &MultiServiceRouter, case: &str, id: &str) -> Value {
    let (status, response) = get_in_region(router, REGION, id).await;
    assert_eq!(
        status, 200,
        "case {case}: GetCloudConnector {id} must succeed, got {status} {response}"
    );
    response
}

/// Assert `response` is the refusal an absent cloud connector carries: HTTP 400,
/// `ResourceNotFoundException`, and the one sentence this service words for it.
fn assert_not_found(case: &str, status: u16, response: &Value) {
    assert_eq!(
        (status, error_code(response).as_str()),
        (400, "ResourceNotFoundException"),
        "case {case}: expected 400 ResourceNotFoundException, got {status} {response}"
    );
    assert_eq!(
        response["Message"].as_str(),
        Some(NOT_FOUND_MESSAGE),
        "case {case}: expected the message {NOT_FOUND_MESSAGE:?}, got {response}"
    );
}

/// One row of the update table: which members the update carries.
#[derive(Debug)]
struct UpdateCase {
    /// What the row is about, named in every assertion.
    case: &'static str,
    /// The `DisplayName` the update carries, or `None` when it omits it.
    display_name: Option<String>,
    /// The `Description` the update carries, or `None` when it omits it.
    description: Option<String>,
    /// The `Configuration` the update carries, or `None` when it omits it.
    configuration: Option<Value>,
}

impl UpdateCase {
    /// The update body for this row, carrying the id and only the members it names.
    fn body(&self, id: &str) -> Value {
        let mut body = json!({ "CloudConnectorId": id });
        if let Some(display_name) = &self.display_name {
            body["DisplayName"] = json!(display_name);
        }
        if let Some(description) = &self.description {
            body["Description"] = json!(description);
        }
        if let Some(configuration) = &self.configuration {
            body["Configuration"] = configuration.clone();
        }
        body
    }
}

/// Every row of the update table: the members an update may carry, in every combination
/// and at the published bounds of their sizes, with configurations that differ in
/// tenant, application and subscription count.
fn update_cases() -> Vec<UpdateCase> {
    let replacement = azure_configuration(
        OTHER_TENANT_ID,
        OTHER_APPLICATION_ID,
        &SUBSCRIPTION_IDS[..3],
    );
    vec![
        UpdateCase {
            case: "the display name alone",
            display_name: Some("Azure staging tenant".to_string()),
            description: None,
            configuration: None,
        },
        UpdateCase {
            case: "the description alone",
            display_name: None,
            description: Some("now pointed at the staging tenant".to_string()),
            configuration: None,
        },
        UpdateCase {
            case: "the configuration alone, three subscriptions",
            display_name: None,
            description: None,
            configuration: Some(replacement.clone()),
        },
        UpdateCase {
            case: "the configuration alone, no targets at all",
            display_name: None,
            description: None,
            configuration: Some(azure_configuration(
                OTHER_TENANT_ID,
                OTHER_APPLICATION_ID,
                &[],
            )),
        },
        UpdateCase {
            case: "the display name and the description",
            display_name: Some("Azure staging tenant".to_string()),
            description: Some("renamed and re-described".to_string()),
            configuration: None,
        },
        UpdateCase {
            case: "every updatable member at once",
            display_name: Some("Azure staging tenant".to_string()),
            description: Some("renamed, re-described, re-pointed".to_string()),
            configuration: Some(replacement),
        },
        UpdateCase {
            case: "no updatable member at all",
            display_name: None,
            description: None,
            configuration: None,
        },
        UpdateCase {
            case: "the description emptied",
            display_name: None,
            description: Some(String::new()),
            configuration: None,
        },
        UpdateCase {
            case: "the description at its published ceiling",
            display_name: None,
            description: Some("d".repeat(DESCRIPTION_MAX)),
            configuration: None,
        },
        UpdateCase {
            case: "the display name at its published floor",
            display_name: Some("x".repeat(DISPLAY_NAME_MIN)),
            description: None,
            configuration: None,
        },
        UpdateCase {
            case: "the display name at its published ceiling",
            display_name: Some("n".repeat(DISPLAY_NAME_MAX)),
            description: None,
            configuration: None,
        },
    ]
}

/// An update replaces the members it carries and leaves every member it omits, and
/// every member that is not an update member at all, exactly as the create persisted
/// them — which a read after the write reports.
#[tokio::test]
async fn an_update_replaces_what_it_carries_and_leaves_the_rest() {
    let router = router();
    for case in update_cases() {
        let id = create_ok(&router, case.case, REGION, &create_body(REGION)).await;
        let before = get_ok(&router, case.case, &id).await;

        let (status, response) = update(&router, &case.body(&id)).await;
        assert_eq!(
            status, 200,
            "case {}: UpdateCloudConnector must succeed, got {status} {response}",
            case.case
        );
        assert_eq!(
            response["CloudConnectorId"].as_str(),
            Some(id.as_str()),
            "case {}: an update reports the id it updated, got {response}",
            case.case
        );

        let after = get_ok(&router, case.case, &id).await;
        let expected_display_name = case
            .display_name
            .clone()
            .map_or_else(|| before["DisplayName"].clone(), |name| json!(name));
        assert_eq!(
            after["DisplayName"], expected_display_name,
            "case {}: expected DisplayName {expected_display_name}, got {after}",
            case.case
        );
        let expected_description = case
            .description
            .clone()
            .map_or_else(|| before["Description"].clone(), |text| json!(text));
        assert_eq!(
            after["Description"], expected_description,
            "case {}: expected Description {expected_description}, got {after}",
            case.case
        );
        let expected_configuration = case
            .configuration
            .clone()
            .unwrap_or_else(|| before["Configuration"].clone());
        assert_eq!(
            after["Configuration"], expected_configuration,
            "case {}: expected Configuration {expected_configuration}, got {after}",
            case.case
        );

        // Nothing an update cannot carry moves: the ARN, the Region it carries, the IAM
        // role, the Config connector ARN and the creation instant are what the create
        // persisted.
        assert_eq!(
            after["CloudConnectorArn"].as_str(),
            Some(cloud_connector_arn(REGION, &id).as_str()),
            "case {}: the ARN is the connector's own, got {after}",
            case.case
        );
        for member in ["CloudConnectorArn", "RoleArn", "ConfigConnectorArn", "CreatedAt"] {
            assert_eq!(
                after[member], before[member],
                "case {}: {member} is unchanged by an update, got {after}",
                case.case
            );
        }
        let created = after["CreatedAt"].as_f64().unwrap_or_default();
        let updated = after["UpdatedAt"].as_f64().unwrap_or_default();
        assert!(
            updated >= created,
            "case {}: UpdatedAt {updated} is at or after CreatedAt {created}, got {after}",
            case.case
        );
    }
}

/// `UpdatedAt` is when the last write to the connector ran, not when it was created:
/// after a wait longer than the timestamp's own resolution, an update moves it while
/// `CreatedAt` stays put.
#[tokio::test]
async fn an_update_stamps_when_it_ran() {
    let router = router();
    let id = create_ok(&router, "the stamped connector", REGION, &create_body(REGION)).await;
    let before = get_ok(&router, "before the update", &id).await;
    assert_eq!(
        before["UpdatedAt"], before["CreatedAt"],
        "a connector nothing has updated was last written when it was created, got {before}"
    );

    tokio::time::sleep(Duration::from_millis(1_100)).await;
    let (status, response) = update(
        &router,
        &json!({ "CloudConnectorId": id, "DisplayName": "Stamped" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the update must succeed, got {status} {response}"
    );

    let after = get_ok(&router, "after the update", &id).await;
    assert_eq!(
        after["CreatedAt"], before["CreatedAt"],
        "case the stamped connector: CreatedAt is unchanged by an update, got {after}"
    );
    let created = after["CreatedAt"].as_f64().unwrap_or_default();
    let updated = after["UpdatedAt"].as_f64().unwrap_or_default();
    assert!(
        updated > created,
        "case the stamped connector: UpdatedAt {updated} moved past CreatedAt {created}, \
         got {after}"
    );
}

/// An update naming a well-formed `CloudConnectorId` the account holds no connector for
/// at the endpoint's Region is refused with `ResourceNotFoundException` and the one
/// sentence this service words for it, writes nothing, and leaves the connector that IS
/// held exactly as it was.
#[tokio::test]
async fn an_update_of_an_unheld_id_is_refused_and_writes_nothing() {
    let router = router();
    let held = create_ok(&router, "the held connector", REGION, &create_body(REGION)).await;
    let before = get_ok(&router, "the held connector", &held).await;
    // A connector of ANOTHER Region: Systems Manager is regional, so its id names no
    // connector at this endpoint.
    let elsewhere = create_ok(
        &router,
        "a connector of another Region",
        "eu-central-1",
        &create_body("eu-central-1"),
    )
    .await;
    // One nibble of the held id changed, so the id is as well formed as the held one and
    // names nothing.
    let mut near_miss: Vec<char> = held.chars().collect();
    let last = near_miss.len() - 1;
    near_miss[last] = if near_miss[last] == 'f' { '0' } else { 'f' };
    let near_miss: String = near_miss.into_iter().collect();

    let cases: Vec<(&str, String)> = vec![
        ("an all-zero id", "00000000-0000-0000-0000-000000000000".to_string()),
        ("an id nothing minted", "7f3b2a19-4c5d-4e6f-8a9b-0c1d2e3f4a5b".to_string()),
        ("the held id with one nibble changed", near_miss),
        ("a connector held at another Region", elsewhere),
    ];
    for (case, id) in cases {
        let (status, response) = update(
            &router,
            &json!({ "CloudConnectorId": id, "DisplayName": "Renamed by a stray update" }),
        )
        .await;
        assert_not_found(case, status, &response);

        // The refusal persisted nothing: the id still names no connector...
        let (read_status, read) = get_in_region(&router, REGION, &id).await;
        assert_not_found(case, read_status, &read);
        // ...and the connector that is held is untouched.
        let after = get_ok(&router, case, &held).await;
        assert_eq!(
            after, before,
            "case {case}: a refused update leaves the held connector as it was, got {after}"
        );
    }
}

/// Five callers updating one unheld id at once all get the same refusal: the connector
/// is resolved before anything else, so a concurrent request is not reported as a
/// conflict and none of them creates anything.
#[tokio::test]
async fn concurrent_updates_of_an_unheld_id_all_get_the_same_refusal() {
    let router = router();
    let id = "4d5e6f70-1a2b-3c4d-5e6f-708192a3b4c5";
    let racer = |nth: usize| {
        let router = &router;
        let body = json!({
            "CloudConnectorId": id,
            "DisplayName": format!("racer {nth}"),
        });
        async move { update(router, &body).await }
    };
    let answers = tokio::join!(racer(0), racer(1), racer(2), racer(3), racer(4));
    for (nth, (status, response)) in [answers.0, answers.1, answers.2, answers.3, answers.4]
        .into_iter()
        .enumerate()
    {
        assert_not_found(&format!("racer {nth} of 5"), status, &response);
    }
    let (status, response) = get_in_region(&router, REGION, id).await;
    assert_not_found("the id after five racing updates", status, &response);
}

/// An update outside a MODELLED constraint — an id that is not a `CloudConnectorId`, a
/// display name or a description past its published ceiling — is refused as a
/// `ValidationException` rather than resolved, and changes nothing.
#[tokio::test]
async fn an_update_outside_a_modelled_constraint_is_refused() {
    let router = router();
    let held = create_ok(&router, "the held connector", REGION, &create_body(REGION)).await;
    let before = get_ok(&router, "the held connector", &held).await;

    let cases: Vec<(&str, Value)> = vec![
        (
            "an id one character short",
            json!({ "CloudConnectorId": held[..35].to_string() }),
        ),
        (
            "an id one character long",
            json!({ "CloudConnectorId": format!("{held}a") }),
        ),
        (
            "an id in upper-case hex",
            json!({ "CloudConnectorId": held.to_uppercase() }),
        ),
        (
            "an id with a non-hex character",
            json!({ "CloudConnectorId": format!("g{}", &held[1..]) }),
        ),
        (
            "an id without its hyphens",
            json!({ "CloudConnectorId": held.replace('-', "0") }),
        ),
        (
            "a display name one character past its ceiling",
            json!({
                "CloudConnectorId": held,
                "DisplayName": "n".repeat(DISPLAY_NAME_MAX + 1),
            }),
        ),
        (
            "an empty display name, one character below its floor",
            json!({ "CloudConnectorId": held, "DisplayName": "" }),
        ),
        (
            "a description one character past its ceiling",
            json!({
                "CloudConnectorId": held,
                "Description": "d".repeat(DESCRIPTION_MAX + 1),
            }),
        ),
        ("no id at all", json!({ "DisplayName": "Renamed" })),
    ];
    for (case, body) in cases {
        let (status, response) = update(&router, &body).await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "ValidationException"),
            "case {case}: expected 400 ValidationException, got {status} {response}"
        );
        let after = get_ok(&router, case, &held).await;
        assert_eq!(
            after, before,
            "case {case}: a refused update leaves the connector as it was, got {after}"
        );
    }
}

/// The connector store is bounded: once it holds as many connectors as the emulator
/// keeps, a further create is refused with `ServiceQuotaExceededException` and writes
/// nothing, while the connectors already held still read and update.
#[tokio::test]
async fn a_create_past_the_stores_ceiling_is_refused() {
    let router = router();
    let mut ids = Vec::new();
    for nth in 0..MAX_CLOUD_CONNECTORS {
        // Spread the connectors over two Regions, so the ceiling is shown to be on the
        // store as a whole rather than on one Region's share of it.
        let region = if nth % 2 == 0 { REGION } else { "eu-central-1" };
        ids.push((
            region,
            create_ok(&router, "filling the store", region, &create_body(region)).await,
        ));
    }

    let (status, response) =
        call_in_region(&router, REGION, "CreateCloudConnector", &create_body(REGION)).await;
    assert_eq!(
        (status, error_code(&response).as_str()),
        (400, "ServiceQuotaExceededException"),
        "case one past the ceiling: expected 400 ServiceQuotaExceededException, got \
         {status} {response}"
    );
    assert!(
        response["QuotaCode"].as_str().is_some(),
        "case one past the ceiling: the refusal names the quota it reports, got {response}"
    );

    // The refusal wrote nothing, and what the store does hold is untouched: the last
    // connector created still reads, and still takes an update.
    let (region, id) = ids.last().expect("the store was filled").clone();
    let (read_status, read) = get_in_region(&router, region, &id).await;
    assert_eq!(
        read_status, 200,
        "case one past the ceiling: the connectors already held still read, got \
         {read_status} {read}"
    );
    let (update_status, updated) = call_in_region(
        &router,
        region,
        "UpdateCloudConnector",
        &json!({ "CloudConnectorId": id, "DisplayName": "Still updatable" }),
    )
    .await;
    assert_eq!(
        update_status, 200,
        "case one past the ceiling: a held connector still updates, got {update_status} \
         {updated}"
    );
}
