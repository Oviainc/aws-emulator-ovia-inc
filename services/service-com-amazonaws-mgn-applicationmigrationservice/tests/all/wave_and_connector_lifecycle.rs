// SPDX-License-Identifier: CC0-1.0

//! Waves, connectors and launch configuration templates over HTTP: what a create
//! stores is what a later read reports.
//!
//! The rules under test:
//!
//! * `CreateWave` mints a `waveID` in the modelled shape and an ARN, stores the wave,
//!   and `ListWaves` reports exactly the stored record — filtered by the request's
//!   `filters` and cut into `maxResults`-sized pages a `nextToken` resumes. The wave's
//!   `totalApplications` is COUNTED from the applications assigned to it, so it follows
//!   the records rather than a second counter. A repeat of the same request is the
//!   `@idempotent` replay of the same wave; a repeat under a taken name with different
//!   members is the modelled `ConflictException`.
//! * `CreateConnector` mints a `connectorID` in the modelled shape and an ARN, stores
//!   the connector with every member the request supplied, and `ListConnectors` reports
//!   it; a repeat of the same request replays the same connector.
//! * `DeleteLaunchConfigurationTemplate` removes the account's template and answers 204
//!   with no body; the same call again is the modelled `ResourceNotFoundException`.
//!
//! Every assertion goes through the router's `dispatch`, so it exercises the same
//! restJson1 deserialize → handle → serialize path a client drives, and reads the
//! service's state through its own operations wherever an operation can reach it.
//!
//! The harness (`Mgn`, `error_code`, the region and account constants) is the one
//! `application_lifecycle.rs` defines.

use serde_json::{json, Value};
use service_com_amazonaws_mgn_applicationmigrationservice::state::{
    Application, LaunchConfigurationTemplate,
};

use crate::application_lifecycle::{error_code, Mgn, EMULATOR_ACCOUNT, REGION};

/// `^wave-[0-9a-zA-Z]{17}$` — the model's `WaveID` pattern (length 22..22).
fn is_wave_id(value: &str) -> bool {
    value.len() == 22
        && value.starts_with("wave-")
        && value[5..].chars().all(|c| c.is_ascii_alphanumeric())
}

/// `^connector-[0-9a-zA-Z]{17}$` — the model's `ConnectorID` pattern (length 27..27).
fn is_connector_id(value: &str) -> bool {
    value.len() == 27
        && value.starts_with("connector-")
        && value[10..].chars().all(|c| c.is_ascii_alphanumeric())
}

/// The model's `ISO8601DatetimeString`: `YYYY-MM-DDTHH:MM:SS(.<digits>)?Z`, 19..32 chars.
fn is_iso8601_z(value: &str) -> bool {
    (19..=32).contains(&value.len())
        && value.ends_with('Z')
        && value.char_indices().take(19).all(|(index, c)| match index {
            4 | 7 => c == '-',
            10 => c == 'T',
            13 | 16 => c == ':',
            _ => c.is_ascii_digit(),
        })
}

/// The `WaveProgressStatus` a wave nothing has started migrating carries. Source:
/// `smithy_query(query="shape",shape="WaveProgressStatus")` → values `NOT_STARTED`,
/// `IN_PROGRESS`, `COMPLETED`.
const PROGRESS_NOT_STARTED: &str = "NOT_STARTED";

fn member(body: &Value, name: &str) -> String {
    body[name].as_str().unwrap_or("<absent>").to_string()
}

/// The class a `CreateWave` request spans: the name's length and shape, the description
/// (absent, empty, long) and the tag count (none, one, several), plus a request that
/// names the caller's own account explicitly. The recorded `CreateWave(name=…)` is one
/// row.
fn wave_cases() -> Vec<(&'static str, Value)> {
    vec![
        ("name only", json!({"name": "cutover-wave"})),
        (
            "name, description and one tag",
            json!({"name": "wave-one", "description": "first cutover", "tags": {"env": "dev"}}),
        ),
        (
            "single-character name, empty description",
            json!({"name": "a", "description": ""}),
        ),
        (
            "interior spaces and punctuation in the name",
            json!({"name": "SAP ERP  prod (wave 1)", "description": "line one, line two"}),
        ),
        (
            "256-character name and 600-character description — the modelled maxima",
            json!({"name": "n".repeat(256), "description": "d".repeat(600)}),
        ),
        (
            "several tags",
            json!({
                "name": "inventory-wave",
                "tags": {"env": "prod", "team": "migration", "cost-center": "4711"}
            }),
        ),
        (
            "explicit accountID — the caller's own account",
            json!({"name": "explicit-account-wave", "accountID": EMULATOR_ACCOUNT}),
        ),
    ]
}

