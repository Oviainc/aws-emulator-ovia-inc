// SPDX-License-Identifier: CC0-1.0

//! Comprehend's inference-endpoint family: `CreateEndpoint`, `DescribeEndpoint` and
//! `ListEndpoints`.
//!
//! RULE (`ListEndpoints`): the operation reports every endpoint the service persists,
//! projected into `EndpointProperties`, in creation order — narrowed by the single
//! filtering parameter the request set, truncated at `MaxResults`, with a `NextToken`
//! that resumes exactly after the last item of the page and no token on the last one.
//! More than one filtering parameter ("you can only set one filter at a time") and a
//! token this service never issued are both refused with `InvalidRequestException`.
//!
//! RULE (`CreateEndpoint`/`DescribeEndpoint`): an endpoint is a resource the create
//! mints an ARN for — from the attached model's own resource type — and persists in
//! `CREATING`; the reads report back exactly what was persisted plus the status the
//! clock already owed it, that status being the outcome of the provisioning that was
//! actually performed (`IN_SERVICE` only with a trained model behind the endpoint,
//! else the modelled `FAILED` with a reason); and an ARN that names no model this
//! account holds is refused with `ResourceNotFoundException`.
//!
//! Each suite asserts those rules over a SET of inputs: the creation rows differ
//! along which optional members are present and the capacity asked for, the
//! settling rows along the status of the attached model, the filter rows along which
//! single filter member is set and its value, and the paging rows along the page
//! size. The recorded `DesiredInferenceUnits=0` refusal is ONE row of the
//! refusal table.

// `std::thread::sleep`, not `tokio::time::sleep`: the workspace's shared `tokio`
// carries no `time` feature, so there is no timer driver. It is safe here only
// because every call below is awaited sequentially on a current-thread runtime with
// nothing in the background.
use std::thread::sleep;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_comprehend_comprehend_20171127::ops::types::{
    Tag, ENDPOINT_STATUS_VALUES,
};
use service_com_amazonaws_comprehend_comprehend_20171127::state::{
    document_classifier_arn, endpoint_arn, service_state, DocumentClassifier, Endpoint,
    EndpointProvisioning,
};

use crate::comprehend_client::{error_code, error_message, Comprehend, ACCOUNT_ID, REGION};

/// The ONE sentence a `ResourceNotFoundException` of this service carries, whichever
/// operation raised it and whatever ARN was asked for
/// (`crate::state::RESOURCE_NOT_FOUND_MESSAGE`): the service's own reason code and
/// NOTHING derived from the request, so the refusal never echoes the ARN back.
/// Written out here rather than imported from the service, so the suite pins the
/// sentence instead of whatever the code currently believes.
const RESOURCE_NOT_FOUND_MESSAGE: &str = "RESOURCE_NOT_FOUND: Could not find specified resource.";

/// Longest a suite waits for the clock to owe an endpoint its settled status. The
/// `ENDPOINT_CREATING_SPAN` in `crate::state` is well under a second; this leaves
/// ample head-room on a loaded machine.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(10);

/// The gap between two consecutive seeded endpoints' creation instants. A whole
/// number of seconds, so a filter bound sent as epoch seconds names one instant
/// exactly.
const CREATION_GAP: Duration = Duration::from_secs(60);

/// A fixed instant in the past, on a whole second, that the seeded creation times are
/// laid out from.
fn base_time() -> SystemTime {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs();
    UNIX_EPOCH + Duration::from_secs(now - 3_600)
}

/// `time` as the epoch seconds a modelled timestamp is carried as on the wire.
fn epoch_secs(time: SystemTime) -> i64 {
    i64::try_from(
        time.duration_since(UNIX_EPOCH)
            .expect("the instant is after the epoch")
            .as_secs(),
    )
    .expect("the instant fits in an i64")
}

/// Seed a custom document classifier in `status` and hand back its ARN — the model an
/// endpoint is attached to.
///
/// Seeded through `state::service_state`, exactly as a handler reaches the store, so
/// the model a created endpoint resolves is the same record `DescribeDocumentClassifier`
/// would serve. The due instants are pushed an hour out so the clock does not move a
/// non-terminal status while the suite runs.
fn seed_model(comprehend: &Comprehend, name: &str, status: &str) -> String {
    let arn = document_classifier_arn(REGION, name, None);
    let state = service_state(&**comprehend.engine())
        .expect("the shared engine carries this service's store");
    let mut store = state.lock();
    let mut record = DocumentClassifier::submitted(
        0,
        arn.clone(),
        name.to_string(),
        "en".to_string(),
        base_time(),
    );
    record.status = status.to_string();
    record.training_due_at = SystemTime::now() + Duration::from_secs(3_600);
    record.terminal_due_at = record.training_due_at + Duration::from_secs(3_600);
    record.seq = store.mint_document_classifier_seq();
    store.insert_document_classifier(record);
    arn
}

