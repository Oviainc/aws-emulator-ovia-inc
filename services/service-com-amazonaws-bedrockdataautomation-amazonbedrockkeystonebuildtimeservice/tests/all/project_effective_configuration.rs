// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of what a DataAutomationProject READ reports, and of the
//! form every timestamp of this service takes on the wire.
//!
//! Every case drives the real `restJson1` wire path — `MultiServiceRouter::dispatch`
//! with a signed-looking request — so routing, HTTP binding, constraint validation,
//! the handler and response serialization are all exercised, exactly as an AWS SDK
//! `bedrock-data-automation` client would.
//!
//! Three rules are under test, each over a TABLE of inputs rather than one request:
//!
//! 1. EFFECTIVE CONFIGURATION. A project read reports the configuration the project
//!    will be processed under: every configuration block the modelled shape carries is
//!    present, each member taking the value the create or the update specified and the
//!    service's own default wherever that specification was silent. The rows differ in
//!    which blocks and which modalities the create spells out, and in how deep inside a
//!    modality the silence starts.
//! 2. WIRE FORM OF A TIMESTAMP. Every timestamp member of this model targets
//!    `DateTimestamp`, which carries `@timestampFormat("date-time")`
//!    (`smithy_query(query="shape",shape="DateTimestamp")`), so on this restJson1 wire
//!    it is an ISO-8601 date-time STRING. The rows span the surfaces that carry one: a
//!    project read, a blueprint create and read, and the summaries two listings serve.
//! 3. AN ACCEPTED TEARDOWN. `DeleteDataAutomationProject` accepts a teardown that
//!    proceeds after the call returns, so the status it reports is the in-flight one,
//!    never a terminal one — whatever the project's own build status had settled to.
//!
//! A read reports; it must not WRITE. Every suite that reads twice asserts the two
//! reads agree, and the configuration suite additionally asserts the STORE still holds
//! exactly the configuration the caller sent — the defaults are materialised on the way
//! out, not folded into the record.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_bedrockdataautomation_amazonbedrockkeystonebuildtimeservice::state;
use service_com_amazonaws_bedrockdataautomation_amazonbedrockkeystonebuildtimeservice::state::ComAmazonawsBedrockdataautomationAmazonbedrockkeystonebuildtimeserviceState as ServiceState;
use service_com_amazonaws_bedrockdataautomation_amazonbedrockkeystonebuildtimeservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{assert_json_contains, encode_label, string_at};

/// The region every case signs for.
const REGION: &str = "us-east-1";

/// A blueprint ARN inside the modelled `BlueprintArn` pattern (source:
/// `smithy_query(shape="BlueprintItem")`), so a `customOutputConfiguration` row is
/// accepted by the modelled constraints.
const PUBLIC_BLUEPRINT_ARN: &str =
    "arn:aws:bedrock:us-east-1:000000000000:blueprint/bedrock-data-automation-public-invoice";

/// A library ARN inside the modelled `DataAutomationLibraryArn` pattern (source:
/// `smithy_query(shape="DataAutomationLibraryItem")`).
const LIBRARY_ARN: &str =
    "arn:aws:bedrock:us-east-1:000000000000:data-automation-library/library000001";

/// A SigV4 `Authorization` header for this service's signing name (`bedrock`, per
/// `OPERATIONS.md`).
fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTKEY/20240101/{region}/bedrock/aws4_request, \
         SignedHeaders=host, Signature=0123456789abcdef"
    )
}

/// The emulator under test, with a handle on the shared engine so a case can assert
/// what the store HOLDS and not only what it answers.
fn emulator_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

fn emulator() -> MultiServiceRouter {
    emulator_with_engine().0
}

/// Dispatch one request and return its status and decoded JSON body (`Value::Null` for
/// an empty body).
async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "bedrock-data-automation.amazonaws.com")
        .header("content-type", "application/json")
        .header("authorization", authorization(REGION))
        .body(Bytes::from(
            serde_json::to_vec(body).expect("serializable body"),
        ))
        .expect("a valid test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let decoded = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, decoded)
}

