// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the `NotificationConfiguration` lifecycle: create, get,
//! list, update and delete, driven over HTTP through the same multi-service
//! router the emulator boots in production.
//!
//! Every case asserts the RULE the operations follow, not one recorded request:
//! each table row varies the input along what the rule generalises over (value,
//! size, enum member, member subset, page size, filter value), and the catalogs
//! the rows draw from are the service's own modelled value sets
//! (`AGGREGATION_DURATION_VALUES`, `NOTIFICATION_CONFIGURATION_STATUS_VALUES`,
//! `NOTIFICATION_CONFIGURATION_SUBTYPE_VALUES`), so a model change moves the
//! test with it.

use serde_json::{json, Value};
use service_com_amazonaws_notifications_notifications::ops::types::{
    AGGREGATION_DURATION_VALUES, NOTIFICATION_CONFIGURATION_STATUS_VALUES,
    NOTIFICATION_CONFIGURATION_SUBTYPE_VALUES,
};
use service_com_amazonaws_notifications_notifications::ComAmazonawsNotificationsNotificationsState;

// The HTTP harness this suite drives the service through, shared with every
// other suite in this crate.
use crate::harness::{encode_path_label, error_code, Emulator};

// The configuration-specific helpers this suite adds to the shared harness. An
// inherent `impl` on a type declared elsewhere in the same crate is what keeps
// them beside the tests that use them.
impl Emulator {
    /// Create a configuration and return its arn, failing the test on anything
    /// other than the modelled 201.
    async fn create(&self, body: Value) -> String {
        let (status, response) = self.send("POST", "/notification-configurations", Some(body)).await;
        assert_eq!(status, 201, "create must answer 201, got {response:?}");
        response["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("create must answer an arn, got {response:?}"))
            .to_string()
    }

    async fn get(&self, arn: &str) -> (u16, Value) {
        let uri = format!("/notification-configurations/{}", encode_path_label(arn));
        self.send("GET", &uri, None).await
    }

    async fn update(&self, arn: &str, patch: Value) -> (u16, Value) {
        let uri = format!("/notification-configurations/{}", encode_path_label(arn));
        self.send("PUT", &uri, Some(patch)).await
    }

    async fn delete(&self, arn: &str) -> (u16, Value) {
        let uri = format!("/notification-configurations/{}", encode_path_label(arn));
        self.send("DELETE", &uri, None).await
    }

    /// List with a raw query string (empty for an unfiltered, unpaged listing).
    async fn list(&self, query: &str) -> (u16, Value) {
        let uri = if query.is_empty() {
            "/notification-configurations".to_string()
        } else {
            format!("/notification-configurations?{query}")
        };
        self.send("GET", &uri, None).await
    }

    /// The tags persisted under `arn`, read straight out of the store: no
    /// implemented operation reports tags yet (`ListTagsForResource` is still a
    /// 501 stub), so this is the only way to assert create persisted them.
    fn stored_tags(&self, arn: &str) -> Option<Vec<(String, String)>> {
        let state = self
            .engine
            .state::<ComAmazonawsNotificationsNotificationsState>();
        let store = state.lock();
        store
            .configurations
            .get(arn)
            .map(|record| record.tags.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
    }
}

/// The `NotificationConfigurationArn` pattern the model declares,
/// `^arn:[a-z-]{3,10}:notifications::[0-9]{12}:configuration/[a-z0-9]{27}$`,
/// checked by hand so the suite needs no regex dependency.
fn is_configuration_arn(value: &str) -> bool {
    let Some(rest) = value.strip_prefix("arn:") else {
        return false;
    };
    let Some((partition, rest)) = rest.split_once(':') else {
        return false;
    };
    if !(3..=10).contains(&partition.len())
        || !partition
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b == b'-')
    {
        return false;
    }
    let Some(rest) = rest.strip_prefix("notifications::") else {
        return false;
    };
    let Some((account, resource)) = rest.split_once(':') else {
        return false;
    };
    if account.len() != 12 || !account.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let Some(id) = resource.strip_prefix("configuration/") else {
        return false;
    };
    id.len() == 27 && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

/// One row of the create/get table.
struct CreateCase {
    case: &'static str,
    name: String,
    description: String,
    aggregation_duration: Option<&'static str>,
    tags: Vec<(&'static str, &'static str)>,
}

/// The create table: rows differ in name size (both published `@length`
/// bounds), description size and alphabet, which `AggregationDuration` member
/// they carry (every modelled value, plus absent) and how many tags they set.
fn create_cases() -> Vec<CreateCase> {
    let mut cases = vec![
        CreateCase {
            // `name` len 1 and `description` len 0 are the modelled minimums.
            case: "smallest permitted name and description, no optional members",
            name: "a".to_string(),
            description: String::new(),
            aggregation_duration: None,
            tags: Vec::new(),
        },
        CreateCase {
            // `name` len 64 and `description` len 256 are the modelled maximums.
            case: "largest permitted name and description",
            name: "n".repeat(64),
            description: "d".repeat(256),
            aggregation_duration: Some("LONG"),
            tags: vec![("env", "prod")],
        },
        CreateCase {
            case: "name using every character class the pattern allows",
            name: "My_config-7".to_string(),
            description: "notificações de demonstração — unicode is not a control char"
                .to_string(),
            aggregation_duration: Some("NONE"),
            tags: vec![("env", "dev"), ("owner", "platform"), ("tier", "2")],
        },
        CreateCase {
            case: "an empty tag map is not the same request as no tag map",
            name: "empty-tag-map".to_string(),
            description: "no tags, but the member is present".to_string(),
            aggregation_duration: Some("SHORT"),
            tags: Vec::new(),
        },
    ];
    // Every modelled `AggregationDuration` value, including the ones no issue
    // cited. Catalog source: the `AggregationDuration` enum shape, re-exported
    // by the service crate as `AGGREGATION_DURATION_VALUES`.
    for value in AGGREGATION_DURATION_VALUES {
        cases.push(CreateCase {
            case: "one row per modelled AggregationDuration value",
            name: format!("aggregation-{}", value.to_ascii_lowercase()),
            description: format!("aggregationDuration={value}"),
            aggregation_duration: Some(value),
            tags: vec![("duration", "tested")],
        });
    }
    cases
}

impl CreateCase {
    fn request_body(&self, with_tag_member: bool) -> Value {
        let mut body = json!({ "name": self.name, "description": self.description });
        if let Some(duration) = self.aggregation_duration {
            body["aggregationDuration"] = json!(duration);
        }
        if with_tag_member {
            let tags: serde_json::Map<String, Value> = self
                .tags
                .iter()
                .map(|(k, v)| ((*k).to_string(), json!(v)))
                .collect();
            body["tags"] = Value::Object(tags);
        }
        body
    }
}

/// RULE: a create mints one new arn, persists the record the request describes,
/// and a later get reports that stored record member for member.
#[tokio::test]
async fn create_persists_the_record_a_get_reports() {
    let emulator = Emulator::new();
    let mut minted: Vec<String> = Vec::new();

    for case in create_cases() {
        // The tag member is present on a row that sets tags, and also on the row
        // that deliberately sends an empty map.
        let with_tag_member = !case.tags.is_empty() || case.case.contains("empty tag map");
        let (status, created) = emulator
            .send(
                "POST",
                "/notification-configurations",
                Some(case.request_body(with_tag_member)),
            )
            .await;
        assert_eq!(
            status, 201,
            "case {:?}: create must answer 201, got {status} {created:?}",
            case.case
        );

        let arn = created["arn"].as_str().unwrap_or_default().to_string();
        assert!(
            is_configuration_arn(&arn),
            "case {:?}: minted arn must match the modelled pattern, got {arn:?}",
            case.case
        );
        assert_eq!(
            created["status"].as_str(),
            Some("ACTIVE"),
            "case {:?}: a created configuration is ACTIVE, got {created:?}",
            case.case
        );
        assert!(
            !minted.contains(&arn),
            "case {:?}: every create mints a NEW arn, but {arn:?} was already minted",
            case.case
        );
        minted.push(arn.clone());

        let (status, got) = emulator.get(&arn).await;
        assert_eq!(
            status, 200,
            "case {:?}: get of a created arn must answer 200, got {status} {got:?}",
            case.case
        );
        assert_eq!(
            got["arn"].as_str(),
            Some(arn.as_str()),
            "case {:?}: get reports the arn it was asked for, got {got:?}",
            case.case
        );
        assert_eq!(
            got["name"].as_str(),
            Some(case.name.as_str()),
            "case {:?}: get reports the persisted name, got {got:?}",
            case.case
        );
        assert_eq!(
            got["description"].as_str(),
            Some(case.description.as_str()),
            "case {:?}: get reports the persisted description, got {got:?}",
            case.case
        );
        assert_eq!(
            got["status"].as_str(),
            Some("ACTIVE"),
            "case {:?}: get reports the persisted status, got {got:?}",
            case.case
        );
        assert_eq!(
            got["subtype"].as_str(),
            Some("ACCOUNT"),
            "case {:?}: a self-created configuration is subtype ACCOUNT, got {got:?}",
            case.case
        );
        assert_eq!(
            got["aggregationDuration"].as_str(),
            case.aggregation_duration,
            "case {:?}: get reports the persisted aggregationDuration, got {got:?}",
            case.case
        );
        // The instant the record was stamped with. This framework renders a
        // timestamp member with no `@timestampFormat` on its own schema as
        // epoch seconds; accept either encoding, but require a real instant —
        // epoch seconds past 2020, or an RFC 3339 string whose year is this
        // century — rather than a zero or a placeholder.
        let creation_time = &got["creationTime"];
        let plausible = match creation_time {
            Value::Number(_) => creation_time.as_f64().is_some_and(|s| s > 1_600_000_000.0),
            Value::String(rendered) => {
                rendered.len() >= 20
                    && rendered.starts_with("20")
                    && rendered[..4].bytes().all(|b| b.is_ascii_digit())
                    && rendered.contains('T')
            }
            _ => false,
        };
        assert!(
            plausible,
            "case {:?}: creationTime must be the instant the record was created, got {creation_time:?}",
            case.case
        );

        // Tags are persisted now even though no implemented operation reports
        // them: the record is what a later tagging slice reads.
        let mut expected_tags: Vec<(String, String)> = case
            .tags
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        expected_tags.sort();
        let mut stored_tags = emulator.stored_tags(&arn).unwrap_or_default();
        stored_tags.sort();
        assert_eq!(
            stored_tags, expected_tags,
            "case {:?}: create must persist the request's tags, got {stored_tags:?}",
            case.case
        );

        // A read changes nothing a later reader sees: the same get answers the
        // same record, creationTime included.
        let (status, again) = emulator.get(&arn).await;
        assert_eq!(status, 200, "case {:?}: a repeated get stays 200", case.case);
        assert_eq!(
            again, got,
            "case {:?}: a get must not change what the next get reports",
            case.case
        );
    }
}

/// RULE: an arn no configuration is stored under does not exist, so every
/// operation keyed by arn answers `ResourceNotFoundException` (404) for it —
/// whatever the arn's own shape.
#[tokio::test]
async fn operations_keyed_by_arn_report_an_unknown_arn_as_not_found() {
    let emulator = Emulator::new();

    // Well-formed arns (they satisfy the modelled pattern, so they reach the
    // handler) that were never created.
    let unknown = [
        "arn:aws:notifications::000000000000:configuration/aaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "arn:aws:notifications::000000000000:configuration/999999999999999999999999999",
        "arn:aws-cn:notifications::000000000000:configuration/z0y9x8w7v6u5t4s3r2q1p0onmlk",
    ];

    // A live configuration exists beside them, so a 404 cannot be "the store is
    // empty".
    let live = emulator
        .create(json!({ "name": "live-config", "description": "present" }))
        .await;

    for arn in unknown {
        for (operation, status, response) in [
            ("get", emulator.get(arn).await),
            ("update", emulator.update(arn, json!({ "name": "renamed" })).await),
            ("delete", emulator.delete(arn).await),
        ]
        .map(|(operation, (status, response))| (operation, status, response))
        {
            assert_eq!(
                status, 404,
                "case {operation} {arn:?}: an unknown arn is a 404, got {status} {response:?}"
            );
            assert_eq!(
                error_code(&response),
                "ResourceNotFoundException",
                "case {operation} {arn:?}: the 404 is a ResourceNotFoundException, got {response:?}"
            );
        }
    }

    let (status, got) = emulator.get(&live).await;
    assert_eq!(
        status, 200,
        "the live configuration must survive the failed calls, got {status} {got:?}"
    );
}

/// One row of the update table: the patch the request carries, and what each
/// member must read as afterwards.
struct UpdateCase {
    case: &'static str,
    patch: Value,
    expected_name: &'static str,
    expected_description: &'static str,
    expected_aggregation: &'static str,
}

/// RULE: an update writes the members the request carries and leaves every
/// omitted member at its persisted value.
#[tokio::test]
async fn update_writes_only_the_members_the_request_carries() {
    const BASE_NAME: &str = "base-config";
    const BASE_DESCRIPTION: &str = "base description";
    const BASE_AGGREGATION: &str = "SHORT";

    // Rows span the subsets of the three updatable members: none, each one
    // alone, a pair, and all three.
    let cases = [
        UpdateCase {
            case: "no updatable member at all",
            patch: json!({}),
            expected_name: BASE_NAME,
            expected_description: BASE_DESCRIPTION,
            expected_aggregation: BASE_AGGREGATION,
        },
        UpdateCase {
            case: "name only",
            patch: json!({ "name": "renamed-config" }),
            expected_name: "renamed-config",
            expected_description: BASE_DESCRIPTION,
            expected_aggregation: BASE_AGGREGATION,
        },
        UpdateCase {
            case: "description only",
            patch: json!({ "description": "updated demo" }),
            expected_name: BASE_NAME,
            expected_description: "updated demo",
            expected_aggregation: BASE_AGGREGATION,
        },
        UpdateCase {
            case: "aggregationDuration only",
            patch: json!({ "aggregationDuration": "LONG" }),
            expected_name: BASE_NAME,
            expected_description: BASE_DESCRIPTION,
            expected_aggregation: "LONG",
        },
        UpdateCase {
            case: "name and description together",
            patch: json!({ "name": "pair-config", "description": "pair description" }),
            expected_name: "pair-config",
            expected_description: "pair description",
            expected_aggregation: BASE_AGGREGATION,
        },
        UpdateCase {
            case: "every updatable member at once",
            patch: json!({
                "name": "all-config",
                "description": "",
                "aggregationDuration": "NONE"
            }),
            expected_name: "all-config",
            expected_description: "",
            expected_aggregation: "NONE",
        },
    ];

    for case in cases {
        let emulator = Emulator::new();
        let arn = emulator
            .create(json!({
                "name": BASE_NAME,
                "description": BASE_DESCRIPTION,
                "aggregationDuration": BASE_AGGREGATION,
                "tags": { "env": "dev" }
            }))
            .await;
        let (_, before) = emulator.get(&arn).await;

        let (status, updated) = emulator.update(&arn, case.patch.clone()).await;
        assert_eq!(
            status, 200,
            "case {:?}: update must answer 200, got {status} {updated:?}",
            case.case
        );
        assert_eq!(
            updated["arn"].as_str(),
            Some(arn.as_str()),
            "case {:?}: update answers the arn it mutated, got {updated:?}",
            case.case
        );

        let (status, after) = emulator.get(&arn).await;
        assert_eq!(
            status, 200,
            "case {:?}: get after update must answer 200, got {status} {after:?}",
            case.case
        );
        assert_eq!(
            after["name"].as_str(),
            Some(case.expected_name),
            "case {:?}: name after update, got {after:?}",
            case.case
        );
        assert_eq!(
            after["description"].as_str(),
            Some(case.expected_description),
            "case {:?}: description after update, got {after:?}",
            case.case
        );
        assert_eq!(
            after["aggregationDuration"].as_str(),
            Some(case.expected_aggregation),
            "case {:?}: aggregationDuration after update, got {after:?}",
            case.case
        );
        // Members this operation cannot set keep the values create persisted.
        assert_eq!(
            after["creationTime"], before["creationTime"],
            "case {:?}: creationTime is set once and never rewritten, got {after:?}",
            case.case
        );
        assert_eq!(
            after["status"], before["status"],
            "case {:?}: an update does not move the status, got {after:?}",
            case.case
        );
        assert_eq!(
            after["subtype"], before["subtype"],
            "case {:?}: an update does not change the subtype, got {after:?}",
            case.case
        );
        assert_eq!(
            emulator.stored_tags(&arn).unwrap_or_default(),
            vec![("env".to_string(), "dev".to_string())],
            "case {:?}: an update leaves the persisted tags alone",
            case.case
        );
    }
}

/// RULE: a delete removes the record, so the configuration stops existing for
/// every reader.
#[tokio::test]
async fn delete_removes_the_record_from_get_and_from_list() {
    let emulator = Emulator::new();
    let mut arns = Vec::new();
    for index in 0..4 {
        arns.push(
            emulator
                .create(json!({
                    "name": format!("config-{index}"),
                    "description": format!("configuration {index}")
                }))
                .await,
        );
    }

    for (index, arn) in arns.iter().enumerate() {
        let (status, deleted) = emulator.delete(arn).await;
        assert_eq!(
            status, 200,
            "case {index}: delete must answer 200, got {status} {deleted:?}"
        );
        assert!(
            deleted.as_object().is_some_and(serde_json::Map::is_empty) || deleted == Value::Null,
            "case {index}: delete answers no members, got {deleted:?}"
        );

        let (status, got) = emulator.get(arn).await;
        assert_eq!(
            status, 404,
            "case {index}: a deleted arn is a 404, got {status} {got:?}"
        );
        assert_eq!(
            error_code(&got),
            "ResourceNotFoundException",
            "case {index}: the 404 after delete is a ResourceNotFoundException, got {got:?}"
        );

        let (status, listed) = emulator.send("GET", "/notification-configurations", None).await;
        assert_eq!(status, 200, "case {index}: list must answer 200");
        let remaining: Vec<&str> = listed["notificationConfigurations"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .map(|item| item["arn"].as_str().unwrap_or_default())
                    .collect()
            })
            .unwrap_or_default();
        assert!(
            !remaining.contains(&arn.as_str()),
            "case {index}: a deleted arn must leave the listing, got {remaining:?}"
        );
        assert_eq!(
            remaining.len(),
            arns.len() - index - 1,
            "case {index}: the listing reports one entry per surviving record, got {remaining:?}"
        );

        // Deleting it again is a 404: there is no record left to remove.
        let (status, again) = emulator.delete(arn).await;
        assert_eq!(
            status, 404,
            "case {index}: a second delete is a 404, got {status} {again:?}"
        );
    }
}

/// RULE: a listing reports one entry per stored configuration, newest first,
/// cut into pages of at most `maxResults`; a page token resumes exactly where
/// the previous page stopped and the final page carries no token.
#[tokio::test]
async fn list_pages_newest_first_and_resumes_from_its_token() {
    let emulator = Emulator::new();
    let total = 7usize;
    let mut created = Vec::new();
    for index in 0..total {
        created.push(
            emulator
                .create(json!({
                    "name": format!("page-config-{index}"),
                    "description": format!("page configuration {index}")
                }))
                .await,
        );
    }
    // "in reverse chronological order (newest first)", from the operation's own
    // documentation in the model.
    let newest_first: Vec<String> = created.iter().rev().cloned().collect();

    // Page sizes spanning the published `maxResults` range: its minimum (1),
    // sizes below, at and above the record count, and its maximum (100).
    for page_size in [1usize, 2, 3, 7, 8, 100] {
        let mut collected: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            pages += 1;
            assert!(
                pages <= total + 1,
                "case maxResults={page_size}: paging did not terminate after {pages} pages"
            );
            let uri = match &token {
                Some(value) => format!(
                    "/notification-configurations?maxResults={page_size}&nextToken={value}"
                ),
                None => format!("/notification-configurations?maxResults={page_size}"),
            };
            let (status, page) = emulator.send("GET", &uri, None).await;
            assert_eq!(
                status, 200,
                "case maxResults={page_size}: list must answer 200, got {status} {page:?}"
            );
            let items = page["notificationConfigurations"]
                .as_array()
                .unwrap_or_else(|| {
                    panic!("case maxResults={page_size}: notificationConfigurations is required, got {page:?}")
                })
                .clone();
            assert!(
                items.len() <= page_size,
                "case maxResults={page_size}: a page must not exceed maxResults, got {} entries",
                items.len()
            );
            for item in &items {
                // Every entry is the stored record, not a fixed row.
                let arn = item["arn"].as_str().unwrap_or_default().to_string();
                assert!(
                    is_configuration_arn(&arn),
                    "case maxResults={page_size}: a listed arn must match the modelled pattern, got {arn:?}"
                );
                assert_eq!(
                    item["status"].as_str(),
                    Some("ACTIVE"),
                    "case maxResults={page_size}: a listed entry reports its persisted status, got {item:?}"
                );
                assert_eq!(
                    item["subtype"].as_str(),
                    Some("ACCOUNT"),
                    "case maxResults={page_size}: a listed entry reports its persisted subtype, got {item:?}"
                );
                collected.push(arn);
            }
            match page.get("nextToken").and_then(Value::as_str) {
                Some(next) => {
                    assert!(
                        collected.len() < total,
                        "case maxResults={page_size}: a token was emitted after the last record"
                    );
                    assert!(
                        !items.is_empty(),
                        "case maxResults={page_size}: an empty page must not carry a token"
                    );
                    token = Some(next.to_string());
                }
                None => break,
            }
        }
        assert_eq!(
            collected, newest_first,
            "case maxResults={page_size}: the pages must concatenate to every stored configuration, newest first"
        );
    }

