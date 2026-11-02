// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `UpdateDataAutomationProject`.
//!
//! Every case drives the real `restJson1` wire path — `MultiServiceRouter::dispatch`
//! with a signed-looking request — so routing, HTTP binding, constraint validation,
//! the handler and response serialization are all exercised, exactly as an AWS SDK
//! `bedrock-data-automation` client would.
//!
//! The rule under test: `UpdateDataAutomationProject` is an `@idempotent` PUT over an
//! existing project's configuration (`standardOutputConfiguration` is `@required`), so
//! the project at the named ARN takes exactly the configuration the request carries —
//! a configuration member the request omits is CLEARED, and a read then reports the
//! service's default for it rather than what an earlier request had set — while its
//! identity (ARN, `projectName`, `projectType`, `creationTime`) and, when the request
//! names no stage, its `projectStage` are untouched; the call moves `lastModifiedTime`
//! and re-starts the project's build, so the project reports `IN_PROGRESS` until the
//! transition the clock now owes falls due. An ARN no project holds is the modelled 404.
//!
//! The suites are TABLE-driven: the rows differ along what that rule generalises over
//! (which configuration members the request carries, the shape of each one, whether
//! the stage is named, re-stated or moved, and the description's length), not over one
//! recorded request.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_bedrockdataautomation_amazonbedrockkeystonebuildtimeservice::state;
use service_com_amazonaws_bedrockdataautomation_amazonbedrockkeystonebuildtimeservice::state::ComAmazonawsBedrockdataautomationAmazonbedrockkeystonebuildtimeserviceState as ServiceState;
use service_com_amazonaws_bedrockdataautomation_amazonbedrockkeystonebuildtimeservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{assert_json_contains, encode_label, error_code, string_at, timestamp_at};

/// The region every case signs for.
const REGION: &str = "us-east-1";

/// A SigV4 `Authorization` header for this service's signing name (`bedrock`, per
/// `OPERATIONS.md`). `can_handle` resolves the route key from it, and the create reads
/// the region out of its credential scope when minting an ARN.
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

/// `UpdateDataAutomationProject`, per its modelled `@http`
/// `{method: PUT, uri: /data-automation-projects/{projectArn}/, code: 200}`.
async fn update_project(router: &MultiServiceRouter, arn: &str, body: &Value) -> (u16, Value) {
    let uri = format!("/data-automation-projects/{}/", encode_label(arn));
    call(router, "PUT", &uri, body).await
}

/// `GetDataAutomationProject`, per its modelled `@http`
/// `{method: POST, uri: /data-automation-projects/{projectArn}/}`.
async fn get_project(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    let uri = format!("/data-automation-projects/{}/", encode_label(arn));
    call(router, "POST", &uri, &json!({})).await
}

/// The `project` structure a successful `GetDataAutomationProject` carries.
async fn read_back(router: &MultiServiceRouter, arn: &str, case: &str) -> Value {
    let (status, payload) = get_project(router, arn).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetDataAutomationProject 200, got {status} with {payload:?}"
    );
    payload.get("project").cloned().unwrap_or_else(|| {
        panic!("case {case:?}: GetDataAutomationProject returned no project: {payload:?}")
    })
}

/// A blueprint ARN inside the modelled `BlueprintArn` pattern
/// (`^arn:aws(|-cn|-us-gov):bedrock:[a-zA-Z0-9-]*:(aws|[0-9]{12}):blueprint/(bedrock-data-automation-public-[a-zA-Z0-9-_]{1,30}|[a-zA-Z0-9-]{12,36})$`,
/// source: `smithy_query(shape="BlueprintItem")`), so a `customOutputConfiguration`
/// row is accepted by the modelled constraints.
const PUBLIC_BLUEPRINT_ARN: &str =
    "arn:aws:bedrock:us-east-1:000000000000:blueprint/bedrock-data-automation-public-invoice";

/// One update request, as the members that make the rows differ.
struct UpdateCase {
    /// The case name every assertion reports.
    case: &'static str,
    /// The stage the project is CREATED in, so a row can leave the stage alone,
    /// re-state it, or move the project to the other one.
    created_stage: &'static str,
    /// `projectStage` the update names, or `None` to leave the member absent.
    stage: Option<&'static str>,
    /// `projectDescription` the update names, or `None` to leave the member absent.
    description: Option<String>,
    /// `standardOutputConfiguration`, the one `@required` member.
    standard_output: Value,
    /// `customOutputConfiguration`, or `None` to leave the member absent.
    custom_output: Option<Value>,
    /// `overrideConfiguration`, or `None` to leave the member absent.
    override_configuration: Option<Value>,
    /// `encryptionConfiguration`, or `None` to leave the member absent.
    encryption: Option<Value>,
}