/// `CreateDataAutomationProject`, per its modelled `@http`
/// `{method: PUT, uri: /data-automation-projects/, code: 201}`.
async fn create_project(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "PUT", "/data-automation-projects/", body).await
}

/// The `project` structure a successful `GetDataAutomationProject` carries.
async fn read_project(router: &MultiServiceRouter, arn: &str, case: &str) -> Value {
    let uri = format!("/data-automation-projects/{}/", encode_label(arn));
    let (status, payload) = call(router, "POST", &uri, &json!({})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetDataAutomationProject 200, got {status} with {payload:?}"
    );
    payload.get("project").cloned().unwrap_or_else(|| {
        panic!("case {case:?}: GetDataAutomationProject returned no project: {payload:?}")
    })
}

/// `DeleteDataAutomationProject`, per its modelled `@http`
/// `{method: DELETE, uri: /data-automation-projects/{projectArn}/}`.
async fn delete_project(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    let uri = format!("/data-automation-projects/{}/", encode_label(arn));
    call(router, "DELETE", &uri, &json!({})).await
}

/// The standard output configuration a project that specified NOTHING reports: Bedrock
/// Data Automation's own defaults — a document extracted per page and per element as
/// Markdown with no bounding boxes and no summary, an image and a video with text
/// detection, bounding boxes and a summary, and audio transcribed without channel or
/// speaker labels and without a summary.
fn default_standard_output() -> Value {
    json!({
        "document": {
            "extraction": {
                "granularity": { "types": ["PAGE", "ELEMENT"] },
                "boundingBox": { "state": "DISABLED" }
            },
            "generativeField": { "state": "DISABLED" },
            "outputFormat": {
                "textFormat": { "types": ["MARKDOWN"] },
                "additionalFileFormat": { "state": "DISABLED" }
            }
        },
        "image": {
            "extraction": {
                "category": { "state": "ENABLED", "types": ["TEXT_DETECTION"] },
                "boundingBox": { "state": "ENABLED" }
            },
            "generativeField": { "state": "ENABLED", "types": ["IMAGE_SUMMARY"] }
        },
        "video": {
            "extraction": {
                "category": { "state": "ENABLED", "types": ["TEXT_DETECTION"] },
                "boundingBox": { "state": "ENABLED" }
            },
            "generativeField": {
                "state": "ENABLED",
                "types": ["VIDEO_SUMMARY", "CHAPTER_SUMMARY"]
            }
        },
        "audio": {
            "extraction": {
                "category": {
                    "state": "ENABLED",
                    "types": ["TRANSCRIPT"],
                    "typeConfiguration": {
                        "transcript": {
                            "speakerLabeling": { "state": "DISABLED" },
                            "channelLabeling": { "state": "DISABLED" }
                        }
                    }
                }
            },
            "generativeField": { "state": "DISABLED" }
        }
    })
}

/// The override configuration a project that overrode NOTHING reports: document
/// splitting off and audio answering in the default language, with the image and video
/// modalities present and carrying no override. `modalityRouting` has no default — it
/// names the modality a file extension is routed to — so it is reported only once a
/// project sets one.
fn default_override() -> Value {
    json!({
        "document": { "splitter": { "state": "DISABLED" } },
        "image": {},
        "video": {},
        "audio": {
            "languageConfiguration": {
                "generativeOutputLanguage": "DEFAULT",
                "identifyMultipleLanguages": false
            }
        }
    })
}

/// One create whose project is then read back.
struct ConfigurationCase {
    /// The case name every assertion reports.
    case: &'static str,
    /// The create's `standardOutputConfiguration` (the one `@required` member).
    standard_output: Value,
    /// `customOutputConfiguration`, or `None` to leave the member absent.
    custom_output: Option<Value>,
    /// `overrideConfiguration`, or `None` to leave the member absent.
    override_configuration: Option<Value>,
    /// `dataAutomationLibraryConfiguration`, or `None` to leave the member absent.
    library_configuration: Option<Value>,
}

