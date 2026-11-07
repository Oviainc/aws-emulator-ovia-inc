// SPDX-License-Identifier: CC0-1.0

//! The `ActionType` every operation of this service reports, over the wire it actually
//! speaks: `awsJson1_1`, `POST /` with `X-Amz-Target: CodePipeline_20150709.<operation>`.
//!
//! Two rules, each asserted over a SET of inputs rather than one recorded call:
//!
//! 1. WIRE FORM — wherever this service serializes an `ActionType`, the answer carries
//!    every member the shape declares: `settings` as the empty structure and
//!    `actionConfigurationProperties` as the empty list when the action type holds
//!    nothing for them, and `id` carrying the awsJson shape discriminator `__type`
//!    beside its four members.
//! 2. CATALOG — each action type the service provides ITSELF reports that provider's
//!    documented artifact counts, configuration properties and deep links, so a client
//!    that discovers providers here can build a valid action declaration for one.
//!
//! Every read goes through the router, so nothing here can pass by reading a handler's
//! internals.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_codepipeline_codepipeline_20150709::state::{
    BuiltInActionType, BUILT_IN_ACTION_TYPES,
};
use service_common::management_events::{ManagementEvent, ManagementEventLog};

/// The `X-Amz-Target` prefix OPERATIONS.md records for this service.
const TARGET_PREFIX: &str = "CodePipeline_20150709";

/// The shape name the awsJson discriminator carries on an `ActionType`'s id.
const ACTION_TYPE_ID_SHAPE: &str = "ActionTypeId";

/// `ActionConfigurationPropertyList`'s modelled `@length(max: 10)`
/// (`smithy_query query="shape" shape="ActionConfigurationPropertyList"`).
const MAX_CONFIGURATION_PROPERTIES: usize = 10;

/// `MinimumArtifactCount` / `MaximumArtifactCount`'s modelled `@range(min: 0, max: 5)`
/// (`smithy_query query="shape" shape="ArtifactDetails"`).
const ARTIFACT_COUNT_BOUNDS: (i64, i64) = (0, 5);

/// `ActionConfigurationKey`'s modelled `@length(min: 1, max: 50)`
/// (`smithy_query query="shape" shape="ActionConfigurationProperty"`).
const PROPERTY_NAME_LENGTH: (usize, usize) = (1, 50);

/// One emulator: the shared engine, the service mounted on it, and the router that
/// dispatches to it — exactly how `core/router` assembles the production emulator.
struct Emulator {
    router: MultiServiceRouter,
    /// Kept so a case can read the account's management-event log back out of the SHARED
    /// store the service records into.
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_codepipeline_codepipeline_20150709::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        Self { router, engine }
    }

    /// The `(eventName, readOnly)` of every call recorded in the account's log, oldest
    /// first.
    fn recorded_calls(&self) -> Vec<(String, bool)> {
        let log = self.engine.state::<ManagementEventLog>();
        let held = log.lock();
        held.iter()
            .map(ManagementEvent::to_cloudtrail_event_json)
            .map(|event| {
                (
                    event["eventName"].as_str().unwrap_or_default().to_string(),
                    event["readOnly"].as_bool().unwrap_or_default(),
                )
            })
            .collect()
    }

    /// Call `operation` and answer its `(status, body)`.
    async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        let authorization = "AWS4-HMAC-SHA256 \
             Credential=AKIDEXAMPLE/20260101/us-east-1/codepipeline/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=0123456789abcdef";
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
            .header("authorization", authorization)
            .body(bytes::Bytes::from(body.to_string()))
            .expect("a valid request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = response.into_body();
        let parsed = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&body).unwrap_or_else(|error| {
                panic!("{operation}: response body is not JSON ({error}): {body:?}")
            })
        };
        (status, parsed)
    }

    /// A successful call.
    async fn ok(&self, operation: &str, body: &Value) -> Value {
        let (status, response) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "{operation} {body}: expected HTTP 200, got {status} with {response}"
        );
        response
    }

    /// Every action type `ListActionTypes` reports for `owner`, paged to the end.
    async fn listed(&self, owner: Option<&str>) -> Vec<Value> {
        let mut all = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut body = match owner {
                Some(owner) => json!({"actionOwnerFilter": owner}),
                None => json!({}),
            };
            if let Some(token) = &token {
                body["nextToken"] = json!(token);
            }
            let answer = self.ok("ListActionTypes", &body).await;
            all.extend(
                answer["actionTypes"]
                    .as_array()
                    .unwrap_or_else(|| panic!("actionTypes is a list, got {answer}"))
                    .clone(),
            );
            match answer.get("nextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break all,
            }
        }
    }
}