    // An unpaged listing reports every record, and reports it the same way twice:
    // a read changes nothing.
    let (status, first) = emulator.send("GET", "/notification-configurations", None).await;
    assert_eq!(status, 200, "case unpaged: list must answer 200");
    let (_, second) = emulator.send("GET", "/notification-configurations", None).await;
    assert_eq!(
        first, second,
        "case unpaged: a list must not change what the next list reports"
    );
}

/// RULE: a listing filter narrows the result to the configurations whose own
/// persisted value matches it; a filter value nothing is stored under selects
/// nothing.
#[tokio::test]
async fn list_filters_select_by_the_persisted_value() {
    let emulator = Emulator::new();
    let total = 3usize;
    for index in 0..total {
        emulator
            .create(json!({
                "name": format!("filter-config-{index}"),
                "description": format!("filter configuration {index}")
            }))
            .await;
    }


    // Catalog source: `NotificationConfigurationStatus`, re-exported as
    // `NOTIFICATION_CONFIGURATION_STATUS_VALUES`. Everything created here is
    // ACTIVE, so that value selects all and every other value selects none.
    for status_value in NOTIFICATION_CONFIGURATION_STATUS_VALUES {
        let expected = if *status_value == "ACTIVE" { total } else { 0 };
        let (code, response) = emulator.list(&format!("status={status_value}")).await;
        assert_eq!(
            code, 200,
            "case status={status_value}: list must answer 200, got {code} {response:?}"
        );
        let count = response["notificationConfigurations"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default();
        assert_eq!(
            count, expected,
            "case status={status_value}: expected {expected} entries, got {response:?}"
        );
    }

    // Catalog source: `NotificationConfigurationSubtype`, re-exported as
    // `NOTIFICATION_CONFIGURATION_SUBTYPE_VALUES`.
    for subtype_value in NOTIFICATION_CONFIGURATION_SUBTYPE_VALUES {
        let expected = if *subtype_value == "ACCOUNT" { total } else { 0 };
        let (code, response) = emulator.list(&format!("subtype={subtype_value}")).await;
        assert_eq!(
            code, 200,
            "case subtype={subtype_value}: list must answer 200, got {code} {response:?}"
        );
        let count = response["notificationConfigurations"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default();
        assert_eq!(
            count, expected,
            "case subtype={subtype_value}: expected {expected} entries, got {response:?}"
        );
    }

    // Two filters that agree select the same set as each alone; two that
    // disagree select nothing.
    for (case, query, expected) in [
        ("both filters match", "status=ACTIVE&subtype=ACCOUNT", total),
        ("status disagrees", "status=INACTIVE&subtype=ACCOUNT", 0),
        ("subtype disagrees", "status=ACTIVE&subtype=ADMIN_MANAGED", 0),
        // An EventRule source or a channel arn is matched against the
        // record's own associations, and a created configuration has none
        // (CreateEventRule / AssociateChannel are still 501), so these
        // filters select nothing.
        ("event rule source filter", "eventRuleSource=aws.ec2", 0),
        (
            "channel arn filter",
            "channelArn=arn%3Aaws%3Achatbot%3Aus-east-1%3A000000000000%3Achat-configuration%2Fslack",
            0,
        ),
    ] {
        let (code, response) = emulator.list(query).await;
        assert_eq!(
            code, 200,
            "case {case:?}: list must answer 200, got {code} {response:?}"
        );
        let count = response["notificationConfigurations"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default();
        assert_eq!(
            count, expected,
            "case {case:?}: expected {expected} entries, got {response:?}"
        );
    }
}

/// RULE: a request outside a modelled constraint is refused with a 400, and a
/// `nextToken` this service never issued is refused rather than silently
/// restarting the listing.
#[tokio::test]
async fn requests_outside_the_modelled_constraints_are_refused() {
    let emulator = Emulator::new();
    emulator
        .create(json!({ "name": "bounds-config", "description": "bounds" }))
        .await;

    // `maxResults` carries range 1..100: both published bounds are accepted and
    // one step past each is refused.
    for (case, value, expected) in [
        ("maxResults at the minimum", 1, 200),
        ("maxResults at the maximum", 100, 200),
        ("maxResults below the minimum", 0, 400),
        ("maxResults above the maximum", 101, 400),
    ] {
        let uri = format!("/notification-configurations?maxResults={value}");
        let (status, response) = emulator.send("GET", &uri, None).await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected {expected}, got {status} {response:?}"
        );
    }

    // `name` carries length 1..64 and `description` length 0..256.
    for (case, body, expected) in [
        (
            "name one character past its maximum",
            json!({ "name": "n".repeat(65), "description": "too long a name" }),
            400,
        ),
        (
            "description one character past its maximum",
            json!({ "name": "long-description", "description": "d".repeat(257) }),
            400,
        ),
        (
            "name outside the permitted character set",
            json!({ "name": "not a valid name!", "description": "bad characters" }),
            400,
        ),
        (
            "aggregationDuration outside the enum",
            json!({ "name": "bad-duration", "description": "x", "aggregationDuration": "FOREVER" }),
            400,
        ),
        (
            "a required member is absent",
            json!({ "name": "no-description" }),
            400,
        ),
    ] {
        let (status, response) = emulator
            .send("POST", "/notification-configurations", Some(body))
            .await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected {expected}, got {status} {response:?}"
        );
    }

    // A token this service did not issue is a validation failure, not an
    // accidental restart at the newest record: rows span a non-hex token, a
    // token of the right alphabet but the wrong width, and a well-formed cursor
    // carrying a tag this process never minted.
    for (case, token) in [
        ("not hex at all", "notatokenthisserviceissued"),
        ("hex but too short", "0000000000000063"),
        (
            "well-formed cursor with a foreign tag",
            "ffffffffffffffff0000000000000001",
        ),
    ] {
        let (status, response) = emulator
            .list(&format!("nextToken={token}"))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {response:?}"
        );
    }
}