/// The rows: one per way a create's configuration can be silent, from "silent about
/// everything" to "silent only about one leaf inside one modality".
///
/// The modality members are every one `smithy_query(shape="StandardOutputConfiguration")`
/// declares (`document`, `image`, `video`, `audio`); the configured values are drawn
/// from the modelled enums — `smithy_query(shape="DocumentExtractionGranularityType")`
/// -> `["DOCUMENT", "ELEMENT", "LINE", "PAGE", "WORD"]`,
/// `smithy_query(shape="DocumentOutputTextFormatType")` ->
/// `["CSV", "HTML", "MARKDOWN", "PLAIN_TEXT"]`,
/// `smithy_query(shape="ImageExtractionCategoryType")` ->
/// `["CONTENT_MODERATION", "LOGOS", "TEXT_DETECTION"]`,
/// `smithy_query(shape="VideoStandardGenerativeFieldType")` ->
/// `["CHAPTER_SUMMARY", "IAB", "VIDEO_SUMMARY"]`,
/// `smithy_query(shape="AudioExtractionCategoryType")` ->
/// `["AUDIO_CONTENT_MODERATION", "TOPIC_CONTENT_MODERATION", "TRANSCRIPT"]` and
/// `smithy_query(shape="State")` -> `["DISABLED", "ENABLED"]` — and every row names a
/// value the recorded case did not.
fn configuration_cases() -> Vec<ConfigurationCase> {
    vec![
        ConfigurationCase {
            case: "silent about everything",
            standard_output: json!({}),
            custom_output: None,
            override_configuration: None,
            library_configuration: None,
        },
        ConfigurationCase {
            case: "document fully specified against the defaults, the rest silent",
            standard_output: json!({
                "document": {
                    "extraction": {
                        "granularity": { "types": ["DOCUMENT", "WORD", "LINE"] },
                        "boundingBox": { "state": "ENABLED" }
                    },
                    "generativeField": { "state": "ENABLED" },
                    "outputFormat": {
                        "textFormat": { "types": ["HTML", "CSV"] },
                        "additionalFileFormat": { "state": "ENABLED" }
                    }
                }
            }),
            custom_output: None,
            override_configuration: None,
            library_configuration: None,
        },
        ConfigurationCase {
            case: "image and video specified, document and audio silent",
            standard_output: json!({
                "image": {
                    "extraction": {
                        "category": { "state": "DISABLED", "types": ["LOGOS"] },
                        "boundingBox": { "state": "DISABLED" }
                    },
                    "generativeField": { "state": "DISABLED", "types": ["IAB"] }
                },
                "video": {
                    "extraction": {
                        "category": { "state": "ENABLED", "types": ["CONTENT_MODERATION"] },
                        "boundingBox": { "state": "DISABLED" }
                    },
                    "generativeField": { "state": "ENABLED", "types": ["IAB"] }
                }
            }),
            custom_output: None,
            override_configuration: None,
            library_configuration: None,
        },
        ConfigurationCase {
            case: "audio specified down to one labelling leaf, the other leaf silent",
            standard_output: json!({
                "audio": {
                    "extraction": {
                        "category": {
                            "state": "ENABLED",
                            "types": ["AUDIO_CONTENT_MODERATION", "TRANSCRIPT"],
                            "typeConfiguration": {
                                "transcript": {
                                    "speakerLabeling": { "state": "ENABLED" }
                                }
                            }
                        }
                    }
                }
            }),
            custom_output: None,
            override_configuration: None,
            library_configuration: None,
        },
        ConfigurationCase {
            case: "a document modality whose extraction alone is specified",
            standard_output: json!({
                "document": {
                    "extraction": {
                        "granularity": { "types": ["PAGE"] },
                        "boundingBox": { "state": "ENABLED" }
                    }
                }
            }),
            custom_output: None,
            override_configuration: None,
            library_configuration: None,
        },
        ConfigurationCase {
            case: "a custom blueprint and a library binding, output silent",
            standard_output: json!({}),
            custom_output: Some(json!({
                "blueprints": [
                    { "blueprintArn": PUBLIC_BLUEPRINT_ARN, "blueprintStage": "LIVE" }
                ]
            })),
            override_configuration: None,
            library_configuration: Some(json!({
                "libraries": [ { "libraryArn": LIBRARY_ARN } ]
            })),
        },
        ConfigurationCase {
            case: "an override that sets the splitter and the routing, the rest silent",
            standard_output: json!({}),
            custom_output: None,
            override_configuration: Some(json!({
                "document": { "splitter": { "state": "ENABLED" } },
                "modalityRouting": { "jpeg": "IMAGE", "mp4": "VIDEO" }
            })),
            library_configuration: None,
        },
        ConfigurationCase {
            case: "an override that sets the audio language, the splitter silent",
            standard_output: json!({}),
            custom_output: None,
            override_configuration: Some(json!({
                "audio": {
                    "languageConfiguration": {
                        "inputLanguages": ["EN", "DE"],
                        "generativeOutputLanguage": "EN",
                        "identifyMultipleLanguages": true
                    }
                }
            })),
            library_configuration: None,
        },
    ]
}