/// The `CreateEndpoint` body for `case`, differing along every member the operation
/// persists.
fn creation(case: &str, model_arn: &str) -> Value {
    match case {
        "every optional member set" => json!({
            "EndpointName": "every-member-endpoint",
            "ModelArn": model_arn,
            "DesiredInferenceUnits": 20,
            "DataAccessRoleArn": "arn:aws:iam::123456789012:role/ComprehendRole",
            "ClientRequestToken": "every-member-token",
            "Tags": [{ "Key": "team", "Value": "research" }, { "Key": "cost-centre" }],
        }),
        "a large desired capacity" => json!({
            "EndpointName": "high-capacity-endpoint",
            "ModelArn": model_arn,
            "DesiredInferenceUnits": 100,
        }),
        _ => json!({
            "EndpointName": "minimal-endpoint",
            "ModelArn": model_arn,
            "DesiredInferenceUnits": 1,
        }),
    }
}

/// The cases [`creation`] builds.
const CREATION_CASES: &[&str] = &[
    "only the required members",
    "every optional member set",
    "a large desired capacity",
];

/// Create an endpoint and return its ARN.
async fn create(comprehend: &Comprehend, case: &str, body: &Value) -> String {
    let output = comprehend.ok("CreateEndpoint", body).await;
    output["EndpointArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected an EndpointArn, got {output}"))
        .to_string()
}

/// The properties `DescribeEndpoint` reports for `arn`.
async fn describe(comprehend: &Comprehend, arn: &str) -> Value {
    let output = comprehend
        .ok("DescribeEndpoint", &json!({ "EndpointArn": arn }))
        .await;
    output["EndpointProperties"].clone()
}

/// A member's string value on a reported item, or `None` when the item does not
/// carry it.
fn member_str(item: &Value, member: &str) -> Option<String> {
    item.get(member)
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Poll `DescribeEndpoint` until the endpoint leaves `CREATING`, returning the
/// properties it then reports.
async fn settled(comprehend: &Comprehend, case: &str, arn: &str) -> Value {
    let deadline = SystemTime::now() + SETTLE_TIMEOUT;
    loop {
        let properties = describe(comprehend, arn).await;
        let status = member_str(&properties, "Status").unwrap_or_default();
        if status != "CREATING" {
            return properties;
        }
        assert!(
            SystemTime::now() < deadline,
            "case {case:?}: the endpoint never left CREATING within {SETTLE_TIMEOUT:?}: \
             {properties}"
        );
        sleep(Duration::from_millis(40));
    }
}

/// One `ListEndpoints` call.
async fn list(comprehend: &Comprehend, request: &Value) -> Value {
    comprehend.ok("ListEndpoints", request).await
}

/// The `EndpointArn` of every item of one page, in the order served.
fn listed_arns(output: &Value) -> Vec<String> {
    output["EndpointPropertiesList"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| {
            item["EndpointArn"]
                .as_str()
                .unwrap_or("<no EndpointArn>")
                .to_string()
        })
        .collect()
}

/// Page through every endpoint the request selects, `page_size` at a time, returning
/// the ARNs in the order they were served.
async fn page_through(comprehend: &Comprehend, request: &Value, page_size: usize) -> Vec<String> {
    let mut collected: Vec<String> = Vec::new();
    let mut next_token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let mut page_request = request.clone();
        page_request["MaxResults"] = json!(page_size);
        if let Some(token) = &next_token {
            page_request["NextToken"] = json!(token);
        }
        let output = list(comprehend, &page_request).await;
        let page = listed_arns(&output);
        assert!(
            page.len() <= page_size,
            "case MaxResults={page_size}: a page returned {} items, got {output}",
            page.len()
        );
        collected.extend(page);
        pages += 1;
        assert!(
            pages <= collected.len() + 1,
            "case MaxResults={page_size}: paging did not terminate after {pages} pages"
        );
        match output["NextToken"].as_str() {
            Some(token) => next_token = Some(token.to_string()),
            None => break,
        }
    }
    collected
}

/// The two models the seeded endpoints are attached to.
fn seeded_model_arns() -> (String, String) {
    (
        document_classifier_arn(REGION, "reviews-classifier", None),
        document_classifier_arn(REGION, "docs-classifier", None),
    )
}

