// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `DeleteTelemetryPipeline`.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers — through
//! the ONE harness this test crate has (`telemetry_rules`), so what is asserted is
//! what a signed AWS SDK request observes.
//!
//! The rule under test: a delete removes the ONE stored pipeline record the identifier
//! (a name or an ARN) names in the region the request was signed for and answers the
//! modelled empty 200, so every handle on that pipeline stops resolving and its name
//! is free again; an identifier that names no pipeline in that region is the modelled
//! 404 `ResourceNotFoundException` and removes nothing.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_observabilityadmin_observabilityadmin::state::{
    ComAmazonawsObservabilityadminObservabilityadminState, PIPELINE_ACTIVATION_DELAY_MS,
    PIPELINE_NOT_FOUND_MESSAGE, PIPELINE_RESOURCE_TYPE,
};

use crate::telemetry_rules::{call, emulator, error_code, Emulator, ACCOUNT};

/// The region most requests below sign for.
const HOME_REGION: &str = "us-west-2";

/// A second region, so "the region the request was signed for" can be shown to scope
/// what a delete can reach.
const OTHER_REGION: &str = "eu-west-1";

/// Long enough after a create for the `CREATING` -> `ACTIVE` transition to have come
/// due, whatever scheduling delay the test run adds on top.
fn past_activation_delay() -> Duration {
    Duration::from_millis(PIPELINE_ACTIVATION_DELAY_MS as u64 + 250)
}

async fn create(emulator: &Emulator, region: &str, body: &Value) -> (u16, Value) {
    call(emulator, region, "/CreateTelemetryPipeline", body).await
}

async fn delete(emulator: &Emulator, region: &str, identifier: &str) -> (u16, Value) {
    call(
        emulator,
        region,
        "/DeleteTelemetryPipeline",
        &json!({ "PipelineIdentifier": identifier }),
    )
    .await
}

async fn get(emulator: &Emulator, region: &str, identifier: &str) -> (u16, Value) {
    call(
        emulator,
        region,
        "/GetTelemetryPipeline",
        &json!({ "PipelineIdentifier": identifier }),
    )
    .await
}

/// The `Arn` a successful create reported.
fn created_arn(case: &str, body: &Value) -> String {
    body.get("Arn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a created Arn, got {body:?}"))
        .to_string()
}

/// Whether the store holds a record for `identifier` in `region` at all — asked of the
/// store itself, so "the record is gone" is not inferred from the read path alone.
fn stored(emulator: &Emulator, region: &str, identifier: &str) -> bool {
    let state = emulator
        .engine
        .state::<ComAmazonawsObservabilityadminObservabilityadminState>();
    let store = state.lock();
    store.pipeline(region, identifier).is_some()
}

/// Which form of the identifier a row deletes the pipeline by: both forms
/// `PipelineIdentifier`'s documentation names ("the identifier (name or ARN)").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Handle {
    Name,
    Arn,
}