/// The `(category, owner, provider, version)` of an `ActionType` the wire reported.
fn id_of(action_type: &Value) -> (String, String, String, String) {
    let id = &action_type["id"];
    let member = |name: &str| {
        id[name]
            .as_str()
            .unwrap_or_else(|| panic!("id.{name} is a string, got {action_type}"))
            .to_string()
    };
    (
        member("category"),
        member("owner"),
        member("provider"),
        member("version"),
    )
}

/// RULE 1, asserted on every `ActionType` in `reported`: the shape discriminator on its
/// id, and both optional structural members reported whatever the action type holds.
fn assert_wire_form(case: &str, reported: &[Value]) {
    assert!(
        !reported.is_empty(),
        "case {case:?}: expected at least one action type to check"
    );
    for action_type in reported {
        let identity = id_of(action_type);
        assert_eq!(
            action_type["id"]["__type"].as_str(),
            Some(ACTION_TYPE_ID_SHAPE),
            "case {case:?}: {identity:?} must carry the id's shape discriminator, got {action_type}"
        );
        let settings = action_type.get("settings").unwrap_or_else(|| {
            panic!("case {case:?}: {identity:?} must report settings, got {action_type}")
        });
        assert!(
            settings.is_object(),
            "case {case:?}: {identity:?} must report settings as a structure, got {settings}"
        );
        let properties = action_type
            .get("actionConfigurationProperties")
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: {identity:?} must report actionConfigurationProperties, \
                     got {action_type}"
                )
            });
        let properties = properties.as_array().unwrap_or_else(|| {
            panic!(
                "case {case:?}: {identity:?} must report actionConfigurationProperties as a \
                 list, got {properties}"
            )
        });
        assert!(
            properties.len() <= MAX_CONFIGURATION_PROPERTIES,
            "case {case:?}: {identity:?} reports {} properties, past the modelled maximum of \
             {MAX_CONFIGURATION_PROPERTIES}",
            properties.len()
        );
        for member in ["inputArtifactDetails", "outputArtifactDetails"] {
            for bound in ["minimumCount", "maximumCount"] {
                let count = action_type[member][bound].as_i64().unwrap_or_else(|| {
                    panic!("case {case:?}: {identity:?} must report {member}.{bound}, got {action_type}")
                });
                assert!(
                    (ARTIFACT_COUNT_BOUNDS.0..=ARTIFACT_COUNT_BOUNDS.1).contains(&count),
                    "case {case:?}: {identity:?} reports {member}.{bound} = {count}, outside \
                     the modelled range {ARTIFACT_COUNT_BOUNDS:?}"
                );
            }
        }
    }
}

/// RULE 1 over every way this service answers with an `ActionType`: each owner's listing,
/// the unfiltered listing, and the create answer — for a create that declared neither
/// `settings` nor `configurationProperties`, one that declared both, and one that
/// declared only settings.
#[tokio::test]
async fn every_reported_action_type_carries_the_members_the_shape_declares() {
    let emulator = Emulator::new();

    // Rows differ along what the rule generalises over: how much the request declared.
    let creates: [(&str, Value); 3] = [
        (
            "a create declaring neither settings nor properties",
            json!({
                "category": "Build",
                "provider": "Bare-Runner",
                "version": "1",
                "inputArtifactDetails": {"minimumCount": 0, "maximumCount": 0},
                "outputArtifactDetails": {"minimumCount": 0, "maximumCount": 0},
            }),
        ),
        (
            "a create declaring only settings",
            json!({
                "category": "Test",
                "provider": "Settings-Only",
                "version": "2",
                "settings": {"entityUrlTemplate": "https://example.test/{Config:Suite}"},
                "inputArtifactDetails": {"minimumCount": 1, "maximumCount": 1},
                "outputArtifactDetails": {"minimumCount": 0, "maximumCount": 1},
            }),
        ),
        (
            "a create declaring settings and properties",
            json!({
                "category": "Invoke",
                "provider": "Full_Runner",
                "version": "3",
                "settings": {
                    "entityUrlTemplate": "https://example.test/entity/{Config:Suite}",
                    "executionUrlTemplate": "https://example.test/execution",
                },
                "configurationProperties": [{
                    "name": "Suite",
                    "required": true,
                    "key": true,
                    "secret": false,
                    "queryable": false,
                }],
                "inputArtifactDetails": {"minimumCount": 0, "maximumCount": 5},
                "outputArtifactDetails": {"minimumCount": 0, "maximumCount": 2},
            }),
        ),
    ];

    for (case, request) in &creates {
        let answer = emulator.ok("CreateCustomActionType", request).await;
        let created = answer
            .get("actionType")
            .unwrap_or_else(|| panic!("case {case:?}: no actionType in {answer}"))
            .clone();
        assert_wire_form(case, &[created]);
    }

    // Every `ActionOwner` value (`smithy_query query="shape" shape="ActionOwner"`), plus
    // the unfiltered listing, which reports all three.
    for owner in [Some("AWS"), Some("ThirdParty"), Some("Custom"), None] {
        let case = format!("ListActionTypes owner={owner:?}");
        let reported = emulator.listed(owner).await;
        assert_wire_form(&case, &reported);
    }
}

