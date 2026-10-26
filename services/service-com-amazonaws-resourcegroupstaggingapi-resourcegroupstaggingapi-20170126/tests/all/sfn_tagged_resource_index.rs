// SPDX-License-Identifier: CC0-1.0

//! AWS Step Functions as an OWNING service of this API: every resource type that
//! service holds a tag set for — a state machine AND an activity — is a row of
//! this API's projection, under the `service:resourceType` key its own ARN
//! carries, so a resource created with `tags` is discoverable through
//! `GetResources` with no tagging call in between and a tag this API writes is
//! the same tag that service's `ListTagsForResource` reads back.
//!
//! Every test drives the real wire path — the multi-service router, `POST /` with
//! an `X-Amz-Target` and an awsJson body — over ONE engine carrying both
//! services, which is how the emulator runs.
//!
//! The tables span the class the rule generalises over: WHICH taggable Step
//! Functions resource type holds the tags (and with it the ARN form and the
//! resource-type key), how many tags the create carried, resources carrying no
//! tag at all, and an ARN of each kind that names nothing. The state machine
//! created with a single tag is one row.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_resourcegroupstaggingapi_resourcegroupstaggingapi_20170126::EmulatedService;
use service_com_amazonaws_sfn_awsstepfunctions as owner_sfn;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for, and therefore the one both
/// endpoints serve.
const REGION: &str = "us-east-1";

/// The account segment every emulated ARN carries.
const ACCOUNT_ID: &str = engine::EMULATOR_ACCOUNT_ID;

/// The SigV4 signing names the router routes on.
const TAGGING: &str = "tagging";
const STATES: &str = "states";

/// A role ARN inside the modelled `Arn` length, for the creates below.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/StepFunctionsRole";

/// A definition that succeeds immediately with a static result.
const HELLO_DEFINITION: &str =
    r#"{"StartAt":"Hello","States":{"Hello":{"Type":"Pass","Result":{"msg":"hi"},"End":true}}}"#;

/// Long enough that a started state-machine deletion has fallen due, taken from
/// the owning service's own span rather than guessed at.
const PAST_THE_DELETE_DELAY: Duration =
    owner_sfn::state::DELETE_SETTLE_DELAY.saturating_add(Duration::from_millis(300));

/// A router carrying the tagging API and Step Functions over ONE engine — the
/// arrangement `core/router` builds.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The tests exercise handler behaviour, not SigV4: the `Authorization` header
    // below is present because it is what routes a request to a signing name, not
    // to be verified.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(owner_sfn::EmulatedService::new(engine));
    router
}

/// Call one operation of one service and return its HTTP status and parsed body.
async fn call(
    router: &MultiServiceRouter,
    signing_name: &str,
    target: &str,
    body: Value,
) -> (u16, Value) {
    let region = REGION;
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", target)
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/{signing_name}/\
                 aws4_request, SignedHeaders=host;x-amz-target, Signature=0"
            ),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("build the request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or_else(|error| {
            panic!(
                "{target} answered a body that is not JSON ({error}): {}",
                String::from_utf8_lossy(response.body())
            )
        })
    };
    (status, parsed)
}

/// One tagging-API operation.
async fn tagging(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    call(
        router,
        TAGGING,
        &format!("ResourceGroupsTaggingAPI_20170126.{operation}"),
        body,
    )
    .await
}

/// One tagging-API operation that must answer 200.
async fn tagging_ok(router: &MultiServiceRouter, operation: &str, body: Value) -> Value {
    let (status, parsed) = tagging(router, operation, body).await;
    assert_eq!(
        status, 200,
        "{operation} must answer 200, got {status} with body {parsed}"
    );
    parsed
}

/// One Step Functions operation.
async fn states(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    call(
        router,
        STATES,
        &format!("AWSStepFunctions.{operation}"),
        body,
    )
    .await
}

/// One Step Functions operation that must answer 200.
async fn states_ok(router: &MultiServiceRouter, operation: &str, body: Value) -> Value {
    let (status, parsed) = states(router, operation, body).await;
    assert_eq!(
        status, 200,
        "{operation} must answer 200, got {status} with body {parsed}"
    );
    parsed
}

/// The ARNs of a `GetResources` page, in the order the service returned them.
fn page_arns(body: &Value) -> Vec<String> {
    body["ResourceTagMappingList"]
        .as_array()
        .unwrap_or_else(|| panic!("a GetResources body with no ResourceTagMappingList: {body}"))
        .iter()
        .map(|mapping| {
            mapping["ResourceARN"]
                .as_str()
                .unwrap_or_else(|| panic!("a mapping with no ResourceARN: {mapping}"))
                .to_string()
        })
        .collect()
}