/// One delete case. The rows differ along what the rule generalises over: which handle
/// names the pipeline, which status the pipeline stands at when the delete arrives,
/// what shape its configuration document has, whether it carries tags, and which
/// region it lives in.
struct Row {
    case: &'static str,
    handle: Handle,
    /// Whether the row waits for the `CREATING` -> `ACTIVE` transition before deleting.
    wait_for_active: bool,
    region: &'static str,
    name: &'static str,
    configuration: &'static str,
    tags: bool,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "delete by name, while the pipeline is still CREATING",
            handle: Handle::Name,
            wait_for_active: false,
            region: HOME_REGION,
            name: "pipeline-creating",
            configuration: "source:\n  http: {}\nsink:\n  - cloudwatch_logs: {}\n",
            tags: false,
        },
        Row {
            case: "delete by ARN, once the pipeline is ACTIVE",
            handle: Handle::Arn,
            wait_for_active: true,
            region: HOME_REGION,
            name: "pipeline-active",
            configuration: "version: \"2\"\nlog-pipeline:\n  source:\n    s3:\n      \
                            bucket: example-telemetry\n  processor:\n    - date:\n        \
                            from_time_received: true\n  sink:\n    - cloudwatch_logs:\n        \
                            log_group: /aws/telemetry/example\n",
            tags: false,
        },
        Row {
            case: "delete by name a tagged pipeline whose configuration is JSON",
            handle: Handle::Name,
            wait_for_active: false,
            region: HOME_REGION,
            name: "pipeline-tagged",
            configuration: "{\"source\": {\"http\": {}}, \"sink\": [{\"cloudwatch_logs\": {}}]}",
            tags: true,
        },
        Row {
            case: "delete by ARN in another region, by a caller signed for that region",
            handle: Handle::Arn,
            wait_for_active: false,
            region: OTHER_REGION,
            name: "pipeline-other-region",
            configuration: "source:\n  cloudwatch_logs:\n    log_group: /aws/lambda/example\n\
                            sink:\n  - s3:\n      bucket: example-archive\n",
            tags: true,
        },
        Row {
            case: "delete by name a pipeline whose name is of the modelled maximum length",
            handle: Handle::Name,
            wait_for_active: true,
            region: HOME_REGION,
            // `TelemetryPipelineName` is `length: min 3, max 28`, as
            // `smithy_query(query="operation",operation="CreateTelemetryPipeline")` reports.
            name: "pipeline-with-a-long-name-28",
            configuration: "source:\n  http:\nsink:\n  - cloudwatch_logs:\n",
            tags: false,
        },
        Row {
            case: "delete by name a pipeline whose name is of the modelled minimum length",
            handle: Handle::Name,
            wait_for_active: false,
            region: HOME_REGION,
            name: "pip",
            configuration: "source:\n  http: {}\nsink:\n  - cloudwatch_logs: {}\n",
            tags: false,
        },
    ]
}

#[tokio::test]
async fn a_delete_removes_the_pipeline_each_identifier_form_names() {
    let emulator = emulator();

    for row in rows() {
        let Row {
            case,
            handle,
            wait_for_active,
            region,
            name,
            configuration,
            tags,
        } = row;
        assert!(
            (3..=28).contains(&name.len()),
            "case {case:?}: the test's own name must satisfy the modelled 3..28 length, \
             got {name:?} of length {}",
            name.len()
        );

        let mut request = json!({ "Name": name, "Configuration": { "Body": configuration } });
        if tags {
            request["Tags"] = json!({ "team": "observability" });
        }
        let (status, created) = create(&emulator, region, &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateTelemetryPipeline to answer 200, \
             got {status} {created:?}"
        );
        let arn = created_arn(case, &created);

        if wait_for_active {
            tokio::time::sleep(past_activation_delay()).await;
            let (status, active) = get(&emulator, region, name).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the pipeline to be readable before the delete, \
                 got {status} {active:?}"
            );
            assert_eq!(
                active
                    .get("Pipeline")
                    .and_then(|pipeline| pipeline.get("Status"))
                    .and_then(Value::as_str),
                Some("ACTIVE"),
                "case {case:?}: expected the pipeline to be ACTIVE before the delete, \
                 got {active:?}"
            );
        }

        let identifier = match handle {
            Handle::Name => name.to_string(),
            Handle::Arn => arn.clone(),
        };
        let (status, deleted) = delete(&emulator, region, &identifier).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DeleteTelemetryPipeline({identifier}) to answer 200, \
             got {status} {deleted:?}"
        );
        assert!(
            matches!(&deleted, Value::Null | Value::Object(_)),
            "case {case:?}: DeleteTelemetryPipeline answers the modelled empty body, \
             got {deleted:?}"
        );
        if let Value::Object(map) = &deleted {
            assert!(
                map.is_empty(),
                "case {case:?}: DeleteTelemetryPipeline models no output member, \
                 got {deleted:?}"
            );
        }

        // Neither handle resolves any longer, and the record itself is gone from the
        // store — not merely hidden from the read path.
        for gone in [name.to_string(), arn.clone()] {
            assert!(
                !stored(&emulator, region, &gone),
                "case {case:?}: the store still holds a record for {gone:?} after the delete"
            );
            let (status, read) = get(&emulator, region, &gone).await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected GetTelemetryPipeline({gone}) to answer 404 after the \
                 delete, got {status} {read:?}"
            );
            assert_eq!(
                error_code(&read),
                "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException for {gone:?}, got {read:?}"
            );

            // A repeat of the delete is the same observation: the pipeline the
            // identifier named is not there.
            let (status, again) = delete(&emulator, region, &gone).await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected a repeated DeleteTelemetryPipeline({gone}) to answer \
                 404, got {status} {again:?}"
            );
            assert_eq!(
                error_code(&again),
                "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException for the repeated delete of \
                 {gone:?}, got {again:?}"
            );
        }
    }
}