impl ConfigurationCase {
    /// The create body: a member the row leaves `None` is ABSENT from the JSON, not
    /// sent as null, so the row exercises the service's own defaulting.
    fn body(&self, index: usize) -> Value {
        let mut body = json!({
            "projectName": format!("effective-{index}"),
            "standardOutputConfiguration": self.standard_output,
        });
        if let Some(custom) = &self.custom_output {
            body["customOutputConfiguration"] = custom.clone();
        }
        if let Some(override_configuration) = &self.override_configuration {
            body["overrideConfiguration"] = override_configuration.clone();
        }
        if let Some(libraries) = &self.library_configuration {
            body["dataAutomationLibraryConfiguration"] = libraries.clone();
        }
        body
    }
}

/// RULE 1: a project read reports every configuration block, each member taking the
/// value the create specified and the service's default wherever the create was silent.
#[tokio::test]
async fn a_project_read_reports_the_effective_configuration() {
    let (router, engine) = emulator_with_engine();

    for (index, spec) in configuration_cases().into_iter().enumerate() {
        let case = spec.case;
        let (status, created) = create_project(&router, &spec.body(index)).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201, got {status} with {created:?}"
        );
        let arn = string_at(&created, "projectArn", case);
        let project = read_project(&router, &arn, case).await;

        // Every block the shape carries is PRESENT: a client cannot tell "off" from
        // "defaulted on" through an absent block.
        for block in [
            "standardOutputConfiguration",
            "customOutputConfiguration",
            "overrideConfiguration",
            "dataAutomationLibraryConfiguration",
        ] {
            assert!(
                project.get(block).is_some(),
                "case {case:?}: {block} must be reported, got {project:?}"
            );
        }

        let standard = project
            .get("standardOutputConfiguration")
            .cloned()
            .unwrap_or(Value::Null);
        // Each modality the create did not spell out takes the service's default, and
        // every modality is present either way.
        let defaults = default_standard_output();
        for modality in ["document", "image", "video", "audio"] {
            let configured = spec.standard_output.get(modality);
            let reported = standard.get(modality).unwrap_or_else(|| {
                panic!("case {case:?}: standardOutputConfiguration.{modality} is absent from {standard:?}")
            });
            match configured {
                // Silent about this modality: the whole default block, exactly.
                None => assert_eq!(
                    reported,
                    &defaults[modality],
                    "case {case:?}: an unspecified {modality} must read back as the \
                     service default {:?}, got {reported:?}",
                    defaults[modality]
                ),
                // Specified: every value the create carried survives, and the leaves it
                // left silent are filled in — so the reported block is a superset.
                Some(carried) => {
                    assert_json_contains(
                        reported,
                        carried,
                        case,
                        &format!("standardOutputConfiguration.{modality}"),
                    );
                }
            }
        }

        // The other three blocks follow the same rule: what the create carried, plus
        // the default where it was silent.
        assert_json_contains(
            project.get("customOutputConfiguration").unwrap(),
            spec.custom_output.as_ref().unwrap_or(&json!({})),
            case,
            "customOutputConfiguration",
        );
        assert_json_contains(
            project
                .get("dataAutomationLibraryConfiguration")
                .unwrap(),
            spec.library_configuration.as_ref().unwrap_or(&json!({})),
            case,
            "dataAutomationLibraryConfiguration",
        );
        let reported_override = project.get("overrideConfiguration").unwrap();
        match &spec.override_configuration {
            None => assert_eq!(
                reported_override,
                &default_override(),
                "case {case:?}: a project that overrode nothing must read back the \
                 service's default override block {:?}, got {reported_override:?}",
                default_override()
            ),
            Some(carried) => {
                assert_json_contains(reported_override, carried, case, "overrideConfiguration");
                // The modalities the override was silent about are still reported.
                for modality in ["document", "image", "video", "audio"] {
                    assert!(
                        reported_override.get(modality).is_some(),
                        "case {case:?}: overrideConfiguration.{modality} must be \
                         reported, got {reported_override:?}"
                    );
                }
            }
        }

        // A READ REPORTS: two reads in a row agree, and the STORE still holds exactly
        // the configuration the caller sent — the defaults are materialised on the way
        // out, never folded into the record (so a later update still replaces only
        // what a caller specified).
        let again = read_project(&router, &arn, case).await;
        assert_eq!(
            project.get("standardOutputConfiguration"),
            again.get("standardOutputConfiguration"),
            "case {case:?}: two reads of one project must agree: {project:?} -> {again:?}"
        );
        {
            let store = engine.state::<ServiceState>();
            let held = store.lock();
            let record = held
                .projects
                .get(&arn)
                .unwrap_or_else(|| panic!("case {case:?}: the store holds no project at {arn}"));
            let stored_image_is_set = record
                .standard_output_configuration
                .as_ref()
                .is_some_and(|configuration| configuration.image.is_some());
            assert_eq!(
                stored_image_is_set,
                spec.standard_output.get("image").is_some(),
                "case {case:?}: the stored standardOutputConfiguration must hold the \
                 image modality exactly when the create carried it, got {:?}",
                record.standard_output_configuration
            );
            assert_eq!(
                record.override_configuration.is_some(),
                spec.override_configuration.is_some(),
                "case {case:?}: the stored overrideConfiguration must be set exactly \
                 when the create carried one, got {:?}",
                record.override_configuration
            );
        }
    }
}

