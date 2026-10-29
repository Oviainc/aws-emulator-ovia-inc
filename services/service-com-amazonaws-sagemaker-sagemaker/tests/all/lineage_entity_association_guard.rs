// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for the referential-integrity guard SageMaker's ML Lineage
//! entity deletes apply: an entity that is still the SOURCE of an association the account
//! holds cannot be deleted, and the refusal changes nothing.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so
//! what these tests exercise is what a real SDK client gets.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call:
//!
//! * a lineage entity delete is refused with HTTP 400 `ValidationException` while any
//!   association the account holds names that entity as its source, whichever entity kind
//!   it is, whichever identifier form addresses it and however many edges start at it;
//! * the refusal leaves the store exactly as it was — the entity is still readable and
//!   every edge is still listed, so nothing is cascaded;
//! * the refusal's message is FIXED text, interpolating neither the entity's name nor its
//!   ARN; and
//! * once every outgoing edge is released by `DeleteAssociation` the delete succeeds, as
//!   it does for an entity that only ever was an association's DESTINATION, or was never
//!   an end of one at all.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The error-body readers this crate's suites share, so a refusal's code and sentence are
/// extracted the same way here as everywhere else.
use crate::inference_deployment_lifecycle::{error_code, message, str_field};

/// The sentence SageMaker refuses the delete of an entity with a live outgoing association
/// with. Entirely fixed text: no entity name, ARN or edge count appears in it.
const GUARD_MESSAGE: &str = "Cannot delete entity with associations.";

/// The lineage entity kinds an `AssociationEntityArn` can name whose create AND delete this
/// crate implements — the entities whose delete can meet a live association.
///
/// Source: `AssociationEntityArn`'s modelled pattern
/// (`^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:(experiment|experiment-trial-component|artifact|action|context)/`,
/// `smithy_query(query="shape",shape="AssociationEntityArn")`) intersected with the deletes
/// this crate implements: `DeleteArtifact`, `DeleteAction`, `DeleteContext`,
/// `DeleteTrialComponent` and `DeleteExperiment` — every kind the pattern admits.
const DELETABLE_KINDS: &[&str] = &[
    "artifact",
    "action",
    "context",
    "experiment-trial-component",
    "experiment",
];

/// Every member of the modelled `AssociationEdgeType` enum, in the order the model lists
/// them (`smithy_query(query="shape", shape="AssociationEdgeType")`, mirrored by the
/// generated `types::ASSOCIATION_EDGE_TYPE_VALUES`).
const EDGE_TYPES: &[&str] = &[
    "AssociatedWith",
    "ContributedTo",
    "DerivedFrom",
    "Produced",
    "SameAs",
];

