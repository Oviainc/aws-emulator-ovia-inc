// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for SageMaker's `ListAssociations`: the reporting side of the ML
//! Lineage association slice `AddAssociation` writes and `DeleteAssociation` releases.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so what
//! these tests exercise is what a real SDK client gets.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call:
//!
//! * a listing reports every stored edge of the Region the caller signed for, each summary
//!   carrying the stored pair, edge type, creation instant and creator, plus the NAME and
//!   TYPE of each end read out of the lineage entity that end's ARN names;
//! * each filter returns only the associations whose corresponding property equals the
//!   value the request names, and the two timestamp bounds keep the edges on their side of
//!   the instant;
//! * `SortBy` orders the result set by the property it names and `SortOrder` reverses it,
//!   with the identity pair as the tie-break either way;
//! * `MaxResults` plus `NextToken` walk the whole result set exactly once;
//! * a release makes the edge stop being reported, and a listing selecting nothing answers
//!   an EMPTY list rather than omitting the member; and
//! * a value outside a documented set is refused with HTTP 400 `ValidationException` and
//!   changes nothing.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::inference_deployment_lifecycle::{
    error_code, f64_field, str_field, Emulator, ACCOUNT, CREATE_SPACING,
};

/// Every member of the modelled `SortAssociationsBy` enum, in the order the model lists
/// them (`smithy_query(query="shape", shape="SortAssociationsBy")`, mirrored by the
/// generated `types::SORT_ASSOCIATIONS_BY_VALUES`).
const SORT_KEYS: &[&str] = &[
    "SourceArn",
    "DestinationArn",
    "SourceType",
    "DestinationType",
    "CreationTime",
];

/// Every member of the modelled `SortOrder` enum.
const SORT_ORDERS: &[&str] = &["Ascending", "Descending"];

/// Every member of the modelled `AssociationEdgeType` enum, in the order the model lists
/// them (`smithy_query(query="shape", shape="AssociationEdgeType")`).
const EDGE_TYPES: &[&str] = &[
    "AssociatedWith",
    "ContributedTo",
    "DerivedFrom",
    "Produced",
    "SameAs",
];

/// `ListAssociations` in `region` with `body`, requiring a 200.
async fn list(emulator: &Emulator, region: &str, body: &Value) -> Value {
    let (status, parsed) = emulator.call_in(region, "ListAssociations", body).await;
    assert_eq!(
        status, 200,
        "case {body} in {region}: ListAssociations must succeed, got {status} with {parsed}"
    );
    parsed
}

/// Call `target` in `region` and require a 200, returning the parsed output.
async fn ok_in(emulator: &Emulator, region: &str, target: &str, body: &Value) -> Value {
    let (status, parsed) = emulator.call_in(region, target, body).await;
    assert_eq!(
        status, 200,
        "case {target} in {region}: expected HTTP 200, got {status} with {parsed}"
    );
    parsed
}

/// One edge's identity as these tests compare it: the `(SourceArn, DestinationArn)` pair,
/// which is an association's whole identity.
type Pair = (String, String);

/// One filter or bound case: its name, the request body, and the edges it must select.
type FilterCase = (String, Value, BTreeSet<Pair>);

/// One summary's sort key as the test reads it off the wire: the key as a string for the
/// four string-valued members of `SortAssociationsBy`, as a number for `CreationTime`, and
/// the identity pair the tie-break uses.
type SortKey = (Option<String>, Option<f64>, Pair);

/// One end of an association as the test knows it: the ARN of a lineage entity it
/// created, plus the name and type a listing must report for that end.
#[derive(Debug, Clone)]
struct End {
    arn: String,
    /// The entity's own name, absent only for an artifact created without one.
    name: Option<String>,
    /// The entity's own type, absent for the two kinds that model none.
    entity_type: Option<String>,
}

