// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the integration family: `PutIntegration`, the reads it is proved
//! through (`GetIntegration` and `ListIntegrations`) and `DeleteIntegration`.
//!
//! The rule under test: an accepted `PutIntegration` persists ONE integration configured
//! exactly as the request's `resourceConfig` named it, hands it back PROVISIONING, and the
//! CLOCK — never a call — is what moves it to ACTIVE; `GetIntegration` reports that
//! integration, the status the clock has made due and the OpenSearch Service resources its
//! setup created, `ListIntegrations` reports the account's integrations in the order they
//! were created narrowed by the filters the request names, and `DeleteIntegration` removes
//! one in whatever status it is in — refusing a name no integration is stored under. A put
//! whose ARNs do not name the resources their members document, one naming an integration
//! the account already holds, and one past the account's single-integration limit persist
//! nothing.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a
//! response that is not computed from the shared store cannot pass.
//!
//! The tables span what the rule generalises over rather than one request: integration
//! names of the shortest and longest length `IntegrationName`'s `@length(1..50)` admits and
//! every character class its `@pattern` admits, `retentionDays` at both published bounds of
//! `CollectionRetentionDays`' `@range(1..30)` and one past each, a KMS key named and not, an
//! existing OpenSearch Service application named and not, dashboard viewer principals of
//! zero, one and two ARNs and of both kinds the member documents, EVERY value of the
//! model's `IntegrationStatus` enum as a `ListIntegrations` filter (source:
//! `smithy_query(query="shape",shape="IntegrationStatus")`), and integrations whose creation
//! order differs from their name order.

use serde_json::{json, Value};

use service_com_amazonaws_cloudwatchlogs_logs_20140328::ops::types::OpenSearchResourceConfig;
use service_com_amazonaws_cloudwatchlogs_logs_20140328::state::{
    ComAmazonawsCloudwatchlogsLogs20140328State, IntegrationRecord,
    INTEGRATION_PROVISIONING_DELAY_MS, INTEGRATION_STATUS_ACTIVE, INTEGRATION_STATUS_FAILED,
    INTEGRATION_STATUS_PROVISIONING, INTEGRATION_TYPE_OPENSEARCH, MAX_INTEGRATIONS,
};

use crate::harness::{now_ms, Logs, REGION};

/// The operations under test.
const PUT: &str = "PutIntegration";
const GET: &str = "GetIntegration";
const LIST: &str = "ListIntegrations";
const DELETE: &str = "DeleteIntegration";

/// The code CloudWatch Logs answers for a parameter it will not accept ("A parameter is
/// specified incorrectly"), which is what every operation of this family declaring it
/// answers for a value outside a modelled constraint as well.
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// The code answered when the resource a request names is not there.
const NOT_FOUND: &str = "ResourceNotFoundException";

/// The code answered when a create would grow the account past a limit.
const LIMIT_EXCEEDED: &str = "LimitExceededException";

/// The code answered for a request whose parameters cannot hold together.
const VALIDATION: &str = "ValidationException";

/// Every value of the model's `IntegrationStatus` enum, in the order the model lists them.
/// Source: `smithy_query(query="shape",shape="IntegrationStatus")`.
const INTEGRATION_STATUSES: [&str; 3] = [
    INTEGRATION_STATUS_PROVISIONING,
    INTEGRATION_STATUS_ACTIVE,
    INTEGRATION_STATUS_FAILED,
];

/// The IAM role `dataSourceRoleArn` names: "the ARN of an IAM role that CloudWatch Logs
/// will use to create the integration".
fn role_arn() -> String {
    format!("arn:aws:iam::{}:role/CloudWatchOpenSearch", account())
}

/// An IAM user ARN, one of the two principal kinds `dashboardViewerPrincipals` documents.
fn user_arn() -> String {
    format!("arn:aws:iam::{}:user/dashboard-viewer", account())
}

/// A KMS key ARN, for the rows that encrypt the vended dashboard data with their own key.
fn kms_key_arn() -> String {
    format!(
        "arn:aws:kms:{REGION}:{}:key/aaaabbbb-cccc-dddd-eeee-ffff00001111",
        account()
    )
}

/// An OpenSearch Service application ARN, for the row that names an existing application.
fn existing_application_arn() -> String {
    format!("arn:aws:opensearch:{REGION}:{}:application/app-7f3c", account())
}

/// The account every ARN this emulator mints carries.
fn account() -> &'static str {
    "000000000000"
}

