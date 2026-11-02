// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the DataAutomationProject lifecycle.
//!
//! Every case drives the real `restJson1` wire path — `MultiServiceRouter::dispatch`
//! with a signed-looking request — so routing, HTTP binding, constraint validation,
//! the handler and response serialization are all exercised, exactly as an AWS SDK
//! `bedrock-data-automation` client would.
//!
//! The rule under test is the one the operation's model states: a well-formed
//! `CreateDataAutomationProject` answers its modelled `@http` success code (201) with
//! `projectArn`, `projectStage` and `status`, and the project it answered about is
//! PERSISTED — never a 5xx, which this operation models only as
//! `InternalServerException`. The suites are TABLE-driven: each asserts that rule over
//! a set of inputs that differ along what it generalises over (project name form and
//! length, stage named or absent, project type, description, the shape of the standard
//! output configuration, tag count, an idempotency token present or absent), not over
//! one recorded request.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_bedrockdataautomation_amazonbedrockkeystonebuildtimeservice::state;
use service_com_amazonaws_bedrockdataautomation_amazonbedrockkeystonebuildtimeservice::state::ComAmazonawsBedrockdataautomationAmazonbedrockkeystonebuildtimeserviceState as ServiceState;
use service_com_amazonaws_bedrockdataautomation_amazonbedrockkeystonebuildtimeservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{assert_json_contains, encode_label, error_code, string_at, timestamp_at};

/// A SigV4 `Authorization` header for this service's signing name (`bedrock`, per
/// `OPERATIONS.md`). `can_handle` resolves the route key from it, and the handlers read
/// the region out of its credential scope when minting an ARN.
fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTKEY/20240101/{region}/bedrock/aws4_request, \
         SignedHeaders=host, Signature=0123456789abcdef"
    )
}

/// The emulator under test: one shared engine, one mounted service, request
/// authentication off so the suite signs nothing for real.
fn emulator() -> MultiServiceRouter {
    emulator_with_engine().0
}

/// The same emulator, with a handle on the shared engine, so a case can assert what the
/// store HOLDS and not only what it answers.
fn emulator_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// Dispatch one request in `region` and return its status and decoded JSON body
/// (`Value::Null` for an empty body).
async fn call_in_region(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: &Value,
    region: &str,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "bedrock-data-automation.amazonaws.com")
        .header("content-type", "application/json")
        .header("authorization", authorization(region))
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

async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: &Value) -> (u16, Value) {
    call_in_region(router, method, uri, body, REGION).await
}

/// The region every case but the ARN-region one signs for.
const REGION: &str = "us-east-1";

/// `CreateDataAutomationProject`, per its modelled `@http`
/// `{method: PUT, uri: /data-automation-projects/, code: 201}`.
async fn create_project(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "PUT", "/data-automation-projects/", body).await
}

/// `GetDataAutomationProject`, per its modelled `@http`
/// `{method: POST, uri: /data-automation-projects/{projectArn}/}`.
async fn get_project(router: &MultiServiceRouter, arn: &str, body: &Value) -> (u16, Value) {
    let uri = format!("/data-automation-projects/{}/", encode_label(arn));
    call(router, "POST", &uri, body).await
}

/// `DeleteDataAutomationProject`, per its modelled `@http`
/// `{method: DELETE, uri: /data-automation-projects/{projectArn}/}`.
async fn delete_project(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    let uri = format!("/data-automation-projects/{}/", encode_label(arn));
    call(router, "DELETE", &uri, &json!({})).await
}