/// The endpoint records the filter and paging suites seed, oldest first.
///
/// `seq` is a placeholder here: [`seed`] replaces it with the sequence number the
/// store mints, which is what the operation pages on. The rows differ along the model
/// they are attached to, the status they carry, the capacity they asked for and which
/// optional members they hold at all.
fn seeded_records(base: SystemTime) -> Vec<Endpoint> {
    let (model_a, model_b) = seeded_model_arns();

    // Row 1: in service with a trained model behind it, so it reports the units
    // actually serving.
    let mut alpha = Endpoint::creating(
        0,
        endpoint_arn(REGION, "document-classifier-endpoint", "alpha"),
        "alpha".to_string(),
        2,
        base,
        EndpointProvisioning::in_service(),
    );
    alpha.model_arn = Some(model_a.clone());
    alpha.status = "IN_SERVICE".to_string();
    alpha.current_inference_units = Some(2);
    alpha.last_modified_time = base + Duration::from_secs(30);

    // Row 2: still creating — its span is pushed an hour out so the clock does not
    // settle it while the suite runs.
    let mut bravo = Endpoint::creating(
        0,
        endpoint_arn(REGION, "document-classifier-endpoint", "bravo"),
        "bravo".to_string(),
        1,
        base + CREATION_GAP,
        EndpointProvisioning::in_service(),
    );
    bravo.model_arn = Some(model_b.clone());
    bravo.ready_due_at = SystemTime::now() + Duration::from_secs(3_600);

    // Row 3: provisioning failed, so it carries the reason and no serving units.
    let mut charlie = Endpoint::creating(
        0,
        endpoint_arn(REGION, "document-classifier-endpoint", "charlie"),
        "charlie".to_string(),
        5,
        base + 2 * CREATION_GAP,
        EndpointProvisioning::failed("The attached model was never trained."),
    );
    charlie.model_arn = Some(model_a.clone());
    charlie.status = "FAILED".to_string();
    charlie.message = Some("The attached model was never trained.".to_string());

    // Row 4: in service, with every optional member of `EndpointProperties` this
    // service fills set.
    let mut delta = Endpoint::creating(
        0,
        endpoint_arn(REGION, "document-classifier-endpoint", "delta"),
        "delta".to_string(),
        10,
        base + 3 * CREATION_GAP,
        EndpointProvisioning::in_service(),
    );
    delta.model_arn = Some(model_b);
    delta.status = "IN_SERVICE".to_string();
    delta.current_inference_units = Some(10);
    delta.data_access_role_arn = Some("arn:aws:iam::210987654321:role/Other".to_string());
    delta.tags = vec![Tag {
        key: "env".to_string(),
        value: Some("staging".to_string()),
    }];

    // Row 5: the newest record, still creating, on the first model again.
    let mut echo = Endpoint::creating(
        0,
        endpoint_arn(REGION, "document-classifier-endpoint", "echo"),
        "echo".to_string(),
        3,
        base + 4 * CREATION_GAP,
        EndpointProvisioning::in_service(),
    );
    echo.model_arn = Some(model_a);
    echo.ready_due_at = SystemTime::now() + Duration::from_secs(3_600);

    vec![alpha, bravo, charlie, delta, echo]
}

/// Seed the store with [`seeded_records`] and hand them back carrying the sequence
/// numbers the store minted, oldest first.
fn seed(comprehend: &Comprehend, base: SystemTime) -> Vec<Endpoint> {
    let state = service_state(&**comprehend.engine())
        .expect("the shared engine carries this service's store");
    let mut store = state.lock();
    let mut seeded = Vec::new();
    for mut record in seeded_records(base) {
        record.seq = store.mint_endpoint_seq();
        store.insert_endpoint(record.clone());
        seeded.push(record);
    }
    seeded
}

/// The ARNs of the seeded rows at `rows`, in creation order.
fn arns_of(seeded: &[Endpoint], rows: &[usize]) -> Vec<String> {
    rows.iter()
        .map(|row| seeded[*row].endpoint_arn.clone())
        .collect()
}

