// SPDX-License-Identifier: CC0-1.0

//! `ListTagsForResource` over the wire: the tags held for the resource an ARN names.
//!
//! Every case goes through the real router (the harness `tests/all/application_lifecycle.rs`
//! owns), so the restJson1 routing of `GET /tags/{resourceArn}`, the boundary constraint
//! walk, the handler and the response serialization all run exactly as they do in the
//! emulator binary. Nothing reaches into `crate::state` directly: a tag is only believed
//! when a `ListTagsForResource` call reports it back out of what a `RegisterApplication`
//! persisted.
//!
//! The rule under test: `ListTagsForResource` reports exactly the tag set persisted for
//! the resource its ARN names — a registered SAP application or one of the SAP HANA
//! databases of that registration, in the Region the request was signed for — and
//! refuses an ARN that names no such resource as not found (HTTP 404) instead of
//! reporting an empty tag set for a resource that does not exist.
//!
//! The table spans what that rule generalises over: tag count (none, one, several), key
//! form across the modelled `TagKey` character class, value length at both modelled
//! bounds, the kind of resource the ARN names (application, database), and the ways an
//! ARN can name nothing (never registered, deregistered, another Region, a database the
//! registration never named, another service's resource).

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};
use service_com_amazonaws_ssmsap_ssmsap::state::RESOURCE_NOT_FOUND_MESSAGE;

use crate::application_lifecycle::{settle, tag_pairs, valid_registration, Emulator, Reply};
use crate::validation_message_rendering::message;

/// The region the suite signs for; it is the region every expected ARN carries.
const REGION: &str = "us-east-1";

/// A second regional endpoint, so an ARN can be read at a Region its resource was not
/// registered in.
const OTHER_REGION: &str = "us-west-2";

/// The account every emulated ARN carries, from the core's single source of truth.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The error code and status an absent resource is answered with: the class the
/// ARN-addressed tagging routes really carry, which is NOT the modelled
/// `ResourceNotFoundException` shape name.
const TAGGING_NOT_FOUND_CODE: &str =
    service_com_amazonaws_ssmsap_ssmsap::scaffolded::NOT_FOUND_CODE;

/// The longest `TagValue` the model admits
/// (`smithy_query(query="shape", shape="TagValue")`: length 1..256).
pub(crate) const TAG_VALUE_MAX_LENGTH: usize = 256;

/// Key forms drawn from the modelled `TagKey` character class
/// (`smithy_query(query="shape", shape="TagKey")`: `^(?!aws:)[a-zA-Z+-=._:/]+$`, whose
/// `+-=` range spans `+ , - . / 0-9 : ; < =`), so the rule is read off the class rather
/// than off one key: lower and upper case letters, a dot, an underscore, a slash, a
/// colon, a hyphen, a plus, an equals sign and a digit.
pub(crate) const TAG_KEY_FORMS: &[&str] = &[
    "env",
    "Owner",
    "cost.center",
    "team_name",
    "path/segment",
    "name:space",
    "a-b",
    "plus+key",
    "eq=key",
    "tier1",
];

/// One registration and the tag set `ListTagsForResource` must report for its
/// application.
struct Case {
    /// How an assertion names the row.
    name: &'static str,
    application_id: &'static str,
    /// The `Tags` the registration carries, which is the set the read must report.
    tags: Vec<(String, String)>,
}

impl Case {
    fn expected(&self) -> BTreeMap<String, String> {
        self.tags.iter().cloned().collect()
    }
}

/// The rows: one registration each, spanning tag count, key form and value length.
fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "a registration carrying no tags",
            application_id: "tags-none",
            tags: Vec::new(),
        },
        Case {
            name: "one tag",
            application_id: "tags-one",
            tags: vec![("env".to_string(), "dev".to_string())],
        },
        Case {
            name: "three tags",
            application_id: "tags-three",
            tags: vec![
                ("env".to_string(), "prod".to_string()),
                ("owner".to_string(), "sap-team".to_string()),
                ("tier".to_string(), "1".to_string()),
            ],
        },
        Case {
            name: "keys spanning the modelled TagKey class",
            application_id: "tags-key-forms",
            tags: TAG_KEY_FORMS
                .iter()
                .enumerate()
                .map(|(index, key)| ((*key).to_string(), format!("value-{index}")))
                .collect(),
        },
        Case {
            name: "a value at each modelled TagValue length bound",
            application_id: "tags-value-bounds",
            tags: vec![
                ("shortest".to_string(), "x".to_string()),
                ("longest".to_string(), "v".repeat(TAG_VALUE_MAX_LENGTH)),
            ],
        },
    ]
}

/// The ARN `RegisterApplication` mints for `application_id` in `region`.
///
/// `pub(crate)` because the suites of this target share one harness:
/// `tests/all/tag_resource.rs` names the SAME resources through the same `/tags`
/// route.
pub(crate) fn application_arn(region: &str, application_id: &str) -> String {
    format!("arn:aws:ssm-sap:{region}:{ACCOUNT}:application/{application_id}")
}