/// RULE 1, round-tripped: the action type a create answered with is the one the listing
/// reports, byte for byte — the same discriminator and the same empty members.
#[tokio::test]
async fn a_created_action_type_is_listed_in_exactly_the_form_it_was_answered_in() {
    let emulator = Emulator::new();
    let request = json!({
        "category": "Deploy",
        "provider": "RoundTrip",
        "version": "1",
        "inputArtifactDetails": {"minimumCount": 1, "maximumCount": 1},
        "outputArtifactDetails": {"minimumCount": 0, "maximumCount": 0},
    });
    let answer = emulator.ok("CreateCustomActionType", &request).await;
    let created = answer["actionType"].clone();

    let listed = emulator.listed(Some("Custom")).await;
    assert_eq!(
        listed,
        vec![created.clone()],
        "the listing must report the created action type exactly as the create answered \
         it, got {listed:?} for {created}"
    );
    assert_eq!(
        created["settings"],
        json!({}),
        "a create that declared no settings reports the empty structure, got {created}"
    );
    assert_eq!(
        created["actionConfigurationProperties"],
        json!([]),
        "a create that declared no properties reports the empty list, got {created}"
    );
}

/// The two operations that serve an `ActionType` are served behind the wire-form wrapper,
/// which must not change what else serving a call does: each is still recorded in the
/// account's management-event log, as a reporting or a mutating call per its own
/// semantics.
#[tokio::test]
async fn serving_an_action_type_call_through_the_wire_form_still_records_it() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateCustomActionType",
            &json!({
                "category": "Build",
                "provider": "Audited",
                "version": "1",
                "inputArtifactDetails": {"minimumCount": 0, "maximumCount": 0},
                "outputArtifactDetails": {"minimumCount": 0, "maximumCount": 0},
            }),
        )
        .await;
    emulator.ok("ListActionTypes", &json!({})).await;

    let recorded = emulator.recorded_calls();
    for (operation, read_only) in [
        ("CreateCustomActionType", false),
        ("ListActionTypes", true),
    ] {
        assert!(
            recorded.contains(&(operation.to_string(), read_only)),
            "case {operation:?}: expected a record with readOnly={read_only}, got {recorded:?}"
        );
    }
}

/// One documented catalog entry: what the provider's reference page states for it.
#[derive(Debug)]
struct DocumentedEntry {
    category: &'static str,
    owner: &'static str,
    provider: &'static str,
    version: &'static str,
    /// `(minimumCount, maximumCount)` of its input and output artifacts.
    input: (i64, i64),
    output: (i64, i64),
    /// Its configuration parameters, in order, as `(name, required, key, secret)`.
    properties: &'static [(&'static str, bool, bool, bool)],
    /// The `settings` members it reports, by name.
    settings: &'static [&'static str],
}