/// The update cases: the whole class a well-formed update spans, one row per way the
/// request can differ.
///
/// The catalogues the rows draw on come from the model, not from a recording:
/// `projectStage` is every value of `DataAutomationProjectStage`
/// (`smithy_query(shape="DataAutomationProjectStage")` -> `["DEVELOPMENT", "LIVE"]`)
/// plus the absent case; the `standardOutputConfiguration` rows walk every modality
/// member `smithy_query(shape="StandardOutputConfiguration")` declares (`document`,
/// `image`, `video`, `audio`) including the empty structure; the description rows walk
/// both published bounds of `@length(0..300)`; and the optional configuration members
/// are every one `smithy_query(operation="UpdateDataAutomationProject")` lists
/// (`customOutputConfiguration`, `overrideConfiguration`, `encryptionConfiguration`),
/// each present in some row and absent in another.
fn update_cases() -> Vec<UpdateCase> {
    vec![
        UpdateCase {
            case: "minimal: only the required standardOutputConfiguration",
            created_stage: "LIVE",
            stage: None,
            description: None,
            standard_output: json!({}),
            custom_output: None,
            override_configuration: None,
            encryption: None,
        },
        UpdateCase {
            case: "stage moved to DEVELOPMENT, document output, description at the 300 bound",
            created_stage: "LIVE",
            stage: Some("DEVELOPMENT"),
            description: Some("d".repeat(300)),
            standard_output: json!({
                "document": {
                    "extraction": {
                        "granularity": { "types": ["PAGE", "ELEMENT"] },
                        "boundingBox": { "state": "ENABLED" }
                    },
                    "outputFormat": {
                        "textFormat": { "types": ["MARKDOWN"] },
                        "additionalFileFormat": { "state": "DISABLED" }
                    }
                }
            }),
            custom_output: Some(json!({
                "blueprints": [
                    { "blueprintArn": PUBLIC_BLUEPRINT_ARN, "blueprintStage": "LIVE" }
                ]
            })),
            override_configuration: None,
            encryption: None,
        },
        UpdateCase {
            case: "stage re-stated LIVE, every modality, override and encryption carried",
            created_stage: "DEVELOPMENT",
            stage: Some("LIVE"),
            description: Some(String::new()),
            standard_output: json!({
                "document": {
                    "outputFormat": {
                        "textFormat": { "types": ["PLAIN_TEXT", "CSV"] },
                        "additionalFileFormat": { "state": "ENABLED" }
                    }
                },
                "image": {
                    "extraction": {
                        "category": { "state": "ENABLED", "types": ["LOGOS"] },
                        "boundingBox": { "state": "ENABLED" }
                    }
                },
                "video": {
                    "extraction": {
                        "category": { "state": "ENABLED", "types": ["TRANSCRIPT"] },
                        "boundingBox": { "state": "DISABLED" }
                    }
                },
                "audio": {
                    "extraction": {
                        "category": { "state": "ENABLED", "types": ["TRANSCRIPT"] }
                    }
                }
            }),
            custom_output: Some(json!({
                "document": {
                    "fallbackBlueprints": [
                        { "blueprintArn": PUBLIC_BLUEPRINT_ARN, "blueprintStage": "DEVELOPMENT" }
                    ]
                }
            })),
            override_configuration: Some(json!({
                "document": { "splitter": { "state": "ENABLED" } },
                "modalityRouting": { "jpeg": "IMAGE", "mp4": "VIDEO" }
            })),
            encryption: Some(json!({
                "kmsKeyId": "arn:aws:kms:us-east-1:000000000000:key/abcd-1234",
                "kmsEncryptionContext": { "tenant": "acme" }
            })),
            },
        UpdateCase {
            case: "stage left alone on a DEVELOPMENT project, audio output, override only",
            created_stage: "DEVELOPMENT",
            stage: None,
            description: Some("an audio-only project".to_string()),
            standard_output: json!({
                "audio": {
                    "extraction": {
                        "category": { "state": "ENABLED", "types": ["AUDIO_CONTENT_MODERATION"] }
                    },
                    "generativeField": { "state": "ENABLED", "types": ["AUDIO_SUMMARY"] }
                }
            }),
            custom_output: None,
            override_configuration: Some(json!({
                "audio": {
                    "modalityProcessing": { "state": "ENABLED" },
                    "languageConfiguration": {
                        "inputLanguages": ["EN", "DE"],
                        "generativeOutputLanguage": "EN",
                        "identifyMultipleLanguages": true
                    }
                }
            })),
            encryption: None,
        },
    ]
}