/// The ARN the database `database_id` of `application_id` carries in `region`.
pub(crate) fn database_arn(region: &str, application_id: &str, database_id: &str) -> String {
    format!("arn:aws:ssm-sap:{region}:{ACCOUNT}:database/{application_id}/{database_id}")
}

/// `ListTagsForResource`'s URI for `resource_arn`.
///
/// `resourceArn` is a NON-greedy `@httpLabel`, so it is one path segment: the `:` and
/// `/` of an ARN travel as `%3A` and `%2F` or the request addresses a different route.
pub(crate) fn tags_path(resource_arn: &str) -> String {
    format!(
        "/tags/{}",
        resource_arn.replace(':', "%3A").replace('/', "%2F")
    )
}

/// `ListTagsForResource` for `resource_arn`, read at `region`'s endpoint: the modelled
/// `GET /tags/{resourceArn}`, which carries no body at all.
pub(crate) async fn list_tags_in_region(
    emulator: &Emulator,
    region: &str,
    resource_arn: &str,
) -> Reply {
    emulator
        .send_request("GET", region, &tags_path(resource_arn), None)
        .await
}

/// `ListTagsForResource` for `resource_arn` at [`REGION`]'s endpoint.
pub(crate) async fn list_tags(emulator: &Emulator, resource_arn: &str) -> Reply {
    list_tags_in_region(emulator, REGION, resource_arn).await
}

/// The `Credentials` list of a registration naming each of `database_names`.
///
/// `CredentialType` is the model's single `CredentialType` value and `SecretId` names a
/// Secrets Manager secret, so each entry is a credential the service would accept.
fn credentials(database_names: &[&str]) -> Value {
    Value::Array(
        database_names
            .iter()
            .map(|name| {
                json!({
                    "DatabaseName": name,
                    "CredentialType": "ADMIN",
                    "SecretId": format!("sap/{name}/admin"),
                })
            })
            .collect(),
    )
}

/// A `RegisterApplication` body for a HANA application carrying `tags`, and the
/// databases `database_names` names when the row asks for its own.
pub(crate) fn registration(
    application_id: &str,
    tags: &[(String, String)],
    database_names: Option<&[&str]>,
) -> Value {
    let mut body = valid_registration(application_id, "HANA");
    let members = body
        .as_object_mut()
        .expect("a registration body is a JSON object");
    if let Some(names) = database_names {
        members.insert("Credentials".to_string(), credentials(names));
    }
    if !tags.is_empty() {
        let mut map = Map::new();
        for (key, value) in tags {
            map.insert(key.clone(), json!(value));
        }
        members.insert("Tags".to_string(), Value::Object(map));
    }
    body
}

/// Register `body`, asserting the registration itself was accepted.
pub(crate) async fn register(emulator: &Emulator, case: &str, body: Value) {
    let reply = emulator.send("/register-application", body).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200 from RegisterApplication, got {reply:?}"
    );
}

/// RULE: `ListTagsForResource` reports exactly the tag set persisted for the resource
/// its ARN names — so the set a registration carried is the set the read reports, and
/// the same set `GetApplication` reports for that application.
#[tokio::test]
async fn the_tag_set_reported_is_the_one_persisted_for_that_resource() {
    let emulator = Emulator::new();

    for case in cases() {
        register(
            &emulator,
            case.name,
            registration(case.application_id, &case.tags, None),
        )
        .await;

        let arn = application_arn(REGION, case.application_id);
        let reply = list_tags(&emulator, &arn).await;
        assert_eq!(
            reply.status, 200,
            "case {:?}: expected 200 from ListTagsForResource, got {reply:?}",
            case.name
        );
        let reported = tag_pairs(reply.member("tags"));
        assert_eq!(
            reported,
            case.expected(),
            "case {:?}: expected the registered tags {:?}, got {reported:?}",
            case.name,
            case.expected()
        );

        // One source of truth: the tags this read reports are the tags every other read
        // of the same application reports.
        let fetched = emulator
            .send(
                "/get-application",
                json!({ "ApplicationId": case.application_id }),
            )
            .await;
        let by_get_application = tag_pairs(fetched.member("Tags"));
        assert_eq!(
            by_get_application, reported,
            "case {:?}: expected GetApplication to report the same tags {reported:?}, got \
             {by_get_application:?}",
            case.name
        );
    }
}