/// RULE: the store is bounded, so a create past its ceiling is refused with the
/// modelled quota error instead of growing the store further; every
/// configuration admitted below the ceiling is still readable afterwards.
#[tokio::test]
async fn a_create_past_the_store_ceiling_is_refused_with_the_modelled_quota_error() {
    let emulator = Emulator::new();
    let ceiling = service_com_amazonaws_notifications_notifications::state::MAX_NOTIFICATION_CONFIGURATIONS;

    for index in 0..ceiling {
        let (status, response) = emulator
            .send(
                "POST",
                "/notification-configurations",
                Some(json!({
                    "name": format!("ceiling-config-{index}"),
                    "description": "filling the store to its ceiling"
                })),
            )
            .await;
        assert_eq!(
            status, 201,
            "case {index} of {ceiling}: a create below the ceiling must be admitted, got {status} {response:?}"
        );
    }

    // Rows past the ceiling: the refusal does not depend on what the request
    // carries.
    for (case, body) in [
        (
            "minimal request past the ceiling",
            json!({ "name": "one-too-many", "description": "" }),
        ),
        (
            "fully populated request past the ceiling",
            json!({
                "name": "one-too-many-with-everything",
                "description": "every optional member set",
                "aggregationDuration": "LONG",
                "tags": { "env": "dev" }
            }),
        ),
    ] {
        let (status, response) = emulator
            .send("POST", "/notification-configurations", Some(body))
            .await;
        assert_eq!(
            status, 402,
            "case {case:?}: a create past the ceiling is the modelled quota refusal, got {status} {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "ServiceQuotaExceededException",
            "case {case:?}: expected a ServiceQuotaExceededException, got {response:?}"
        );
    }

    // The refusal did not consume a slot, and deleting one frees exactly one.
    let (status, page) = emulator.list("maxResults=1").await;
    assert_eq!(status, 200, "case after refusal: list must answer 200");
    let newest = page["notificationConfigurations"][0]["arn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let (status, _) = emulator.delete(&newest).await;
    assert_eq!(status, 200, "case after refusal: delete must answer 200");
    let (status, response) = emulator
        .send(
            "POST",
            "/notification-configurations",
            Some(json!({ "name": "freed-slot", "description": "a delete freed a slot" })),
        )
        .await;
    assert_eq!(
        status, 201,
        "case after a delete: the freed slot must admit one create, got {status} {response:?}"
    );
}