/// One accepted `PutIntegration`: the configuration it names, each member of which a read
/// must report back exactly as the create stored it.
struct Row {
    /// What this row varies, for the assertion messages.
    case: &'static str,
    name: String,
    kms_key_arn: Option<String>,
    application_arn: Option<String>,
    dashboard_viewer_principals: Vec<String>,
    retention_days: i64,
}

impl Row {
    /// A row naming nothing but the required configuration, under `name`.
    fn bare(case: &'static str, name: &str) -> Self {
        Self {
            case,
            name: name.to_string(),
            kms_key_arn: None,
            application_arn: None,
            dashboard_viewer_principals: vec![role_arn()],
            retention_days: 1,
        }
    }
}

/// The accepted corpus: every optional member absent, each present on its own, both
/// published bounds of `retentionDays`, principal lists of zero, one and two ARNs of both
/// documented kinds, and integration names at both ends of `IntegrationName`'s length and
/// over every character class its pattern admits.
fn rows() -> Vec<Row> {
    vec![
        Row::bare("nothing but the required configuration", "integration"),
        Row {
            retention_days: 30,
            ..Row::bare("retentionDays at the published maximum of 30", "retain-30")
        },
        Row {
            kms_key_arn: Some(kms_key_arn()),
            ..Row::bare("a customer KMS key for the dashboard data", "with-kms")
        },
        Row {
            application_arn: Some(existing_application_arn()),
            ..Row::bare("an existing OpenSearch application", "with-application")
        },
        Row {
            dashboard_viewer_principals: vec![role_arn(), user_arn()],
            ..Row::bare("a role and a user as dashboard viewers", "two-viewers")
        },
        Row {
            dashboard_viewer_principals: Vec::new(),
            ..Row::bare("no dashboard viewers at all", "no-viewers")
        },
        Row {
            kms_key_arn: Some(kms_key_arn()),
            application_arn: Some(existing_application_arn()),
            dashboard_viewer_principals: vec![user_arn()],
            retention_days: 30,
            ..Row::bare("every optional member at once", "everything")
        },
        Row::bare("the shortest name IntegrationName admits", "a"),
        Row::bare(
            "every character class the IntegrationName pattern admits",
            "Ops.team-logs_2/dash#1",
        ),
        Row::bare(
            "the longest name IntegrationName admits",
            &"n".repeat(50),
        ),
    ]
}

/// The request body one accepted row sends.
fn put_body(row: &Row) -> Value {
    let mut config = json!({
        "dataSourceRoleArn": role_arn(),
        "dashboardViewerPrincipals": row.dashboard_viewer_principals,
        "retentionDays": row.retention_days,
    });
    if let Some(kms_key_arn) = &row.kms_key_arn {
        config["kmsKeyArn"] = json!(kms_key_arn);
    }
    if let Some(application_arn) = &row.application_arn {
        config["applicationArn"] = json!(application_arn);
    }
    json!({
        "integrationName": row.name,
        "integrationType": INTEGRATION_TYPE_OPENSEARCH,
        "resourceConfig": {"openSearchResourceConfig": config},
    })
}

/// One string member of a response body.
fn member<'a>(body: &'a Value, key: &str) -> Option<&'a str> {
    body.get(key).and_then(Value::as_str)
}

/// The statuses an integration created at `creation_ms` may be reported in by a read that
/// happened somewhere between `read_from_ms` and `read_to_ms`: PROVISIONING until it is due
/// to become active, ACTIVE after that.
///
/// A read window that straddles the due instant admits the statuses on both sides of it, so
/// these cases assert what the clock allows and never a race against it.
fn admissible_statuses(creation_ms: i64, read_from_ms: i64, read_to_ms: i64) -> Vec<&'static str> {
    let active_due = creation_ms + INTEGRATION_PROVISIONING_DELAY_MS;
    let mut admissible = Vec::new();
    if read_from_ms < active_due {
        admissible.push(INTEGRATION_STATUS_PROVISIONING);
    }
    if read_to_ms >= active_due {
        admissible.push(INTEGRATION_STATUS_ACTIVE);
    }
    admissible
}

/// The status every OpenSearch Service resource of an integration in `status` is reported
/// in: the setup creates them with the integration, so none of them is there until it is
/// active.
fn expected_resource_status(status: &str) -> &'static str {
    if status == INTEGRATION_STATUS_ACTIVE {
        "ACTIVE"
    } else {
        "NOT_FOUND"
    }
}

