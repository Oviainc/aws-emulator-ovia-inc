// SPDX-License-Identifier: CC0-1.0

//! Which handle a telemetry-pipeline MUTATION resolves its target by, end to end over
//! HTTP.
//!
//! The rule under test: `DeleteTelemetryPipeline` and `UpdateTelemetryPipeline` resolve
//! `PipelineIdentifier` through the ONE lookup every pipeline-scoped operation goes through
//! (`crate::state::…::resolve_pipeline_key`), so either handle a stored pipeline answers to
//! — its name or its ARN — addresses it, and the handle is resolved only within the region
//! the request was signed for: a handle that names no pipeline THERE is the modelled 404
//! `ResourceNotFoundException`, carrying this service's own sentence under `Message` with
//! the identifier the request carried as `ResourceId`, and the mutation changes nothing.
//!
//! What the two operations answer is therefore decided by what the store holds and by
//! nothing else — the authorization pre-check in front of them
//! (`tests/all/front_door_authorization.rs`) is independent of the handle, so an admitted
//! caller gets these modelled answers whichever form it sent.
//!
//! The suites beside this one cover each operation on its own
//! (`tests/all/delete_telemetry_pipeline.rs`, `tests/all/telemetry_pipeline_updates.rs`);
//! what this table adds is the PAIR crossed with both handles and with a region that is not
//! the caller's, plus the round trip through `GetTelemetryPipeline` for each accepted
//! mutation.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`, restJson1
//! serialization, the modelled-constraint walk and the handlers — through the ONE harness
//! this test crate has (`telemetry_rules`).

use serde_json::{json, Value};
use service_com_amazonaws_observabilityadmin_observabilityadmin::state::{
    pipeline_arn, ComAmazonawsObservabilityadminObservabilityadminState, PIPELINE_NOT_FOUND_MESSAGE,
    PIPELINE_RESOURCE_TYPE,
};

use crate::telemetry_rules::{call, emulator, error_code, Emulator, ACCOUNT};

/// The region the requests below sign for.
const HOME_REGION: &str = "us-west-2";

/// A second region, so a handle that names a pipeline outside the caller's own endpoint
/// can be shown to resolve to nothing.
const OTHER_REGION: &str = "eu-west-1";

/// The document a pipeline is created with.
const CONFIGURATION: &str = "source:\n  http: {}\nsink:\n  - cloudwatch_logs: {}\n";

/// The document an accepted update stores, so the round trip can tell the two apart.
const UPDATED_CONFIGURATION: &str =
    "source:\n  http: {}\nprocessor:\n  - parse_json: {}\nsink:\n  - cloudwatch_logs: {}\n";

/// Which of the two handles a row addresses the pipeline by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Handle {
    Name,
    Arn,
}

/// Which mutation a row drives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mutation {
    Delete,
    Update,
}

impl Mutation {
    fn uri(self) -> &'static str {
        match self {
            Self::Delete => "/DeleteTelemetryPipeline",
            Self::Update => "/UpdateTelemetryPipeline",
        }
    }

    /// The request body: an update carries the required `Configuration` beside the
    /// identifier, a delete carries the identifier alone.
    fn body(self, identifier: &str) -> Value {
        match self {
            Self::Delete => json!({ "PipelineIdentifier": identifier }),
            Self::Update => json!({
                "PipelineIdentifier": identifier,
                "Configuration": { "Body": UPDATED_CONFIGURATION },
            }),
        }
    }
}

const MUTATIONS: [Mutation; 2] = [Mutation::Delete, Mutation::Update];
const HANDLES: [Handle; 2] = [Handle::Name, Handle::Arn];