/// The tag map a `GetResources` page reports for one ARN.
fn page_tags(body: &Value, arn: &str) -> BTreeMap<String, String> {
    let mapping = body["ResourceTagMappingList"]
        .as_array()
        .unwrap_or_else(|| panic!("a GetResources body with no ResourceTagMappingList: {body}"))
        .iter()
        .find(|mapping| mapping["ResourceARN"] == json!(arn))
        .unwrap_or_else(|| panic!("{arn} is not in the page: {body}"));
    mapping["Tags"]
        .as_array()
        .unwrap_or_else(|| panic!("a mapping with no Tags: {mapping}"))
        .iter()
        .map(|tag| {
            (
                tag["Key"].as_str().expect("Tag.Key").to_string(),
                tag["Value"].as_str().expect("Tag.Value").to_string(),
            )
        })
        .collect()
}

/// `GetResources` restricted to one ARN.
async fn resources_for(router: &MultiServiceRouter, arn: &str) -> Value {
    tagging_ok(router, "GetResources", json!({ "ResourceARNList": [arn] })).await
}

/// The ARNs `GetResources` reports for one `ResourceTypeFilters` value, sorted.
async fn arns_of_type(router: &MultiServiceRouter, filters: &[&str]) -> Vec<String> {
    let body = tagging_ok(
        router,
        "GetResources",
        json!({ "ResourceTypeFilters": filters }),
    )
    .await;
    let mut arns = page_arns(&body);
    arns.sort();
    arns
}