/// The configuration a seeded integration carries. Seeding goes through the SAME record
/// constructor `PutIntegration` uses, so what the readers see is what a create stores.
fn seed_config() -> OpenSearchResourceConfig {
    OpenSearchResourceConfig {
        kms_key_arn: None,
        data_source_role_arn: role_arn(),
        dashboard_viewer_principals: vec![role_arn()],
        application_arn: None,
        retention_days: 7,
    }
}

/// Put one integration in the shared store directly, created `age_ms` ago, optionally
/// pinned inside its provisioning window so a case about PROVISIONING is not a race against
/// the clock.
///
/// The wire create admits one integration per account, which is `PutIntegration`'s own rule;
/// the reporting and delete rules are about whatever the account HOLDS, so the cases for
/// them seed the store and then go over the wire like any client's call.
fn seed(logs: &Logs, name: &str, age_ms: i64, pin_provisioning: bool) -> i64 {
    let created = now_ms() - age_ms;
    let state = logs
        .engine()
        .state::<ComAmazonawsCloudwatchlogsLogs20140328State>();
    let mut store = state.lock();
    let seq = store.take_integration_seq();
    let mut record = IntegrationRecord::new(
        REGION,
        name,
        INTEGRATION_TYPE_OPENSEARCH,
        &seed_config(),
        created,
        seq,
    );
    if pin_provisioning {
        record.status_due_ms = Some(now_ms() + 60 * 60 * 1_000);
    }
    store.integrations.insert(name.to_string(), record);
    created
}

/// The names `ListIntegrations` reports, in the order it reports them.
async fn listed_names(logs: &Logs, filters: &Value) -> Vec<String> {
    let page = logs.ok(LIST, filters).await;
    page.get("integrationSummaries")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!("ListIntegrations {filters} must report integrationSummaries, got {page}")
        })
        .iter()
        .map(|summary| {
            member(summary, "integrationName")
                .unwrap_or_else(|| panic!("a summary carries integrationName, got {summary}"))
                .to_string()
        })
        .collect()
}