#[tokio::test]
async fn a_created_endpoint_is_persisted_and_described_from_the_store() {
    let comprehend = Comprehend::new();
    let model_arn = seed_model(&comprehend, "reviews-classifier", "TRAINED");

    for case in CREATION_CASES {
        let body = creation(case, &model_arn);
        let name = body["EndpointName"].as_str().expect("the case names one");
        let arn = create(&comprehend, case, &body).await;

        let expected_arn = format!(
            "arn:aws:comprehend:{REGION}:{ACCOUNT_ID}:document-classifier-endpoint/{name}"
        );
        assert_eq!(
            arn, expected_arn,
            "case {case:?}: expected the endpoint ARN {expected_arn}, got {arn}"
        );

        // The round trip: the create's own output names the model it attached, and
        // the reader reports back exactly what was persisted.
        let properties = describe(&comprehend, &arn).await;
        assert_eq!(
            member_str(&properties, "EndpointArn"),
            Some(arn.clone()),
            "case {case:?}: expected the described endpoint to carry its own ARN, got \
             {properties}"
        );
        assert_eq!(
            member_str(&properties, "ModelArn"),
            Some(model_arn.clone()),
            "case {case:?}: expected the attached model {model_arn}, got {properties}"
        );
        assert_eq!(
            member_str(&properties, "Status").as_deref(),
            Some("CREATING"),
            "case {case:?}: expected a freshly created endpoint to be CREATING, got \
             {properties}"
        );
        assert_eq!(
            properties["DesiredInferenceUnits"], body["DesiredInferenceUnits"],
            "case {case:?}: expected the capacity the request asked for, got {properties}"
        );
        assert!(
            properties.get("CurrentInferenceUnits").is_none(),
            "case {case:?}: nothing serves inference before the endpoint is in service, \
             got {properties}"
        );
        assert!(
            properties.get("CreationTime").is_some()
                && properties.get("LastModifiedTime").is_some(),
            "case {case:?}: expected a creation and a last-modified time, got {properties}"
        );
        assert_eq!(
            member_str(&properties, "DataAccessRoleArn"),
            body["DataAccessRoleArn"].as_str().map(str::to_string),
            "case {case:?}: expected the data-access role the request named, got {properties}"
        );

        // And the list reports the same resource the describe does.
        let listed = page_through(&comprehend, &json!({}), 500).await;
        assert!(
            listed.contains(&arn),
            "case {case:?}: expected ListEndpoints to report {arn}, got {listed:?}"
        );
    }
}

/// The settled status is the outcome of the provisioning that ran: a trained model
/// behind the endpoint means `IN_SERVICE`, anything else means the modelled `FAILED`
/// with a reason.
#[tokio::test]
async fn an_endpoint_settles_into_the_outcome_of_its_provisioning() {
    // `ModelStatus` values an untrained model can carry, from the modelled
    // `MODEL_STATUS_VALUES` vocabulary: a model in any of them has fitted nothing.
    let cases: &[(&str, &str, &str)] = &[
        ("a trained model", "TRAINED", "IN_SERVICE"),
        ("a model trained with warnings", "TRAINED_WITH_WARNING", "IN_SERVICE"),
        ("a model whose training failed", "IN_ERROR", "FAILED"),
        ("a model still queued for training", "SUBMITTED", "FAILED"),
        ("a model mid-training", "TRAINING", "FAILED"),
        ("a model whose training was stopped", "STOPPED", "FAILED"),
    ];

    for (case, model_status, expected_status) in cases {
        let comprehend = Comprehend::new();
        let model_arn = seed_model(&comprehend, "reviews-classifier", model_status);
        let body = json!({
            "EndpointName": "settling-endpoint",
            "ModelArn": model_arn,
            "DesiredInferenceUnits": 4,
        });
        let arn = create(&comprehend, case, &body).await;

        let properties = settled(&comprehend, case, &arn).await;
        let status = member_str(&properties, "Status").unwrap_or_default();
        assert_eq!(
            &status, expected_status,
            "case {case:?}: expected the endpoint to settle into {expected_status}, got \
             {properties}"
        );
        assert!(
            ENDPOINT_STATUS_VALUES.contains(&status.as_str()),
            "case {case:?}: {status} is not a modelled EndpointStatus, got {properties}"
        );

        if expected_status == &"IN_SERVICE" {
            assert_eq!(
                properties["CurrentInferenceUnits"], json!(4),
                "case {case:?}: an endpoint in service serves the capacity it asked for, \
                 got {properties}"
            );
            assert!(
                properties.get("Message").is_none(),
                "case {case:?}: an endpoint in service has no failure reason, got {properties}"
            );
        } else {
            let message = member_str(&properties, "Message").unwrap_or_default();
            assert!(
                message.contains(&model_arn) && message.contains(model_status),
                "case {case:?}: expected a failure reason naming the model and its status, \
                 got {message:?}"
            );
            assert!(
                properties.get("CurrentInferenceUnits").is_none(),
                "case {case:?}: a failed endpoint serves no inference units, got {properties}"
            );
        }
    }
}