/// One emulator: the shared engine plus the SageMaker service mounted on the router.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These tests exercise handler behaviour, not SigV4; the request below carries a
        // credential scope only so the ARN region is the one the caller signed for.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_sagemaker_sagemaker::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// Call `target` with a SigV4 credential scope naming `region`.
    async fn call_in(&self, region: &str, target: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("SageMaker.{target}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/sagemaker/\
                     aws4_request, SignedHeaders=host;x-amz-target, Signature=0000"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = response.body();
        let parsed = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(body).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Call `target` in `region` and require a 200, returning the parsed output.
    async fn ok_in(&self, region: &str, target: &str, body: &Value) -> Value {
        let (status, parsed) = self.call_in(region, target, body).await;
        assert_eq!(
            status, 200,
            "case {target} in {region}: expected HTTP 200, got {status} with {parsed}"
        );
        parsed
    }
}

/// One lineage entity a case works with: the ARN its create minted, the name the
/// name-addressed operations spell it by and the source URI it was created from.
#[derive(Clone, Debug)]
struct Entity {
    kind: &'static str,
    name: String,
    arn: String,
    source_uri: String,
}

/// Create a lineage entity of `kind` in `region`, so a table row names an end that really
/// exists. `label` is an `ExperimentEntityName`, so it must be unique per case for the
/// name-keyed kinds.
async fn create_entity(
    emulator: &Emulator,
    region: &str,
    kind: &'static str,
    label: &str,
) -> Entity {
    let source_uri = format!("s3://bucket/{label}");
    let (target, body, arn_member) = match kind {
        "artifact" => (
            "CreateArtifact",
            json!({
                "ArtifactName": label,
                "Source": { "SourceUri": source_uri },
                "ArtifactType": "DataSet",
            }),
            "ArtifactArn",
        ),
        "action" => (
            "CreateAction",
            json!({
                "ActionName": label,
                "Source": { "SourceUri": source_uri },
                "ActionType": "ModelDeployment",
            }),
            "ActionArn",
        ),
        "context" => (
            "CreateContext",
            json!({
                "ContextName": label,
                "Source": { "SourceUri": source_uri, "SourceType": "Endpoint" },
                "ContextType": "Endpoint",
            }),
            "ContextArn",
        ),
        "experiment-trial-component" => (
            "CreateTrialComponent",
            json!({ "TrialComponentName": label }),
            "TrialComponentArn",
        ),
        "experiment" => (
            "CreateExperiment",
            json!({ "ExperimentName": label }),
            "ExperimentArn",
        ),
        other => panic!("case {other}: no implemented create for this lineage entity kind"),
    };
    let created = emulator.ok_in(region, target, &body).await;
    Entity {
        kind,
        name: label.to_string(),
        arn: str_field(&created, arn_member),
        source_uri,
    }
}

/// The delete request that releases `entity`: the operation name and the body addressing it
/// by the identifier its own input member takes.
fn delete_request(entity: &Entity) -> (&'static str, Value) {
    match entity.kind {
        "artifact" => ("DeleteArtifact", json!({ "ArtifactArn": entity.arn })),
        "action" => ("DeleteAction", json!({ "ActionName": entity.name })),
        "context" => ("DeleteContext", json!({ "ContextName": entity.name })),
        "experiment-trial-component" => (
            "DeleteTrialComponent",
            json!({ "TrialComponentName": entity.name }),
        ),
        "experiment" => (
            "DeleteExperiment",
            json!({ "ExperimentName": entity.name }),
        ),
        other => panic!("case {other}: no implemented delete for this lineage entity kind"),
    }
}

/// The read that reports `entity`, so a case can tell a refused delete (the entity is still
/// there) from a performed one.
fn describe_request(entity: &Entity) -> (&'static str, Value) {
    match entity.kind {
        "artifact" => ("DescribeArtifact", json!({ "ArtifactArn": entity.arn })),
        "action" => ("DescribeAction", json!({ "ActionName": entity.name })),
        "context" => ("DescribeContext", json!({ "ContextName": entity.name })),
        "experiment-trial-component" => (
            "DescribeTrialComponent",
            json!({ "TrialComponentName": entity.name }),
        ),
        "experiment" => (
            "DescribeExperiment",
            json!({ "ExperimentName": entity.name }),
        ),
        other => panic!("case {other}: no implemented reader for this lineage entity kind"),
    }
}

async fn delete_entity(emulator: &Emulator, region: &str, entity: &Entity) -> (u16, Value) {
    let (target, body) = delete_request(entity);
    emulator.call_in(region, target, &body).await
}

async fn describe_entity(emulator: &Emulator, region: &str, entity: &Entity) -> (u16, Value) {
    let (target, body) = describe_request(entity);
    emulator.call_in(region, target, &body).await
}

/// How many associations `region` holds whose source is `source_arn`, as `ListAssociations`
/// reports them.
async fn outgoing_edges(emulator: &Emulator, region: &str, source_arn: &str) -> usize {
    let listed = emulator
        .ok_in(
            region,
            "ListAssociations",
            &json!({ "SourceArn": source_arn, "MaxResults": 100 }),
        )
        .await;
    listed
        .get("AssociationSummaries")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected AssociationSummaries in {listed}"))
        .len()
}

/// Assert that `status`/`body` is the fixed referential-integrity refusal, and that the
/// sentence interpolates neither the entity's name nor its ARN.
fn assert_guard_refusal(case: &str, entity: &Entity, status: u16, body: &Value) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected the delete to be refused with HTTP 400, got {status} \
         with {body}"
    );
    assert_eq!(
        error_code(body),
        "ValidationException",
        "case {case:?}: expected error code ValidationException, got {body:?}"
    );
    let sentence = message(body);
    assert_eq!(
        sentence, GUARD_MESSAGE,
        "case {case:?}: expected the fixed refusal sentence {GUARD_MESSAGE:?}, got \
         {sentence:?}"
    );
    assert!(
        !sentence.contains(&entity.name) && !sentence.contains(&entity.arn),
        "case {case:?}: the refusal sentence is fixed text and names no entity, got \
         {sentence:?}"
    );
}