/// Creating an integration persists it, and reading it back reports every member the create
/// stored — the configuration on the record, and the OpenSearch Service resources derived
/// from it — in the status the clock has made due.
#[tokio::test]
async fn a_created_integration_is_reported_back_exactly_as_it_was_configured() {
    for row in rows() {
        let case = row.case;
        // One integration per account, so each row gets its own account's store.
        let logs = Logs::new();

        let before = now_ms();
        let created = logs.ok(PUT, &put_body(&row)).await;
        assert_eq!(
            member(&created, "integrationName"),
            Some(row.name.as_str()),
            "case {case:?}: a create answers the name it created, got {created}"
        );
        assert_eq!(
            member(&created, "integrationStatus"),
            Some(INTEGRATION_STATUS_PROVISIONING),
            "case {case:?}: a create hands back a PROVISIONING integration, got {created}"
        );

        let reported = logs.ok(GET, &json!({"integrationName": row.name})).await;
        let after = now_ms();
        assert_eq!(
            member(&reported, "integrationName"),
            Some(row.name.as_str()),
            "case {case:?}: expected the created name, got {reported}"
        );
        assert_eq!(
            member(&reported, "integrationType"),
            Some(INTEGRATION_TYPE_OPENSEARCH),
            "case {case:?}: expected integrationType {INTEGRATION_TYPE_OPENSEARCH}, got {reported}"
        );

        // The creation instant is the store's, so the window the status is judged against is
        // the one spanning the create and this read.
        let status = member(&reported, "integrationStatus")
            .unwrap_or_else(|| {
                panic!("case {case:?}: an integration carries integrationStatus, got {reported}")
            })
            .to_string();
        let admissible = admissible_statuses(before, before, after);
        assert!(
            admissible.contains(&status.as_str()),
            "case {case:?}: expected one of {admissible:?}, got {status:?}"
        );

        let details = reported
            .get("integrationDetails")
            .and_then(|details| details.get("openSearchIntegrationDetails"))
            .unwrap_or_else(|| {
                panic!("case {case:?}: an OPENSEARCH integration reports openSearchIntegrationDetails, got {reported}")
            })
            .clone();

        // Every OpenSearch Service resource the setup created is named after the integration
        // and reported in the status that follows the integration's own.
        let expected_application_arn = row.application_arn.clone().unwrap_or_else(|| {
            format!(
                "arn:aws:opensearch:{REGION}:{}:application/{}-application",
                account(),
                row.name
            )
        });
        let expected_application_id = row
            .application_arn
            .as_ref()
            .map_or_else(|| format!("{}-application", row.name), |_| "app-7f3c".to_string());
        // (the resource, the member naming it, the value expected there)
        let named: [(&str, &str, String); 8] = [
            ("dataSource", "dataSourceName", row.name.clone()),
            ("application", "applicationArn", expected_application_arn),
            ("application", "applicationId", expected_application_id),
            (
                "collection",
                "collectionArn",
                format!(
                    "arn:aws:aoss:{REGION}:{}:collection/{}-collection",
                    account(),
                    row.name
                ),
            ),
            ("workspace", "workspaceId", format!("{}-workspace", row.name)),
            (
                "encryptionPolicy",
                "policyName",
                format!("{}-encryption-policy", row.name),
            ),
            (
                "networkPolicy",
                "policyName",
                format!("{}-network-policy", row.name),
            ),
            (
                "accessPolicy",
                "policyName",
                format!("{}-access-policy", row.name),
            ),
        ];
        for (resource, key, expected) in named {
            let reported_resource = details.get(resource).unwrap_or_else(|| {
                panic!("case {case:?}: the details carry {resource}, got {details}")
            });
            assert_eq!(
                member(reported_resource, key),
                Some(expected.as_str()),
                "case {case:?}: expected {resource}.{key} {expected:?}, got {reported_resource}"
            );
            let resource_status = reported_resource
                .get("status")
                .and_then(|status| status.get("status"))
                .and_then(Value::as_str);
            assert_eq!(
                resource_status,
                Some(expected_resource_status(&status)),
                "case {case:?}: an integration in {status} reports {resource} as {:?}, got \
                 {reported_resource}",
                expected_resource_status(&status)
            );
        }
        // No endpoint is reported: this emulator stands up no OpenSearch Service collection
        // or application, so there is no host a client could reach.
        for (resource, key) in [
            ("application", "applicationEndpoint"),
            ("collection", "collectionEndpoint"),
        ] {
            assert_eq!(
                details.get(resource).and_then(|r| r.get(key)),
                None,
                "case {case:?}: no {resource}.{key} is reported, got {details}"
            );
        }

        // The list reports the same one integration, by the same name and type.
        let listed = listed_names(&logs, &json!({})).await;
        assert_eq!(
            listed,
            vec![row.name.clone()],
            "case {case:?}: the account's one integration is listed, got {listed:?}"
        );

        // No modelled member reports the configuration the create was given, so it is
        // checked where it is stored; the operations that report it arrive with the
        // dashboards themselves.
        let state = logs
            .engine()
            .state::<ComAmazonawsCloudwatchlogsLogs20140328State>();
        let store = state.lock();
        let stored = store
            .integrations
            .get(&row.name)
            .unwrap_or_else(|| panic!("case {case:?}: the created integration is stored"));
        assert_eq!(
            stored.kms_key_arn, row.kms_key_arn,
            "case {case:?}: expected the stored kmsKeyArn {:?}, got {:?}",
            row.kms_key_arn, stored.kms_key_arn
        );
        assert_eq!(
            stored.existing_application_arn, row.application_arn,
            "case {case:?}: expected the stored applicationArn {:?}, got {:?}",
            row.application_arn, stored.existing_application_arn
        );
        assert_eq!(
            stored.dashboard_viewer_principals, row.dashboard_viewer_principals,
            "case {case:?}: expected the stored principals {:?}, got {:?}",
            row.dashboard_viewer_principals, stored.dashboard_viewer_principals
        );
        assert_eq!(
            i64::from(stored.retention_days),
            row.retention_days,
            "case {case:?}: expected the stored retentionDays {}, got {}",
            row.retention_days,
            stored.retention_days
        );
        assert_eq!(
            stored.data_source_role_arn,
            role_arn(),
            "case {case:?}: expected the stored dataSourceRoleArn {:?}, got {:?}",
            role_arn(),
            stored.data_source_role_arn
        );
    }
}