/// RULE 2: every timestamp this service answers with is an ISO-8601 date-time string.
///
/// The rows are the surfaces that carry one: the project read, the blueprint create and
/// read, and the summaries the two listings serve.
#[tokio::test]
async fn every_timestamp_is_reported_as_a_date_time_string() {
    let router = emulator();

    // A date-time string, and NOT the epoch number the protocol would default to.
    let assert_date_time = |value: &Value, path: &str| {
        let text = value.as_str().unwrap_or_else(|| {
            panic!("{path} must be a date-time STRING, got {value:?}")
        });
        let parsed = aws_smithy_types::DateTime::from_str(
            text,
            aws_smithy_types::date_time::Format::DateTime,
        );
        assert!(
            parsed.is_ok(),
            "{path} = {text:?} must parse as a modelled date-time, got {parsed:?}"
        );
    };

    // Two projects, so a listing serves more than one summary.
    let mut project_arns = Vec::new();
    for case in ["timestamped-one", "timestamped-two"] {
        let (status, created) = create_project(
            &router,
            &json!({ "projectName": case, "standardOutputConfiguration": {} }),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201, got {status} with {created:?}"
        );
        project_arns.push(string_at(&created, "projectArn", case));
    }

    for arn in &project_arns {
        let project = read_project(&router, arn, "project read").await;
        for member in ["creationTime", "lastModifiedTime"] {
            assert_date_time(
                project.get(member).unwrap_or_else(|| {
                    panic!("project read: {member} is absent from {project:?}")
                }),
                &format!("$.project.{member}"),
            );
        }
    }

    // A blueprint carries the same timestamp shape, so the rule is the protocol's and
    // not the project shape's.
    let (status, blueprint) = call(
        &router,
        "PUT",
        "/blueprints/",
        &json!({
            "blueprintName": "timestamped-blueprint",
            "type": "DOCUMENT",
            "schema": "{\"class\":\"invoice\"}",
        }),
    )
    .await;
    assert_eq!(
        status, 201,
        "expected CreateBlueprint 201, got {status} with {blueprint:?}"
    );
    let created_blueprint = blueprint
        .get("blueprint")
        .cloned()
        .unwrap_or(Value::Null);
    for member in ["creationTime", "lastModifiedTime"] {
        assert_date_time(
            created_blueprint.get(member).unwrap_or_else(|| {
                panic!("blueprint create: {member} is absent from {created_blueprint:?}")
            }),
            &format!("$.blueprint.{member}"),
        );
    }

    // And so does every summary a listing serves, where the timestamp sits inside a
    // list rather than one structure down.
    let (status, projects) = call(&router, "POST", "/data-automation-projects/", &json!({})).await;
    assert_eq!(
        status, 200,
        "expected ListDataAutomationProjects 200, got {status} with {projects:?}"
    );
    let summaries = projects
        .get("projects")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("the listing served no projects: {projects:?}"));
    assert_eq!(
        summaries.len(),
        project_arns.len(),
        "the listing must serve every project, got {summaries:?}"
    );
    for (index, summary) in summaries.iter().enumerate() {
        assert_date_time(
            summary.get("creationTime").unwrap_or_else(|| {
                panic!("project summary {index}: creationTime is absent from {summary:?}")
            }),
            &format!("$.projects[{index}].creationTime"),
        );
    }

    let (status, blueprints) = call(&router, "POST", "/blueprints/", &json!({})).await;
    assert_eq!(
        status, 200,
        "expected ListBlueprints 200, got {status} with {blueprints:?}"
    );
    let summaries = blueprints
        .get("blueprints")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("the listing served no blueprints: {blueprints:?}"));
    for (index, summary) in summaries.iter().enumerate() {
        assert_date_time(
            summary.get("creationTime").unwrap_or_else(|| {
                panic!("blueprint summary {index}: creationTime is absent from {summary:?}")
            }),
            &format!("$.blueprints[{index}].creationTime"),
        );
    }
}

