// SPDX-License-Identifier: CC0-1.0

//! The Inspector-classic core loop, end to end over the wire: resource group ->
//! assessment target -> assessment template -> assessment run -> describe -> delete.
//!
//! Every call here goes through the real router: a `POST /` with
//! `X-Amz-Target: InspectorService.<Operation>` and an awsJson1_1 body, dispatched to this
//! service exactly as a signed SDK request is. The assertions are about the RULE each
//! operation follows, so the tables span the inputs the rule generalises over (region, tag
//! count, duration across both published bounds, rules-package count, user attributes,
//! caller-supplied vs derived run name) rather than one recorded request.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_inspector_inspectorservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The emulator's canonical account id, which every ARN this service mints carries.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// Rules package ARNs AWS publishes for Inspector classic in `us-east-1` (the
/// Common Vulnerabilities and Exposures, CIS Operating System Security Configuration
/// Benchmarks, Security Best Practices and Runtime Behavior Analysis packages). They are
/// opaque inputs to this service: a template keeps whatever it is given, and a run reports
/// what its template kept.
const RULES_PACKAGES: [&str; 4] = [
    "arn:aws:inspector:us-east-1:316112463485:rulespackage/0-gEjTy7T7",
    "arn:aws:inspector:us-east-1:316112463485:rulespackage/0-rExsr2X8",
    "arn:aws:inspector:us-east-1:316112463485:rulespackage/0-R01qwB5Q",
    "arn:aws:inspector:us-east-1:316112463485:rulespackage/0-gBONHN9h",
];

/// An ARN naming nothing this store holds, for the `failedItems` half of a batch describe.
const ABSENT_RUN_ARN: &str = "arn:aws:inspector:us-east-1:000000000000:target/0-AAAAAAAA/template/0-BBBBBBBB/run/0-CCCCCCCC";

/// One service, one engine, behind the production router. Returns the engine too, so a test
/// that has to bring a stored due-instant forward can reach the store the handlers write.
fn harness() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The signature itself is not under test here; the request still carries a SigV4
    // credential scope, because that is where this service reads the region of the ARNs it
    // mints from.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// An awsJson1_1 request for `operation`, signed for `region` the way an SDK client signs.
fn request(operation: &str, region: &str, body: &Value) -> http::Request<Bytes> {
    http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("InspectorService.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/inspector/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0123456789abcdef"
            ),
        )
        .body(Bytes::from(
            serde_json::to_vec(body).expect("serializable request body"),
        ))
        .expect("a valid request")
}