/// The status a read reports is the one the CLOCK has made due, and reading changes nothing:
/// two reads of the same integration report the same thing — save for the transition the
/// clock itself owes between them — and no read creates or removes one.
#[tokio::test]
async fn the_status_the_clock_owes_is_reported_and_reading_changes_nothing() {
    let logs = Logs::new();

    // (case, how long ago it was created, pinned inside the provisioning window)
    let ages: [(&str, &str, i64, bool); 4] = [
        ("pinned inside its provisioning window", "pinned", 0, true),
        (
            "created one millisecond before it was due to become active",
            "just-created",
            INTEGRATION_PROVISIONING_DELAY_MS - 1,
            false,
        ),
        (
            "created after it became due to be active",
            "just-due",
            INTEGRATION_PROVISIONING_DELAY_MS + 1,
            false,
        ),
        ("created an hour ago", "long-ago", 60 * 60 * 1_000, false),
    ];
    let mut seeded: Vec<(&str, &str, i64, bool)> = Vec::new();
    for (case, name, age, pinned) in ages {
        let created = seed(&logs, name, age, pinned);
        seeded.push((case, name, created, pinned));
    }

    for (case, name, created, pinned) in &seeded {
        let before = now_ms();
        let first = logs.ok(GET, &json!({"integrationName": name})).await;
        let after = now_ms();
        let status = member(&first, "integrationStatus")
            .unwrap_or_else(|| {
                panic!("case {case:?}: an integration carries integrationStatus, got {first}")
            })
            .to_string();
        let admissible = if *pinned {
            vec![INTEGRATION_STATUS_PROVISIONING]
        } else {
            admissible_statuses(*created, before, after)
        };
        assert!(
            admissible.contains(&status.as_str()),
            "case {case:?}: expected one of {admissible:?}, got {status:?}"
        );

        let before_second = now_ms();
        let second = logs.ok(GET, &json!({"integrationName": name})).await;
        let after_second = now_ms();
        let second_status = member(&second, "integrationStatus")
            .unwrap_or_else(|| {
                panic!("case {case:?}: an integration carries integrationStatus, got {second}")
            })
            .to_string();
        if second_status == status {
            // The clock owed nothing between the two reads, so they must agree byte for
            // byte: reading changes nothing.
            assert_eq!(
                second, first,
                "case {case:?}: reading twice must report the same thing, expected {first}, got \
                 {second}"
            );
        } else {
            // The one change admissible between two reads is the one the CLOCK made due
            // in between — a row created just short of its active instant crosses it
            // while the test runs, which is a transition the clock owed and not one a
            // read performed. So the second read must still report a status its own read
            // window allows, and the status may only move FORWARD.
            let admissible_second = if *pinned {
                vec![INTEGRATION_STATUS_PROVISIONING]
            } else {
                admissible_statuses(*created, before_second, after_second)
            };
            assert!(
                admissible_second.contains(&second_status.as_str()),
                "case {case:?}: expected the second read to report one of \
                 {admissible_second:?}, got {second_status:?}"
            );
            assert_eq!(
                (status.as_str(), second_status.as_str()),
                (INTEGRATION_STATUS_PROVISIONING, INTEGRATION_STATUS_ACTIVE),
                "case {case:?}: a read may only move the status forward over the clock's \
                 own transition, got {status:?} then {second_status:?}"
            );
            // Every OpenSearch resource is reported in the status the settled integration
            // owes it, not left behind in the provisioning one.
            for (resource, detail) in second["integrationDetails"]
                ["openSearchIntegrationDetails"]
                .as_object()
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: an OpenSearch integration carries its details, got \
                         {second}"
                    )
                })
            {
                assert_eq!(
                    detail["status"]["status"].as_str(),
                    Some(expected_resource_status(&second_status)),
                    "case {case:?}: {resource} must be reported in the status an \
                     {second_status} integration owes it, got {detail}"
                );
            }
        }
    }

    // The one created an hour ago has long since finished provisioning, so there is nothing
    // admissible but ACTIVE for it.
    let long_ago = logs
        .ok(GET, &json!({"integrationName": "long-ago"}))
        .await;
    assert_eq!(
        member(&long_ago, "integrationStatus"),
        Some(INTEGRATION_STATUS_ACTIVE),
        "case \"created an hour ago\": expected {INTEGRATION_STATUS_ACTIVE}, got {long_ago}"
    );

    let state = logs
        .engine()
        .state::<ComAmazonawsCloudwatchlogsLogs20140328State>();
    let mut stored: Vec<String> = state.lock().integrations.keys().cloned().collect();
    stored.sort();
    let mut expected: Vec<String> = seeded
        .iter()
        .map(|(_, name, _, _)| (*name).to_string())
        .collect();
    expected.sort();
    assert_eq!(
        stored, expected,
        "reading integrations creates and removes none, expected {expected:?}, got {stored:?}"
    );
}

/// Reading a name no integration is stored under is a missing resource, not an empty report,
/// and an account that holds none lists an empty array rather than omitting the member.
#[tokio::test]
async fn a_name_no_integration_is_stored_under_is_a_missing_resource() {
    let logs = Logs::new();

    let listed = listed_names(&logs, &json!({})).await;
    assert!(
        listed.is_empty(),
        "case \"an account with no integration\": expected an empty list, got {listed:?}"
    );

    for case in ["nothing-created", "a-dot.name", &"x".repeat(50)] {
        let (code, message) = logs.refusal(GET, &json!({"integrationName": case})).await;
        assert_eq!(
            code, NOT_FOUND,
            "case {case:?}: expected {NOT_FOUND}, got {code} {message:?}"
        );
        assert_eq!(
            message,
            format!("Integration with name {case} does not exist."),
            "case {case:?}: expected the refusal to name the integration, got {message:?}"
        );
    }
}