/// Structural check of a minted project ARN against the modeled
/// `DataAutomationProjectArn` pattern
/// `^arn:aws(|-cn|-us-gov):bedrock:[a-zA-Z0-9-]*:(aws|[0-9]{12}):data-automation-project/[a-zA-Z0-9-]{12,36}$`
/// (source: `smithy_query(shape="DataAutomationProject")`), plus its 128-character
/// bound.
fn assert_well_formed_arn(arn: &str, expected_region: &str, case: &str) {
    assert!(
        arn.len() <= 128,
        "case {case:?}: projectArn is {} characters, over the modeled 128: {arn}",
        arn.len()
    );
    let fields: Vec<&str> = arn.splitn(6, ':').collect();
    assert_eq!(
        fields.len(),
        6,
        "case {case:?}: projectArn has {} colon-separated fields, expected 6: {arn}",
        fields.len()
    );
    assert_eq!(fields[0], "arn", "case {case:?}: ARN prefix, got {arn}");
    assert_eq!(fields[1], "aws", "case {case:?}: ARN partition, got {arn}");
    assert_eq!(fields[2], "bedrock", "case {case:?}: ARN service, got {arn}");
    assert_eq!(
        fields[3], expected_region,
        "case {case:?}: ARN region, got {arn}"
    );
    assert!(
        fields[4].len() == 12 && fields[4].bytes().all(|b| b.is_ascii_digit()),
        "case {case:?}: ARN account must be 12 digits, got {arn}"
    );
    let id = fields[5]
        .strip_prefix("data-automation-project/")
        .unwrap_or_else(|| {
            panic!("case {case:?}: ARN resource must be data-automation-project/<id>, got {arn}")
        });
    assert!(
        (12..=36).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'),
        "case {case:?}: ARN resource id must be 12..36 of [a-zA-Z0-9-], got {id:?}"
    );
}

/// One create request, as the members that make the rows differ.
struct CreateCase {
    /// The case name every assertion reports.
    case: &'static str,
    /// `projectName` (modelled `@length(1..128)`, `@pattern ^[a-zA-Z0-9-_]+$`).
    name: String,
    /// `projectStage`, or `None` to leave the member absent.
    stage: Option<&'static str>,
    /// `projectType`, or `None` to leave the member absent (modelled
    /// `@default("ASYNC")`).
    project_type: Option<&'static str>,
    /// `projectDescription`, or `None` to leave the member absent.
    description: Option<String>,
    /// `standardOutputConfiguration`, the one required structure.
    standard_output: Value,
    /// How many tags the request carries; `None` leaves `tags` absent.
    tags: Option<usize>,
}

/// The create cases: the whole class a well-formed create spans, one row per way the
/// request can differ.
///
/// The catalogues the rows draw on come from the model, not from a recording:
/// `projectStage` is every value of `DataAutomationProjectStage`
/// (`smithy_query(shape="DataAutomationProjectStage")` -> `["DEVELOPMENT", "LIVE"]`)
/// plus the absent case, `projectType` every value of `DataAutomationProjectType`
/// (`["ASYNC", "SYNC"]`) plus the absent case that takes its modelled `@default`, and
/// the `standardOutputConfiguration` rows walk every modality member
/// `smithy_query(shape="StandardOutputConfiguration")` declares (`document`, `image`,
/// `video`, `audio`) including the empty structure. The name rows walk both published
/// bounds of `@length(1..128)` and the whole `^[a-zA-Z0-9-_]+$` character class.
fn create_cases() -> Vec<CreateCase> {
    vec![
        CreateCase {
            case: "minimal: only the two required members",
            name: "p".to_string(),
            stage: None,
            project_type: None,
            description: None,
            standard_output: json!({}),
            tags: None,
        },
        CreateCase {
            case: "development stage, SYNC, document output",
            name: "project-with-hyphens-1".to_string(),
            stage: Some("DEVELOPMENT"),
            project_type: Some("SYNC"),
            description: Some("a document extraction project".to_string()),
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
            tags: Some(1),
        },
        CreateCase {
            case: "live stage, ASYNC, image and video output",
            name: "project_with_underscores_2".to_string(),
            stage: Some("LIVE"),
            project_type: Some("ASYNC"),
            description: Some(String::new()),
            standard_output: json!({
                "image": {
                    "extraction": {
                        "category": { "state": "ENABLED", "types": ["TEXT_DETECTION"] },
                        "boundingBox": { "state": "ENABLED" }
                    }
                },
                "video": {
                    "extraction": {
                        "category": { "state": "DISABLED" },
                        "boundingBox": { "state": "DISABLED" }
                    }
                }
            }),
            tags: Some(0),
        },
        CreateCase {
            case: "name at the 128-character upper bound, audio output, many tags",
            name: "a".repeat(128),
            stage: None,
            project_type: Some("ASYNC"),
            // `@length(0..300)` on `projectDescription`: its upper bound.
            description: Some("d".repeat(300)),
            standard_output: json!({
                "audio": {
                    "extraction": {
                        "category": { "state": "ENABLED", "types": ["TRANSCRIPT"] }
                    }
                }
            }),
            tags: Some(5),
        },
        CreateCase {
            case: "digits-only name, every modality configured",
            name: "20261022".to_string(),
            stage: Some("DEVELOPMENT"),
            project_type: Some("SYNC"),
            description: None,
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
                        "boundingBox": { "state": "ENABLED" }
                    }
                },
                "audio": {
                    "extraction": {
                        "category": { "state": "ENABLED", "types": ["AUDIO_CONTENT_MODERATION"] }
                    }
                }
            }),
            tags: Some(2),
        },
    ]
}