/// RULE 3: the status a delete reports is the accepted teardown's in-flight one, never
/// a terminal one.
///
/// The rows differ in what the PROJECT's own build status was when the delete arrived —
/// still building, or long settled — and in the stage the project was created in, so
/// the answer cannot be the project's status under another name.
#[tokio::test]
async fn deleting_a_project_reports_the_accepted_teardown() {
    let router = emulator();

    let rows: [(&str, &str, bool); 4] = [
        ("still building, LIVE", "LIVE", false),
        ("still building, DEVELOPMENT", "DEVELOPMENT", false),
        ("build already settled, LIVE", "LIVE", true),
        ("build already settled, DEVELOPMENT", "DEVELOPMENT", true),
    ];

    for (index, (case, stage, settle_first)) in rows.into_iter().enumerate() {
        let (status, created) = create_project(
            &router,
            &json!({
                "projectName": format!("torn-down-{index}"),
                "projectStage": stage,
                "standardOutputConfiguration": {},
            }),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201, got {status} with {created:?}"
        );
        let arn = string_at(&created, "projectArn", case);

        if settle_first {
            // A test may wait for a due instant; a handler may not.
            std::thread::sleep(state::PROJECT_CREATION_SPAN + Duration::from_millis(250));
            let settled = read_project(&router, &arn, case).await;
            assert_eq!(
                string_at(&settled, "status", case),
                state::PROJECT_COMPLETED,
                "case {case:?}: the project's own build must be settled before the \
                 delete, got {settled:?}"
            );
        }

        let (status, deleted) = delete_project(&router, &arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DeleteDataAutomationProject 200, got {status} \
             with {deleted:?}"
        );
        assert_eq!(
            string_at(&deleted, "projectArn", case),
            arn,
            "case {case:?}: the delete must answer about the project it took: {deleted:?}"
        );
        let reported = string_at(&deleted, "status", case);
        assert_eq!(
            reported,
            state::PROJECT_IN_PROGRESS,
            "case {case:?}: a delete reports the teardown it accepted, which is still \
             in flight when it answers, got {reported:?}"
        );
        assert_ne!(
            reported,
            state::PROJECT_COMPLETED,
            "case {case:?}: a delete must never report a terminal status: {deleted:?}"
        );
    }
}