/// `ListIntegrations` reports the account's integrations in the order they were CREATED —
/// not in name order — and each filter narrows that report to the integrations it names.
#[tokio::test]
async fn the_list_is_in_creation_order_and_every_filter_narrows_it() {
    let logs = Logs::new();

    // Created in an order that is NOT their name order, so an implementation enumerating a
    // name-keyed store cannot pass. "zebra" and "zephyr" share a prefix; "alpha" is
    // seeded second and sorts first.
    seed(&logs, "zebra", 0, true);
    seed(&logs, "alpha", 0, true);
    seed(&logs, "zephyr", 60 * 60 * 1_000, false);

    let creation_order = vec![
        "zebra".to_string(),
        "alpha".to_string(),
        "zephyr".to_string(),
    ];

    // (case, filters, the names expected in order)
    let cases: Vec<(&str, Value, Vec<String>)> = vec![
        ("no filter at all", json!({}), creation_order.clone()),
        (
            "a prefix two integrations share",
            json!({"integrationNamePrefix": "ze"}),
            vec!["zebra".to_string(), "zephyr".to_string()],
        ),
        (
            "a prefix one integration has",
            json!({"integrationNamePrefix": "alpha"}),
            vec!["alpha".to_string()],
        ),
        (
            "a prefix no integration has",
            json!({"integrationNamePrefix": "q"}),
            Vec::new(),
        ),
        (
            "the only integration type there is",
            json!({"integrationType": INTEGRATION_TYPE_OPENSEARCH}),
            creation_order.clone(),
        ),
        (
            "the provisioning status",
            json!({"integrationStatus": INTEGRATION_STATUS_PROVISIONING}),
            vec!["zebra".to_string(), "alpha".to_string()],
        ),
        (
            "the active status",
            json!({"integrationStatus": INTEGRATION_STATUS_ACTIVE}),
            vec!["zephyr".to_string()],
        ),
        (
            "a status nothing in this account is in",
            json!({"integrationStatus": INTEGRATION_STATUS_FAILED}),
            Vec::new(),
        ),
        (
            "a prefix and a status together",
            json!({
                "integrationNamePrefix": "ze",
                "integrationStatus": INTEGRATION_STATUS_PROVISIONING,
            }),
            vec!["zebra".to_string()],
        ),
        (
            "a prefix and a status that select nothing together",
            json!({
                "integrationNamePrefix": "alpha",
                "integrationStatus": INTEGRATION_STATUS_ACTIVE,
            }),
            Vec::new(),
        ),
    ];

    for (case, filters, expected) in cases {
        let listed = listed_names(&logs, &filters).await;
        assert_eq!(
            listed, expected,
            "case {case:?}: expected {expected:?}, got {listed:?}"
        );
        // The order and the contents do not move between two identical calls.
        let again = listed_names(&logs, &filters).await;
        assert_eq!(
            again, listed,
            "case {case:?}: listing twice must report the same thing, expected {listed:?}, got \
             {again:?}"
        );
    }

    // Every status the model declares is a filter the operation accepts, including the one
    // no integration in this account is in.
    for status in INTEGRATION_STATUSES {
        let page = logs.ok(LIST, &json!({"integrationStatus": status})).await;
        assert!(
            page.get("integrationSummaries").is_some(),
            "case {status:?}: a filtered list reports integrationSummaries, got {page}"
        );
    }
}

