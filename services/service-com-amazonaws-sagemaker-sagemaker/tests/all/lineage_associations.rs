// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for SageMaker's ML Lineage associations: `AddAssociation` and
//! the `DeleteAssociation` that releases one.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so
//! what these tests exercise is what a real SDK client gets.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call:
//!
//! * an association is an EDGE between two lineage entities the service HOLDS, so it is
//!   recorded for every pair of stored entity kinds and every modelled edge type, and
//!   persists until `DeleteAssociation` releases it;
//! * an end naming no stored entity — an ARN nothing created, an entity kind whose
//!   create is not implemented, another Region, account or partition, a deleted entity —
//!   is refused with the modelled `ResourceNotFound` and stores nothing;
//! * an entity CAN be the source and the destination of one association: neither end
//!   carries a "must differ" constraint, so a self-loop is recorded like any other edge;
//! * `AssociationType`, when sent, is one member of the modelled `AssociationEdgeType`
//!   enum; and
//! * the pair is the association's identity, so adding it twice leaves ONE edge.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The one `message` reader this crate's suites share, so the recorded refusal sentence is
/// extracted the same way here as everywhere else.
use crate::inference_deployment_lifecycle::message;

/// The canonical account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The lineage entity kinds an `AssociationEntityArn` can name whose `Create*` this
/// emulator implements, i.e. the ends an association can really have.
///
/// Source: `AssociationEntityArn`'s modelled pattern
/// (`^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:(experiment|experiment-trial-component|artifact|action|context)/`)
/// intersected with the operations this crate implements — `CreateArtifact`,
/// `CreateAction`, `CreateTrialComponent`, `CreateContext` and `CreateExperiment` — every
/// kind the pattern admits, so each is covered with a STORED end as well as a not-found one.
const IMPLEMENTED_KINDS: &[&str] =
    &["artifact", "action", "experiment-trial-component", "context", "experiment"];