/// Every created wave is minted, stored and reported from state: the record a create
/// answers is the record `ListWaves` reports, filters select it by id and by archival
/// status, and a repeat of the same request replays the same wave.
#[tokio::test]
async fn a_wave_is_created_and_listed_through_state() {
    for (case, request) in wave_cases() {
        let mgn = Mgn::new();
        mgn.initialize().await;

        let (status, created) = mgn.call("CreateWave", &request).await;
        assert_eq!(
            status, 201,
            "case {case:?}: CreateWave expected 201, got {status} with {created:?}"
        );

        let id = member(&created, "waveID");
        assert!(
            is_wave_id(&id),
            "case {case:?}: waveID must match ^wave-[0-9a-zA-Z]{{17}}$, got {id:?}"
        );
        assert_eq!(
            member(&created, "arn"),
            format!("arn:aws:mgn:{REGION}:{EMULATOR_ACCOUNT}:wave/{id}"),
            "case {case:?}: arn must name this region, the emulator account and the minted id, \
             got {:?}",
            created["arn"]
        );
        assert_eq!(
            created["name"], request["name"],
            "case {case:?}: name must be the stored name, got {:?}",
            created["name"]
        );
        assert_eq!(
            created["description"], request["description"],
            "case {case:?}: description must be the stored description, got {:?}",
            created["description"]
        );
        assert_eq!(
            created["isArchived"],
            json!(false),
            "case {case:?}: a new wave is not archived, got {:?}",
            created["isArchived"]
        );
        let expected_tags = request
            .get("tags")
            .cloned()
            .filter(|tags| !tags.as_object().map(serde_json::Map::is_empty).unwrap_or(true));
        assert_eq!(
            created.get("tags").cloned(),
            expected_tags,
            "case {case:?}: tags must be the stored tags, got {:?}",
            created["tags"]
        );
        for stamp in ["creationDateTime", "lastModifiedDateTime"] {
            let value = member(&created, stamp);
            assert!(
                is_iso8601_z(&value),
                "case {case:?}: {stamp} must be an ISO-8601 instant, got {value:?}"
            );
        }
        assert_eq!(
            created["creationDateTime"], created["lastModifiedDateTime"],
            "case {case:?}: a wave nothing has modified was last modified when it was created, \
             got {:?} and {:?}",
            created["creationDateTime"], created["lastModifiedDateTime"]
        );
        let status_member = &created["waveAggregatedStatus"];
        assert_eq!(
            status_member["progressStatus"].as_str(),
            Some(PROGRESS_NOT_STARTED),
            "case {case:?}: a new wave has started nothing, got {status_member:?}"
        );
        assert_eq!(
            status_member["totalApplications"],
            json!(0),
            "case {case:?}: a new wave holds no application, got {status_member:?}"
        );

        // The listing reports exactly the created record.
        let (status, listed) = mgn.call("ListWaves", &json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListWaves expected 200, got {status} with {listed:?}"
        );
        assert_eq!(
            listed["items"].as_array().map(Vec::as_slice),
            Some([created.clone()].as_slice()),
            "case {case:?}: ListWaves must report the created wave unchanged, got {listed:?}"
        );

        // A filter on the minted id selects it; one on another id selects nothing.
        let (_, selected) = mgn
            .call("ListWaves", &json!({"filters": {"waveIDs": [id]}}))
            .await;
        assert_eq!(
            selected["items"].as_array().map(Vec::len),
            Some(1),
            "case {case:?}: the waveIDs filter must select the created wave, got {selected:?}"
        );
        let (_, other) = mgn
            .call(
                "ListWaves",
                &json!({"filters": {"waveIDs": ["wave-zzzzzzzzzzzzzzzzz"]}}),
            )
            .await;
        assert_eq!(
            other["items"].as_array().map(Vec::len),
            Some(0),
            "case {case:?}: a filter naming another wave must select none, got {other:?}"
        );
        let (_, archived) = mgn
            .call("ListWaves", &json!({"filters": {"isArchived": true}}))
            .await;
        assert_eq!(
            archived["items"].as_array().map(Vec::len),
            Some(0),
            "case {case:?}: no wave is archived, so the filter selects none, got {archived:?}"
        );

        // The same request again is the `@idempotent` replay of the same wave.
        let (status, replayed) = mgn.call("CreateWave", &request).await;
        assert_eq!(
            (status, replayed.clone()),
            (201, created.clone()),
            "case {case:?}: a repeat of the same request must replay the same wave, got {status} \
             with {replayed:?}"
        );
        let (_, after_replay) = mgn.call("ListWaves", &json!({})).await;
        assert_eq!(
            after_replay["items"].as_array().map(Vec::len),
            Some(1),
            "case {case:?}: a replay must not store a second wave, got {after_replay:?}"
        );
    }
}