#[tokio::test]
async fn an_entity_sourcing_an_association_is_refused_until_the_edge_is_released() {
    let emulator = Emulator::new();

    // Rows differ along what the rule generalises over: the Region the entities were
    // created in, the KIND of the entity being deleted and the kind at the other end of
    // the edge (every ordered pair of the deletable kinds, a kind pointing at its own kind
    // included), and the `AssociationType` — absent, then every member of the modelled
    // enum, cycled over the rows.
    let mut row = 0usize;
    for region in ["us-east-1", "eu-west-2"] {
        for source_kind in DELETABLE_KINDS {
            for destination_kind in DELETABLE_KINDS {
                let association_type = if row.is_multiple_of(EDGE_TYPES.len() + 1) {
                    None
                } else {
                    Some(EDGE_TYPES[(row % (EDGE_TYPES.len() + 1)) - 1])
                };
                let case = format!(
                    "{region}/{source_kind}->{destination_kind}/type={association_type:?}"
                );

                let source =
                    create_entity(&emulator, region, source_kind, &format!("src{row}")).await;
                let destination =
                    create_entity(&emulator, region, destination_kind, &format!("dst{row}"))
                        .await;
                row += 1;

                let mut add = json!({
                    "SourceArn": source.arn,
                    "DestinationArn": destination.arn,
                });
                if let Some(association_type) = association_type {
                    add["AssociationType"] = json!(association_type);
                }
                emulator.ok_in(region, "AddAssociation", &add).await;

                // The entity is the SOURCE of a live edge, so its delete is refused.
                let (status, body) = delete_entity(&emulator, region, &source).await;
                assert_guard_refusal(&case, &source, status, &body);

                // The refusal changed NOTHING: the edge is still listed and the entity is
                // still readable, so no caller can observe a half-performed delete.
                let edges = outgoing_edges(&emulator, region, &source.arn).await;
                assert_eq!(
                    edges, 1,
                    "case {case:?}: the association must survive the refusal, got {edges} \
                     edges out of the source"
                );
                let (described, read_back) = describe_entity(&emulator, region, &source).await;
                assert_eq!(
                    described, 200,
                    "case {case:?}: the refused entity must still be readable, got \
                     {described} with {read_back}"
                );

                // Releasing the edge clears the condition, and only then does the delete
                // go through — the ordering a client relying on the refusal depends on.
                emulator
                    .ok_in(
                        region,
                        "DeleteAssociation",
                        &json!({
                            "SourceArn": source.arn,
                            "DestinationArn": destination.arn,
                        }),
                    )
                    .await;
                let (status, body) = delete_entity(&emulator, region, &source).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: the delete must succeed once the edge is released, got \
                     {status} with {body}"
                );

                // The delete really happened: the entity's own reader no longer finds it.
                let (described, read_back) = describe_entity(&emulator, region, &source).await;
                assert_eq!(
                    described, 400,
                    "case {case:?}: the deleted entity must be gone, got {described} with \
                     {read_back}"
                );
                assert_eq!(
                    error_code(&read_back),
                    "ResourceNotFound",
                    "case {case:?}: expected ResourceNotFound for the deleted entity, got \
                     {read_back:?}"
                );

                // The entity at the other end was never touched by any of this.
                let (described, read_back) = describe_entity(&emulator, region, &destination).await;
                assert_eq!(
                    described, 200,
                    "case {case:?}: the destination entity must be untouched, got \
                     {described} with {read_back}"
                );
            }
        }
    }
}

#[tokio::test]
async fn an_entity_is_refused_until_every_one_of_its_outgoing_edges_is_released() {
    let emulator = Emulator::new();
    let region = "ap-southeast-1";

    // Rows differ along the entity kind and along the COUNT of edges starting at it: the
    // rule is about any edge, so one left behind still refuses the delete.
    let mut row = 0usize;
    for kind in DELETABLE_KINDS {
        for edge_count in 1..=3usize {
            let case = format!("{kind}/edges={edge_count}");
            let source = create_entity(&emulator, region, kind, &format!("multi{row}")).await;

            let mut destinations = Vec::new();
            for edge in 0..edge_count {
                let destination = create_entity(
                    &emulator,
                    region,
                    DELETABLE_KINDS[edge % DELETABLE_KINDS.len()],
                    &format!("multidst{row}x{edge}"),
                )
                .await;
                emulator
                    .ok_in(
                        region,
                        "AddAssociation",
                        &json!({
                            "SourceArn": source.arn,
                            "DestinationArn": destination.arn,
                            "AssociationType": EDGE_TYPES[edge % EDGE_TYPES.len()],
                        }),
                    )
                    .await;
                destinations.push(destination);
            }
            row += 1;

            // Released one at a time: the delete stays refused while any edge remains.
            for (released, destination) in destinations.iter().enumerate() {
                let remaining = edge_count - released;
                let step = format!("{case}/remaining={remaining}");
                let edges = outgoing_edges(&emulator, region, &source.arn).await;
                assert_eq!(
                    edges, remaining,
                    "case {step:?}: expected {remaining} edges out of the source, got {edges}"
                );

                let (status, body) = delete_entity(&emulator, region, &source).await;
                assert_guard_refusal(&step, &source, status, &body);

                emulator
                    .ok_in(
                        region,
                        "DeleteAssociation",
                        &json!({
                            "SourceArn": source.arn,
                            "DestinationArn": destination.arn,
                        }),
                    )
                    .await;
            }

            let (status, body) = delete_entity(&emulator, region, &source).await;
            assert_eq!(
                status, 200,
                "case {case:?}: with every edge released the delete must succeed, got \
                 {status} with {body}"
            );
        }
    }
}