/// The entity kinds the ARN pattern admits but no implemented create mints, so no ARN of
/// one names a stored entity. Same source as [`IMPLEMENTED_KINDS`]: empty now that every
/// kind has a create, and kept so a kind added to the model ahead of its create has a home.
const UNIMPLEMENTED_KINDS: &[&str] = &[];

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
        self.dispatch(request).await
    }

    /// Call `target` unsigned: no credential scope, so the ARN region falls back to the
    /// emulator default (`state::DEFAULT_REGION`, `us-east-1`).
    async fn call_unsigned(&self, target: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("SageMaker.{target}"))
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        self.dispatch(request).await
    }

    async fn dispatch(&self, request: http::Request<bytes::Bytes>) -> (u16, Value) {
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

    async fn call(&self, target: &str, body: &Value) -> (u16, Value) {
        self.call_in("us-east-1", target, body).await
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

    async fn ok(&self, target: &str, body: &Value) -> Value {
        self.ok_in("us-east-1", target, body).await
    }
}

/// The `__type` an error body carries, with any `namespace#` prefix stripped.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

fn str_field(body: &Value, field: &str) -> String {
    body.get(field)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected a string {field} in {body}"))
        .to_string()
}

/// Create a lineage entity of `kind` in `region` and answer the ARN of it, so a table row
/// names an end that really exists. `label` is an `ExperimentEntityName`, so it must be
/// unique per case for the name-keyed kinds.
async fn create_entity(emulator: &Emulator, region: &str, kind: &str, label: &str) -> String {
    match kind {
        "artifact" => str_field(
            &emulator
                .ok_in(
                    region,
                    "CreateArtifact",
                    &json!({
                        "ArtifactName": label,
                        "Source": { "SourceUri": format!("s3://bucket/{label}.csv") },
                        "ArtifactType": "DataSet",
                    }),
                )
                .await,
            "ArtifactArn",
        ),
        "action" => str_field(
            &emulator
                .ok_in(
                    region,
                    "CreateAction",
                    &json!({
                        "ActionName": label,
                        "Source": { "SourceUri": format!("s3://bucket/{label}") },
                        "ActionType": "ModelDeployment",
                    }),
                )
                .await,
            "ActionArn",
        ),
        "experiment-trial-component" => str_field(
            &emulator
                .ok_in(
                    region,
                    "CreateTrialComponent",
                    &json!({ "TrialComponentName": label }),
                )
                .await,
            "TrialComponentArn",
        ),
        "context" => str_field(
            &emulator
                .ok_in(
                    region,
                    "CreateContext",
                    &json!({
                        "ContextName": label,
                        "Source": { "SourceUri": format!("s3://bucket/{label}") },
                        "ContextType": "Endpoint",
                    }),
                )
                .await,
            "ContextArn",
        ),
        "experiment" => str_field(
            &emulator
                .ok_in(region, "CreateExperiment", &json!({ "ExperimentName": label }))
                .await,
            "ExperimentArn",
        ),
        other => panic!("case {other}: no implemented create for this lineage entity kind"),
    }
}

/// An `AssociationEntityArn` of `kind` nothing created, in the shape this service mints.
fn unused_arn(region: &str, kind: &str, name: &str) -> String {
    format!("arn:aws:sagemaker:{region}:{ACCOUNT}:{kind}/{name}")
}

/// Whether the emulator holds the association `source` -> `destination`, decided by the
/// only reader of one this crate implements: `DeleteAssociation` releases a stored edge
/// with a 200 and refuses an absent one with `ResourceNotFound`. It CONSUMES the edge,
/// so a case that asserts persistence does it once.
async fn delete_association(
    emulator: &Emulator,
    region: &str,
    source: &str,
    destination: &str,
) -> (u16, Value) {
    emulator
        .call_in(
            region,
            "DeleteAssociation",
            &json!({ "SourceArn": source, "DestinationArn": destination }),
        )
        .await
}

/// Assert that no association `source` -> `destination` is stored.
async fn assert_no_association(
    emulator: &Emulator,
    region: &str,
    source: &str,
    destination: &str,
    case: &str,
) {
    let (status, parsed) = delete_association(emulator, region, source, destination).await;
    assert_eq!(
        status, 400,
        "case {case:?}: no association must be stored, got {status} with {parsed}"
    );
    assert_eq!(
        error_code(&parsed),
        "ResourceNotFound",
        "case {case:?}: expected ResourceNotFound for an absent association, got {parsed:?}"
    );
}

#[tokio::test]
async fn an_association_between_two_stored_entities_is_recorded_until_it_is_deleted() {
    let emulator = Emulator::new();

    // Rows differ along what the rule generalises over: the Region (so the ARN both ends
    // carry is the one the caller signed for), the KIND of each end — every ordered pair
    // of the implemented lineage entity kinds, including an entity kind pointing at its
    // own kind — and the `AssociationType` (absent, plus every member of the modelled
    // enum, cycled over the rows).
    let regions = ["us-east-1", "eu-west-2", "ap-southeast-1"];
    let mut row = 0usize;

    for region in regions {
        for source_kind in IMPLEMENTED_KINDS {
            for destination_kind in IMPLEMENTED_KINDS {
                // `None` for row 0, then each modelled edge type in turn.
                let association_type = if row.is_multiple_of(EDGE_TYPES.len() + 1) {
                    None
                } else {
                    Some(EDGE_TYPES[(row % (EDGE_TYPES.len() + 1)) - 1])
                };
                let case = format!(
                    "{region}/{source_kind}->{destination_kind}/type={association_type:?}"
                );

                let source = create_entity(&emulator, region, source_kind, &format!("src{row}")).await;
                let destination =
                    create_entity(&emulator, region, destination_kind, &format!("dst{row}")).await;

                let mut body = json!({ "SourceArn": source, "DestinationArn": destination });
                if let Some(association_type) = association_type {
                    body["AssociationType"] = json!(association_type);
                }

                let added = emulator.ok_in(region, "AddAssociation", &body).await;
                assert_eq!(
                    str_field(&added, "SourceArn"),
                    source,
                    "case {case:?}: the response echoes the source, got {added:?}"
                );
                assert_eq!(
                    str_field(&added, "DestinationArn"),
                    destination,
                    "case {case:?}: the response echoes the destination, got {added:?}"
                );

                // The edge is PERSISTED: the delete that releases it answers 200, which it
                // only does for an association the store holds.
                let (status, released) =
                    delete_association(&emulator, region, &source, &destination).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: the stored association must be releasable, got {status} \
                     with {released}"
                );
                assert_eq!(
                    (
                        str_field(&released, "SourceArn"),
                        str_field(&released, "DestinationArn")
                    ),
                    (source.clone(), destination.clone()),
                    "case {case:?}: the delete echoes both ends, got {released:?}"
                );

                // And it is GONE: releasing it twice is refused.
                assert_no_association(&emulator, region, &source, &destination, &case).await;

                row += 1;
            }
        }
    }
}