/// A wave's `totalApplications` is counted from the applications assigned to it, so it
/// is whatever the records say — never a value the create fixed.
#[tokio::test]
async fn a_waves_total_applications_counts_the_applications_assigned_to_it() {
    let mgn = Mgn::new();
    mgn.initialize().await;

    let (_, created) = mgn.call("CreateWave", &json!({"name": "cutover"})).await;
    let wave_id = member(&created, "waveID");

    // Two applications of this account join the wave, one stays outside it, and one of
    // ANOTHER account names the same wave id: only the two count.
    {
        let store = mgn.store();
        let mut state = store.lock();
        for (index, (account, wave)) in [
            (EMULATOR_ACCOUNT, Some(wave_id.clone())),
            (EMULATOR_ACCOUNT, Some(wave_id.clone())),
            (EMULATOR_ACCOUNT, None),
            ("123456789012", Some(wave_id.clone())),
        ]
        .into_iter()
        .enumerate()
        {
            let application_id = format!("app-0000000000000000{index}");
            let stored = state.insert_application(Application {
                arn: format!("arn:aws:mgn:{REGION}:{account}:application/{application_id}"),
                application_id,
                name: format!("app-{index}"),
                account_id: account.to_string(),
                wave_id: wave,
                ..Application::default()
            });
            assert!(stored, "seeding application {index} must store it");
        }
    }

    let (_, listed) = mgn.call("ListWaves", &json!({})).await;
    let total = &listed["items"][0]["waveAggregatedStatus"]["totalApplications"];
    assert_eq!(
        total,
        &json!(2),
        "totalApplications must count only this account's applications in the wave, got {listed:?}"
    );
}

/// `ListWaves` reports the account's waves in creation order and pages them: a
/// `maxResults` page carries a `nextToken` only while waves remain, and following the
/// tokens reports every wave exactly once.
#[tokio::test]
async fn list_waves_pages_in_creation_order() {
    let mgn = Mgn::new();
    mgn.initialize().await;

    let mut created = Vec::new();
    for index in 0..5 {
        let (status, wave) = mgn
            .call("CreateWave", &json!({"name": format!("wave-{index}")}))
            .await;
        assert_eq!(status, 201, "CreateWave {index} expected 201, got {status}");
        created.push(member(&wave, "waveID"));
    }

    for page_size in [1usize, 2, 5, 1000] {
        let mut seen = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut request = json!({"maxResults": page_size});
            if let Some(token) = token.as_deref() {
                request["nextToken"] = json!(token);
            }
            let (status, page) = mgn.call("ListWaves", &request).await;
            assert_eq!(
                status, 200,
                "page size {page_size}: ListWaves expected 200, got {status} with {page:?}"
            );
            let items = page["items"].as_array().cloned().unwrap_or_default();
            assert!(
                items.len() <= page_size,
                "page size {page_size}: a page carries at most that many waves, got {}",
                items.len()
            );
            seen.extend(items.iter().map(|wave| member(wave, "waveID")));
            token = page["nextToken"].as_str().map(ToString::to_string);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, created,
            "page size {page_size}: following the tokens must report every wave once, in \
             creation order, got {seen:?}"
        );
    }

    // A token this service never issued cannot be resumed.
    let (status, answer) = mgn
        .call("ListWaves", &json!({"nextToken": "not-a-token"}))
        .await;
    assert_eq!(
        (status, error_code(&answer)),
        (400, "ValidationException"),
        "an unissued nextToken must be refused, got {status} with {answer:?}"
    );
}