/// An identifier that names no pipeline in the region the request was signed for is the
/// modelled 404, and the delete removes nothing.
///
/// The rows span what the rule generalises over: a name nothing ever created, the name
/// of a pipeline that lives in ANOTHER region, and an ARN that differs from the stored
/// one in exactly one field — its region, its account, its partition or its resource
/// type — so a handle that is not this pipeline's own handle in this caller's own
/// region resolves to nothing.
#[tokio::test]
async fn a_delete_of_an_identifier_naming_no_pipeline_is_a_modelled_404() {
    let emulator = emulator();

    // One pipeline in each region, sharing a name, so a refusal cannot be mistaken for
    // an empty store.
    let shared_name = "survivor-pipeline";
    let configuration = "source:\n  http: {}\nsink:\n  - cloudwatch_logs: {}\n";
    let mut arns = Vec::new();
    for region in [HOME_REGION, OTHER_REGION] {
        let (status, created) = create(
            &emulator,
            region,
            &json!({ "Name": shared_name, "Configuration": { "Body": configuration } }),
        )
        .await;
        assert_eq!(
            status, 200,
            "expected a create in {region} to answer 200, got {status} {created:?}"
        );
        arns.push(created_arn(region, &created));
    }
    let home_arn = arns[0].clone();
    let other_arn = arns[1].clone();
    let id = home_arn
        .rsplit('/')
        .next()
        .expect("the ARN carries the pipeline id")
        .to_string();

    let cases: Vec<(&str, String)> = vec![
        ("a name nothing ever created", "no-such-pipeline".to_string()),
        (
            "a name whose pipeline lives in another region",
            format!("{shared_name}-elsewhere"),
        ),
        (
            "the ARN of the pipeline that lives in the other region",
            other_arn.clone(),
        ),
        (
            "an ARN differing from the stored one only in its region",
            format!("arn:aws:observabilityadmin:us-east-1:{ACCOUNT}:telemetry-pipeline/{id}"),
        ),
        (
            "an ARN differing from the stored one only in its account",
            format!("arn:aws:observabilityadmin:{HOME_REGION}:210987654321:telemetry-pipeline/{id}"),
        ),
        (
            "an ARN differing from the stored one only in its partition",
            format!("arn:aws-us-gov:observabilityadmin:{HOME_REGION}:{ACCOUNT}:telemetry-pipeline/{id}"),
        ),
        (
            "an ARN differing from the stored one only in its resource type",
            format!("arn:aws:observabilityadmin:{HOME_REGION}:{ACCOUNT}:telemetry-rule/{id}"),
        ),
        (
            "an ARN whose id names no pipeline",
            format!("arn:aws:observabilityadmin:{HOME_REGION}:{ACCOUNT}:telemetry-pipeline/0123456789abcdef"),
        ),
    ];

    for (case, identifier) in cases {
        let (status, refused) = delete(&emulator, HOME_REGION, &identifier).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected DeleteTelemetryPipeline({identifier}) to answer 404, \
             got {status} {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {refused:?}"
        );
        assert_eq!(
            refused.get("Message").and_then(Value::as_str),
            Some(PIPELINE_NOT_FOUND_MESSAGE),
            "case {case:?}: expected the service's own not-found sentence, got {refused:?}"
        );
        assert_eq!(
            refused.get("ResourceId").and_then(Value::as_str),
            Some(identifier.as_str()),
            "case {case:?}: the refusal reports the identifier the request carried, \
             got {refused:?}"
        );
        assert_eq!(
            refused.get("ResourceType").and_then(Value::as_str),
            Some(PIPELINE_RESOURCE_TYPE),
            "case {case:?}: the refusal names the resource type, got {refused:?}"
        );

        // Nothing was removed: both pipelines are still there, each in its own region.
        for (region, arn) in [(HOME_REGION, &home_arn), (OTHER_REGION, &other_arn)] {
            let (status, read) = get(&emulator, region, arn).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the refused delete must leave the pipeline in {region} alone, \
                 got {status} {read:?}"
            );
        }
    }
}