#[tokio::test]
async fn an_association_added_unsigned_lands_in_the_default_region() {
    let emulator = Emulator::new();

    // The unsigned fallback is its own row of the Region table above: no credential scope,
    // so both ends carry `us-east-1` and the association is stored against exactly those
    // ARNs.
    let source = str_field(
        &emulator
            .ok(
                "CreateArtifact",
                &json!({
                    "Source": { "SourceUri": "s3://bucket/unsigned-source.csv" },
                    "ArtifactType": "DataSet",
                }),
            )
            .await,
        "ArtifactArn",
    );
    let destination = str_field(
        &emulator
            .ok(
                "CreateAction",
                &json!({
                    "ActionName": "unsigned-destination",
                    "Source": { "SourceUri": "s3://bucket/unsigned-destination" },
                    "ActionType": "ModelDeployment",
                }),
            )
            .await,
        "ActionArn",
    );

    let body = json!({
        "SourceArn": source,
        "DestinationArn": destination,
        "AssociationType": "ContributedTo",
    });
    let (status, added) = emulator.call_unsigned("AddAssociation", &body).await;
    assert_eq!(
        status, 200,
        "case unsigned: expected HTTP 200, got {status} with {added}"
    );
    assert_eq!(
        str_field(&added, "DestinationArn"),
        destination,
        "case unsigned: the response echoes the destination, got {added:?}"
    );

    let (status, released) = emulator
        .call_unsigned(
            "DeleteAssociation",
            &json!({ "SourceArn": source, "DestinationArn": destination }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case unsigned: the stored association must be releasable, got {status} with \
         {released}"
    );
}

#[tokio::test]
async fn an_end_that_names_no_stored_lineage_entity_is_refused_with_resource_not_found() {
    let emulator = Emulator::new();

    let present = create_entity(&emulator, "us-east-1", "artifact", "present").await;
    let elsewhere = create_entity(&emulator, "eu-west-2", "artifact", "elsewhere").await;

    // An action created and then deleted: its ARN is well formed and was once real, so
    // the rule is about what the store HOLDS, not about the ARN's shape.
    let deleted = create_entity(&emulator, "us-east-1", "action", "goneaction").await;
    emulator
        .ok("DeleteAction", &json!({ "ActionName": "goneaction" }))
        .await;

    // Rows differ along WHICH end is unresolvable and WHY: never created, a kind whose
    // create is not implemented (so nothing of that kind can exist), another Region,
    // another account, another partition, and an entity that was deleted.
    let mut cases: Vec<(String, String, String)> = vec![
        (
            "both-ends-never-created".to_string(),
            unused_arn("us-east-1", "artifact", &"0".repeat(32)),
            unused_arn("us-east-1", "artifact", &"1".repeat(32)),
        ),
        (
            "source-never-created".to_string(),
            unused_arn("us-east-1", "artifact", &"2".repeat(32)),
            present.clone(),
        ),
        (
            "destination-never-created".to_string(),
            present.clone(),
            unused_arn("us-east-1", "action", "nosuchaction"),
        ),
        (
            "trial-component-never-created".to_string(),
            present.clone(),
            unused_arn("us-east-1", "experiment-trial-component", "nosuchcomponent"),
        ),
        (
            "entity-in-another-region".to_string(),
            elsewhere.clone(),
            present.clone(),
        ),
        (
            "entity-in-another-account".to_string(),
            present.clone(),
            format!("arn:aws:sagemaker:us-east-1:999999999999:artifact/{}", "3".repeat(32)),
        ),
        (
            "entity-in-another-partition".to_string(),
            format!("arn:aws-cn:sagemaker:us-east-1:{ACCOUNT}:artifact/{}", "4".repeat(32)),
            present.clone(),
        ),
        (
            "deleted-entity-as-source".to_string(),
            deleted.clone(),
            present.clone(),
        ),
        (
            "deleted-entity-as-destination".to_string(),
            present.clone(),
            deleted.clone(),
        ),
    ];
    // Each kind the ARN pattern admits but nothing can create: it names no entity at
    // either end.
    for kind in UNIMPLEMENTED_KINDS {
        cases.push((
            format!("{kind}-kind-has-no-create"),
            unused_arn("us-east-1", kind, &format!("{kind}-one")),
            present.clone(),
        ));
        cases.push((
            format!("{kind}-kind-has-no-create-as-destination"),
            present.clone(),
            unused_arn("us-east-1", kind, &format!("{kind}-two")),
        ));
    }

    for (case, source, destination) in cases {
        let (status, parsed) = emulator
            .call(
                "AddAssociation",
                &json!({
                    "SourceArn": source,
                    "DestinationArn": destination,
                    "AssociationType": "ContributedTo",
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: an end naming no stored entity must be refused, got {status} \
             with {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ResourceNotFound",
            "case {case:?}: expected ResourceNotFound, got {parsed:?}"
        );
        // The refusal is the lineage family's recorded sentence, naming the ARN of the
        // FIRST end the request could not resolve — the source unless it is the stored
        // one — in single quotes, and naming the end generically because an
        // `AssociationEntityArn` admits five entity kinds and an unresolvable one names
        // no kind to report.
        let unresolved = if source == present {
            &destination
        } else {
            &source
        };
        assert_eq!(
            message(&parsed),
            format!("Resource '{unresolved}' does not exist."),
            "case {case:?}: refusal message, got {parsed:?}"
        );
        // The refusal stored nothing.
        assert_no_association(&emulator, "us-east-1", &source, &destination, &case).await;
    }
}

#[tokio::test]
async fn an_entity_can_be_the_source_and_the_destination_of_one_association() {
    let emulator = Emulator::new();

    // Rows differ along the entity kind of the self-referencing end, and along the
    // `AssociationType` the request carries (absent, plus a modelled member cycled over
    // the kinds): the two ends carry no "must differ" constraint, so a self-loop is an
    // edge like any other — whatever kind it is between, and whatever type it carries.
    for (row, kind) in IMPLEMENTED_KINDS.iter().enumerate() {
        let arn = create_entity(
            &emulator,
            "us-east-1",
            kind,
            &format!("self{}", kind.replace('-', "")),
        )
        .await;
        let association_type = if row == 0 {
            None
        } else {
            Some(EDGE_TYPES[row % EDGE_TYPES.len()])
        };
        let case = format!("stored-{kind}/type={association_type:?}");

        let mut body = json!({ "SourceArn": arn, "DestinationArn": arn });
        if let Some(association_type) = association_type {
            body["AssociationType"] = json!(association_type);
        }
        let (status, parsed) = emulator.call("AddAssociation", &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a self-referencing edge must be recorded, got {status} with \
             {parsed}"
        );
        assert_eq!(
            (
                parsed.get("SourceArn").and_then(Value::as_str),
                parsed.get("DestinationArn").and_then(Value::as_str),
            ),
            (Some(arn.as_str()), Some(arn.as_str())),
            "case {case:?}: both ends must answer the entity's own ARN, got {parsed:?}"
        );

        // The edge is really stored: the only reader of one this crate implements
        // releases it with a 200, and finds nothing the second time.
        let (status, released) = delete_association(&emulator, "us-east-1", &arn, &arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the stored self-association must be releasable, got {status} \
             with {released}"
        );
        assert_no_association(&emulator, "us-east-1", &arn, &arn, &case).await;
    }

    // An end that names no stored entity is still refused, self-referencing or not: the
    // rule that was removed is the inequality one, not the existence one.
    for (case, arn) in [
        (
            "never-created-artifact",
            unused_arn("us-east-1", "artifact", &"5".repeat(32)),
        ),
        (
            "never-created-context",
            unused_arn("us-east-1", "context", "uncreatedcontext"),
        ),
    ] {
        let (status, parsed) = emulator
            .call(
                "AddAssociation",
                &json!({ "SourceArn": arn, "DestinationArn": arn }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: an end naming no stored entity must be refused, got {status} \
             with {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ResourceNotFound",
            "case {case:?}: expected ResourceNotFound, got {parsed:?}"
        );
        assert_eq!(
            message(&parsed),
            format!("Resource '{arn}' does not exist."),
            "case {case:?}: refusal message, got {parsed:?}"
        );
        assert_no_association(&emulator, "us-east-1", &arn, &arn, case).await;
    }
}

#[tokio::test]
async fn association_type_must_be_a_member_of_the_modelled_enum() {
    let emulator = Emulator::new();

    let source = create_entity(&emulator, "us-east-1", "artifact", "typesource").await;
    let destination = create_entity(&emulator, "us-east-1", "action", "typedestination").await;

    // Every modelled member is accepted — including `SameAs`, which the issue's own case
    // never names — and the edge it records is released again so the next row starts from
    // the same store.
    for association_type in EDGE_TYPES {
        let body = json!({
            "SourceArn": source,
            "DestinationArn": destination,
            "AssociationType": association_type,
        });
        let (status, parsed) = emulator.call("AddAssociation", &body).await;
        assert_eq!(
            status, 200,
            "case {association_type:?}: a modelled edge type must be accepted, got {status} \
             with {parsed}"
        );
        let (status, released) =
            delete_association(&emulator, "us-east-1", &source, &destination).await;
        assert_eq!(
            status, 200,
            "case {association_type:?}: the stored association must be releasable, got \
             {status} with {released}"
        );
    }

    // Rows differ along HOW the value misses the enum: wrong case, padded, empty, a
    // plausible-but-absent member, and a member of a DIFFERENT enum of this service.
    for association_type in [
        "associatedwith",
        "ASSOCIATEDWITH",
        "Produced ",
        "",
        "Unknown",
        "CreationTime",
    ] {
        let (status, parsed) = emulator
            .call(
                "AddAssociation",
                &json!({
                    "SourceArn": source,
                    "DestinationArn": destination,
                    "AssociationType": association_type,
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {association_type:?}: a value outside the modelled enum must be refused, \
             got {status} with {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ValidationException",
            "case {association_type:?}: expected ValidationException, got {parsed:?}"
        );
        assert_no_association(
            &emulator,
            "us-east-1",
            &source,
            &destination,
            association_type,
        )
        .await;
    }
}

#[tokio::test]
async fn the_pair_is_the_associations_identity_so_adding_it_twice_leaves_one_edge() {
    let emulator = Emulator::new();

    let source = create_entity(&emulator, "us-east-1", "artifact", "twinsource").await;
    let destination = create_entity(&emulator, "us-east-1", "artifact", "twindestination").await;

    // Rows: the same pair added again with no type, with the same type and with a
    // different type. Whatever the edge type, the pair identifies ONE association.
    for association_type in [None, Some("ContributedTo"), Some("ContributedTo"), Some("SameAs")] {
        let mut body = json!({ "SourceArn": source, "DestinationArn": destination });
        if let Some(association_type) = association_type {
            body["AssociationType"] = json!(association_type);
        }
        let (status, parsed) = emulator.call("AddAssociation", &body).await;
        assert_eq!(
            status, 200,
            "case {association_type:?}: re-adding the pair must be accepted, got {status} \
             with {parsed}"
        );
    }

    // Exactly one edge was filed: the first release succeeds, the second finds nothing.
    let (status, released) = delete_association(&emulator, "us-east-1", &source, &destination).await;
    assert_eq!(
        status, 200,
        "case four-adds: the one stored association must be releasable, got {status} with \
         {released}"
    );
    assert_no_association(
        &emulator,
        "us-east-1",
        &source,
        &destination,
        "four-adds-then-one-delete",
    )
    .await;

    // The reverse edge is a DIFFERENT association: the pair is ordered.
    let body = json!({ "SourceArn": destination, "DestinationArn": source });
    let (status, parsed) = emulator.call("AddAssociation", &body).await;
    assert_eq!(
        status, 200,
        "case reversed: the reverse edge is its own association, got {status} with {parsed}"
    );
    assert_no_association(
        &emulator,
        "us-east-1",
        &source,
        &destination,
        "reversed-leaves-the-forward-edge-absent",
    )
    .await;
    let (status, released) = delete_association(&emulator, "us-east-1", &destination, &source).await;
    assert_eq!(
        status, 200,
        "case reversed: the reverse edge must be releasable, got {status} with {released}"
    );
}

#[tokio::test]
async fn requests_outside_the_modelled_constraints_are_refused() {
    let emulator = Emulator::new();

    let present = create_entity(&emulator, "us-east-1", "artifact", "constraintartifact").await;

    // Rows: one past each published bound of the members these operations read —
    // `AssociationEntityArn` is len 0..256 and must match the lineage-entity ARN pattern,
    // and both ARNs are required.
    let too_long = format!(
        "arn:aws:sagemaker:us-east-1:{ACCOUNT}:artifact/{}",
        "a".repeat(256)
    );
    let cases: Vec<(&str, Value)> = vec![
        (
            "source-is-not-an-arn",
            json!({ "SourceArn": "not-an-arn", "DestinationArn": present }),
        ),
        (
            "destination-is-not-a-lineage-entity-kind",
            json!({
                "SourceArn": present,
                "DestinationArn": format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:model/m"),
            }),
        ),
        (
            "source-longer-than-256",
            json!({ "SourceArn": too_long, "DestinationArn": present }),
        ),
        ("source-missing", json!({ "DestinationArn": present })),
        ("destination-missing", json!({ "SourceArn": present })),
    ];

    for (case, body) in cases {
        for target in ["AddAssociation", "DeleteAssociation"] {
            let (status, parsed) = emulator.call(target, &body).await;
            assert_eq!(
                status, 400,
                "case {target}/{case:?}: a request outside the modelled constraints must be \
                 refused, got {status} with {parsed}"
            );
            assert_eq!(
                error_code(&parsed),
                "ValidationException",
                "case {target}/{case:?}: expected ValidationException, got {parsed:?}"
            );
        }
    }
}