/// The class a `CreateConnector` request spans: both modelled `ssmInstanceID` forms and
/// lengths, with and without `ssmCommandConfig`, and tag counts from none to several.
/// The recorded `CreateConnector(name=…, ssmInstanceID='mi-…', tags={…})` is one row.
fn connector_cases() -> Vec<(&'static str, Value)> {
    vec![
        (
            "instance id, no optional members",
            json!({"name": "migration-connector", "ssmInstanceID": "i-0123456789abcdefg"}),
        ),
        (
            "managed-instance id and tags",
            json!({
                "name": "managed_connector-1",
                "ssmInstanceID": "mi-00000000000000000",
                "tags": {"env": "prod", "team": "migration"},
            }),
        ),
        (
            "mixed-case id and a command config with both outputs",
            json!({
                "name": "A",
                "ssmInstanceID": "mi-A1b2C3d4E5f6G7h8I",
                "ssmCommandConfig": {
                    "s3OutputEnabled": true,
                    "outputS3BucketName": "migration-logs",
                    "cloudWatchOutputEnabled": true,
                    "cloudWatchLogGroupName": "/mgn/connector",
                },
            }),
        ),
        (
            "all-letter id and a command config with neither output",
            json!({
                "name": "n".repeat(256),
                "ssmInstanceID": "i-abcdefghijklmnopq",
                "ssmCommandConfig": {"s3OutputEnabled": false, "cloudWatchOutputEnabled": false},
            }),
        ),
    ]
}

/// Every created connector is minted, stored and reported from state: the record a
/// create answers is the record `ListConnectors` reports, and a repeat of the same
/// request replays the same connector rather than minting a second one.
#[tokio::test]
async fn a_connector_is_created_and_listed_through_state() {
    for (case, request) in connector_cases() {
        let mgn = Mgn::new();
        mgn.initialize().await;

        let (status, created) = mgn.call("CreateConnector", &request).await;
        assert_eq!(
            status, 201,
            "case {case:?}: CreateConnector expected 201, got {status} with {created:?}"
        );

        let id = member(&created, "connectorID");
        assert!(
            is_connector_id(&id),
            "case {case:?}: connectorID must match ^connector-[0-9a-zA-Z]{{17}}$, got {id:?}"
        );
        assert_eq!(
            member(&created, "arn"),
            format!("arn:aws:mgn:{REGION}:{EMULATOR_ACCOUNT}:connector/{id}"),
            "case {case:?}: arn must name this region, the emulator account and the minted id, \
             got {:?}",
            created["arn"]
        );
        for echoed in ["name", "ssmInstanceID", "ssmCommandConfig"] {
            assert_eq!(
                created.get(echoed).cloned(),
                request.get(echoed).cloned(),
                "case {case:?}: {echoed} must be the stored value, got {:?}",
                created[echoed]
            );
        }
        assert_eq!(
            created.get("tags").cloned(),
            request.get("tags").cloned(),
            "case {case:?}: tags must be the stored tags, got {:?}",
            created["tags"]
        );

        let (status, listed) = mgn.call("ListConnectors", &json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListConnectors expected 200, got {status} with {listed:?}"
        );
        assert_eq!(
            listed["items"].as_array().map(Vec::as_slice),
            Some([created.clone()].as_slice()),
            "case {case:?}: ListConnectors must report the created connector unchanged, got \
             {listed:?}"
        );
        let (_, selected) = mgn
            .call(
                "ListConnectors",
                &json!({"filters": {"connectorIDs": [id]}}),
            )
            .await;
        assert_eq!(
            selected["items"].as_array().map(Vec::len),
            Some(1),
            "case {case:?}: the connectorIDs filter must select the created connector, got \
             {selected:?}"
        );
        let (_, other) = mgn
            .call(
                "ListConnectors",
                &json!({"filters": {"connectorIDs": ["connector-zzzzzzzzzzzzzzzzz"]}}),
            )
            .await;
        assert_eq!(
            other["items"].as_array().map(Vec::len),
            Some(0),
            "case {case:?}: a filter naming another connector must select none, got {other:?}"
        );

        // The same request again replays the same connector.
        let (status, replayed) = mgn.call("CreateConnector", &request).await;
        assert_eq!(
            (status, replayed.clone()),
            (201, created.clone()),
            "case {case:?}: a repeat of the same request must replay the same connector, got \
             {status} with {replayed:?}"
        );

        // A request differing in one member is a different connector.
        let mut different = request.clone();
        different["name"] = json!("second-connector");
        let (status, second) = mgn.call("CreateConnector", &different).await;
        assert_eq!(
            status, 201,
            "case {case:?}: a request differing in name expected 201, got {status} with {second:?}"
        );
        assert_ne!(
            member(&second, "connectorID"),
            id,
            "case {case:?}: a different request must mint a different connector"
        );
        let (_, both) = mgn.call("ListConnectors", &json!({})).await;
        assert_eq!(
            both["items"].as_array().map(Vec::len),
            Some(2),
            "case {case:?}: the store must hold exactly the two connectors, got {both:?}"
        );
    }
}