/// Call `operation` and return its HTTP status and parsed body (`Value::Null` when empty).
async fn call(
    router: &MultiServiceRouter,
    operation: &str,
    region: &str,
    body: Value,
) -> (u16, Value) {
    let response = router.dispatch(request(operation, region, &body)).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            panic!(
                "{operation} answered {status} with a body that is not JSON ({e}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, parsed)
}

/// The error code an awsJson error body carries, without any namespace prefix.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit(['#', '/'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// A string member of a JSON object, or the empty string.
fn string_at(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Whether `arn` is `arn:aws:inspector:<region>:<account>:<resource>/0-xxxxxxxx` with eight
/// id characters, the shape of every Inspector-classic ARN.
fn is_inspector_arn(arn: &str, region: &str, prefix: &str) -> bool {
    let expected = format!("arn:aws:inspector:{region}:{ACCOUNT}:{prefix}/0-");
    let Some(id) = arn.strip_prefix(&expected) else {
        return false;
    };
    id.len() == 8 && id.chars().all(|c| c.is_ascii_alphanumeric())
}

/// Whether `arn` is `<parent>/<kind>/0-xxxxxxxx`.
fn is_child_arn(arn: &str, parent: &str, kind: &str) -> bool {
    let expected = format!("{parent}/{kind}/0-");
    let Some(id) = arn.strip_prefix(&expected) else {
        return false;
    };
    id.len() == 8 && id.chars().all(|c| c.is_ascii_alphanumeric())
}

/// One row of the core-loop table: an input set the rules must hold over.
#[derive(Debug)]
struct Case {
    label: &'static str,
    region: &'static str,
    /// Resource group tags, as `(key, value)` pairs; a `None` value exercises the optional
    /// `ResourceGroupTag$value`.
    tags: &'static [(&'static str, Option<&'static str>)],
    target_name: &'static str,
    /// Whether the target names a resource group at all (`resourceGroupArn` is optional).
    attach_resource_group: bool,
    template_name: &'static str,
    /// `durationInSeconds`, over the modelled 180..86400 range.
    duration: u32,
    /// How many of [`RULES_PACKAGES`] the template attaches.
    rules_packages: usize,
    user_attributes: &'static [(&'static str, Option<&'static str>)],
    /// `None` exercises the name the service derives from the template.
    run_name: Option<&'static str>,
}

const CASES: &[Case] = &[
    Case {
        label: "one tag, minimum duration, single rules package, derived run name",
        region: "us-east-1",
        tags: &[("Name", Some("web"))],
        target_name: "demo-target",
        attach_resource_group: true,
        template_name: "demo-template",
        duration: 180,
        rules_packages: 1,
        user_attributes: &[],
        run_name: None,
    },
    Case {
        label: "three tags, mid duration, three rules packages, named run",
        region: "eu-west-1",
        tags: &[
            ("Name", Some("api")),
            ("Env", Some("staging")),
            ("Owner", None),
        ],
        target_name: "api-staging",
        attach_resource_group: true,
        template_name: "weekly",
        duration: 3600,
        rules_packages: 3,
        user_attributes: &[("Team", Some("payments")), ("Tier", None)],
        run_name: Some("run-1"),
    },
    Case {
        label: "maximum duration, all rules packages, no resource group, named run",
        region: "ap-southeast-2",
        tags: &[("Name", Some("batch"))],
        target_name: "every-instance",
        attach_resource_group: false,
        template_name: "full-day",
        duration: 86400,
        rules_packages: 4,
        user_attributes: &[("Wave", Some("1"))],
        run_name: Some("full-day-run"),
    },
];

fn tag_json(tags: &[(&str, Option<&str>)]) -> Value {
    Value::Array(
        tags.iter()
            .map(|(key, value)| match value {
                Some(value) => json!({ "key": key, "value": value }),
                None => json!({ "key": key }),
            })
            .collect(),
    )
}

#[tokio::test]
async fn the_core_loop_persists_every_field_it_later_reports() {
    for case in CASES {
        let label = case.label;
        let (router, _engine) = harness();

        // --- CreateResourceGroup: the group, and its tags, must survive the call.
        let (status, body) = call(
            &router,
            "CreateResourceGroup",
            case.region,
            json!({ "resourceGroupTags": tag_json(case.tags) }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected CreateResourceGroup 200, got {status} {body:?}"
        );
        let group_arn = string_at(&body, "resourceGroupArn");
        assert!(
            is_inspector_arn(&group_arn, case.region, "resourcegroup"),
            "case {label:?}: expected a resourcegroup/0-xxxxxxxx ARN in {}, got {group_arn:?}",
            case.region
        );

        let (status, body) = call(
            &router,
            "DescribeResourceGroups",
            case.region,
            json!({ "resourceGroupArns": [group_arn] }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected DescribeResourceGroups 200, got {status} {body:?}"
        );
        let groups = body["resourceGroups"]
            .as_array()
            .unwrap_or_else(|| panic!("case {label:?}: resourceGroups must be present: {body:?}"));
        assert_eq!(
            groups.len(),
            1,
            "case {label:?}: expected exactly the one stored group, got {groups:?}"
        );
        assert_eq!(
            groups[0]["arn"].as_str().unwrap_or_default(),
            group_arn,
            "case {label:?}: the described group must carry the ARN it was created under, got {:?}",
            groups[0]
        );
        assert_eq!(
            groups[0]["tags"],
            tag_json(case.tags),
            "case {label:?}: expected the tags the create sent, got {:?}",
            groups[0]["tags"]
        );
        assert!(
            groups[0]["createdAt"].is_number(),
            "case {label:?}: expected a createdAt timestamp, got {:?}",
            groups[0]["createdAt"]
        );
        assert_eq!(
            body["failedItems"],
            json!({}),
            "case {label:?}: expected no failed items, got {:?}",
            body["failedItems"]
        );

        // --- CreateAssessmentTarget.
        let mut target_request = json!({ "assessmentTargetName": case.target_name });
        if case.attach_resource_group {
            target_request["resourceGroupArn"] = json!(group_arn);
        }
        let (status, body) = call(
            &router,
            "CreateAssessmentTarget",
            case.region,
            target_request,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected CreateAssessmentTarget 200, got {status} {body:?}"
        );
        let target_arn = string_at(&body, "assessmentTargetArn");
        assert!(
            is_inspector_arn(&target_arn, case.region, "target"),
            "case {label:?}: expected a target/0-xxxxxxxx ARN in {}, got {target_arn:?}",
            case.region
        );

        let (status, body) = call(
            &router,
            "DescribeAssessmentTargets",
            case.region,
            json!({ "assessmentTargetArns": [target_arn] }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected DescribeAssessmentTargets 200, got {status} {body:?}"
        );
        let target = &body["assessmentTargets"][0];
        assert_eq!(
            target["name"].as_str().unwrap_or_default(),
            case.target_name,
            "case {label:?}: expected the stored target name, got {target:?}"
        );
        if case.attach_resource_group {
            assert_eq!(
                target["resourceGroupArn"].as_str().unwrap_or_default(),
                group_arn,
                "case {label:?}: expected the resource group the create named, got {target:?}"
            );
        } else {
            assert!(
                target.get("resourceGroupArn").is_none(),
                "case {label:?}: a target created without a resource group must report none, got {target:?}"
            );
        }
        assert_eq!(
            target["createdAt"], target["updatedAt"],
            "case {label:?}: a never-updated target must report updatedAt == createdAt, got {target:?}"
        );

        // --- CreateAssessmentTemplate.
        let rules: Vec<&str> = RULES_PACKAGES[..case.rules_packages].to_vec();
        let attributes = tag_json(case.user_attributes);
        let (status, body) = call(
            &router,
            "CreateAssessmentTemplate",
            case.region,
            json!({
                "assessmentTargetArn": target_arn,
                "assessmentTemplateName": case.template_name,
                "durationInSeconds": case.duration,
                "rulesPackageArns": rules,
                "userAttributesForFindings": attributes,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected CreateAssessmentTemplate 200, got {status} {body:?}"
        );
        let template_arn = string_at(&body, "assessmentTemplateArn");
        assert!(
            is_child_arn(&template_arn, &target_arn, "template"),
            "case {label:?}: expected <target>/template/0-xxxxxxxx, got {template_arn:?}"
        );

        // --- StartAssessmentRun.
        let mut run_request = json!({ "assessmentTemplateArn": template_arn });
        if let Some(name) = case.run_name {
            run_request["assessmentRunName"] = json!(name);
        }
        let (status, body) =
            call(&router, "StartAssessmentRun", case.region, run_request).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected StartAssessmentRun 200, got {status} {body:?}"
        );
        let run_arn = string_at(&body, "assessmentRunArn");
        assert!(
            is_child_arn(&run_arn, &template_arn, "run"),
            "case {label:?}: expected <template>/run/0-xxxxxxxx, got {run_arn:?}"
        );

        // --- DescribeAssessmentRuns: the run reports the TEMPLATE's configuration, and an
        // ARN the store does not hold lands in failedItems.
        let (status, body) = call(
            &router,
            "DescribeAssessmentRuns",
            case.region,
            json!({ "assessmentRunArns": [run_arn, ABSENT_RUN_ARN] }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected DescribeAssessmentRuns 200, got {status} {body:?}"
        );
        let runs = body["assessmentRuns"]
            .as_array()
            .unwrap_or_else(|| panic!("case {label:?}: assessmentRuns must be present: {body:?}"));
        assert_eq!(
            runs.len(),
            1,
            "case {label:?}: expected only the stored run, got {runs:?}"
        );
        let run = &runs[0];
        match case.run_name {
            Some(name) => assert_eq!(
                run["name"].as_str().unwrap_or_default(),
                name,
                "case {label:?}: expected the run name the caller chose, got {run:?}"
            ),
            None => {
                let name = run["name"].as_str().unwrap_or_default();
                assert!(
                    name.starts_with(case.template_name) && name.len() > case.template_name.len(),
                    "case {label:?}: expected a name derived from the template, got {name:?}"
                );
                assert!(
                    name.chars().count() <= 140,
                    "case {label:?}: a derived name must respect the modelled 140-character limit, got {} characters",
                    name.chars().count()
                );
            }
        }
        assert_eq!(
            run["assessmentTemplateArn"].as_str().unwrap_or_default(),
            template_arn,
            "case {label:?}: expected the run to name its template, got {run:?}"
        );
        assert_eq!(
            run["durationInSeconds"].as_u64().unwrap_or_default(),
            u64::from(case.duration),
            "case {label:?}: expected the template's duration, got {run:?}"
        );
        assert_eq!(
            run["rulesPackageArns"],
            json!(rules),
            "case {label:?}: expected the template's rules packages, got {:?}",
            run["rulesPackageArns"]
        );
        assert_eq!(
            run["userAttributesForFindings"], attributes,
            "case {label:?}: expected the template's user attributes, got {:?}",
            run["userAttributesForFindings"]
        );
        assert_eq!(
            run["state"].as_str().unwrap_or_default(),
            "CREATED",
            "case {label:?}: a run observed straight after the start call is still CREATED, got {run:?}"
        );
        assert_eq!(
            run["dataCollected"].as_bool(),
            Some(false),
            "case {label:?}: nothing has been collected yet, got {run:?}"
        );
        assert_eq!(
            run["stateChanges"].as_array().map(Vec::len),
            Some(1),
            "case {label:?}: expected the single CREATED state change, got {:?}",
            run["stateChanges"]
        );
        assert_eq!(
            run["findingCounts"],
            json!({}),
            "case {label:?}: no finding has been reported, so there is no count, got {:?}",
            run["findingCounts"]
        );
        assert_eq!(
            run["createdAt"], run["startedAt"],
            "case {label:?}: startedAt is the time StartAssessmentRun was called, got {run:?}"
        );
        assert!(
            run.get("completedAt").is_none(),
            "case {label:?}: a run that has not completed reports no completedAt, got {run:?}"
        );
        assert_eq!(
            body["failedItems"],
            json!({ ABSENT_RUN_ARN: { "failureCode": "ITEM_DOES_NOT_EXIST", "retryable": false } }),
            "case {label:?}: expected the absent ARN in failedItems, got {:?}",
            body["failedItems"]
        );

        // --- The template now counts its run.
        let state = service_com_amazonaws_inspector_inspectorservice::state::service_state(
            &*_engine,
        )
        .expect("the service store");
        let store = state.lock();
        let template = store
            .assessment_templates
            .get(&template_arn)
            .unwrap_or_else(|| panic!("case {label:?}: the template must still be stored"));
        assert_eq!(
            template.assessment_run_count, 1,
            "case {label:?}: expected assessmentRunCount 1, got {}",
            template.assessment_run_count
        );
        assert_eq!(
            template.last_assessment_run_arn.as_deref(),
            Some(run_arn.as_str()),
            "case {label:?}: expected lastAssessmentRunArn to name the started run, got {:?}",
            template.last_assessment_run_arn
        );
    }
}

/// Every minted ARN is distinct and well-formed, however many resources one store holds: an
/// id is minted from the store's counter, not reused. And the store is BOUNDED — the create
/// past the account's resource-group quota is refused instead of growing the map.
#[tokio::test]
async fn minted_arns_are_unique_and_the_store_is_bounded() {
    let (router, _engine) = harness();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    // The quota this service enforces on resource groups (the same 50 the modelled
    // `CreateAssessmentTarget` documentation states for assessment targets).
    let quota = 50;
    for index in 0..quota {
        let (status, body) = call(
            &router,
            "CreateResourceGroup",
            "us-east-1",
            json!({ "resourceGroupTags": [{ "key": "Name", "value": format!("group-{index}") }] }),
        )
        .await;
        assert_eq!(
            status, 200,
            "group {index}: expected 200 inside the quota, got {status} {body:?}"
        );
        let arn = string_at(&body, "resourceGroupArn");
        assert!(
            is_inspector_arn(&arn, "us-east-1", "resourcegroup"),
            "group {index}: expected a resourcegroup/0-xxxxxxxx ARN, got {arn:?}"
        );
        assert!(
            seen.insert(arn.clone()),
            "group {index}: ARN {arn:?} was minted twice"
        );
    }

    let (status, body) = call(
        &router,
        "CreateResourceGroup",
        "us-east-1",
        json!({ "resourceGroupTags": [{ "key": "Name", "value": "one-too-many" }] }),
    )
    .await;
    assert_eq!(
        status, 400,
        "the create past the quota must be refused, got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "LimitExceededException",
        "expected LimitExceededException past the quota, got {body:?}"
    );
    assert_eq!(
        string_at(&body, "errorCode"),
        "RESOURCE_GROUP_LIMIT_EXCEEDED",
        "expected the resource-group limit error code, got {body:?}"
    );
}

/// A run AWS returns in a non-terminal state owes its later transitions to the clock: data
/// collection starts shortly after the start call, and the run completes once its collection
/// window has elapsed. Neither is caused by the read that reports it.
#[tokio::test]
async fn a_started_run_advances_on_the_clock_not_on_the_read() {
    let (router, engine) = harness();
    let region = "us-east-1";

    let (_, body) = call(
        &router,
        "CreateAssessmentTarget",
        region,
        json!({ "assessmentTargetName": "clock-target" }),
    )
    .await;
    let target_arn = string_at(&body, "assessmentTargetArn");
    let (_, body) = call(
        &router,
        "CreateAssessmentTemplate",
        region,
        json!({
            "assessmentTargetArn": target_arn,
            "assessmentTemplateName": "clock-template",
            "durationInSeconds": 180,
            "rulesPackageArns": [RULES_PACKAGES[1]],
        }),
    )
    .await;
    let template_arn = string_at(&body, "assessmentTemplateArn");
    let before_start = SystemTime::now();
    let (_, body) = call(
        &router,
        "StartAssessmentRun",
        region,
        json!({ "assessmentTemplateArn": template_arn }),
    )
    .await;
    let run_arn = string_at(&body, "assessmentRunArn");

    // Immediately: still CREATED — two reads in a row agree, so the read did not move it.
    for attempt in 0..2 {
        let (_, body) = call(
            &router,
            "DescribeAssessmentRuns",
            region,
            json!({ "assessmentRunArns": [run_arn] }),
        )
        .await;
        assert_eq!(
            body["assessmentRuns"][0]["state"].as_str().unwrap_or_default(),
            "CREATED",
            "read {attempt}: a read must not advance the run, got {:?}",
            body["assessmentRuns"][0]
        );
    }

    // The transition is DUE, not immediate: the stored due instant is a real span after the
    // start call, which is why the reads above still see CREATED. Asserted against the stored
    // instant rather than by sleeping, so the suite does not depend on wall-clock timing.
    let state = service_com_amazonaws_inspector_inspectorservice::state::service_state(&*engine)
        .expect("the service store");
    let due = state
        .lock()
        .assessment_runs
        .get(&run_arn)
        .expect("the stored run")
        .collecting_data_due_at;
    assert!(
        due >= before_start + Duration::from_millis(250)
            && due <= before_start + Duration::from_millis(750),
        "the data-collection transition must fall due 250-750 ms after the start call, got {:?} after",
        due.duration_since(before_start)
    );

    // Once that instant has passed, the next read materialises the transition. The due
    // instant is brought forward instead of sleeping for it.
    state
        .lock()
        .assessment_runs
        .get_mut(&run_arn)
        .expect("the stored run")
        .collecting_data_due_at = SystemTime::now();
    let (_, body) = call(
        &router,
        "DescribeAssessmentRuns",
        region,
        json!({ "assessmentRunArns": [run_arn] }),
    )
    .await;
    let run = &body["assessmentRuns"][0];
    assert_eq!(
        run["state"].as_str().unwrap_or_default(),
        "COLLECTING_DATA",
        "once the start delay is due the run must be collecting data, got {run:?}"
    );
    assert_eq!(
        run["stateChanges"].as_array().map(Vec::len),
        Some(2),
        "the transition must be recorded in stateChanges, got {:?}",
        run["stateChanges"]
    );

    // And once the collection window closes, the run completes. No implemented operation
    // ends a run early yet (StopAssessmentRun and DeleteAssessmentRun are still 501), so the
    // window's own due instant is brought forward the same way.
    state
        .lock()
        .assessment_runs
        .get_mut(&run_arn)
        .expect("the stored run")
        .completion_due_at = SystemTime::now();
    let (_, body) = call(
        &router,
        "DescribeAssessmentRuns",
        region,
        json!({ "assessmentRunArns": [run_arn] }),
    )
    .await;
    let run = &body["assessmentRuns"][0];
    assert_eq!(
        run["state"].as_str().unwrap_or_default(),
        "COMPLETED",
        "after the collection window the run must be COMPLETED, got {run:?}"
    );
    assert_eq!(
        run["dataCollected"].as_bool(),
        Some(true),
        "a completed run has finished collecting, got {run:?}"
    );
    assert!(
        run["completedAt"].is_number(),
        "a completed run reports completedAt, got {run:?}"
    );
    assert_eq!(
        run["findingCounts"],
        json!({}),
        "no agent reported a finding, so no count is invented, got {:?}",
        run["findingCounts"]
    );
}

/// Deleting a target removes everything beneath it, and is refused while any of its runs is
/// still in progress.
#[tokio::test]
async fn deleting_a_target_cascades_and_waits_for_its_runs() {
    let (router, engine) = harness();
    let region = "us-west-2";

    let (_, body) = call(
        &router,
        "CreateResourceGroup",
        region,
        json!({ "resourceGroupTags": [{ "key": "Name", "value": "cascade" }] }),
    )
    .await;
    let group_arn = string_at(&body, "resourceGroupArn");
    let (_, body) = call(
        &router,
        "CreateAssessmentTarget",
        region,
        json!({ "assessmentTargetName": "cascade-target", "resourceGroupArn": group_arn }),
    )
    .await;
    let target_arn = string_at(&body, "assessmentTargetArn");
    let (_, body) = call(
        &router,
        "CreateAssessmentTemplate",
        region,
        json!({
            "assessmentTargetArn": target_arn,
            "assessmentTemplateName": "cascade-template",
            "durationInSeconds": 900,
            "rulesPackageArns": [RULES_PACKAGES[0], RULES_PACKAGES[2]],
        }),
    )
    .await;
    let template_arn = string_at(&body, "assessmentTemplateArn");
    let (_, body) = call(
        &router,
        "StartAssessmentRun",
        region,
        json!({ "assessmentTemplateArn": template_arn, "assessmentRunName": "cascade-run" }),
    )
    .await;
    let run_arn = string_at(&body, "assessmentRunArn");

    // While the run is in progress the delete is refused, and says which run blocks it.
    let (status, body) = call(
        &router,
        "DeleteAssessmentTarget",
        region,
        json!({ "assessmentTargetArn": target_arn }),
    )
    .await;
    assert_eq!(
        status, 400,
        "a target with a run in progress must not be deleted, got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "AssessmentRunInProgressException",
        "expected AssessmentRunInProgressException, got {body:?}"
    );
    assert_eq!(
        body["assessmentRunArns"],
        json!([run_arn]),
        "the refusal must name the run in progress, got {body:?}"
    );
    assert_eq!(
        body["assessmentRunArnsTruncated"].as_bool(),
        Some(false),
        "one ARN is not a truncated list, got {body:?}"
    );

    // Let the run reach a terminal state (see the clock test for why this is done here).
    let state =
        service_com_amazonaws_inspector_inspectorservice::state::service_state(&*engine)
            .expect("the service store");
    {
        let mut store = state.lock();
        let record = store
            .assessment_runs
            .get_mut(&run_arn)
            .expect("the stored run");
        record.collecting_data_due_at = SystemTime::now();
        record.completion_due_at = SystemTime::now();
    }

    let (status, body) = call(
        &router,
        "DeleteAssessmentTarget",
        region,
        json!({ "assessmentTargetArn": target_arn }),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected DeleteAssessmentTarget 200 once no run is in progress, got {status} {body:?}"
    );

    // The target, its template and its run are all gone; the resource group is not.
    let (_, body) = call(
        &router,
        "DescribeAssessmentTargets",
        region,
        json!({ "assessmentTargetArns": [target_arn] }),
    )
    .await;
    assert_eq!(
        body["assessmentTargets"],
        json!([]),
        "a deleted target must not be described, got {body:?}"
    );
    assert_eq!(
        body["failedItems"][&target_arn]["failureCode"]
            .as_str()
            .unwrap_or_default(),
        "ITEM_DOES_NOT_EXIST",
        "a deleted target must appear in failedItems, got {body:?}"
    );

    let (_, body) = call(
        &router,
        "DescribeAssessmentRuns",
        region,
        json!({ "assessmentRunArns": [run_arn] }),
    )
    .await;
    assert_eq!(
        body["assessmentRuns"],
        json!([]),
        "a cascaded run must not be described, got {body:?}"
    );
    assert_eq!(
        body["failedItems"][&run_arn]["failureCode"]
            .as_str()
            .unwrap_or_default(),
        "ITEM_DOES_NOT_EXIST",
        "a cascaded run must appear in failedItems, got {body:?}"
    );

    let (_, body) = call(
        &router,
        "DescribeResourceGroups",
        region,
        json!({ "resourceGroupArns": [group_arn] }),
    )
    .await;
    assert_eq!(
        body["resourceGroups"][0]["arn"].as_str().unwrap_or_default(),
        group_arn,
        "the resource group outlives the target that used it, got {body:?}"
    );

    let store = state.lock();
    assert!(
        store.assessment_templates.is_empty() && store.assessment_runs.is_empty(),
        "the cascade must leave no template or run behind: templates {:?}, runs {:?}",
        store.assessment_templates.keys().collect::<Vec<_>>(),
        store.assessment_runs.keys().collect::<Vec<_>>()
    );
}

/// The references an operation resolves must exist, and names the service keeps unique must
/// be refused when repeated — each with the modelled error and error code.
#[tokio::test]
async fn references_and_unique_names_are_enforced() {
    let (router, _engine) = harness();
    let region = "us-east-1";
    let absent_group = format!("arn:aws:inspector:{region}:{ACCOUNT}:resourcegroup/0-ZZZZZZZZ");
    let absent_target = format!("arn:aws:inspector:{region}:{ACCOUNT}:target/0-ZZZZZZZZ");
    let absent_template = format!("{absent_target}/template/0-ZZZZZZZZ");

    // A target pointing at a group that does not exist.
    let (status, body) = call(
        &router,
        "CreateAssessmentTarget",
        region,
        json!({ "assessmentTargetName": "orphan", "resourceGroupArn": absent_group }),
    )
    .await;
    assert_eq!(
        status, 400,
        "an unknown resource group must be refused, got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "NoSuchEntityException",
        "expected NoSuchEntityException, got {body:?}"
    );
    assert_eq!(
        string_at(&body, "errorCode"),
        "RESOURCE_GROUP_DOES_NOT_EXIST",
        "expected the resource-group error code, got {body:?}"
    );

    // A template under a target that does not exist.
    let (status, body) = call(
        &router,
        "CreateAssessmentTemplate",
        region,
        json!({
            "assessmentTargetArn": absent_target,
            "assessmentTemplateName": "orphan",
            "durationInSeconds": 3600,
            "rulesPackageArns": [RULES_PACKAGES[0]],
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "an unknown assessment target must be refused, got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "NoSuchEntityException",
        "expected NoSuchEntityException, got {body:?}"
    );
    assert_eq!(
        string_at(&body, "errorCode"),
        "ASSESSMENT_TARGET_DOES_NOT_EXIST",
        "expected the assessment-target error code, got {body:?}"
    );

    // A run from a template that does not exist.
    let (status, body) = call(
        &router,
        "StartAssessmentRun",
        region,
        json!({ "assessmentTemplateArn": absent_template }),
    )
    .await;
    assert_eq!(
        status, 400,
        "an unknown assessment template must be refused, got {status} {body:?}"
    );
    assert_eq!(
        string_at(&body, "errorCode"),
        "ASSESSMENT_TEMPLATE_DOES_NOT_EXIST",
        "expected the assessment-template error code, got {body:?}"
    );

    // Deleting a target that does not exist.
    let (status, body) = call(
        &router,
        "DeleteAssessmentTarget",
        region,
        json!({ "assessmentTargetArn": absent_target }),
    )
    .await;
    assert_eq!(
        status, 400,
        "deleting an unknown target must be refused, got {status} {body:?}"
    );
    assert_eq!(
        string_at(&body, "errorCode"),
        "ASSESSMENT_TARGET_DOES_NOT_EXIST",
        "expected the assessment-target error code, got {body:?}"
    );

    // Target names are unique per account.
    let (status, _) = call(
        &router,
        "CreateAssessmentTarget",
        region,
        json!({ "assessmentTargetName": "taken" }),
    )
    .await;
    assert_eq!(status, 200, "the first target with a name must be created");
    let (status, body) = call(
        &router,
        "CreateAssessmentTarget",
        region,
        json!({ "assessmentTargetName": "taken" }),
    )
    .await;
    assert_eq!(
        status, 400,
        "a repeated target name must be refused, got {status} {body:?}"
    );
    assert_eq!(
        string_at(&body, "errorCode"),
        "ASSESSMENT_TARGET_NAME_ALREADY_TAKEN",
        "expected the name-already-taken error code, got {body:?}"
    );

    // Template names are unique WITHIN a target, and free across targets.
    let (_, body) = call(
        &router,
        "CreateAssessmentTarget",
        region,
        json!({ "assessmentTargetName": "other" }),
    )
    .await;
    let other_target = string_at(&body, "assessmentTargetArn");
    let (_, body) = call(
        &router,
        "DescribeAssessmentTargets",
        region,
        json!({ "assessmentTargetArns": [other_target] }),
    )
    .await;
    let first_target = body["assessmentTargets"][0]["arn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let template = json!({
        "assessmentTargetArn": first_target,
        "assessmentTemplateName": "shared-name",
        "durationInSeconds": 1800,
        "rulesPackageArns": [RULES_PACKAGES[3]],
    });
    let (status, _) = call(&router, "CreateAssessmentTemplate", region, template.clone()).await;
    assert_eq!(status, 200, "the first template with a name must be created");
    let (status, body) = call(&router, "CreateAssessmentTemplate", region, template).await;
    assert_eq!(
        status, 400,
        "a repeated template name under one target must be refused, got {status} {body:?}"
    );
    assert_eq!(
        string_at(&body, "errorCode"),
        "ASSESSMENT_TEMPLATE_NAME_ALREADY_TAKEN",
        "expected the template-name-already-taken error code, got {body:?}"
    );

    let (_, body) = call(
        &router,
        "CreateAssessmentTarget",
        region,
        json!({ "assessmentTargetName": "third" }),
    )
    .await;
    let third_target = string_at(&body, "assessmentTargetArn");
    let (status, body) = call(
        &router,
        "CreateAssessmentTemplate",
        region,
        json!({
            "assessmentTargetArn": third_target,
            "assessmentTemplateName": "shared-name",
            "durationInSeconds": 1800,
            "rulesPackageArns": [RULES_PACKAGES[3]],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the same template name under a different target is free, got {status} {body:?}"
    );

    // User attribute keys are unique within a template.
    let (status, body) = call(
        &router,
        "CreateAssessmentTemplate",
        region,
        json!({
            "assessmentTargetArn": third_target,
            "assessmentTemplateName": "duplicate-attributes",
            "durationInSeconds": 1800,
            "rulesPackageArns": [RULES_PACKAGES[0]],
            "userAttributesForFindings": [{ "key": "Team", "value": "a" }, { "key": "Team", "value": "b" }],
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "a repeated user attribute key must be refused, got {status} {body:?}"
    );
    assert_eq!(
        string_at(&body, "errorCode"),
        "INVALID_USER_ATTRIBUTE",
        "expected the user-attribute error code, got {body:?}"
    );

    // A template with no rules packages has nothing to evaluate, so no run can start.
    let (_, body) = call(
        &router,
        "CreateAssessmentTemplate",
        region,
        json!({
            "assessmentTargetArn": third_target,
            "assessmentTemplateName": "no-rules",
            "durationInSeconds": 1800,
            "rulesPackageArns": [],
        }),
    )
    .await;
    let empty_template = string_at(&body, "assessmentTemplateArn");
    let (status, body) = call(
        &router,
        "StartAssessmentRun",
        region,
        json!({ "assessmentTemplateArn": empty_template }),
    )
    .await;
    assert_eq!(
        status, 400,
        "a run needs at least one rules package, got {status} {body:?}"
    );
    assert_eq!(
        string_at(&body, "errorCode"),
        "INVALID_NUMBER_OF_RULES_PACKAGE_ARNS",
        "expected the rules-package-count error code, got {body:?}"
    );
}

/// A value outside a modelled bound, or an absent required member, is refused as
/// `InvalidInputException` — the client-input error this service's model declares — on every
/// published bound and one step past it.
#[tokio::test]
async fn values_outside_the_modelled_bounds_are_refused_as_invalid_input() {
    let (router, _engine) = harness();
    let region = "us-east-1";
    let (_, body) = call(
        &router,
        "CreateAssessmentTarget",
        region,
        json!({ "assessmentTargetName": "bounds" }),
    )
    .await;
    let target_arn = string_at(&body, "assessmentTargetArn");

    // `durationInSeconds` is modelled as range 180..86400.
    for (index, duration) in [180_i64, 86400].into_iter().enumerate() {
        let (status, body) = call(
            &router,
            "CreateAssessmentTemplate",
            region,
            json!({
                "assessmentTargetArn": target_arn,
                "assessmentTemplateName": format!("in-range-{index}"),
                "durationInSeconds": duration,
                "rulesPackageArns": [RULES_PACKAGES[0]],
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "duration {duration} is on a published bound and must be accepted, got {status} {body:?}"
        );
    }
    for duration in [179_i64, 86401] {
        let (status, body) = call(
            &router,
            "CreateAssessmentTemplate",
            region,
            json!({
                "assessmentTargetArn": target_arn,
                "assessmentTemplateName": format!("out-of-range-{duration}"),
                "durationInSeconds": duration,
                "rulesPackageArns": [RULES_PACKAGES[0]],
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "duration {duration} is outside the modelled range and must be refused, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInputException",
            "duration {duration}: expected InvalidInputException, got {body:?}"
        );
        assert!(
            string_at(&body, "message").contains("durationInSeconds"),
            "duration {duration}: the refusal must name the member that failed, got {body:?}"
        );
        // The pre-dispatch refusal channel renders only `__type` + `message`, so the
        // modelled `errorCode` member travels only on refusals a HANDLER raises (asserted
        // in `references_and_unique_names_are_enforced`). Pinned here so the limitation
        // documented in `service.rs::override_refusal` cannot drift unnoticed.
        assert!(
            body.get("errorCode").is_none(),
            "duration {duration}: a framework refusal carries no modelled errorCode, got {body:?}"
        );
    }

    // `assessmentRunArns` is modelled as a 1..10 list, and `resourceGroupTags` as 1..10.
    let run_arns: Vec<String> = (0..11).map(|i| format!("{ABSENT_RUN_ARN}{i}")).collect();
    let (status, body) = call(
        &router,
        "DescribeAssessmentRuns",
        region,
        json!({ "assessmentRunArns": run_arns }),
    )
    .await;
    assert_eq!(
        status, 400,
        "11 ARNs exceed the modelled 1..10 bound and must be refused, got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidInputException",
        "expected InvalidInputException for the over-long list, got {body:?}"
    );

    let (status, body) = call(
        &router,
        "CreateResourceGroup",
        region,
        json!({ "resourceGroupTags": [] }),
    )
    .await;
    assert_eq!(
        status, 400,
        "an empty tag list is below the modelled 1..10 bound, got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidInputException",
        "expected InvalidInputException for the empty tag list, got {body:?}"
    );

    // An absent required member.
    let (status, body) = call(&router, "StartAssessmentRun", region, json!({})).await;
    assert_eq!(
        status, 400,
        "an absent required member must be refused, got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidInputException",
        "expected InvalidInputException for the absent member, got {body:?}"
    );
}

/// Ten ARNs, some stored and some not, are reported in the right halves of one batch
/// describe: the stored ones in the list, the rest in `failedItems`.
#[tokio::test]
async fn a_batch_describe_splits_stored_from_absent_arns() {
    let (router, _engine) = harness();
    let region = "eu-central-1";
    let mut stored: Vec<String> = Vec::new();
    for index in 0..4 {
        let (_, body) = call(
            &router,
            "CreateResourceGroup",
            region,
            json!({ "resourceGroupTags": [{ "key": "Name", "value": format!("batch-{index}") }] }),
        )
        .await;
        stored.push(string_at(&body, "resourceGroupArn"));
    }
    let absent: Vec<String> = (0..6)
        .map(|i| format!("arn:aws:inspector:{region}:{ACCOUNT}:resourcegroup/0-Absent{i}0"))
        .collect();
    let requested: Vec<String> = stored.iter().chain(absent.iter()).cloned().collect();

    let (status, body) = call(
        &router,
        "DescribeResourceGroups",
        region,
        json!({ "resourceGroupArns": requested }),
    )
    .await;
    assert_eq!(status, 200, "expected 200, got {status} {body:?}");
    let described: Vec<String> = body["resourceGroups"]
        .as_array()
        .expect("resourceGroups must be present")
        .iter()
        .map(|group| group["arn"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        described, stored,
        "expected the stored groups in request order, got {described:?}"
    );
    let failed = body["failedItems"]
        .as_object()
        .expect("failedItems must be present");
    assert_eq!(
        failed.len(),
        absent.len(),
        "expected one failed item per absent ARN, got {failed:?}"
    );
    for arn in &absent {
        assert_eq!(
            failed[arn]["failureCode"].as_str().unwrap_or_default(),
            "ITEM_DOES_NOT_EXIST",
            "ARN {arn:?}: expected ITEM_DOES_NOT_EXIST, got {:?}",
            failed.get(arn)
        );
        assert_eq!(
            failed[arn]["retryable"].as_bool(),
            Some(false),
            "ARN {arn:?}: a missing item is not retryable, got {:?}",
            failed.get(arn)
        );
    }
}