/// Catalog entries as the provider's own structure reference documents them — the page
/// `ActionTypeId$provider` links to, "Valid action types and providers in CodePipeline"
/// (reference-pipeline-structure.html#actions-valid-providers) and its per-action
/// configuration-parameter tables.
///
/// Rows differ along the category, the owner, both artifact bounds, how many parameters
/// the action takes, whether it has a secret one and which deep links it carries. NONE of
/// them is an entry this issue's report cited (it cited `Source/AWS/S3` and
/// `Test/ThirdParty/GhostInspector`).
const DOCUMENTED_ENTRIES: &[DocumentedEntry] = &[
    DocumentedEntry {
        category: "Source",
        owner: "AWS",
        provider: "CodeCommit",
        version: "1",
        input: (0, 0),
        output: (1, 1),
        properties: &[
            ("RepositoryName", true, true, false),
            ("BranchName", true, true, false),
            ("PollForSourceChanges", false, false, false),
            ("OutputArtifactFormat", false, false, false),
        ],
        settings: &["entityUrlTemplate"],
    },
    DocumentedEntry {
        category: "Build",
        owner: "AWS",
        provider: "CodeBuild",
        version: "1",
        input: (1, 5),
        output: (0, 5),
        properties: &[
            ("ProjectName", true, true, false),
            ("PrimarySource", false, false, false),
            ("BatchEnabled", false, false, false),
            ("CombineArtifacts", false, false, false),
            ("EnvironmentVariables", false, false, false),
        ],
        settings: &["entityUrlTemplate", "executionUrlTemplate"],
    },
    DocumentedEntry {
        category: "Deploy",
        owner: "AWS",
        provider: "CodeDeploy",
        version: "1",
        input: (1, 1),
        output: (0, 0),
        properties: &[
            ("ApplicationName", true, true, false),
            ("DeploymentGroupName", true, true, false),
        ],
        settings: &["entityUrlTemplate"],
    },
    DocumentedEntry {
        category: "Invoke",
        owner: "AWS",
        provider: "Lambda",
        version: "1",
        input: (0, 5),
        output: (0, 5),
        properties: &[
            ("FunctionName", true, true, false),
            ("UserParameters", false, false, false),
        ],
        settings: &["entityUrlTemplate"],
    },
    DocumentedEntry {
        category: "Approval",
        owner: "AWS",
        provider: "Manual",
        version: "1",
        input: (0, 0),
        output: (0, 0),
        properties: &[
            ("NotificationArn", false, false, false),
            ("CustomData", false, false, false),
            ("ExternalEntityLink", false, false, false),
        ],
        settings: &[],
    },
    DocumentedEntry {
        category: "Source",
        owner: "ThirdParty",
        provider: "GitHub",
        version: "1",
        input: (0, 0),
        output: (1, 1),
        properties: &[
            ("Owner", true, true, false),
            ("Repo", true, true, false),
            ("Branch", true, true, false),
            // The caller's credential for the external system: secret, so a read of the
            // pipeline does not hand it back.
            ("OAuthToken", true, false, true),
            ("PollForSourceChanges", false, false, false),
        ],
        settings: &[
            "entityUrlTemplate",
            "revisionUrlTemplate",
            "thirdPartyConfigurationUrl",
        ],
    },
];

/// RULE 2: every documented entry is reported with that provider's artifact counts,
/// configuration parameters and deep links.
#[tokio::test]
async fn the_catalog_reports_each_providers_documented_configuration() {
    let emulator = Emulator::new();
    let listed = emulator.listed(None).await;

    for case in DOCUMENTED_ENTRIES {
        let wanted = (
            case.category.to_string(),
            case.owner.to_string(),
            case.provider.to_string(),
            case.version.to_string(),
        );
        let found = listed
            .iter()
            .find(|action_type| id_of(action_type) == wanted)
            .unwrap_or_else(|| panic!("case {case:?}: not listed, got {listed:?}"));

        for (member, (min, max)) in [
            ("inputArtifactDetails", case.input),
            ("outputArtifactDetails", case.output),
        ] {
            assert_eq!(
                (
                    found[member]["minimumCount"].as_i64(),
                    found[member]["maximumCount"].as_i64()
                ),
                (Some(min), Some(max)),
                "case {case:?}: unexpected {member}, got {found}"
            );
        }

        let reported: Vec<(String, bool, bool, bool)> = found["actionConfigurationProperties"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: properties are a list, got {found}"))
            .iter()
            .map(|property| {
                let flag = |name: &str| {
                    property[name].as_bool().unwrap_or_else(|| {
                        panic!("case {case:?}: {name} is a boolean, got {property}")
                    })
                };
                (
                    property["name"]
                        .as_str()
                        .unwrap_or_else(|| panic!("case {case:?}: name is a string, got {property}"))
                        .to_string(),
                    flag("required"),
                    flag("key"),
                    flag("secret"),
                )
            })
            .collect();
        let expected: Vec<(String, bool, bool, bool)> = case
            .properties
            .iter()
            .map(|(name, required, key, secret)| {
                (name.to_string(), *required, *key, *secret)
            })
            .collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: unexpected configuration properties, got {found}"
        );

        let mut settings: Vec<&str> = found["settings"]
            .as_object()
            .unwrap_or_else(|| panic!("case {case:?}: settings are a structure, got {found}"))
            .keys()
            .map(String::as_str)
            .collect();
        settings.sort_unstable();
        let mut wanted_settings: Vec<&str> = case.settings.to_vec();
        wanted_settings.sort_unstable();
        assert_eq!(
            settings, wanted_settings,
            "case {case:?}: unexpected settings members, got {found}"
        );
    }
}