/// A delete reaches only the region the request was signed for, and the name it frees
/// is available to a later create — which is a NEW pipeline, with its own ARN.
#[tokio::test]
async fn a_delete_is_scoped_to_its_region_and_frees_the_name() {
    let emulator = emulator();

    let name = "regional-pipeline";
    let configuration = "source:\n  http: {}\nsink:\n  - cloudwatch_logs: {}\n";
    let request = json!({ "Name": name, "Configuration": { "Body": configuration } });

    let (status, home) = create(&emulator, HOME_REGION, &request).await;
    assert_eq!(status, 200, "expected 200, got {status} {home:?}");
    let home_arn = created_arn("the home-region create", &home);

    let (status, other) = create(&emulator, OTHER_REGION, &request).await;
    assert_eq!(status, 200, "expected 200, got {status} {other:?}");
    let other_arn = created_arn("the other-region create", &other);

    let (status, deleted) = delete(&emulator, HOME_REGION, name).await;
    assert_eq!(status, 200, "expected 200, got {status} {deleted:?}");

    // The same name in the other region is a DIFFERENT pipeline, untouched by the
    // delete the caller signed for the home region.
    let (status, survivor) = get(&emulator, OTHER_REGION, name).await;
    assert_eq!(
        status, 200,
        "the delete must not reach the pipeline of the same name in another region, \
         got {status} {survivor:?}"
    );
    assert_eq!(
        survivor
            .get("Pipeline")
            .and_then(|pipeline| pipeline.get("Arn"))
            .and_then(Value::as_str),
        Some(other_arn.as_str()),
        "the surviving pipeline is the other region's own, got {survivor:?}"
    );

    // The freed name is available again, and what the second create stores is a new
    // pipeline rather than the deleted one coming back.
    let (status, recreated) = create(&emulator, HOME_REGION, &request).await;
    assert_eq!(
        status, 200,
        "the deleted name is free for a later create, got {status} {recreated:?}"
    );
    let recreated_arn = created_arn("the create after the delete", &recreated);
    assert_ne!(
        recreated_arn, home_arn,
        "a create after a delete mints a new pipeline, so its ARN is not the deleted \
         pipeline's: got {recreated_arn}"
    );
    let (status, read) = get(&emulator, HOME_REGION, name).await;
    assert_eq!(status, 200, "expected 200, got {status} {read:?}");
    assert_eq!(
        read.get("Pipeline")
            .and_then(|pipeline| pipeline.get("Arn"))
            .and_then(Value::as_str),
        Some(recreated_arn.as_str()),
        "the read reports the pipeline the second create stored, got {read:?}"
    );
}