impl UpdateCase {
    /// The create this row updates: a project that already carries a description, a
    /// custom output configuration and tags, so a row that omits them shows whether
    /// the update clears a configuration member and leaves identity alone.
    fn create_body(&self, index: usize) -> Value {
        json!({
            "projectName": format!("updated-project-{index}"),
            "projectStage": self.created_stage,
            "projectDescription": "the configuration the create left behind",
            "standardOutputConfiguration": {
                "image": {
                    "extraction": {
                        "category": { "state": "DISABLED" },
                        "boundingBox": { "state": "DISABLED" }
                    }
                }
            },
            "customOutputConfiguration": {
                "blueprints": [ { "blueprintArn": PUBLIC_BLUEPRINT_ARN } ]
            },
            "tags": [ { "key": "k0", "value": "v0" } ],
        })
    }

    /// The update request body: a member the row leaves `None` is ABSENT from the
    /// JSON, not sent as null.
    fn update_body(&self) -> Value {
        let mut body = json!({ "standardOutputConfiguration": self.standard_output });
        if let Some(stage) = self.stage {
            body["projectStage"] = json!(stage);
        }
        if let Some(description) = &self.description {
            body["projectDescription"] = json!(description);
        }
        if let Some(custom) = &self.custom_output {
            body["customOutputConfiguration"] = custom.clone();
        }
        if let Some(override_configuration) = &self.override_configuration {
            body["overrideConfiguration"] = override_configuration.clone();
        }
        if let Some(encryption) = &self.encryption {
            body["encryptionConfiguration"] = encryption.clone();
        }
        body
    }

    /// The stage the project is in after the update: the one the request named, or the
    /// one it was created in when the request named none.
    fn expected_stage(&self) -> &str {
        self.stage.unwrap_or(self.created_stage)
    }
}