impl CreateCase {
    /// The request body this case sends: a member the case leaves `None` is ABSENT from
    /// the JSON, not sent as null, so the row exercises the handler's own defaulting.
    fn body(&self) -> Value {
        let mut body = json!({
            "projectName": self.name,
            "standardOutputConfiguration": self.standard_output,
        });
        if let Some(stage) = self.stage {
            body["projectStage"] = json!(stage);
        }
        if let Some(project_type) = self.project_type {
            body["projectType"] = json!(project_type);
        }
        if let Some(description) = &self.description {
            body["projectDescription"] = json!(description);
        }
        if let Some(count) = self.tags {
            body["tags"] = Value::Array(
                (0..count)
                    .map(|index| json!({ "key": format!("k{index}"), "value": format!("v{index}") }))
                    .collect(),
            );
        }
        body
    }

    /// The `projectStage` the response must carry: the one the request named, or the
    /// stage a project with no stage named is created in.
    fn expected_stage(&self) -> &str {
        self.stage.unwrap_or(state::DEFAULT_PROJECT_STAGE)
    }

    /// The `projectType` the stored project must carry: the one the request named, or
    /// the member's modelled `@default("ASYNC")`.
    fn expected_type(&self) -> &str {
        self.project_type.unwrap_or("ASYNC")
    }
}