/// `DeleteLaunchConfigurationTemplate` removes the account's template and answers 204
/// with no body; the same id again is the modelled not-found, and a template of another
/// account is never this one's to delete.
#[tokio::test]
async fn a_launch_configuration_template_is_deleted_from_state() {
    let mgn = Mgn::new();
    mgn.initialize().await;

    // Ids differing in their body characters, inside the modelled
    // `^lct-[0-9a-zA-Z]{17}$` (length 21..21).
    let template_ids = [
        "lct-00000000000000000",
        "lct-zzzzzzzzzzzzzzzzz",
        "lct-A1b2C3d4E5f6G7h8I",
    ];
    {
        let store = mgn.store();
        let mut state = store.lock();
        for template_id in template_ids {
            let seeded = state.insert_launch_configuration_template(LaunchConfigurationTemplate {
                launch_configuration_template_id: template_id.to_string(),
                arn: format!(
                    "arn:aws:mgn:{REGION}:{EMULATOR_ACCOUNT}:launch-configuration-template/{template_id}"
                ),
                account_id: EMULATOR_ACCOUNT.to_string(),
            });
            assert!(
                seeded,
                "seeding {template_id:?} must store it (the store bound was not reached)"
            );
        }
    }

    for template_id in template_ids {
        let request = json!({"launchConfigurationTemplateID": template_id});
        let (status, _, raw) = mgn
            .call_http_raw("POST", "/DeleteLaunchConfigurationTemplate", Some(&request))
            .await;
        assert_eq!(
            status, 204,
            "case {template_id:?}: expected 204, got {status} with {:?}",
            String::from_utf8_lossy(&raw)
        );
        assert!(
            raw.is_empty(),
            "case {template_id:?}: a 204 carries no body, got {:?}",
            String::from_utf8_lossy(&raw)
        );

        // The store no longer holds it, through the store and through the operation.
        assert!(
            mgn.store()
                .lock()
                .launch_configuration_template_of(EMULATOR_ACCOUNT, template_id)
                .is_none(),
            "case {template_id:?}: the deleted template must be gone from the store"
        );
        let (status, answer) = mgn
            .call("DeleteLaunchConfigurationTemplate", &request)
            .await;
        assert_eq!(
            (status, error_code(&answer)),
            (404, "ResourceNotFoundException"),
            "case {template_id:?}: deleting it again must be 404 ResourceNotFoundException, got \
             {status} with {answer:?}"
        );
        assert_eq!(
            answer["message"].as_str(),
            Some(
                format!("LaunchConfigurationTemplate with Id: '{template_id}' was not found")
                    .as_str()
            ),
            "case {template_id:?}: the refusal names the kind and the requested id, got {:?}",
            answer["message"]
        );
    }
}