/// The tag map Step Functions' own `ListTagsForResource` reports for one ARN.
async fn states_tags(router: &MultiServiceRouter, arn: &str) -> BTreeMap<String, String> {
    let body = states_ok(router, "ListTagsForResource", json!({ "resourceArn": arn })).await;
    body["tags"]
        .as_array()
        .unwrap_or_else(|| panic!("a ListTagsForResource body with no tags: {body}"))
        .iter()
        .map(|tag| {
            (
                tag["key"].as_str().expect("tag.key").to_string(),
                tag["value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// A tag map as the expected `BTreeMap`.
fn expected(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// A Step Functions ARN of the form this emulator mints, in the serving Region
/// and account: the resource type and the name are separated by `:`.
fn states_arn(kind: &str, name: &str) -> String {
    format!("arn:aws:states:{REGION}:{ACCOUNT_ID}:{kind}:{name}")
}

/// One taggable Step Functions resource: the create that makes it, the ARN it
/// answers to, the tags that create recorded and the resource-type key this API
/// names its kind by.
struct Row {
    case: &'static str,
    operation: &'static str,
    request: Value,
    arn: String,
    /// The member of the create's output that carries the minted ARN.
    arn_member: &'static str,
    /// The `service:resourceType` key a qualified `ResourceTypeFilters` entry
    /// selects this resource by.
    resource_type: &'static str,
    tags: &'static [(&'static str, &'static str)],
}

/// The taggable kinds, spanning the tag counts a create can carry: the two
/// resource types Step Functions holds a tag set for ("the ARN for the Step
/// Functions state machine or activity", `TagResource`'s `resourceArn`), each
/// created with tags and each created with none. The first row is a state machine
/// created with a single tag.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a state machine with one tag",
            operation: "CreateStateMachine",
            request: json!({
                "name": "tagged-machine",
                "definition": HELLO_DEFINITION,
                "roleArn": ROLE_ARN,
                "tags": [{ "key": "k", "value": "v" }],
            }),
            arn: states_arn("stateMachine", "tagged-machine"),
            arn_member: "stateMachineArn",
            resource_type: "states:stateMachine",
            tags: &[("k", "v")],
        },
        Row {
            case: "a state machine with three tags",
            operation: "CreateStateMachine",
            request: json!({
                "name": "Billing_Pipeline",
                "definition": HELLO_DEFINITION,
                "roleArn": ROLE_ARN,
                "tags": [
                    { "key": "Environment", "value": "prod" },
                    { "key": "Project", "value": "project1" },
                    { "key": "owner", "value": "billing" },
                ],
            }),
            arn: states_arn("stateMachine", "Billing_Pipeline"),
            arn_member: "stateMachineArn",
            resource_type: "states:stateMachine",
            tags: &[
                ("Environment", "prod"),
                ("Project", "project1"),
                ("owner", "billing"),
            ],
        },
        Row {
            case: "a state machine created with no tags",
            operation: "CreateStateMachine",
            request: json!({
                "name": "untagged-machine",
                "definition": HELLO_DEFINITION,
                "roleArn": ROLE_ARN,
            }),
            arn: states_arn("stateMachine", "untagged-machine"),
            arn_member: "stateMachineArn",
            resource_type: "states:stateMachine",
            tags: &[],
        },
        Row {
            case: "an activity with one tag",
            operation: "CreateActivity",
            request: json!({
                "name": "tagged-activity",
                "tags": [{ "key": "k", "value": "v" }],
            }),
            arn: states_arn("activity", "tagged-activity"),
            arn_member: "activityArn",
            resource_type: "states:activity",
            tags: &[("k", "v")],
        },
        Row {
            case: "an activity with two tags",
            operation: "CreateActivity",
            request: json!({
                "name": "Invoice.Review",
                "tags": [
                    { "key": "Environment", "value": "dev" },
                    { "key": "team", "value": "finance" },
                ],
            }),
            arn: states_arn("activity", "Invoice.Review"),
            arn_member: "activityArn",
            resource_type: "states:activity",
            tags: &[("Environment", "dev"), ("team", "finance")],
        },
        Row {
            case: "an activity created with no tags",
            operation: "CreateActivity",
            request: json!({ "name": "untagged-activity" }),
            arn: states_arn("activity", "untagged-activity"),
            arn_member: "activityArn",
            resource_type: "states:activity",
            tags: &[],
        },
    ]
}

/// Create one row's resource and require the ARN the owning service minted to be
/// the one this API addresses it by.
async fn create(router: &MultiServiceRouter, row: &Row) {
    let body = states_ok(router, row.operation, row.request.clone()).await;
    let minted = body[row.arn_member].as_str().unwrap_or_else(|| {
        panic!(
            "case {}: expected {} in the create's body, got {body}",
            row.case, row.arn_member
        )
    });
    assert_eq!(
        minted, row.arn,
        "case {}: expected the create to mint {}, got {minted}",
        row.case, row.arn
    );
}

/// The tags a Step Functions create recorded are the resource's own, so
/// `GetResources` queried by that resource's ARN reports exactly one mapping
/// carrying every pair the create carried — with no tagging call in between — and
/// that service's own `ListTagsForResource` reports the same pairs, because there
/// is ONE tag set and both operations read it.
///
/// A resource created with no tags is in neither answer's mapping list:
/// "GetResources does not return untagged resources."
#[tokio::test]
async fn create_time_step_functions_tags_are_reported_by_get_resources() {
    let router = emulator();
    for row in rows() {
        create(&router, &row).await;
        let want = expected(row.tags);

        let page = resources_for(&router, &row.arn).await;
        let arns = page_arns(&page);
        if row.tags.is_empty() {
            assert!(
                arns.is_empty(),
                "case {}: an untagged resource must not be reported, got {arns:?}",
                row.case
            );
        } else {
            assert_eq!(
                arns,
                vec![row.arn.clone()],
                "case {}: expected exactly one mapping for {}, got {arns:?}",
                row.case,
                row.arn
            );
            let got = page_tags(&page, &row.arn);
            assert_eq!(
                got, want,
                "case {}: expected GetResources to report {want:?} for {}, got {got:?}",
                row.case, row.arn
            );
        }

        let echoed = states_tags(&router, &row.arn).await;
        assert_eq!(
            echoed, want,
            "case {}: expected ListTagsForResource to report {want:?} for {}, got {echoed:?}",
            row.case, row.arn
        );
    }
}

/// `ResourceTypeFilters` select Step Functions resources by the
/// `service[:resourceType]` key their ARN carries: the bare service token selects
/// every tagged resource of the service — both taggable kinds — and a qualified
/// token only that kind. A kind nothing tagged is an empty page, not an error.
#[tokio::test]
async fn resource_type_filters_select_each_taggable_step_functions_kind() {
    let router = emulator();
    for row in rows() {
        create(&router, &row).await;
    }

    /// Every tagged row whose resource type key is `kind`, ARN-sorted.
    fn tagged_of_kind(kind: &str) -> Vec<String> {
        let mut arns: Vec<String> = rows()
            .iter()
            .filter(|row| !row.tags.is_empty() && row.resource_type == kind)
            .map(|row| row.arn.clone())
            .collect();
        arns.sort();
        arns
    }
    let mut every_tagged: Vec<String> = rows()
        .iter()
        .filter(|row| !row.tags.is_empty())
        .map(|row| row.arn.clone())
        .collect();
    every_tagged.sort();

    let cases: Vec<(&str, Vec<&str>, Vec<String>)> = vec![
        ("the bare service token", vec!["states"], every_tagged),
        (
            "only state machines",
            vec!["states:stateMachine"],
            tagged_of_kind("states:stateMachine"),
        ),
        (
            "only activities",
            vec!["states:activity"],
            tagged_of_kind("states:activity"),
        ),
        (
            "both qualified kinds",
            vec!["states:activity", "states:stateMachine"],
            {
                let mut both = tagged_of_kind("states:activity");
                both.extend(tagged_of_kind("states:stateMachine"));
                both.sort();
                both
            },
        ),
        (
            // An execution carries no tag set of its own, so nothing answers to
            // this kind.
            "a kind nothing tagged",
            vec!["states:execution"],
            Vec::new(),
        ),
    ];
    for (case, filters, want) in cases {
        let got = arns_of_type(&router, &filters).await;
        assert_eq!(
            got, want,
            "case {case}: expected ResourceTypeFilters={filters:?} to select {want:?}, got {got:?}"
        );
    }
}

/// `TagFilters` select Step Functions resources by the tags the owning service
/// holds for them: a key alone matches that key with any value, a key with values
/// matches only those values, and the filters are AND-ed across keys.
#[tokio::test]
async fn tag_filters_select_step_functions_resources_by_their_own_tags() {
    let router = emulator();
    for row in rows() {
        create(&router, &row).await;
    }

    let machine = states_arn("stateMachine", "tagged-machine");
    let activity = states_arn("activity", "tagged-activity");
    let pipeline = states_arn("stateMachine", "Billing_Pipeline");
    let review = states_arn("activity", "Invoice.Review");

    // Rows differ along the shape of the filter: a bare key (shared by one
    // resource of each kind), a key with the matching value, a key with a value
    // nothing carries, a key no resource carries, and two filters AND-ed.
    let cases: Vec<(&str, Value, Vec<String>)> = vec![
        (
            "a bare key both kinds carry",
            json!([{ "Key": "k" }]),
            {
                let mut want = vec![activity.clone(), machine.clone()];
                want.sort();
                want
            },
        ),
        (
            "a key with the stored value",
            json!([{ "Key": "k", "Values": ["v"] }]),
            {
                let mut want = vec![activity.clone(), machine.clone()];
                want.sort();
                want
            },
        ),
        (
            "a key with a value nothing carries",
            json!([{ "Key": "k", "Values": ["other"] }]),
            Vec::new(),
        ),
        (
            "a key one value of which one resource carries",
            json!([{ "Key": "Environment", "Values": ["dev"] }]),
            vec![review.clone()],
        ),
        (
            "two values OR-ed inside one filter",
            json!([{ "Key": "Environment", "Values": ["dev", "prod"] }]),
            {
                let mut want = vec![pipeline.clone(), review.clone()];
                want.sort();
                want
            },
        ),
        (
            "two filters AND-ed",
            json!([{ "Key": "Environment", "Values": ["prod"] }, { "Key": "owner" }]),
            vec![pipeline.clone()],
        ),
        (
            "a key no resource carries",
            json!([{ "Key": "absent" }]),
            Vec::new(),
        ),
    ];
    for (case, filters, want) in cases {
        let body = tagging_ok(&router, "GetResources", json!({ "TagFilters": filters })).await;
        let mut got = page_arns(&body);
        got.sort();
        assert_eq!(
            got, want,
            "case {case}: expected TagFilters={filters} to select {want:?}, got {got:?}"
        );
    }
}

/// Step Functions holds the tag set of both resource types this API can tag, so
/// each one has ONE tag home: that service's own record. A tag written through
/// this API is the resource's own tag, which `ListTagsForResource` reports; the
/// keys the create recorded and the keys this API did not name are left exactly as
/// they were; and an untag removes the key from that same record, which
/// `GetResources` then reports without it.
///
/// The rows differ along the resource kind, so the rule is shown to be
/// per-resource and not per-service.
#[tokio::test]
async fn step_functions_tags_live_in_that_services_own_records() {
    let router = emulator();
    for row in rows() {
        create(&router, &row).await;

        let body = tagging_ok(
            &router,
            "TagResources",
            json!({ "ResourceARNList": [&row.arn], "Tags": { "CostCentre": "cc-42" } }),
        )
        .await;
        assert_eq!(
            body["FailedResourcesMap"],
            json!({}),
            "case {}: expected no failed resource for {}, got {}",
            row.case,
            row.arn,
            body["FailedResourcesMap"]
        );

        let mut want = expected(row.tags);
        want.insert("CostCentre".to_string(), "cc-42".to_string());
        let got = states_tags(&router, &row.arn).await;
        assert_eq!(
            got, want,
            "case {}: expected ListTagsForResource to report {want:?} for {}, got {got:?}",
            row.case, row.arn
        );
        let page = resources_for(&router, &row.arn).await;
        let got = page_tags(&page, &row.arn);
        assert_eq!(
            got, want,
            "case {}: expected GetResources to report {want:?} for {}, got {got:?}",
            row.case, row.arn
        );

        let body = tagging_ok(
            &router,
            "UntagResources",
            json!({ "ResourceARNList": [&row.arn], "TagKeys": ["CostCentre"] }),
        )
        .await;
        assert_eq!(
            body["FailedResourcesMap"],
            json!({}),
            "case {}: expected no failed resource for {}, got {}",
            row.case,
            row.arn,
            body["FailedResourcesMap"]
        );
        let want = expected(row.tags);
        let got = states_tags(&router, &row.arn).await;
        assert_eq!(
            got, want,
            "case {}: expected the untag to leave {want:?} on {}, got {got:?}",
            row.case, row.arn
        );
    }
}

/// A Step Functions ARN this API cannot resolve is answered per RESOURCE, with
/// that service's own refusal: a well-formed ARN of either taggable kind that
/// names nothing gets its `ResourceNotFound` with `StatusCode` 400, and the other
/// ARNs of the same batch are tagged.
#[tokio::test]
async fn an_unresolvable_step_functions_arn_fails_with_that_services_refusal() {
    let router = emulator();
    let present = rows().remove(0);
    create(&router, &present).await;

    for (case, arn) in [
        (
            "a state machine that was never created",
            states_arn("stateMachine", "ghost"),
        ),
        (
            "an activity that was never created",
            states_arn("activity", "ghost"),
        ),
    ] {
        let body = tagging_ok(
            &router,
            "TagResources",
            json!({
                "ResourceARNList": [&arn, &present.arn],
                "Tags": { "Project": "project1" },
            }),
        )
        .await;
        let failure = &body["FailedResourcesMap"][arn.as_str()];
        assert_eq!(
            failure["ErrorCode"],
            json!("ResourceNotFound"),
            "case {case}: expected ResourceNotFound for {arn}, got {}",
            body["FailedResourcesMap"]
        );
        assert_eq!(
            failure["StatusCode"],
            json!(400),
            "case {case}: expected StatusCode 400 for {arn}, got {failure}"
        );
        assert_eq!(
            body["FailedResourcesMap"]
                .as_object()
                .map(serde_json::Map::len),
            Some(1),
            "case {case}: the other ARN of the batch must be tagged, got {}",
            body["FailedResourcesMap"]
        );
    }
}

/// A state machine whose started deletion has fallen due is gone from the owning
/// service, so this API stops reporting it — while the activity beside it, which
/// no deletion touched, keeps its row.
#[tokio::test]
async fn a_state_machine_whose_deletion_came_due_leaves_the_index() {
    let router = emulator();
    let machine = rows().remove(0);
    let activity = rows().remove(3);
    create(&router, &machine).await;
    create(&router, &activity).await;

    states_ok(
        &router,
        "DeleteStateMachine",
        json!({ "stateMachineArn": &machine.arn }),
    )
    .await;

    // "This is an asynchronous operation": the deletion is not applied by the call
    // that starts it, so the ARN is still reported until that span falls due.
    let still_there = arns_of_type(&router, &["states:stateMachine"]).await;
    assert_eq!(
        still_there,
        vec![machine.arn.clone()],
        "case deletion started: expected {} to still be reported, got {still_there:?}",
        machine.arn
    );

    tokio::time::sleep(PAST_THE_DELETE_DELAY).await;

    let after = arns_of_type(&router, &["states:stateMachine"]).await;
    assert!(
        after.is_empty(),
        "case deletion due: expected the index to drop {}, got {after:?}",
        machine.arn
    );
    let activities = arns_of_type(&router, &["states:activity"]).await;
    assert_eq!(
        activities,
        vec![activity.arn.clone()],
        "case deletion due: expected {} to be untouched, got {activities:?}",
        activity.arn
    );
}