#[tokio::test]
async fn an_update_replaces_the_project_configuration_and_persists_it() {
    let (router, engine) = emulator_with_engine();

    for (index, spec) in update_cases().into_iter().enumerate() {
        let case = spec.case;
        let create_body = spec.create_body(index);
        let (status, created) = create_project(&router, &create_body).await;
        assert_eq!(
            status, 201,
            "case {case:?}: the project to update must be created, got {status} with {created:?}"
        );
        let arn = string_at(&created, "projectArn", case);
        let before = read_back(&router, &arn, case).await;
        let created_at = timestamp_at(&before, "creationTime", case);

        let (status, updated) = update_project(&router, &arn, &spec.update_body()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the modeled UpdateDataAutomationProject 200, \
             got {status} with {updated:?}"
        );
        assert_eq!(
            string_at(&updated, "projectArn", case),
            arn,
            "case {case:?}: the update must answer about the project it took: {updated:?}"
        );
        assert_eq!(
            string_at(&updated, "projectStage", case),
            spec.expected_stage(),
            "case {case:?}: the update returned the wrong projectStage: {updated:?}"
        );
        // The update re-specifies what the project produces, so it re-starts the
        // build: the status it answers is the transient one.
        assert_eq!(
            string_at(&updated, "status", case),
            state::PROJECT_IN_PROGRESS,
            "case {case:?}: an updated project is {} until the clock settles it: {updated:?}",
            state::PROJECT_IN_PROGRESS
        );

        // ROUND TRIP: the read answers with the configuration the update stored, plus
        // the service's default wherever the update was silent (which is why these are
        // containments and not equalities — see `project_effective_configuration`).
        // A configuration member the update CLEARED therefore reads back as the
        // service's default for it, never as an absent block.
        let after = read_back(&router, &arn, case).await;
        assert_json_contains(
            after
                .get("standardOutputConfiguration")
                .unwrap_or_else(|| panic!(
                    "case {case:?}: no standardOutputConfiguration read back: {after:?}"
                )),
            &spec.standard_output,
            case,
            "standardOutputConfiguration",
        );
        assert_json_contains(
            after.get("customOutputConfiguration").unwrap_or_else(|| {
                panic!("case {case:?}: no customOutputConfiguration read back: {after:?}")
            }),
            spec.custom_output.as_ref().unwrap_or(&json!({})),
            case,
            "customOutputConfiguration",
        );
        if spec.custom_output.is_none() {
            assert_eq!(
                after.get("customOutputConfiguration"),
                Some(&json!({})),
                "case {case:?}: an update that carried no customOutputConfiguration \
                 clears the binding, which reads back as the empty one: {after:?}"
            );
        }
        assert_json_contains(
            after.get("overrideConfiguration").unwrap_or_else(|| {
                panic!("case {case:?}: no overrideConfiguration read back: {after:?}")
            }),
            spec.override_configuration.as_ref().unwrap_or(&json!({})),
            case,
            "overrideConfiguration",
        );
        assert_eq!(
            after.get("projectDescription").and_then(Value::as_str),
            spec.description.as_deref(),
            "case {case:?}: projectDescription must be exactly what the update carried \
             (cleared when it carried none): {after:?}"
        );
        assert_eq!(
            after
                .get("kmsKeyId")
                .and_then(Value::as_str)
                .map(str::to_string),
            spec.encryption
                .as_ref()
                .map(|config| string_at(config, "kmsKeyId", case)),
            "case {case:?}: kmsKeyId must follow the update's encryptionConfiguration: \
             {after:?}"
        );
        assert_eq!(
            string_at(&after, "projectStage", case),
            spec.expected_stage(),
            "case {case:?}: the wrong projectStage read back: {after:?}"
        );

        // IDENTITY: an update moves no part of the project's identity.
        for member in ["projectArn", "projectName", "projectType"] {
            assert_eq!(
                after.get(member),
                before.get(member),
                "case {case:?}: the update moved {member}, which it must not: \
                 {before:?} -> {after:?}"
            );
        }
        assert_eq!(
            timestamp_at(&after, "creationTime", case),
            created_at,
            "case {case:?}: the update moved creationTime, which it must not: {after:?}"
        );
        assert!(
            timestamp_at(&after, "lastModifiedTime", case) >= created_at,
            "case {case:?}: lastModifiedTime must not precede creationTime, got {:?} \
             against {created_at:?}",
            timestamp_at(&after, "lastModifiedTime", case)
        );

        // PERSISTED: the store itself holds the update, with a lastModifiedTime that
        // has strictly moved past the creation instant.
        {
            let store = engine.state::<ServiceState>();
            let held = store.lock();
            let record = held
                .projects
                .get(&arn)
                .unwrap_or_else(|| panic!("case {case:?}: the store holds no project at {arn}"));
            assert_eq!(
                record.project_name,
                format!("updated-project-{index}"),
                "case {case:?}: the stored project carries the wrong projectName"
            );
            assert_eq!(
                record.tags.len(),
                1,
                "case {case:?}: the update is not a member of the tag list, so the tag \
                 the create set must survive it"
            );
            let modified = (
                record.last_modified_time.secs(),
                record.last_modified_time.subsec_nanos(),
            );
            let born = (
                record.creation_time.secs(),
                record.creation_time.subsec_nanos(),
            );
            assert!(
                modified > born,
                "case {case:?}: the update must move lastModifiedTime past creationTime, \
                 got {modified:?} against {born:?}"
            );
        }
    }
}