/// RULE: the tag set belongs to the RESOURCE the ARN names, so the databases of a
/// registration report their own sets rather than the application's.
#[tokio::test]
async fn a_database_reports_its_own_tag_set_and_not_its_applications() {
    let emulator = Emulator::new();

    // Rows: how many databases the registration names, which is what the rule
    // generalises over here.
    let rows: &[(&str, &str, &[&str])] = &[
        ("one database", "db-one", &["HDB"]),
        (
            "three databases",
            "db-three",
            &["SYSTEMDB", "TNT1", "TNT2"],
        ),
    ];

    let tags = vec![("env".to_string(), "dev".to_string())];
    for (name, application_id, database_names) in rows {
        register(
            &emulator,
            name,
            registration(application_id, &tags, Some(database_names)),
        )
        .await;

        // The application itself carries the registration's tags.
        let reply = list_tags(&emulator, &application_arn(REGION, application_id)).await;
        let reported = tag_pairs(reply.member("tags"));
        assert_eq!(
            reported,
            tags.iter().cloned().collect::<BTreeMap<_, _>>(),
            "case {name:?}: expected the application's own tags, got {reported:?}"
        );

        for database_id in *database_names {
            let arn = database_arn(REGION, application_id, database_id);
            let reply = list_tags(&emulator, &arn).await;
            assert_eq!(
                reply.status, 200,
                "case {name:?}/{database_id:?}: expected 200 for a database of the \
                 registration, got {reply:?}"
            );
            let reported = tag_pairs(reply.member("tags"));
            assert!(
                reported.is_empty(),
                "case {name:?}/{database_id:?}: nothing tagged the database, expected an \
                 empty tag set, got {reported:?}"
            );
        }
    }
}

/// RULE: an ARN that names no resource of this account and Region is refused as not
/// found (HTTP 404) with the fixed sentence that reports nothing of the ARN — never an
/// empty tag set for a resource that does not exist.
#[tokio::test]
async fn an_arn_naming_no_resource_here_is_refused_as_not_found() {
    let emulator = Emulator::new();

    let tags = vec![("env".to_string(), "dev".to_string())];
    register(
        &emulator,
        "a registered application",
        registration("tags-present", &tags, Some(&["HDB"])),
    )
    .await;
    register(
        &emulator,
        "an application to deregister",
        registration("tags-gone", &tags, None),
    )
    .await;
    let reply = emulator
        .send(
            "/deregister-application",
            json!({ "ApplicationId": "tags-gone" }),
        )
        .await;
    assert_eq!(
        reply.status, 200,
        "expected 200 from DeregisterApplication, got {reply:?}"
    );
    // Deregistration is asynchronous: the tags held for the application's ARN go with it
    // when the removal the clock owes it falls due.
    settle().await;

    // (label, the region the read is made in, the ARN it names)
    let rows: Vec<(&str, &str, String)> = vec![
        (
            "an application never registered",
            REGION,
            application_arn(REGION, "tags-never-registered"),
        ),
        (
            "an application that was deregistered",
            REGION,
            application_arn(REGION, "tags-gone"),
        ),
        (
            "a registered application read at another regional endpoint",
            OTHER_REGION,
            application_arn(REGION, "tags-present"),
        ),
        (
            "an ARN carrying another Region",
            REGION,
            application_arn(OTHER_REGION, "tags-present"),
        ),
        (
            "a database the registration never named",
            REGION,
            database_arn(REGION, "tags-present", "NOPE"),
        ),
        (
            "a database of an application that is not registered",
            REGION,
            database_arn(REGION, "tags-never-registered", "HDB"),
        ),
        (
            "another service's resource",
            REGION,
            "arn:aws:s3:::a-bucket/an-object".to_string(),
        ),
    ];

    for (label, region, arn) in rows {
        let reply = list_tags_in_region(&emulator, region, &arn).await;
        assert_eq!(
            reply.status, 404,
            "case {label:?}: expected 404, got {}: {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.code(),
            TAGGING_NOT_FOUND_CODE,
            "case {label:?}: expected {TAGGING_NOT_FOUND_CODE}, got {:?}",
            reply.code()
        );
        let stated = message(&reply);
        assert_eq!(
            stated, RESOURCE_NOT_FOUND_MESSAGE,
            "case {label:?}: expected {RESOURCE_NOT_FOUND_MESSAGE:?}, got {stated:?}"
        );
    }
}

/// RULE: listing tags is a READ — it changes nothing a later caller sees, so repeated
/// reads of an unchanged resource report the same set and the resource stays reachable.
#[tokio::test]
async fn listing_tags_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();

    let tags = vec![
        ("env".to_string(), "dev".to_string()),
        ("owner".to_string(), "sap-team".to_string()),
    ];
    register(
        &emulator,
        "a repeated read",
        registration("tags-repeat", &tags, Some(&["HDB"])),
    )
    .await;

    let arn = application_arn(REGION, "tags-repeat");
    let expected: BTreeMap<String, String> = tags.iter().cloned().collect();
    for attempt in 1..=3 {
        let reply = list_tags(&emulator, &arn).await;
        assert_eq!(
            reply.status, 200,
            "case \"read {attempt}\": expected 200, got {reply:?}"
        );
        let reported = tag_pairs(reply.member("tags"));
        assert_eq!(
            reported, expected,
            "case \"read {attempt}\": expected {expected:?}, got {reported:?}"
        );
    }

    // And the resource the reads named is still listed as it was.
    let listed = emulator.send("/list-applications", json!({})).await;
    let ids: Vec<String> = listed
        .member("Applications")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("Id").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        ids,
        vec!["tags-repeat".to_string()],
        "case \"after the reads\": expected the one registered application, got {ids:?}"
    );
}