/// The whole family over the public API, with no store seeding at all: a model
/// created through `CreateDocumentClassifier` is a model an endpoint can be attached
/// to, and the endpoint it mints is reported by both readers.
#[tokio::test]
async fn an_endpoint_can_be_created_for_a_model_created_through_the_api() {
    let comprehend = Comprehend::new();
    let case = "a model created through the API";

    let created = comprehend
        .ok(
            "CreateDocumentClassifier",
            &json!({
                "DocumentClassifierName": "api-classifier",
                "DataAccessRoleArn": "arn:aws:iam::123456789012:role/ComprehendRole",
                "LanguageCode": "en",
                "InputDataConfig": { "S3Uri": "s3://bucket/training/" },
            }),
        )
        .await;
    let model_arn = created["DocumentClassifierArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a classifier ARN, got {created}"))
        .to_string();

    let arn = create(
        &comprehend,
        case,
        &json!({
            "EndpointName": "api-endpoint",
            "ModelArn": model_arn,
            "DesiredInferenceUnits": 2,
        }),
    )
    .await;

    let properties = settled(&comprehend, case, &arn).await;
    // This emulator fits no custom model, so the attached model is never trained and
    // the endpoint settles into the modelled failure state rather than reporting a
    // readiness nothing provisioned.
    assert_eq!(
        member_str(&properties, "Status").as_deref(),
        Some("FAILED"),
        "case {case:?}: expected the endpoint on an untrained model to settle FAILED, got \
         {properties}"
    );
    let listed = page_through(&comprehend, &json!({}), 500).await;
    assert_eq!(
        listed,
        vec![arn],
        "case {case:?}: expected ListEndpoints to report exactly the created endpoint"
    );
}

#[tokio::test]
async fn an_empty_store_lists_no_endpoint() {
    let comprehend = Comprehend::new();

    let output = list(&comprehend, &json!({})).await;
    assert_eq!(
        listed_arns(&output),
        Vec::<String>::new(),
        "case \"an empty store\": expected no endpoint, got {output}"
    );
    assert!(
        output["NextToken"].is_null(),
        "case \"an empty store\": expected no NextToken, got {output}"
    );
}