#[tokio::test]
async fn repeating_one_update_leaves_the_same_project() {
    let router = emulator();

    // `@idempotent`: the rows are updates that differ in which members they carry, and
    // each is applied twice. What the project HOLDS afterwards must not depend on how
    // many times the same request arrived — only the build the second call re-started
    // (`status`) and the instant it happened (`lastModifiedTime`) may differ.
    for (index, spec) in update_cases().into_iter().enumerate() {
        let case = spec.case;
        let (status, created) = create_project(&router, &spec.create_body(100 + index)).await;
        assert_eq!(
            status, 201,
            "case {case:?}: the project to update must be created, got {status} with {created:?}"
        );
        let arn = string_at(&created, "projectArn", case);

        let body = spec.update_body();
        let (status, first) = update_project(&router, &arn, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the first update must succeed, got {status} with {first:?}"
        );
        let after_first = read_back(&router, &arn, case).await;

        let (status, second) = update_project(&router, &arn, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: repeating the update must succeed too, got {status} with {second:?}"
        );
        assert_eq!(
            first, second,
            "case {case:?}: the same update must answer the same way twice"
        );
        let after_second = read_back(&router, &arn, case).await;

        let strip = |project: &Value| {
            let mut copy = project.clone();
            if let Some(object) = copy.as_object_mut() {
                object.remove("lastModifiedTime");
                object.remove("status");
            }
            copy
        };
        assert_eq!(
            strip(&after_first),
            strip(&after_second),
            "case {case:?}: repeating the update changed the project: \
             {after_first:?} -> {after_second:?}"
        );
    }
}

#[tokio::test]
async fn an_update_restarts_the_build_the_clock_settles() {
    let router = emulator();
    let case = "re-started build";
    let body = json!({
        "projectName": "rebuilt-project",
        "standardOutputConfiguration": {},
    });
    let (status, created) = create_project(&router, &body).await;
    assert_eq!(status, 201, "expected 201, got {status} with {created:?}");
    let arn = string_at(&created, "projectArn", case);

    // A test may wait for a due instant; a handler may not.
    std::thread::sleep(state::PROJECT_CREATION_SPAN + Duration::from_millis(250));
    let settled = read_back(&router, &arn, case).await;
    assert_eq!(
        string_at(&settled, "status", case),
        state::PROJECT_COMPLETED,
        "the create's completion must be due by now: {settled:?}"
    );

    // The update re-specifies the project's output, so the project is building again —
    // over a non-zero span, so a caller reading straight after cannot have missed it.
    let (status, updated) = update_project(
        &router,
        &arn,
        &json!({
            "standardOutputConfiguration": {
                "document": {
                    "outputFormat": {
                        "textFormat": { "types": ["HTML"] },
                        "additionalFileFormat": { "state": "ENABLED" }
                    }
                }
            }
        }),
    )
    .await;
    assert_eq!(status, 200, "expected 200, got {status} with {updated:?}");
    assert_eq!(
        string_at(&updated, "status", case),
        state::PROJECT_IN_PROGRESS,
        "an update re-starts the project's build: {updated:?}"
    );
    let building = read_back(&router, &arn, case).await;
    assert_eq!(
        string_at(&building, "status", case),
        state::PROJECT_IN_PROGRESS,
        "a project read inside the update's span is still IN_PROGRESS: {building:?}"
    );

    std::thread::sleep(state::PROJECT_CREATION_SPAN + Duration::from_millis(250));

    // Nobody called in between, and the transition came due anyway. Two reads in a row
    // agree, because reporting the status writes nothing.
    for attempt in 1..=2 {
        let project = read_back(&router, &arn, case).await;
        assert_eq!(
            string_at(&project, "status", case),
            state::PROJECT_COMPLETED,
            "read #{attempt}: once the clock makes the completion due the project is \
             COMPLETED: {project:?}"
        );
    }
}

#[tokio::test]
async fn updating_an_unknown_project_is_the_modeled_404() {
    let (router, engine) = emulator_with_engine();

    // Well-formed ARNs no create minted: the rows differ in region and resource id, so
    // the rule is about the record and not about the shape of the ARN.
    let absent = [
        "arn:aws:bedrock:us-east-1:000000000000:data-automation-project/000000000000",
        "arn:aws:bedrock:eu-west-1:000000000000:data-automation-project/deadbeefdeadbeef",
        "arn:aws-us-gov:bedrock:us-gov-west-1:000000000000:data-automation-project/abc-123456789",
    ];

    for arn in absent {
        let (status, payload) = update_project(
            &router,
            arn,
            &json!({ "standardOutputConfiguration": {} }),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload).as_str()),
            (404, "ResourceNotFoundException"),
            "case {arn:?}: UpdateDataAutomationProject on an absent project is the \
             modeled 404, got {status} with {payload:?}"
        );
        // A refused update creates nothing: the store holds no project at that ARN.
        let store = engine.state::<ServiceState>();
        let held = store.lock();
        assert!(
            !held.projects.contains_key(arn),
            "case {arn:?}: a refused update must leave the store untouched"
        );
    }
}