#[tokio::test]
async fn a_well_formed_create_answers_the_modeled_201_and_persists_the_project() {
    let (router, engine) = emulator_with_engine();

    for spec in create_cases() {
        let case = spec.case;
        let (status, created) = create_project(&router, &spec.body()).await;

        // The rule this suite exists for: the modelled `@http` code, never a 5xx.
        assert_eq!(
            status, 201,
            "case {case:?}: expected the modeled CreateDataAutomationProject 201, \
             got {status} with {created:?}"
        );
        let arn = string_at(&created, "projectArn", case);
        assert_well_formed_arn(&arn, REGION, case);
        assert_eq!(
            string_at(&created, "projectStage", case),
            spec.expected_stage(),
            "case {case:?}: CreateDataAutomationProject returned the wrong projectStage: \
             {created:?}"
        );
        // The create starts the project's build, so the status it answers is the
        // transient one: the transition to COMPLETED is owed at a non-zero span, so no
        // caller can miss IN_PROGRESS.
        assert_eq!(
            string_at(&created, "status", case),
            state::PROJECT_IN_PROGRESS,
            "case {case:?}: a just-created project is {} until the clock settles it: \
             {created:?}",
            state::PROJECT_IN_PROGRESS
        );

        // PERSISTED: the store holds the record the response named, under the name and
        // the values the request carried.
        {
            let store = engine.state::<ServiceState>();
            let held = store.lock();
            let record = held
                .projects
                .get(&arn)
                .unwrap_or_else(|| panic!("case {case:?}: the store holds no project at {arn}"));
            assert_eq!(
                record.project_name, spec.name,
                "case {case:?}: the stored project carries the wrong projectName"
            );
            assert_eq!(
                held.project_names.get(&spec.name).map(String::as_str),
                Some(arn.as_str()),
                "case {case:?}: the name index must resolve {:?} to {arn}",
                spec.name
            );
            assert_eq!(
                record.tags.len(),
                spec.tags.unwrap_or(0),
                "case {case:?}: the stored project carries the wrong tag count"
            );
        }

        // ROUND TRIP: the read answers with the record that was stored.
        let (status, payload) = get_project(&router, &arn, &json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetDataAutomationProject 200, got {status} with {payload:?}"
        );
        let fetched = payload
            .get("project")
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: GetDataAutomationProject returned no project: {payload:?}"));
        assert_eq!(
            string_at(&fetched, "projectArn", case),
            arn,
            "case {case:?}: GetDataAutomationProject answered about another project: {fetched:?}"
        );
        assert_eq!(
            string_at(&fetched, "projectName", case),
            spec.name,
            "case {case:?}: the wrong projectName read back: {fetched:?}"
        );
        assert_eq!(
            string_at(&fetched, "projectStage", case),
            spec.expected_stage(),
            "case {case:?}: the wrong projectStage read back: {fetched:?}"
        );
        assert_eq!(
            string_at(&fetched, "projectType", case),
            spec.expected_type(),
            "case {case:?}: the wrong projectType read back: {fetched:?}"
        );
        assert_eq!(
            fetched.get("projectDescription").and_then(Value::as_str),
            spec.description.as_deref(),
            "case {case:?}: the wrong projectDescription read back: {fetched:?}"
        );
        // The read reports the EFFECTIVE configuration: every value the create
        // carried, plus the service's default wherever the create was silent (which is
        // why this is a containment and not an equality — see
        // `project_effective_configuration` for the rule and the exact defaults).
        let read_standard_output = fetched
            .get("standardOutputConfiguration")
            .unwrap_or_else(|| {
                panic!("case {case:?}: no standardOutputConfiguration read back: {fetched:?}")
            });
        assert_json_contains(
            read_standard_output,
            &spec.standard_output,
            case,
            "standardOutputConfiguration",
        );
        // Every modality is reported whether or not the create named it: an absent one
        // would leave a client unable to tell "off" from "defaulted on".
        for modality in ["document", "image", "video", "audio"] {
            assert!(
                read_standard_output.get(modality).is_some(),
                "case {case:?}: standardOutputConfiguration.{modality} must be \
                 reported, got {read_standard_output:?}"
            );
        }
        let created_at = timestamp_at(&fetched, "creationTime", case);
        assert_eq!(
            timestamp_at(&fetched, "lastModifiedTime", case),
            created_at,
            "case {case:?}: a project nothing has modified must report \
             lastModifiedTime == creationTime: {fetched:?}"
        );
        let read_status = string_at(&fetched, "status", case);
        assert!(
            [state::PROJECT_IN_PROGRESS, state::PROJECT_COMPLETED].contains(&read_status.as_str()),
            "case {case:?}: a created project reads back IN_PROGRESS or COMPLETED, \
             got {read_status:?}"
        );
    }
}

#[tokio::test]
async fn every_create_mints_its_own_project_arn() {
    let router = emulator();
    let mut seen: Vec<String> = Vec::new();

    for spec in create_cases() {
        let case = spec.case;
        let (status, created) = create_project(&router, &spec.body()).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201, got {status} with {created:?}"
        );
        let arn = string_at(&created, "projectArn", case);
        assert!(
            !seen.contains(&arn),
            "case {case:?}: projectArn {arn} was already minted for another project: {seen:?}"
        );
        seen.push(arn);
    }
}

#[tokio::test]
async fn the_arn_region_is_the_one_the_request_was_signed_for() {
    let router = emulator();

    // Every row is the same create in a different credential scope, so the only thing
    // that may differ in the answer is the ARN's region field.
    for region in ["us-east-1", "eu-west-1", "ap-southeast-2", "us-gov-west-1"] {
        let body = json!({
            "projectName": format!("region-{}", region.replace('-', "_")),
            "standardOutputConfiguration": {},
        });
        let (status, created) =
            call_in_region(&router, "PUT", "/data-automation-projects/", &body, region).await;
        assert_eq!(
            status, 201,
            "case {region:?}: expected 201, got {status} with {created:?}"
        );
        assert_well_formed_arn(&string_at(&created, "projectArn", region), region, region);
    }
}