/// Create a lineage entity of `kind` in `region` and answer the end it is.
///
/// The kinds are the ones `AssociationEntityArn`'s modelled pattern admits
/// (`^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:(experiment|experiment-trial-component|artifact|action|context)/`).
/// Three of them carry a type of their own — an artifact's `ArtifactType`, an action's
/// `ActionType` and a context's `ContextType` — while an experiment and a trial component
/// model none, so `entity_type` is `None` for those two. `label` is an
/// `ExperimentEntityName`, so it must be unique per case for the name-keyed kinds.
async fn create_end(
    emulator: &Emulator,
    region: &str,
    kind: &str,
    label: &str,
    entity_type: &str,
) -> End {
    match kind {
        "artifact" => End {
            arn: str_field(
                &ok_in(
                    emulator,
                    region,
                    "CreateArtifact",
                    &json!({
                        "ArtifactName": label,
                        "Source": { "SourceUri": format!("s3://bucket/{label}.csv") },
                        "ArtifactType": entity_type,
                    }),
                )
                .await,
                "ArtifactArn",
            ),
            name: Some(label.to_string()),
            entity_type: Some(entity_type.to_string()),
        },
        // An artifact is the one kind whose name is optional, so this row covers an end
        // whose `SourceName`/`DestinationName` must stay ABSENT.
        "artifact-without-a-name" => End {
            arn: str_field(
                &ok_in(
                    emulator,
                    region,
                    "CreateArtifact",
                    &json!({
                        "Source": { "SourceUri": format!("s3://bucket/{label}.bin") },
                        "ArtifactType": entity_type,
                    }),
                )
                .await,
                "ArtifactArn",
            ),
            name: None,
            entity_type: Some(entity_type.to_string()),
        },
        "action" => End {
            arn: str_field(
                &ok_in(
                    emulator,
                    region,
                    "CreateAction",
                    &json!({
                        "ActionName": label,
                        "Source": { "SourceUri": format!("s3://bucket/{label}") },
                        "ActionType": entity_type,
                    }),
                )
                .await,
                "ActionArn",
            ),
            name: Some(label.to_string()),
            entity_type: Some(entity_type.to_string()),
        },
        "context" => End {
            arn: str_field(
                &ok_in(
                    emulator,
                    region,
                    "CreateContext",
                    &json!({
                        "ContextName": label,
                        "Source": { "SourceUri": format!("s3://bucket/{label}") },
                        "ContextType": entity_type,
                    }),
                )
                .await,
                "ContextArn",
            ),
            name: Some(label.to_string()),
            entity_type: Some(entity_type.to_string()),
        },
        "experiment-trial-component" => End {
            arn: str_field(
                &ok_in(
                    emulator,
                    region,
                    "CreateTrialComponent",
                    &json!({ "TrialComponentName": label }),
                )
                .await,
                "TrialComponentArn",
            ),
            name: Some(label.to_string()),
            entity_type: None,
        },
        "experiment" => End {
            arn: str_field(
                &ok_in(
                    emulator,
                    region,
                    "CreateExperiment",
                    &json!({ "ExperimentName": label }),
                )
                .await,
                "ExperimentArn",
            ),
            name: Some(label.to_string()),
            entity_type: None,
        },
        other => panic!("case {other}: no implemented create for this lineage entity kind"),
    }
}

/// The summaries a `ListAssociations` response carries. The member is always present, so
/// a response without it is a failure rather than an empty page.
fn summaries(body: &Value) -> Vec<Value> {
    body.get("AssociationSummaries")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected an AssociationSummaries array in {body}"))
        .clone()
}

/// The `(SourceArn, DestinationArn)` pairs a response reports, in the order it reports
/// them. That pair is an association's whole identity.
fn pairs(body: &Value) -> Vec<Pair> {
    summaries(body)
        .iter()
        .map(|summary| {
            (
                str_field(summary, "SourceArn"),
                str_field(summary, "DestinationArn"),
            )
        })
        .collect()
}