#[tokio::test]
async fn list_endpoints_reports_every_persisted_endpoint_in_creation_order() {
    let comprehend = Comprehend::new();
    let base = base_time();
    let seeded = seed(&comprehend, base);

    let output = list(&comprehend, &json!({})).await;
    assert_eq!(
        listed_arns(&output),
        arns_of(&seeded, &[0, 1, 2, 3, 4]),
        "case \"no filter\": expected every seeded endpoint in creation order, got {output}"
    );

    // Every member of the record is projected, for every row.
    let items = output["EndpointPropertiesList"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    for (record, item) in seeded.iter().zip(items.iter()) {
        let case = &record.endpoint_name;
        assert_eq!(
            member_str(item, "Status").as_deref(),
            Some(record.status.as_str()),
            "case {case:?}: expected the persisted status, got {item}"
        );
        assert_eq!(
            member_str(item, "ModelArn"),
            record.model_arn.clone(),
            "case {case:?}: expected the persisted model ARN, got {item}"
        );
        assert_eq!(
            item["DesiredInferenceUnits"],
            json!(record.desired_inference_units),
            "case {case:?}: expected the persisted desired capacity, got {item}"
        );
        assert_eq!(
            item["CreationTime"].as_i64(),
            Some(epoch_secs(record.creation_time)),
            "case {case:?}: expected the persisted creation time, got {item}"
        );
        assert_eq!(
            member_str(item, "Message"),
            record.message.clone(),
            "case {case:?}: expected the persisted message, got {item}"
        );
        assert_eq!(
            item["CurrentInferenceUnits"].as_i64(),
            record.current_inference_units.map(i64::from),
            "case {case:?}: expected the persisted serving capacity, got {item}"
        );
    }
}

#[tokio::test]
async fn list_endpoints_honours_the_single_filter_the_request_sets() {
    let comprehend = Comprehend::new();
    let base = base_time();
    let seeded = seed(&comprehend, base);
    let (model_a, model_b) = seeded_model_arns();
    let bound = epoch_secs(base + 2 * CREATION_GAP);

    let cases: Vec<(&str, Value, Vec<usize>)> = vec![
        ("ModelArn: the first model", json!({ "ModelArn": model_a }), vec![0, 2, 4]),
        ("ModelArn: the second model", json!({ "ModelArn": model_b }), vec![1, 3]),
        (
            "ModelArn: a model with no endpoint",
            json!({ "ModelArn": document_classifier_arn(REGION, "unused-classifier", None) }),
            vec![],
        ),
        ("Status: IN_SERVICE", json!({ "Status": "IN_SERVICE" }), vec![0, 3]),
        ("Status: CREATING", json!({ "Status": "CREATING" }), vec![1, 4]),
        ("Status: FAILED", json!({ "Status": "FAILED" }), vec![2]),
        // A modelled status no seeded record carries: the filter selects nothing
        // rather than everything.
        ("Status: DELETING", json!({ "Status": "DELETING" }), vec![]),
        (
            "CreationTimeBefore: the third record's instant",
            json!({ "CreationTimeBefore": bound }),
            vec![0, 1],
        ),
        (
            "CreationTimeAfter: the third record's instant",
            json!({ "CreationTimeAfter": bound }),
            vec![3, 4],
        ),
        (
            "CreationTimeBefore: before every record",
            json!({ "CreationTimeBefore": epoch_secs(base) }),
            vec![],
        ),
        (
            "CreationTimeAfter: after every record",
            json!({ "CreationTimeAfter": epoch_secs(base + 4 * CREATION_GAP) }),
            vec![],
        ),
    ];

    for (case, filter, rows) in cases {
        let output = list(&comprehend, &json!({ "Filter": filter })).await;
        assert_eq!(
            listed_arns(&output),
            arns_of(&seeded, &rows),
            "case {case:?}: the filter selected the wrong set, got {output}"
        );
    }
}

/// "Specifies a date before/after which the returned endpoint or endpoints were
/// created": a record created exactly AT the bound is on neither side of it.
#[tokio::test]
async fn the_creation_time_bounds_are_strict() {
    let comprehend = Comprehend::new();
    let base = base_time();
    let seeded = seed(&comprehend, base);

    for record in &seeded {
        let bound = epoch_secs(record.creation_time);
        let case = &record.endpoint_name;

        for member in ["CreationTimeBefore", "CreationTimeAfter"] {
            let listed = page_through(
                &comprehend,
                &json!({ "Filter": { member: bound } }),
                500,
            )
            .await;
            assert!(
                !listed.contains(&record.endpoint_arn),
                "case {case:?}: {member}={bound} must exclude the endpoint created at it, \
                 got {listed:?}"
            );
            let expected: Vec<String> = seeded
                .iter()
                .filter(|other| {
                    let created = epoch_secs(other.creation_time);
                    if member == "CreationTimeBefore" {
                        created < bound
                    } else {
                        created > bound
                    }
                })
                .map(|other| other.endpoint_arn.clone())
                .collect();
            assert_eq!(
                listed, expected,
                "case {case:?}: {member}={bound} selected the wrong set"
            );
        }
    }
}

#[tokio::test]
async fn list_endpoints_pages_at_max_results_and_resumes_after_the_last_item() {
    let comprehend = Comprehend::new();
    let base = base_time();
    let seeded = seed(&comprehend, base);
    let every = arns_of(&seeded, &[0, 1, 2, 3, 4]);

    for page_size in [1usize, 2, 3, 5, 500] {
        let listed = page_through(&comprehend, &json!({}), page_size).await;
        assert_eq!(
            listed, every,
            "case MaxResults={page_size}: paging must serve every endpoint exactly once, \
             in creation order"
        );
    }

    // The first page of a truncated request carries a token; the page that exhausts
    // the set carries none.
    let first = list(&comprehend, &json!({ "MaxResults": 2 })).await;
    assert_eq!(
        listed_arns(&first),
        arns_of(&seeded, &[0, 1]),
        "case MaxResults=2: expected the two oldest endpoints, got {first}"
    );
    let token = first["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("case MaxResults=2: expected a NextToken, got {first}"))
        .to_string();
    let second = list(
        &comprehend,
        &json!({ "MaxResults": 3, "NextToken": token }),
    )
    .await;
    assert_eq!(
        listed_arns(&second),
        arns_of(&seeded, &[2, 3, 4]),
        "case \"resume\": expected the page to resume after the first one, got {second}"
    );
    assert!(
        second["NextToken"].is_null(),
        "case \"resume\": the last page carries no NextToken, got {second}"
    );

    // A filter and a page size compose: the token resumes within the filtered set.
    let filtered = page_through(
        &comprehend,
        &json!({ "Filter": { "Status": "CREATING" } }),
        1,
    )
    .await;
    assert_eq!(
        filtered,
        arns_of(&seeded, &[1, 4]),
        "case \"MaxResults=1 within a filter\": expected the filtered set, in order"
    );
}

#[tokio::test]
async fn a_bad_list_request_is_refused() {
    let comprehend = Comprehend::new();
    let base = base_time();
    let seeded = seed(&comprehend, base);
    let (model_a, _) = seeded_model_arns();
    let bound = epoch_secs(base + 2 * CREATION_GAP);

    // Every pair of filter members is more than the one filter the shape allows, and
    // a token has to be one this service issued.
    let cases: Vec<(&str, Value)> = vec![
        (
            "two filters: ModelArn and Status",
            json!({ "Filter": { "ModelArn": model_a, "Status": "IN_SERVICE" } }),
        ),
        (
            "two filters: both creation-time bounds",
            json!({ "Filter": { "CreationTimeBefore": bound, "CreationTimeAfter": bound } }),
        ),
        (
            "four filters",
            json!({ "Filter": {
                "ModelArn": seeded[0].model_arn.clone(),
                "Status": "CREATING",
                "CreationTimeBefore": bound,
                "CreationTimeAfter": bound,
            } }),
        ),
        ("a token that is not hex", json!({ "NextToken": "not-a-token" })),
        ("a token of the wrong length", json!({ "NextToken": "00ff" })),
        (
            "a well-formed token naming no page",
            json!({ "NextToken": "00000000000000ff" }),
        ),
    ];

    for (case, request) in cases {
        let (status, body) = comprehend.call("ListEndpoints", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "case {case:?}: expected InvalidRequestException, got {body}"
        );
    }
}

#[tokio::test]
async fn a_create_that_names_no_usable_model_is_refused() {
    let comprehend = Comprehend::new();
    let model_arn = seed_model(&comprehend, "reviews-classifier", "TRAINED");

    // (case, body, expected HTTP status, expected error code, the message the rule
    // requires — `None` where the message is an example rather than a rule)
    //
    // THE RULE for the not-found rows: an ARN that names no resource this account
    // holds is refused with the service's ONE fixed, value-free sentence
    // ([`RESOURCE_NOT_FOUND_MESSAGE`], `crate::state::RESOURCE_NOT_FOUND_MESSAGE`),
    // whichever resource kind the ARN named — a classifier, a model version, a
    // recognizer or a flywheel — so no row discloses more about the request than
    // another.
    let cases: Vec<(&str, Value, u16, &str, Option<&str>)> = vec![
        (
            "a classifier ARN that was never created",
            json!({
                "EndpointName": "missing-model-endpoint",
                "ModelArn": document_classifier_arn(REGION, "never-created", None),
                "DesiredInferenceUnits": 1,
            }),
            400,
            "ResourceNotFoundException",
            Some(RESOURCE_NOT_FOUND_MESSAGE),
        ),
        (
            "a version of a model that exists unversioned",
            json!({
                "EndpointName": "missing-version-endpoint",
                "ModelArn": document_classifier_arn(REGION, "reviews-classifier", Some("v9")),
                "DesiredInferenceUnits": 1,
            }),
            400,
            "ResourceNotFoundException",
            Some(RESOURCE_NOT_FOUND_MESSAGE),
        ),
        (
            "an entity-recognizer model this emulator persists none of",
            json!({
                "EndpointName": "recognizer-endpoint",
                "ModelArn": format!(
                    "arn:aws:comprehend:{REGION}:{ACCOUNT_ID}:entity-recognizer/recognizer"
                ),
                "DesiredInferenceUnits": 1,
            }),
            400,
            "ResourceNotFoundException",
            Some(RESOURCE_NOT_FOUND_MESSAGE),
        ),
        (
            "a flywheel this emulator persists none of",
            json!({
                "EndpointName": "flywheel-endpoint",
                "FlywheelArn": format!(
                    "arn:aws:comprehend:{REGION}:{ACCOUNT_ID}:flywheel/reviews-flywheel"
                ),
                "DesiredInferenceUnits": 1,
            }),
            400,
            "ResourceNotFoundException",
            Some(RESOURCE_NOT_FOUND_MESSAGE),
        ),
        // The refusal for a request that named NO resource ARN at all is built from
        // the request: `null`.
        (
            "neither a model nor a flywheel",
            json!({
                "EndpointName": "unattached-endpoint",
                "DesiredInferenceUnits": 1,
            }),
            400,
            "InvalidRequestException",
            Some("Invalid resource ARN: null"),
        ),
        // The recorded case: `DesiredInferenceUnits` is `@range min 1`, so the
        // framework refuses the request before the handler ever runs.
        (
            "a capacity below the modelled minimum",
            json!({
                "EndpointName": "zero-capacity-endpoint",
                "ModelArn": model_arn,
                "DesiredInferenceUnits": 0,
            }),
            400,
            "ValidationException",
            None,
        ),
    ];

    for (case, body, expected_status, expected_code, expected_message) in cases {
        let (status, response) = comprehend.call("CreateEndpoint", &body).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status}: {response}"
        );
        assert_eq!(
            error_code(&response),
            expected_code,
            "case {case:?}: expected {expected_code}, got {response}"
        );
        let message = error_message(&response);
        assert!(
            !message.is_empty(),
            "case {case:?}: expected a refusal message, got {response}"
        );
        if let Some(expected) = expected_message {
            assert_eq!(
                message, expected,
                "case {case:?}: expected the message {expected:?}, got {message:?}"
            );
        }
    }

    // A refused creation persists nothing.
    let listed = page_through(&comprehend, &json!({}), 500).await;
    assert_eq!(
        listed,
        Vec::<String>::new(),
        "case \"a refused creation\": expected no endpoint to be persisted, got {listed:?}"
    );
}