/// A put the service will not accept persists nothing: an ARN that does not name the
/// resource its member documents, a value outside a modelled constraint, a name the account
/// already holds an integration under, and one past the account's integration limit.
#[tokio::test]
async fn a_put_the_service_will_not_accept_persists_nothing() {
    let logs = Logs::new();

    // (case, the body, the expected code)
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "a dataSourceRoleArn that is not an ARN at all",
            put_with_config(json!({
                "dataSourceRoleArn": "CloudWatchOpenSearch",
                "dashboardViewerPrincipals": [role_arn()],
                "retentionDays": 7,
            })),
            INVALID_PARAMETER,
        ),
        (
            "a dataSourceRoleArn naming an IAM user instead of a role",
            put_with_config(json!({
                "dataSourceRoleArn": user_arn(),
                "dashboardViewerPrincipals": [role_arn()],
                "retentionDays": 7,
            })),
            INVALID_PARAMETER,
        ),
        (
            "a dataSourceRoleArn naming a log group",
            put_with_config(json!({
                "dataSourceRoleArn": format!("arn:aws:logs:{REGION}:{}:log-group:g", account()),
                "dashboardViewerPrincipals": [role_arn()],
                "retentionDays": 7,
            })),
            INVALID_PARAMETER,
        ),
        (
            "a dashboard viewer principal that is not an ARN",
            put_with_config(json!({
                "dataSourceRoleArn": role_arn(),
                "dashboardViewerPrincipals": [role_arn(), "dashboard-viewer"],
                "retentionDays": 7,
            })),
            INVALID_PARAMETER,
        ),
        (
            "a dashboard viewer principal naming an OpenSearch collection",
            put_with_config(json!({
                "dataSourceRoleArn": role_arn(),
                "dashboardViewerPrincipals": [
                    format!("arn:aws:aoss:{REGION}:{}:collection/c", account()),
                ],
                "retentionDays": 7,
            })),
            INVALID_PARAMETER,
        ),
        (
            "a kmsKeyArn that is a bare key id rather than an ARN",
            put_with_config(json!({
                "dataSourceRoleArn": role_arn(),
                "dashboardViewerPrincipals": [role_arn()],
                "retentionDays": 7,
                "kmsKeyArn": "aaaabbbb-cccc-dddd-eeee-ffff00001111",
            })),
            INVALID_PARAMETER,
        ),
        (
            "a kmsKeyArn naming another service's resource",
            put_with_config(json!({
                "dataSourceRoleArn": role_arn(),
                "dashboardViewerPrincipals": [role_arn()],
                "retentionDays": 7,
                "kmsKeyArn": "arn:aws:s3:::bucket/key",
            })),
            INVALID_PARAMETER,
        ),
        (
            "an applicationArn naming an OpenSearch collection rather than an application",
            put_with_config(json!({
                "dataSourceRoleArn": role_arn(),
                "dashboardViewerPrincipals": [role_arn()],
                "retentionDays": 7,
                "applicationArn": format!("arn:aws:aoss:{REGION}:{}:collection/c", account()),
            })),
            INVALID_PARAMETER,
        ),
        (
            "retentionDays one below the published minimum of 1",
            put_with_config(json!({
                "dataSourceRoleArn": role_arn(),
                "dashboardViewerPrincipals": [role_arn()],
                "retentionDays": 0,
            })),
            INVALID_PARAMETER,
        ),
        (
            "retentionDays one above the published maximum of 30",
            put_with_config(json!({
                "dataSourceRoleArn": role_arn(),
                "dashboardViewerPrincipals": [role_arn()],
                "retentionDays": 31,
            })),
            INVALID_PARAMETER,
        ),
        (
            "an integrationType outside the modelled enum",
            json!({
                "integrationName": "refused",
                "integrationType": "ELASTICSEARCH",
                "resourceConfig": {"openSearchResourceConfig": {
                    "dataSourceRoleArn": role_arn(),
                    "dashboardViewerPrincipals": [role_arn()],
                    "retentionDays": 7,
                }},
            }),
            INVALID_PARAMETER,
        ),
        (
            "an integration name the IntegrationName pattern does not admit",
            json!({
                "integrationName": "has a space",
                "integrationType": INTEGRATION_TYPE_OPENSEARCH,
                "resourceConfig": {"openSearchResourceConfig": {
                    "dataSourceRoleArn": role_arn(),
                    "dashboardViewerPrincipals": [role_arn()],
                    "retentionDays": 7,
                }},
            }),
            INVALID_PARAMETER,
        ),
        (
            "no resourceConfig at all",
            json!({
                "integrationName": "refused",
                "integrationType": INTEGRATION_TYPE_OPENSEARCH,
            }),
            INVALID_PARAMETER,
        ),
    ];

    for (case, body, expected) in cases {
        let code = logs.err(PUT, &body).await;
        assert_eq!(
            code, expected,
            "case {case:?}: expected {expected}, got {code}"
        );
    }

    let state = logs
        .engine()
        .state::<ComAmazonawsCloudwatchlogsLogs20140328State>();
    let stored = state.lock().integrations.len();
    assert_eq!(
        stored, 0,
        "a refused put persists nothing, expected an empty store, got {stored} integration(s)"
    );
    drop(state);

    // An accepted put fills the account's single integration slot.
    let first = Row::bare("the account's one integration", "the-one");
    logs.ok(PUT, &put_body(&first)).await;

    // "You can use this operation only to create a new integration. You can't modify an
    // existing integration."
    let (code, message) = logs.refusal(PUT, &put_body(&first)).await;
    assert_eq!(
        code, VALIDATION,
        "case \"a second put under the same name\": expected {VALIDATION}, got {code} {message:?}"
    );
    assert!(
        message.contains("the-one"),
        "case \"a second put under the same name\": the refusal names the integration, got \
         {message:?}"
    );

    // "Currently you can have only one integration in your account."
    let (code, message) = logs
        .refusal(
            PUT,
            &put_body(&Row::bare("one integration too many", "another-one")),
        )
        .await;
    assert_eq!(
        code, LIMIT_EXCEEDED,
        "case \"a put past the account limit\": expected {LIMIT_EXCEEDED}, got {code} {message:?}"
    );
    assert!(
        message.contains(&MAX_INTEGRATIONS.to_string()),
        "case \"a put past the account limit\": the refusal states the limit \
         ({MAX_INTEGRATIONS}), got {message:?}"
    );

    let listed = listed_names(&logs, &json!({})).await;
    assert_eq!(
        listed,
        vec!["the-one".to_string()],
        "a refused put creates nothing, expected the one integration, got {listed:?}"
    );
}