#[tokio::test]
async fn an_entity_with_no_outgoing_edge_is_deleted() {
    let emulator = Emulator::new();
    let region = "us-west-2";

    // Rows differ along the entity kind and along WHY the entity has no outgoing edge: it
    // never was an end of one, or it is only an association's DESTINATION — the guard is
    // about the edges that START at the entity.
    let mut row = 0usize;
    for kind in DELETABLE_KINDS {
        for only_destination in [false, true] {
            let case = format!("{kind}/only_destination={only_destination}");
            let entity = create_entity(&emulator, region, kind, &format!("free{row}")).await;
            let other = if only_destination {
                let other =
                    create_entity(&emulator, region, "action", &format!("freesrc{row}")).await;
                emulator
                    .ok_in(
                        region,
                        "AddAssociation",
                        &json!({
                            "SourceArn": other.arn,
                            "DestinationArn": entity.arn,
                            "AssociationType": "ContributedTo",
                        }),
                    )
                    .await;
                Some(other)
            } else {
                None
            };
            row += 1;

            let (status, body) = delete_entity(&emulator, region, &entity).await;
            assert_eq!(
                status, 200,
                "case {case:?}: an entity with no outgoing edge must be deletable, got \
                 {status} with {body}"
            );
            let (described, read_back) = describe_entity(&emulator, region, &entity).await;
            assert_eq!(
                described, 400,
                "case {case:?}: the deleted entity must be gone, got {described} with \
                 {read_back}"
            );

            // Nothing was cascaded: the edge that pointed AT the deleted entity is still
            // the one its own source holds, and only `DeleteAssociation` releases it.
            if let Some(other) = other {
                let edges = outgoing_edges(&emulator, region, &other.arn).await;
                assert_eq!(
                    edges, 1,
                    "case {case:?}: the incoming edge must survive the delete, got {edges} \
                     edges out of its source"
                );
            }
        }
    }
}

#[tokio::test]
async fn the_guard_holds_whichever_identifier_form_addresses_the_artifact() {
    // `DeleteArtifact` resolves the artifact by `ArtifactArn` or by its `Source`'s URI, so
    // the guard is a property of the entity the request RESOLVED to and not of the form the
    // request took. Rows: both identifier forms.
    for by_arn in [false, true] {
        let emulator = Emulator::new();
        let region = "eu-central-1";
        let case = format!("artifact/by_arn={by_arn}");

        let artifact = create_entity(&emulator, region, "artifact", "guardedartifact").await;
        let destination = create_entity(&emulator, region, "context", "guardeddst").await;
        emulator
            .ok_in(
                region,
                "AddAssociation",
                &json!({
                    "SourceArn": artifact.arn,
                    "DestinationArn": destination.arn,
                }),
            )
            .await;

        let body = if by_arn {
            json!({ "ArtifactArn": artifact.arn })
        } else {
            json!({ "Source": { "SourceUri": artifact.source_uri } })
        };
        let (status, refused) = emulator.call_in(region, "DeleteArtifact", &body).await;
        assert_guard_refusal(&case, &artifact, status, &refused);

        let edges = outgoing_edges(&emulator, region, &artifact.arn).await;
        assert_eq!(
            edges, 1,
            "case {case:?}: the association must survive the refusal, got {edges} edges out \
             of the artifact"
        );
        let (described, read_back) = describe_entity(&emulator, region, &artifact).await;
        assert_eq!(
            described, 200,
            "case {case:?}: the refused artifact must still be readable, got {described} \
             with {read_back}"
        );
    }
}