#[tokio::test]
async fn a_second_project_under_a_taken_name_is_the_modeled_conflict() {
    let router = emulator();

    // The rule is about the NAME, so the rows differ in everything else: a repeat of
    // the same request, and a repeat that changes stage, type and output configuration.
    let first = json!({
        "projectName": "contended-name",
        "standardOutputConfiguration": {},
    });
    let (status, created) = create_project(&router, &first).await;
    assert_eq!(
        status, 201,
        "the first create must succeed, got {status} with {created:?}"
    );

    let rows: [(&str, Value); 2] = [
        ("the identical request again", first.clone()),
        (
            "the same name with different members",
            json!({
                "projectName": "contended-name",
                "projectStage": "DEVELOPMENT",
                "projectType": "SYNC",
                "projectDescription": "another project, same name",
                "standardOutputConfiguration": {
                    "audio": {
                        "extraction": {
                            "category": { "state": "ENABLED", "types": ["TRANSCRIPT"] }
                        }
                    }
                },
            }),
        ),
    ];

    for (case, body) in rows {
        let (status, payload) = create_project(&router, &body).await;
        assert_eq!(
            (status, error_code(&payload).as_str()),
            (409, "ConflictException"),
            "case {case:?}: expected the modeled 409 ConflictException, \
             got {status} with {payload:?}"
        );
    }
}

#[tokio::test]
async fn an_idempotency_token_replay_returns_the_first_project() {
    let (router, engine) = emulator_with_engine();

    // `clientToken` is modelled `@length(33..256)`, `@pattern
    // ^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,256}$`: the rows walk that bound (the shortest
    // token the model accepts and a longer one) and both halves of the pattern.
    let rows: [(&str, &str); 2] = [
        ("token at the 33-character lower bound", "a234567890123456789012345678901234"),
        (
            "a longer hyphenated token",
            "replay-token-0000000000000000000000000000000000000001",
        ),
    ];

    for (case, token) in rows {
        let body = json!({
            "projectName": format!("idempotent-{}", token.len()),
            "standardOutputConfiguration": {},
            "clientToken": token,
        });

        let (status, first) = create_project(&router, &body).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 on the first call, got {status} with {first:?}"
        );
        let (status, replay) = create_project(&router, &body).await;
        assert_eq!(
            status, 201,
            "case {case:?}: an @idempotencyToken replay answers 201 again, \
             got {status} with {replay:?}"
        );
        assert_eq!(
            string_at(&replay, "projectArn", case),
            string_at(&first, "projectArn", case),
            "case {case:?}: the replay must answer about the project the first call \
             created: {replay:?}"
        );

        let created_count = {
            let store = engine.state::<ServiceState>();
            let held = store.lock();
            held.projects.len()
        };
        assert_eq!(
            created_count, 1,
            "case {case:?}: a replayed token must not create a second project, \
             the store holds {created_count}"
        );

        // Release the record so the next row starts from an empty store.
        let (status, payload) =
            delete_project(&router, &string_at(&first, "projectArn", case)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DeleteDataAutomationProject 200, \
             got {status} with {payload:?}"
        );
    }
}

#[tokio::test]
async fn the_create_owes_a_completion_the_clock_makes_due() {
    let router = emulator();
    let body = json!({
        "projectName": "settling-project",
        "standardOutputConfiguration": {},
    });
    let (status, created) = create_project(&router, &body).await;
    assert_eq!(
        status, 201,
        "expected 201, got {status} with {created:?}"
    );
    let arn = string_at(&created, "projectArn", "settling");

    // Before the span is up the project is still building. The span is non-zero, so a
    // caller reading straight after the create cannot have missed it.
    let (_, payload) = get_project(&router, &arn, &json!({})).await;
    let project = payload.get("project").cloned().unwrap_or(Value::Null);
    assert_eq!(
        string_at(&project, "status", "before the completion is due"),
        state::PROJECT_IN_PROGRESS,
        "a project read inside the creation span is still IN_PROGRESS: {project:?}"
    );

    // A test may wait for a due instant; a handler may not. `tokio`'s timer is not in
    // this workspace's feature set, so the current-thread test runtime parks instead.
    std::thread::sleep(state::PROJECT_CREATION_SPAN + Duration::from_millis(250));

    // Nobody called in between, and the transition came due anyway. Two reads in a row
    // agree, because reporting the status writes nothing.
    for attempt in 1..=2 {
        let (status, payload) = get_project(&router, &arn, &json!({})).await;
        assert_eq!(
            status, 200,
            "read #{attempt} after the completion is due: expected 200, \
             got {status} with {payload:?}"
        );
        let project = payload.get("project").cloned().unwrap_or(Value::Null);
        assert_eq!(
            string_at(&project, "status", "after the completion is due"),
            state::PROJECT_COMPLETED,
            "read #{attempt}: once the clock makes the completion due the project is \
             COMPLETED: {project:?}"
        );
    }
}