/// `DeleteIntegration` removes the integration it names and everything reported from it,
/// whatever status it is in and whether or not the request forces it, and refuses a name no
/// integration is stored under.
#[tokio::test]
async fn deleting_an_integration_removes_it_and_refuses_what_it_must() {
    let logs = Logs::new();

    // (case, the name, whether the delete forces it)
    for (case, name, force) in [
        ("a name nothing created", "nothing-here", false),
        ("a name nothing created, forced", "nothing-here", true),
    ] {
        let (code, message) = logs
            .refusal(DELETE, &json!({"integrationName": name, "force": force}))
            .await;
        assert_eq!(
            code, NOT_FOUND,
            "case {case:?}: expected {NOT_FOUND}, got {code} {message:?}"
        );
        assert_eq!(
            message,
            format!("Integration with name {name} does not exist."),
            "case {case:?}: expected the refusal to name the integration, got {message:?}"
        );
    }

    // Every combination of the status an integration can be in and the `force` the request
    // may carry removes it: no vended logs dashboard exists in this account for the
    // condition that makes `force` mandatory to be true of, and the delete does not depend
    // on the status either. Each row deletes the account's one integration, which frees the
    // slot for the next.
    // (case, how long ago it was created, pinned provisioning, the force the delete carries)
    let cases: [(&str, i64, bool, Option<bool>); 5] = [
        ("provisioning, no force member at all", 0, true, None),
        ("provisioning, force false", 0, true, Some(false)),
        ("provisioning, force true", 0, true, Some(true)),
        ("active, force false", 60 * 60 * 1_000, false, Some(false)),
        ("active, force true", 60 * 60 * 1_000, false, Some(true)),
    ];
    for (case, age, pinned, force) in cases {
        seed(&logs, "the-one", age, pinned);
        let mut body = json!({"integrationName": "the-one"});
        if let Some(force) = force {
            body["force"] = json!(force);
        }
        logs.ok(DELETE, &body).await;

        let code = logs.err(GET, &json!({"integrationName": "the-one"})).await;
        assert_eq!(
            code, NOT_FOUND,
            "case {case:?}: the deleted integration is gone, got {code}"
        );
        let listed = listed_names(&logs, &json!({})).await;
        assert!(
            listed.is_empty(),
            "case {case:?}: the account holds none after the delete, got {listed:?}"
        );
    }

    // The slot the deletes freed takes a new integration under a name that was used before,
    // which is what makes the removal complete.
    let row = Row::bare("a name a deleted integration used", "the-one");
    let created = logs.ok(PUT, &put_body(&row)).await;
    assert_eq!(
        member(&created, "integrationStatus"),
        Some(INTEGRATION_STATUS_PROVISIONING),
        "case {:?}: the re-created integration starts provisioning, got {created}",
        row.case
    );
}

/// A `PutIntegration` body carrying `config` as its OpenSearch resource configuration, under
/// a name the refusal cases share.
fn put_with_config(config: Value) -> Value {
    json!({
        "integrationName": "refused",
        "integrationType": INTEGRATION_TYPE_OPENSEARCH,
        "resourceConfig": {"openSearchResourceConfig": config},
    })
}