#[tokio::test]
async fn one_name_names_one_endpoint_and_a_repeated_token_resolves_to_it() {
    let comprehend = Comprehend::new();
    let model_arn = seed_model(&comprehend, "reviews-classifier", "TRAINED");

    let body = json!({
        "EndpointName": "contended-endpoint",
        "ModelArn": model_arn,
        "DesiredInferenceUnits": 1,
    });
    let arn = create(&comprehend, "the first creation", &body).await;

    // A second creation under the same name, with no token, collides.
    let (status, response) = comprehend.call("CreateEndpoint", &body).await;
    assert_eq!(
        status, 400,
        "case \"the same name again\": expected HTTP 400, got {status}: {response}"
    );
    assert_eq!(
        error_code(&response),
        "ResourceInUseException",
        "case \"the same name again\": expected ResourceInUseException, got {response}"
    );

    // Under a `ClientRequestToken`, the retry of one creation resolves to the
    // endpoint that creation already made — the first call's ARN, once.
    let mut tokened = body.clone();
    tokened["EndpointName"] = json!("tokened-endpoint");
    tokened["ClientRequestToken"] = json!("one-token");
    let first = create(&comprehend, "the first call under a token", &tokened).await;
    for case in ["the same token again", "the same token a third time"] {
        let repeated = create(&comprehend, case, &tokened).await;
        assert_eq!(
            repeated, first,
            "case {case:?}: expected the retry to resolve to {first}"
        );
    }

    let listed = page_through(&comprehend, &json!({}), 500).await;
    assert_eq!(
        listed,
        vec![arn, first],
        "case \"retries under one token\": expected exactly two endpoints to be persisted"
    );
}