/// The pairs a response reports, as a SET: a filter's answer is a set of edges, and the
/// order is the sort rule's business rather than a filter's.
fn pair_set(body: &Value) -> BTreeSet<Pair> {
    pairs(body).into_iter().collect()
}

#[tokio::test]
async fn every_stored_edge_is_reported_with_the_name_and_type_of_each_end() {
    let emulator = Emulator::new();

    // Rows differ along what the rule generalises over: the Region, the KIND of each end
    // (both the kinds that carry a type of their own and the two that model none appear on
    // each side, plus the one kind whose name is optional), and the `AssociationType`
    // (absent, then every member of the modelled enum in turn).
    let kinds = [
        ("artifact", "DataSet"),
        ("action", "ModelDeployment"),
        ("context", "Endpoint"),
        ("experiment-trial-component", ""),
        ("experiment", ""),
        ("artifact-without-a-name", "Model"),
    ];
    let regions = ["us-east-1", "eu-west-2"];
    let mut row = 0usize;

    for region in regions {
        for (source_kind, source_type) in kinds {
            for (destination_kind, destination_type) in kinds {
                // `None` for row 0, then each modelled edge type in turn.
                let association_type = if row.is_multiple_of(EDGE_TYPES.len() + 1) {
                    None
                } else {
                    Some(EDGE_TYPES[(row % (EDGE_TYPES.len() + 1)) - 1])
                };
                let case =
                    format!("{region}/{source_kind}->{destination_kind}/type={association_type:?}");

                let source = create_end(
                    &emulator,
                    region,
                    source_kind,
                    &format!("src{row}"),
                    source_type,
                )
                .await;
                let destination = create_end(
                    &emulator,
                    region,
                    destination_kind,
                    &format!("dst{row}"),
                    destination_type,
                )
                .await;

                let mut add = json!({
                    "SourceArn": source.arn,
                    "DestinationArn": destination.arn,
                });
                if let Some(association_type) = association_type {
                    add["AssociationType"] = json!(association_type);
                }
                ok_in(&emulator, region, "AddAssociation", &add).await;

                // The `SourceArn` filter narrows the listing to the one edge this row
                // added, so the assertions below read exactly that summary however many
                // edges the store already holds.
                let listed = list(
                    &emulator,
                    region,
                    &json!({ "SourceArn": source.arn, "MaxResults": 50 }),
                )
                .await;
                let page = summaries(&listed);
                assert_eq!(
                    page.len(),
                    1,
                    "case {case:?}: expected exactly the edge just added, got {listed:?}"
                );
                let summary = &page[0];

                assert_eq!(
                    str_field(summary, "SourceArn"),
                    source.arn,
                    "case {case:?}: SourceArn, got {summary:?}"
                );
                assert_eq!(
                    str_field(summary, "DestinationArn"),
                    destination.arn,
                    "case {case:?}: DestinationArn, got {summary:?}"
                );
                assert_eq!(
                    summary.get("AssociationType").and_then(Value::as_str),
                    association_type,
                    "case {case:?}: AssociationType is replayed as stored, got {summary:?}"
                );
                // The end members are the ENTITY's own name and type, so each is reported
                // where the kind has one and absent where it does not.
                assert_eq!(
                    summary.get("SourceName").and_then(Value::as_str),
                    source.name.as_deref(),
                    "case {case:?}: SourceName is the source entity's name, got {summary:?}"
                );
                assert_eq!(
                    summary.get("DestinationName").and_then(Value::as_str),
                    destination.name.as_deref(),
                    "case {case:?}: DestinationName is the destination's name, got {summary:?}"
                );
                assert_eq!(
                    summary.get("SourceType").and_then(Value::as_str),
                    source.entity_type.as_deref(),
                    "case {case:?}: SourceType is the source entity's type, got {summary:?}"
                );
                assert_eq!(
                    summary.get("DestinationType").and_then(Value::as_str),
                    destination.entity_type.as_deref(),
                    "case {case:?}: DestinationType is the destination's type, got {summary:?}"
                );
                assert!(
                    summary.get("CreationTime").and_then(Value::as_f64).is_some(),
                    "case {case:?}: an edge carries the instant it was added, got {summary:?}"
                );
                assert!(
                    summary
                        .get("CreatedBy")
                        .and_then(|created_by| created_by.get("IamIdentity"))
                        .and_then(|identity| identity.get("Arn"))
                        .and_then(Value::as_str)
                        .is_some_and(|arn| arn.starts_with("arn:aws:iam:")),
                    "case {case:?}: an edge carries the identity that added it, got {summary:?}"
                );

                row += 1;
            }
        }
    }
}