/// The `{Config:<property>}` placeholders a URL template names, in the order they appear.
fn placeholders(template: &str) -> Vec<&str> {
    template
        .match_indices("{Config:")
        .filter_map(|(start, opener)| {
            let rest = &template[start + opener.len()..];
            rest.split_once('}').map(|(name, _)| name)
        })
        .collect()
}

/// RULE 2 as an INVARIANT over the whole catalog, so a provider added later cannot break
/// it: every entry stays inside the bounds the model declares for the members it reports,
/// and every property a deep link substitutes is a property that entry declares and marks
/// as a key.
#[test]
fn every_catalog_entry_stays_inside_the_modelled_bounds() {
    for entry in &BUILT_IN_ACTION_TYPES {
        let BuiltInActionType {
            category,
            owner,
            provider,
            version,
            ..
        } = entry;
        let case = format!("{category}/{owner}/{provider}/{version}");

        for (member, (min, max)) in [
            ("inputArtifactDetails", entry.input_artifacts),
            ("outputArtifactDetails", entry.output_artifacts),
        ] {
            let (floor, ceiling) = ARTIFACT_COUNT_BOUNDS;
            for (bound, count) in [("minimumCount", min), ("maximumCount", max)] {
                let count = i64::from(count);
                assert!(
                    (floor..=ceiling).contains(&count),
                    "case {case:?}: {member}.{bound} = {count}, outside the modelled range \
                     {ARTIFACT_COUNT_BOUNDS:?}"
                );
            }
            assert!(
                min <= max,
                "case {case:?}: {member} reports a minimum of {min} above its maximum of {max}"
            );
        }

        let properties = entry.configuration_properties;
        assert!(
            properties.len() <= MAX_CONFIGURATION_PROPERTIES,
            "case {case:?}: declares {} configuration properties, past the modelled maximum \
             of {MAX_CONFIGURATION_PROPERTIES}",
            properties.len()
        );
        for property in properties {
            let length = property.name.chars().count();
            assert!(
                (PROPERTY_NAME_LENGTH.0..=PROPERTY_NAME_LENGTH.1).contains(&length),
                "case {case:?}: the property name {:?} is {length} characters, outside the \
                 modelled length {PROPERTY_NAME_LENGTH:?}",
                property.name
            );
        }
        let mut names: Vec<&str> = properties.iter().map(|property| property.name).collect();
        let reported = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(
            names.len(),
            reported,
            "case {case:?}: names a configuration property more than once, got {:?}",
            properties
                .iter()
                .map(|property| property.name)
                .collect::<Vec<_>>()
        );

        // A deep link is built from the action's own key properties, so every placeholder
        // it substitutes must be one of them.
        for template in [
            entry.entity_url_template,
            entry.execution_url_template,
            entry.revision_url_template,
        ]
        .into_iter()
        .flatten()
        {
            for name in placeholders(template) {
                let property = properties
                    .iter()
                    .find(|property| property.name == name)
                    .unwrap_or_else(|| {
                        panic!(
                            "case {case:?}: the template {template:?} substitutes {name:?}, \
                             which this action type does not declare"
                        )
                    });
                assert!(
                    property.key,
                    "case {case:?}: {name:?} is substituted into {template:?}, so it must be \
                     reported as a key property"
                );
            }
        }

        // Only a `ThirdParty` provider has a sign-up page, and only a provider outside
        // this account can need a secret credential.
        if entry.owner == "AWS" {
            assert!(
                entry.third_party_configuration_url.is_none(),
                "case {case:?}: an AWS-owned action type has no third-party sign-up page"
            );
            assert!(
                properties.iter().all(|property| !property.secret),
                "case {case:?}: an AWS-owned action type needs no secret of the caller's"
            );
        }
    }

    // The catalog spans both owners it can: the service's own actions and the partner
    // integrations a pipeline can use.
    for owner in ["AWS", "ThirdParty"] {
        assert!(
            BUILT_IN_ACTION_TYPES
                .iter()
                .any(|entry| entry.owner == owner),
            "the catalog must hold at least one {owner}-owned action type"
        );
    }
    // A partner's credential IS secret where one is needed, so the flag is exercised in
    // both directions.
    assert!(
        BUILT_IN_ACTION_TYPES.iter().any(|entry| entry
            .configuration_properties
            .iter()
            .any(|property| property.secret)),
        "the catalog must hold a provider whose credential property is secret"
    );
}