async fn create(emulator: &Emulator, region: &str, name: &str) -> String {
    let (status, created) = call(
        emulator,
        region,
        "/CreateTelemetryPipeline",
        &json!({ "Name": name, "Configuration": { "Body": CONFIGURATION } }),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: CreateTelemetryPipeline({name}) in {region} must answer 200, got {created:?}"
    );
    created
        .get("Arn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("setup: the create reports the minted Arn, got {created:?}"))
        .to_string()
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

/// The configuration body the store holds for `identifier` in `region`, asked of the store
/// itself so "nothing changed" is not inferred from a read path.
fn stored_configuration(emulator: &Emulator, region: &str, identifier: &str) -> Option<String> {
    let state = emulator
        .engine
        .state::<ComAmazonawsObservabilityadminObservabilityadminState>();
    let store = state.lock();
    store
        .pipeline(region, identifier)
        .map(|record| record.configuration_body.clone())
}

/// RULE: either handle a stored pipeline answers to addresses it, so each mutation is
/// served through both — and what it did is observable through `GetTelemetryPipeline`.
#[tokio::test]
async fn each_mutation_is_served_through_either_handle() {
    for mutation in MUTATIONS {
        for handle in HANDLES {
            let emulator = emulator();
            let name = "handled-pipeline";
            let arn = create(&emulator, HOME_REGION, name).await;
            let case = format!("{mutation:?} by {handle:?}");
            let identifier = match handle {
                Handle::Name => name.to_string(),
                Handle::Arn => arn.clone(),
            };

            let (status, answered) =
                call(&emulator, HOME_REGION, mutation.uri(), &mutation.body(&identifier)).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the modelled 200, got {status} {answered:?}"
            );

            match mutation {
                Mutation::Update => {
                    assert_eq!(
                        stored_configuration(&emulator, HOME_REGION, &arn).as_deref(),
                        Some(UPDATED_CONFIGURATION),
                        "case {case:?}: the update's body is what the store holds"
                    );
                    let (status, read) = get(&emulator, HOME_REGION, name).await;
                    assert_eq!(
                        status, 200,
                        "case {case:?}: the pipeline is still readable, got {read:?}"
                    );
                    assert_eq!(
                        read.get("Pipeline")
                            .and_then(|pipeline| pipeline.get("Configuration"))
                            .and_then(|configuration| configuration.get("Body"))
                            .and_then(Value::as_str),
                        Some(UPDATED_CONFIGURATION),
                        "case {case:?}: the accepted body is what a read reports, got {read:?}"
                    );
                }
                Mutation::Delete => {
                    assert_eq!(
                        stored_configuration(&emulator, HOME_REGION, &arn),
                        None,
                        "case {case:?}: the record is gone from the store"
                    );
                    // Both handles stop resolving together.
                    for gone in [name.to_string(), arn.clone()] {
                        let (status, read) = get(&emulator, HOME_REGION, &gone).await;
                        assert_eq!(
                            status, 404,
                            "case {case:?}: expected 404 for {gone:?} after the delete, \
                             got {status} {read:?}"
                        );
                    }
                }
            }
        }
    }
}

/// RULE: a handle that names no pipeline in the region the request was signed for is the
/// modelled 404 with this service's own sentence, and the mutation changes nothing.
///
/// The rows span what the rule generalises over: a name nothing created, the name of a
/// pipeline that lives in ANOTHER region, that pipeline's own ARN, an ARN differing from
/// the stored one in exactly one field (region, account, partition, resource type), and an
/// ARN whose id names nothing.
#[tokio::test]
async fn a_handle_naming_no_pipeline_of_this_region_is_a_modelled_404() {
    let emulator = emulator();

    // One pipeline in each region, sharing a name, so a refusal cannot be mistaken for an
    // empty store.
    let shared_name = "survivor-pipeline";
    let home_arn = create(&emulator, HOME_REGION, shared_name).await;
    let other_arn = create(&emulator, OTHER_REGION, shared_name).await;
    let id = home_arn
        .rsplit('/')
        .next()
        .expect("a pipeline ARN carries its id")
        .to_string();

    let names_nothing: Vec<(&str, String)> = vec![
        ("a name nothing ever created", "no-such-pipeline".to_string()),
        (
            "a name whose pipeline lives in another region",
            format!("{shared_name}-elsewhere"),
        ),
        (
            "the ARN of the pipeline in the other region",
            other_arn.clone(),
        ),
        (
            "an ARN differing only in its region",
            pipeline_arn("us-east-1", &id),
        ),
        (
            "an ARN differing only in its account",
            format!("arn:aws:observabilityadmin:{HOME_REGION}:210987654321:telemetry-pipeline/{id}"),
        ),
        (
            "an ARN differing only in its partition",
            format!(
                "arn:aws-us-gov:observabilityadmin:{HOME_REGION}:{ACCOUNT}:telemetry-pipeline/{id}"
            ),
        ),
        (
            "an ARN differing only in its resource type",
            format!("arn:aws:observabilityadmin:{HOME_REGION}:{ACCOUNT}:telemetry-rule/{id}"),
        ),
        (
            "an ARN whose id names no pipeline",
            pipeline_arn(HOME_REGION, "0123456789abcdef0123456789abcdef"),
        ),
    ];

    for mutation in MUTATIONS {
        for (row, identifier) in &names_nothing {
            let case = format!("{mutation:?} by {row}");
            let (status, refused) =
                call(&emulator, HOME_REGION, mutation.uri(), &mutation.body(identifier)).await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected the modelled 404, got {status} {refused:?}"
            );
            assert_eq!(
                error_code(&refused),
                "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {refused:?}"
            );
            assert_eq!(
                refused.get("Message").and_then(Value::as_str),
                Some(PIPELINE_NOT_FOUND_MESSAGE),
                "case {case:?}: the refusal carries this service's own sentence under its \
                 capitalised member, got {refused:?}"
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

            // Both pipelines are untouched, each in its own region, with the configuration
            // their creates wrote.
            for (region, arn) in [(HOME_REGION, &home_arn), (OTHER_REGION, &other_arn)] {
                assert_eq!(
                    stored_configuration(&emulator, region, arn).as_deref(),
                    Some(CONFIGURATION),
                    "case {case:?}: the refused mutation must leave the pipeline in {region} \
                     exactly as it was"
                );
            }
        }
    }
}