/// A fixed set of edges in one Region, for the filter, sort and pagination rules: every
/// end kind that carries a type appears on both sides, two sources share a type, and the
/// edge types repeat so a filter can select more than one row.
struct Seeded {
    ends: Vec<End>,
    /// `(source index, destination index, association type)` for each stored edge.
    edges: Vec<(usize, usize, &'static str)>,
    /// Each edge's reported `CreationTime`, in the order [`Self::edges`] lists them.
    created: Vec<f64>,
}

impl Seeded {
    fn pair(&self, edge: usize) -> Pair {
        let (source, destination, _) = self.edges[edge];
        (
            self.ends[source].arn.clone(),
            self.ends[destination].arn.clone(),
        )
    }

    /// The pairs of the edges `wanted` names, as a set.
    fn pairs_of(&self, wanted: &[usize]) -> BTreeSet<Pair> {
        wanted.iter().map(|edge| self.pair(*edge)).collect()
    }

    fn every_pair(&self) -> BTreeSet<Pair> {
        (0..self.edges.len()).map(|edge| self.pair(edge)).collect()
    }
}

/// Seed five edges between five entities in `region`, spaced so their `CreationTime` order
/// is unambiguous and a bound can be placed strictly between two of them.
async fn seed(emulator: &Emulator, region: &str) -> Seeded {
    let ends = vec![
        create_end(emulator, region, "artifact", "dataset-one", "DataSet").await,
        create_end(emulator, region, "artifact", "model-two", "Model").await,
        create_end(emulator, region, "context", "endpoint-three", "Endpoint").await,
        create_end(emulator, region, "action", "deploy-four", "ModelDeployment").await,
        create_end(emulator, region, "experiment", "experiment-five", "").await,
    ];
    let edges = vec![
        (0usize, 2usize, "ContributedTo"),
        (1, 2, "Produced"),
        (0, 3, "ContributedTo"),
        (1, 4, "DerivedFrom"),
        (2, 3, "AssociatedWith"),
    ];
    let mut created = Vec::new();
    for (source, destination, association_type) in &edges {
        ok_in(
            emulator,
            region,
            "AddAssociation",
            &json!({
                "SourceArn": ends[*source].arn,
                "DestinationArn": ends[*destination].arn,
                "AssociationType": association_type,
            }),
        )
        .await;
        // Read the instant back off the service rather than guessing it, so the bounds in
        // the timestamp case are placed against stored state.
        let listed = list(
            emulator,
            region,
            &json!({
                "SourceArn": ends[*source].arn,
                "DestinationArn": ends[*destination].arn,
            }),
        )
        .await;
        created.push(f64_field(&summaries(&listed)[0], "CreationTime"));
        tokio::time::sleep(CREATE_SPACING).await;
    }
    Seeded {
        ends,
        edges,
        created,
    }
}

#[tokio::test]
async fn each_filter_selects_only_the_associations_carrying_that_value() {
    let emulator = Emulator::new();
    let region = "us-west-2";
    let seeded = seed(&emulator, region).await;

    // Rows differ along WHICH filter is applied, and along the kind of value it names: an
    // ARN on each side, a type on each side, an edge type, two filters at once, and a
    // value nothing carries.
    let cases: Vec<FilterCase> = vec![
        (
            "no-filter".to_string(),
            json!({ "MaxResults": 50 }),
            seeded.every_pair(),
        ),
        (
            "source-arn".to_string(),
            json!({ "MaxResults": 50, "SourceArn": seeded.ends[0].arn }),
            seeded.pairs_of(&[0, 2]),
        ),
        (
            "destination-arn".to_string(),
            json!({ "MaxResults": 50, "DestinationArn": seeded.ends[2].arn }),
            seeded.pairs_of(&[0, 1]),
        ),
        (
            "source-type".to_string(),
            json!({ "MaxResults": 50, "SourceType": "DataSet" }),
            seeded.pairs_of(&[0, 2]),
        ),
        (
            "destination-type".to_string(),
            json!({ "MaxResults": 50, "DestinationType": "ModelDeployment" }),
            seeded.pairs_of(&[2, 4]),
        ),
        (
            "association-type".to_string(),
            json!({ "MaxResults": 50, "AssociationType": "ContributedTo" }),
            seeded.pairs_of(&[0, 2]),
        ),
        (
            "source-type-and-association-type".to_string(),
            json!({
                "MaxResults": 50,
                "SourceType": "DataSet",
                "AssociationType": "ContributedTo",
            }),
            seeded.pairs_of(&[0, 2]),
        ),
        (
            "source-arn-and-destination-arn".to_string(),
            json!({
                "MaxResults": 50,
                "SourceArn": seeded.ends[0].arn,
                "DestinationArn": seeded.ends[3].arn,
            }),
            seeded.pairs_of(&[2]),
        ),
        (
            // An end of a kind that models no type carries none, so no `SourceType` value
            // selects an edge out of one.
            "source-type-of-a-kind-that-models-none".to_string(),
            json!({ "MaxResults": 50, "SourceType": "Experiment" }),
            BTreeSet::new(),
        ),
        (
            "type-nothing-carries".to_string(),
            json!({ "MaxResults": 50, "SourceType": "NoSuchType" }),
            BTreeSet::new(),
        ),
        (
            "arn-nothing-created".to_string(),
            json!({
                "MaxResults": 50,
                "SourceArn": format!("arn:aws:sagemaker:{region}:{ACCOUNT}:artifact/absent"),
            }),
            BTreeSet::new(),
        ),
    ];

    for (case, body, expected) in &cases {
        let listed = list(&emulator, region, body).await;
        let actual = pair_set(&listed);
        assert_eq!(
            &actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn the_creation_time_bounds_keep_the_edges_on_their_side_of_the_instant() {
    let emulator = Emulator::new();
    let region = "us-east-2";
    let seeded = seed(&emulator, region).await;

    // The bounds are placed strictly BETWEEN neighbouring instants, which is the finest
    // split the `awsJson1_1` epoch-seconds `Timestamp` represents without loss (the same
    // placement `list_artifacts`' bound case uses). Rows: a split after each edge but the
    // last, so every prefix/suffix of the result set is the expected answer once, plus a
    // window around a single edge and bounds outside the whole set.
    let mut cases: Vec<FilterCase> = Vec::new();
    for edge in 0..seeded.edges.len() - 1 {
        let split = (seeded.created[edge] + seeded.created[edge + 1]) / 2.0;
        cases.push((
            format!("created-before-the-split-after-{edge}"),
            json!({ "MaxResults": 50, "CreatedBefore": split }),
            seeded.pairs_of(&(0..=edge).collect::<Vec<usize>>()),
        ));
        cases.push((
            format!("created-after-the-split-after-{edge}"),
            json!({ "MaxResults": 50, "CreatedAfter": split }),
            seeded.pairs_of(&(edge + 1..seeded.edges.len()).collect::<Vec<usize>>()),
        ));
    }
    let around_the_middle_edge = (
        (seeded.created[1] + seeded.created[2]) / 2.0,
        (seeded.created[2] + seeded.created[3]) / 2.0,
    );
    cases.push((
        "both-bounds-around-one-edge".to_string(),
        json!({
            "MaxResults": 50,
            "CreatedAfter": around_the_middle_edge.0,
            "CreatedBefore": around_the_middle_edge.1,
        }),
        seeded.pairs_of(&[2]),
    ));
    cases.push((
        "a-window-before-the-oldest-edge-selects-nothing".to_string(),
        json!({ "MaxResults": 50, "CreatedBefore": seeded.created[0] - 60.0 }),
        BTreeSet::new(),
    ));
    cases.push((
        "a-window-after-the-newest-edge-selects-nothing".to_string(),
        json!({ "MaxResults": 50, "CreatedAfter": seeded.created[4] + 60.0 }),
        BTreeSet::new(),
    ));
    cases.push((
        "a-bound-composed-with-a-value-filter".to_string(),
        json!({
            "MaxResults": 50,
            "CreatedAfter": around_the_middle_edge.0,
            "AssociationType": "ContributedTo",
        }),
        seeded.pairs_of(&[2]),
    ));

    for (case, body, expected) in &cases {
        let listed = list(&emulator, region, body).await;
        let actual = pair_set(&listed);
        assert_eq!(
            &actual, expected,
            "case {case}: expected {expected:?}, got {actual:?}"
        );
    }

    // A listing that selects nothing answers an EMPTY list rather than omitting the
    // member.
    let empty = list(
        &emulator,
        region,
        &json!({ "MaxResults": 50, "CreatedBefore": seeded.created[0] - 60.0 }),
    )
    .await;
    assert_eq!(
        summaries(&empty),
        Vec::<Value>::new(),
        "a window holding no edge answers an empty list, got {empty:?}"
    );
}

#[tokio::test]
async fn sort_by_orders_by_the_property_it_names_and_sort_order_reverses_it() {
    let emulator = Emulator::new();
    let region = "ap-south-1";
    let seeded = seed(&emulator, region).await;

    // Rows: every member of `SortAssociationsBy` against every member of `SortOrder`,
    // plus the documented defaults (`CreationTime` / `Descending`) taken by sending
    // neither.
    let mut cases: Vec<(String, Value)> = Vec::new();
    for sort_by in SORT_KEYS {
        for sort_order in SORT_ORDERS {
            cases.push((
                format!("{sort_by}/{sort_order}"),
                json!({ "MaxResults": 50, "SortBy": sort_by, "SortOrder": sort_order }),
            ));
        }
    }
    cases.push(("documented-defaults".to_string(), json!({ "MaxResults": 50 })));

    for (case, body) in &cases {
        let listed = list(&emulator, region, body).await;
        let page = summaries(&listed);
        assert_eq!(
            page.len(),
            seeded.edges.len(),
            "case {case}: the whole result set fits one page, got {listed:?}"
        );

        let sort_by = body
            .get("SortBy")
            .and_then(Value::as_str)
            .unwrap_or("CreationTime");
        let descending = body
            .get("SortOrder")
            .and_then(Value::as_str)
            .unwrap_or("Descending")
            == "Descending";

        // The key is read off the summaries the service answered, so this checks the
        // ORDER it chose rather than re-deriving its values. `CreationTime` is compared as
        // a number and the other keys as the strings they are; a key an edge has no value
        // for is `None`, which orders before every value.
        let keys: Vec<SortKey> = page
            .iter()
            .map(|summary| {
                (
                    summary.get(sort_by).and_then(Value::as_str).map(str::to_string),
                    summary.get(sort_by).and_then(Value::as_f64),
                    (
                        str_field(summary, "SourceArn"),
                        str_field(summary, "DestinationArn"),
                    ),
                )
            })
            .collect();

        for window in keys.windows(2) {
            let (left_text, left_number, left_pair) = &window[0];
            let (right_text, right_number, right_pair) = &window[1];
            let ordered = if sort_by == "CreationTime" {
                let left = left_number.expect("a CreationTime is a number");
                let right = right_number.expect("a CreationTime is a number");
                if descending {
                    left >= right
                } else {
                    left <= right
                }
            } else if left_text == right_text {
                // The tie-break is the identity pair ascending, whichever way the sort
                // key runs, so a page boundary is stable in both directions.
                left_pair <= right_pair
            } else if descending {
                left_text > right_text
            } else {
                left_text < right_text
            };
            assert!(
                ordered,
                "case {case}: {left_text:?}/{left_number:?}/{left_pair:?} must precede \
                 {right_text:?}/{right_number:?}/{right_pair:?}, got {keys:?}"
            );
        }
    }
}

#[tokio::test]
async fn max_results_and_next_token_walk_the_whole_result_set_once() {
    let emulator = Emulator::new();
    let region = "eu-central-1";
    let seeded = seed(&emulator, region).await;

    // Rows: a page size below, at and above the result set's size, against each sort
    // order, so the walk is exercised where it must and must not mint a token.
    for page_size in [1usize, 2, seeded.edges.len(), seeded.edges.len() + 1] {
        for sort_order in SORT_ORDERS {
            let case = format!("page={page_size}/{sort_order}");
            let mut seen: Vec<Pair> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0usize;

            loop {
                let mut body = json!({ "MaxResults": page_size, "SortOrder": sort_order });
                if let Some(token) = &token {
                    body["NextToken"] = json!(token);
                }
                let listed = list(&emulator, region, &body).await;
                let page = pairs(&listed);
                assert!(
                    page.len() <= page_size,
                    "case {case}: a page holds at most MaxResults, got {listed:?}"
                );
                seen.extend(page);
                pages += 1;
                assert!(
                    pages <= seeded.edges.len() + 1,
                    "case {case}: the walk must terminate, got {seen:?}"
                );
                match listed.get("NextToken").and_then(Value::as_str) {
                    Some(next) => token = Some(next.to_string()),
                    None => break,
                }
            }

            assert_eq!(
                seen.len(),
                seeded.edges.len(),
                "case {case}: the walk reports every edge exactly once, got {seen:?}"
            );
            let unique: BTreeSet<Pair> = seen.iter().cloned().collect();
            assert_eq!(
                unique,
                seeded.every_pair(),
                "case {case}: the walk reports each edge once, got {seen:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_listing_reports_only_the_region_the_caller_signed_for() {
    let emulator = Emulator::new();

    // Rows: the same shape of edge seeded in three Regions, so a listing in one must
    // report its own and none of the others'.
    let regions = ["us-east-1", "eu-west-1", "ap-northeast-1"];
    let mut per_region: Vec<(&str, BTreeSet<Pair>)> = Vec::new();
    for (index, region) in regions.iter().enumerate() {
        let source = create_end(
            &emulator,
            region,
            "artifact",
            &format!("regional-source-{index}"),
            "DataSet",
        )
        .await;
        let destination = create_end(
            &emulator,
            region,
            "context",
            &format!("regional-destination-{index}"),
            "Endpoint",
        )
        .await;
        ok_in(
            &emulator,
            region,
            "AddAssociation",
            &json!({ "SourceArn": source.arn, "DestinationArn": destination.arn }),
        )
        .await;
        per_region.push((
            region,
            BTreeSet::from([(source.arn.clone(), destination.arn.clone())]),
        ));
    }

    for (region, expected) in &per_region {
        let listed = list(&emulator, region, &json!({ "MaxResults": 50 })).await;
        let actual = pair_set(&listed);
        assert_eq!(
            &actual, expected,
            "case {region}: only that Region's edges are listed, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn a_released_edge_stops_being_reported_and_an_empty_store_lists_nothing() {
    let emulator = Emulator::new();
    let region = "ca-central-1";

    // An account holding no association answers an EMPTY list, not a missing member.
    let empty = list(&emulator, region, &json!({ "MaxResults": 50 })).await;
    assert_eq!(
        summaries(&empty),
        Vec::<Value>::new(),
        "an account with no association lists an empty set, got {empty:?}"
    );

    let seeded = seed(&emulator, region).await;

    // Rows: each stored edge released in turn, so the listing shrinks by exactly that
    // edge every time and by nothing else.
    for released in 0..seeded.edges.len() {
        let (source, destination) = seeded.pair(released);
        ok_in(
            &emulator,
            region,
            "DeleteAssociation",
            &json!({ "SourceArn": source, "DestinationArn": destination }),
        )
        .await;

        let listed = list(&emulator, region, &json!({ "MaxResults": 50 })).await;
        let actual = pair_set(&listed);
        let expected = seeded.pairs_of(&(released + 1..seeded.edges.len()).collect::<Vec<usize>>());
        assert_eq!(
            actual, expected,
            "case released-{released}: a released edge is no longer reported, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn list_associations_refuses_inputs_outside_the_documented_value_sets() {
    let emulator = Emulator::new();
    let region = "us-east-1";
    let seeded = seed(&emulator, region).await;

    let token_of_the_whole_result_set = {
        let page = list(
            &emulator,
            region,
            &json!({ "MaxResults": 1, "SortOrder": "Ascending" }),
        )
        .await;
        page.get("NextToken")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("expected a NextToken in {page}"))
            .to_string()
    };

    // Rows: one outside each documented value set — `SortAssociationsBy`, `SortOrder`,
    // `AssociationEdgeType`, `MaxResults` one past each published bound (`range 1..100`),
    // `SourceType`/`DestinationType` past `String256`'s 256, `NextToken` past
    // `len 0..8192`, an ARN outside `AssociationEntityArn`'s pattern, a token that names
    // no boundary, and a token minted against another result set.
    let cases: Vec<(&str, Value)> = vec![
        (
            "sort-by-outside-the-enum",
            json!({ "SortBy": "SourceName" }),
        ),
        (
            "sort-order-outside-the-enum",
            json!({ "SortOrder": "Sideways" }),
        ),
        (
            "association-type-outside-the-enum",
            json!({ "AssociationType": "PointsAt" }),
        ),
        ("max-results-below-1", json!({ "MaxResults": 0 })),
        ("max-results-above-100", json!({ "MaxResults": 101 })),
        (
            "source-type-one-past-the-maximum",
            json!({ "SourceType": "T".repeat(257) }),
        ),
        (
            "destination-type-one-past-the-maximum",
            json!({ "DestinationType": "T".repeat(257) }),
        ),
        (
            "next-token-one-past-the-maximum",
            json!({ "NextToken": "a".repeat(8193) }),
        ),
        (
            "source-arn-outside-the-pattern",
            json!({ "SourceArn": "not-an-arn" }),
        ),
        (
            "destination-arn-of-another-service",
            json!({ "DestinationArn": format!("arn:aws:s3:{region}:{ACCOUNT}:artifact/x") }),
        ),
        (
            "next-token-that-names-no-boundary",
            json!({ "NextToken": "not-a-token" }),
        ),
        (
            // A token minted against the whole result set names an edge the filtered one
            // does not hold, so it is refused rather than silently restarting the walk.
            "next-token-of-another-result-set",
            json!({
                "NextToken": token_of_the_whole_result_set,
                "AssociationType": "DerivedFrom",
                "SortOrder": "Ascending",
            }),
        ),
    ];

    for (case, body) in &cases {
        let (status, parsed) = emulator.call_in(region, "ListAssociations", body).await;
        assert_eq!(
            status, 400,
            "case {case}: ListAssociations must be refused, got {status} with {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ValidationException",
            "case {case}: refusal code, got {parsed:?}"
        );
    }

    // Nothing above reached state: every seeded edge is still reported.
    let listed = list(&emulator, region, &json!({ "MaxResults": 50 })).await;
    assert_eq!(
        pair_set(&listed),
        seeded.every_pair(),
        "a refused request must change nothing, got {listed:?}"
    );
}