#[tokio::test]
async fn a_project_is_read_only_through_the_stage_it_was_created_in() {
    let router = emulator();

    // Every `DataAutomationProjectStage` the model declares, each created and then read
    // back both with its own stage named and with the other one.
    for (case, created_stage, other_stage) in [
        ("development", "DEVELOPMENT", "LIVE"),
        ("live", "LIVE", "DEVELOPMENT"),
    ] {
        let body = json!({
            "projectName": format!("staged-{case}"),
            "projectStage": created_stage,
            "standardOutputConfiguration": {},
        });
        let (status, created) = create_project(&router, &body).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201, got {status} with {created:?}"
        );
        let arn = string_at(&created, "projectArn", case);

        let (status, payload) =
            get_project(&router, &arn, &json!({ "projectStage": created_stage })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the project's own stage must read it back, \
             got {status} with {payload:?}"
        );

        let (status, payload) =
            get_project(&router, &arn, &json!({ "projectStage": other_stage })).await;
        assert_eq!(
            (status, error_code(&payload).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: a stage the project is not in has no project to report, \
             got {status} with {payload:?}"
        );
    }
}

#[tokio::test]
async fn an_unknown_project_arn_is_the_modeled_404_on_every_reader() {
    let router = emulator();

    // Well-formed ARNs no create minted: one per account form the pattern allows and
    // one in another region, so the rule is about the record and not about the shape.
    let absent = [
        "arn:aws:bedrock:us-east-1:000000000000:data-automation-project/000000000000",
        "arn:aws:bedrock:eu-west-1:000000000000:data-automation-project/deadbeefdeadbeef",
    ];

    for arn in absent {
        let (status, payload) = get_project(&router, arn, &json!({})).await;
        assert_eq!(
            (status, error_code(&payload).as_str()),
            (404, "ResourceNotFoundException"),
            "case {arn:?}: GetDataAutomationProject on an absent project is the modeled \
             404, got {status} with {payload:?}"
        );
        let (status, payload) = delete_project(&router, arn).await;
        assert_eq!(
            (status, error_code(&payload).as_str()),
            (404, "ResourceNotFoundException"),
            "case {arn:?}: DeleteDataAutomationProject on an absent project is the modeled \
             404, got {status} with {payload:?}"
        );
    }
}

#[tokio::test]
async fn a_deleted_project_releases_its_name_and_is_gone_from_every_reader() {
    let (router, engine) = emulator_with_engine();

    // The rows differ in the stage the project was created in, so the release is not
    // keyed to one of them.
    for (case, stage) in [("development", "DEVELOPMENT"), ("live", "LIVE")] {
        let body = json!({
            "projectName": format!("recycled-{case}"),
            "projectStage": stage,
            "standardOutputConfiguration": {},
        });
        let (status, created) = create_project(&router, &body).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201, got {status} with {created:?}"
        );
        let arn = string_at(&created, "projectArn", case);

        let (status, deleted) = delete_project(&router, &arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DeleteDataAutomationProject 200, \
             got {status} with {deleted:?}"
        );
        assert_eq!(
            string_at(&deleted, "projectArn", case),
            arn,
            "case {case:?}: the delete must answer about the project it took: {deleted:?}"
        );

        let (status, payload) = get_project(&router, &arn, &json!({})).await;
        assert_eq!(
            (status, error_code(&payload).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: a deleted project is gone, got {status} with {payload:?}"
        );
        {
            let store = engine.state::<ServiceState>();
            let held = store.lock();
            assert!(
                !held.projects.contains_key(&arn),
                "case {case:?}: the store still holds {arn}"
            );
            assert!(
                !held.project_names.contains_key(&format!("recycled-{case}")),
                "case {case:?}: the deleted project still reserves its name"
            );
        }

        // The released name is free again, and the new project gets its own ARN.
        let (status, recreated) = create_project(&router, &body).await;
        assert_eq!(
            status, 201,
            "case {case:?}: the released name must be creatable again, \
             got {status} with {recreated:?}"
        );
        assert_ne!(
            string_at(&recreated, "projectArn", case),
            arn,
            "case {case:?}: a re-created project must not reuse the deleted ARN: {recreated:?}"
        );
    }
}