#[tokio::test]
async fn describing_an_endpoint_that_does_not_exist_is_refused() {
    let comprehend = Comprehend::new();
    let model_arn = seed_model(&comprehend, "reviews-classifier", "TRAINED");
    create(
        &comprehend,
        "a created endpoint",
        &json!({
            "EndpointName": "described-endpoint",
            "ModelArn": model_arn,
            "DesiredInferenceUnits": 1,
        }),
    )
    .await;

    let cases: Vec<(&str, String)> = vec![
        (
            "a name no endpoint was created under",
            endpoint_arn(REGION, "document-classifier-endpoint", "never-created"),
        ),
        (
            "the other endpoint resource type",
            endpoint_arn(REGION, "entity-recognizer-endpoint", "described-endpoint"),
        ),
        (
            "the same name in another Region",
            endpoint_arn("us-east-2", "document-classifier-endpoint", "described-endpoint"),
        ),
    ];

    for (case, arn) in cases {
        let (status, body) = comprehend
            .call("DescribeEndpoint", &json!({ "EndpointArn": &arn }))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body}"
        );
        // The service's ONE fixed sentence, whichever ARN was asked for — and,
        // stated as the property it is, the ARN the request carried appears nowhere
        // in the refusal.
        let message = error_message(&body);
        assert_eq!(
            message, RESOURCE_NOT_FOUND_MESSAGE,
            "case {case:?}: expected the service's fixed sentence, got {body}"
        );
        assert!(
            !message.contains(&arn),
            "case {case:?}: the refusal echoed the submitted ARN {arn:?}: {message:?}"
        );
    }
}
