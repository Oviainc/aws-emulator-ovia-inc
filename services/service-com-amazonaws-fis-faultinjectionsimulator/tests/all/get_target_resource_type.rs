// SPDX-License-Identifier: CC0-1.0

//! AWS FIS `GetTargetResourceType`, end to end over HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the handler and serialization — the same path a client takes. The rule
//! under test is the one FIS states for this operation ("Gets information about the
//! specified resource type"), read against a SERVICE-owned catalog:
//!
//! * a read reports the published resource type the request names — that id echoed, a
//!   description, and the parameter contract of the type, each parameter carrying its own
//!   description and `required` flag — and nothing a caller could have created, because no
//!   API creates, updates or deletes a resource type;
//! * the reported parameter map is present for every published type, EMPTY for a type that
//!   takes no parameters: the type has a parameter contract either way;
//! * an id the catalog does not publish — including a differently cased, truncated or
//!   suffixed one — answers `ResourceNotFoundException` (HTTP 404) rather than a fabricated
//!   resource type, and an id outside `TargetResourceTypeId`'s modelled
//!   `@length(min: 0, max: 128)` is refused with HTTP 400 before any lookup;
//! * reading changes nothing: the same request answers the same body afterwards, and the
//!   read agrees with what `ListTargetResourceTypes` publishes for the same type.
//!
//! SOURCE for the expected rows below: the AWS FIS actions reference
//! (<https://docs.aws.amazon.com/fis/latest/userguide/fis-actions-reference.html>), which
//! publishes the resource types its actions target, and the target documentation those
//! actions point at for the parameters a target of a type may set. The rows are written
//! out here rather than imported from `src/state.rs`, so a wrong entry in that table fails
//! this suite instead of agreeing with it.
//!
//! The HTTP harness below is local to this suite, as each sibling suite's is.

use std::collections::BTreeSet;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::Value;
use service_com_amazonaws_fis_faultinjectionsimulator::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// `TargetResourceTypeId`'s published ceiling (`@length(min: 0, max: 128)`), spelled out
/// here rather than imported from the crate so a wrong bound fails this suite.
const RESOURCE_TYPE_CEILING: usize = 128;

/// `TargetResourceTypeDescription`'s published ceiling (`@length(min: 0, max: 512)`).
const DESCRIPTION_CEILING: usize = 512;

/// `TargetResourceTypeParameterName`'s published ceiling (`@length(min: 0, max: 64)`).
const PARAMETER_NAME_CEILING: usize = 64;

/// The region the test's requests are signed for.
const REGION: &str = "us-east-1";

/// A SigV4 credential scope naming this service. The service field is what routes the
/// request to fis.
fn signed_for(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/fis/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    /// The `targetResourceType` this answer carries.
    fn resource_type(&self) -> &Value {
        &self.body["targetResourceType"]
    }
}

/// One `GetTargetResourceType` call.
async fn get_resource_type(router: &MultiServiceRouter, id: &str) -> Answer {
    let request = ::http::Request::builder()
        .method("GET")
        .uri(format!("/targetResourceTypes/{id}"))
        .header("host", "fis.us-east-1.amazonaws.com")
        .header("authorization", signed_for(REGION))
        .header("content-type", "application/json")
        .body(Bytes::new())
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "GET /targetResourceTypes/{id} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

/// One page of `ListTargetResourceTypes`, used to check the two reports of the same
/// service-owned table agree.
async fn list_page(router: &MultiServiceRouter) -> Vec<Value> {
    let request = ::http::Request::builder()
        .method("GET")
        .uri("/targetResourceTypes?maxResults=100")
        .header("host", "fis.us-east-1.amazonaws.com")
        .header("authorization", signed_for(REGION))
        .header("content-type", "application/json")
        .body(Bytes::new())
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    assert_eq!(
        response.status().as_u16(),
        200,
        "case list: expected HTTP 200 from ListTargetResourceTypes"
    );
    let body: Value = serde_json::from_slice(&response.into_body())
        .expect("ListTargetResourceTypes answers JSON");
    body["targetResourceTypes"]
        .as_array()
        .expect("ListTargetResourceTypes answers a list")
        .clone()
}

/// One expected catalog row: the resource type's id and its parameters as
/// (name, required) pairs.
struct Row {
    id: &'static str,
    parameters: &'static [(&'static str, bool)],
}

/// The rows the rule is asserted over.
///
/// They differ along everything the rule generalises over: how many parameters a resource
/// type takes (none, two, five), whether those parameters are all required or required and
/// optional mixed, which AWS service the id belongs to, and which family of resource it
/// names — compute, containers, storage, databases, networking and identity. The rows with
/// no parameters are the ones that pin the empty-map half of the rule.
fn rows() -> Vec<Row> {
    vec![
        // No parameters: a target of these types is named by its ids, ARNs or tags alone.
        Row {
            id: "aws:ec2:instance",
            parameters: &[],
        },
        Row {
            id: "aws:ec2:autoscaling-group",
            parameters: &[],
        },
        Row {
            id: "aws:ec2:subnet",
            parameters: &[],
        },
        Row {
            id: "aws:ec2:transit-gateway",
            parameters: &[],
        },
        Row {
            id: "aws:dynamodb:global-table",
            parameters: &[],
        },
        Row {
            id: "aws:elasticache:replicationgroup",
            parameters: &[],
        },
        Row {
            id: "aws:iam:role",
            parameters: &[],
        },
        Row {
            id: "aws:lambda:function",
            parameters: &[],
        },
        Row {
            id: "aws:s3:bucket",
            parameters: &[],
        },
        Row {
            id: "aws:rds:cluster",
            parameters: &[],
        },
        // Two parameters, both required: a task is identified through its cluster and its
        // service.
        Row {
            id: "aws:ecs:task",
            parameters: &[("cluster", true), ("service", true)],
        },
        // Five parameters, required and optional mixed: a pod is identified through its
        // cluster, namespace and selector, optionally narrowed by Availability Zone.
        Row {
            id: "aws:eks:pod",
            parameters: &[
                ("availabilityZoneIdentifier", false),
                ("clusterIdentifier", true),
                ("namespace", true),
                ("selectorType", true),
                ("selectorValue", false),
            ],
        },
    ]
}

/// The (name, required) pairs an answered resource type reports, sorted by name.
fn reported_parameters(case: &str, reported: &Value) -> Vec<(String, bool)> {
    let map = reported["parameters"].as_object().unwrap_or_else(|| {
        panic!("case {case:?}: expected a parameters map on the resource type: {reported:?}")
    });
    let mut pairs: Vec<(String, bool)> = map
        .iter()
        .map(|(name, parameter)| {
            let required = parameter["required"].as_bool().unwrap_or_else(|| {
                panic!("case {case:?}: parameter {name:?} carries no required flag: {parameter:?}")
            });
            let description = parameter["description"].as_str().unwrap_or_else(|| {
                panic!("case {case:?}: parameter {name:?} carries no description: {parameter:?}")
            });
            // `TargetResourceTypeParameterDescription` is modelled `@length(0..512)`,
            // `pattern ^[\s\S]+$` — which admits no empty value.
            assert!(
                !description.is_empty() && description.chars().count() <= DESCRIPTION_CEILING,
                "case {case:?}: parameter {name:?} expected a non-empty description of at most \
                 {DESCRIPTION_CEILING} characters, got {description:?}"
            );
            // `TargetResourceTypeParameterName` is modelled `@length(0..64)`,
            // `pattern ^[\S]+$`.
            assert!(
                !name.is_empty()
                    && name.chars().count() <= PARAMETER_NAME_CEILING
                    && !name.chars().any(char::is_whitespace),
                "case {case:?}: expected a non-empty whitespace-free parameter name of at most \
                 {PARAMETER_NAME_CEILING} characters, got {name:?}"
            );
            let members: BTreeSet<&str> = parameter
                .as_object()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: expected an object for {name:?}, got {parameter:?}")
                })
                .keys()
                .map(String::as_str)
                .collect();
            assert_eq!(
                members,
                BTreeSet::from(["description", "required"]),
                "case {case:?}: parameter {name:?} expected only the modelled members, got \
                 {members:?}"
            );
            (name.clone(), required)
        })
        .collect();
    pairs.sort();
    pairs
}

#[tokio::test]
async fn a_published_resource_type_is_reported_with_its_description_and_parameters() {
    let router = emulator();

    for row in rows() {
        let case = row.id;
        let answer = get_resource_type(&router, row.id).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {} with {:?}",
            answer.status, answer.body
        );
        let reported = answer.resource_type();

        assert_eq!(
            reported["resourceType"].as_str(),
            Some(row.id),
            "case {case:?}: expected the requested id echoed, got {reported:?}"
        );

        // `TargetResourceTypeDescription` is modelled `@length(0..512)`, `pattern
        // ^[\s\S]+$` — which admits no empty value.
        let description = reported["description"].as_str().unwrap_or_else(|| {
            panic!("case {case:?}: expected a description on the resource type: {reported:?}")
        });
        assert!(
            !description.is_empty() && description.chars().count() <= DESCRIPTION_CEILING,
            "case {case:?}: expected a non-empty description of at most {DESCRIPTION_CEILING} \
             characters, got {description:?}"
        );

        // The parameter contract is reported for every type, empty for one that takes no
        // parameters.
        let expected: Vec<(String, bool)> = row
            .parameters
            .iter()
            .map(|(name, required)| ((*name).to_string(), *required))
            .collect();
        let reported_pairs = reported_parameters(case, reported);
        assert_eq!(
            reported_pairs, expected,
            "case {case:?}: expected the published parameter contract, got {reported_pairs:?}"
        );

        // A resource type is service-owned: it carries the modelled members and nothing a
        // caller could have stored on it — no ARN, no tags, no timestamps.
        let members: BTreeSet<&str> = reported
            .as_object()
            .unwrap_or_else(|| panic!("case {case:?}: expected an object, got {reported:?}"))
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            members,
            BTreeSet::from(["resourceType", "description", "parameters"]),
            "case {case:?}: expected only the modelled members, got {members:?}"
        );
    }
}

#[tokio::test]
async fn every_listed_resource_type_is_readable_and_the_two_reports_agree() {
    // The invariant that ties the two reports of one service-owned table together: a type
    // the list publishes is a type a read serves, with the same id and description.
    let router = emulator();
    let listed = list_page(&router).await;
    assert!(
        listed.len() > 1,
        "case list: expected the catalog to publish more than one resource type, got {listed:?}"
    );

    for summary in &listed {
        let id = summary["resourceType"]
            .as_str()
            .unwrap_or_else(|| panic!("a listed resource type carries no id: {summary:?}"));
        let answer = get_resource_type(&router, id).await;
        assert_eq!(
            answer.status, 200,
            "case {id:?}: expected HTTP 200 for a listed resource type, got {} with {:?}",
            answer.status, answer.body
        );
        let reported = answer.resource_type();
        assert_eq!(
            reported["resourceType"], summary["resourceType"],
            "case {id:?}: expected the read to report the listed id, got {reported:?}"
        );
        assert_eq!(
            reported["description"], summary["description"],
            "case {id:?}: expected the read to report the listed description, got {reported:?}"
        );
        assert!(
            reported["parameters"].is_object(),
            "case {id:?}: expected a parameters map on the read resource type, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn an_unpublished_resource_type_is_not_found_and_an_over_long_id_is_refused() {
    let router = emulator();

    // Each row is an id the catalog publishes no resource type for: a plain word, a
    // differently cased id, a truncated and a suffixed published id, a type of a service
    // FIS targets under another type, an action id (the sibling catalog's key), an
    // experiment template id, and the longest id the model admits.
    for (case, id) in [
        ("a-plain-word", "nonsense".to_string()),
        ("an-upper-cased-published-id", "AWS:EC2:INSTANCE".to_string()),
        ("a-mixed-cased-published-id", "aws:EC2:Instance".to_string()),
        ("a-truncated-published-id", "aws:ec2:instanc".to_string()),
        ("a-suffixed-published-id", "aws:ec2:instances".to_string()),
        ("a-service-prefix-alone", "aws:ec2".to_string()),
        ("an-unpublished-ec2-type", "aws:ec2:placement-group".to_string()),
        ("an-unpublished-service", "aws:sqs:queue".to_string()),
        ("an-action-id", "aws:ec2:stop-instances".to_string()),
        ("an-experiment-template-id", "EXT000000000001".to_string()),
        ("at-the-id-ceiling", "a".repeat(RESOURCE_TYPE_CEILING)),
    ] {
        let answer = get_resource_type(&router, &id).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?} ({id:?}): expected HTTP 404, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ResourceNotFoundException",
            "case {case:?} ({id:?}): expected ResourceNotFoundException, got {:?}",
            answer.body
        );
    }

    // One past the published ceiling is refused for the bound, before any lookup.
    let too_long = "a".repeat(RESOURCE_TYPE_CEILING + 1);
    let answer = get_resource_type(&router, &too_long).await;
    assert_eq!(
        answer.status, 400,
        "case above-the-id-ceiling: expected HTTP 400, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "ValidationException",
        "case above-the-id-ceiling: expected ValidationException, got {:?}",
        answer.body
    );
}

#[tokio::test]
async fn reading_a_resource_type_changes_nothing() {
    let router = emulator();

    // A read must not change what a later caller sees: each row is read, then read again
    // after reads of its siblings and of an id that is not published at all.
    for row in rows() {
        let case = row.id;
        let before = get_resource_type(&router, row.id).await;
        let _ = get_resource_type(&router, "aws:eks:pod").await;
        let _ = get_resource_type(&router, "aws:ecs:task").await;
        let _ = get_resource_type(&router, "aws:ec2:placement-group").await;
        let after = get_resource_type(&router, row.id).await;
        assert_eq!(
            after.status, before.status,
            "case {case:?}: expected the same status on a second read, got {} after {}",
            after.status, before.status
        );
        assert_eq!(
            after.body, before.body,
            "case {case:?}: expected the same body on a second read, got {:?} after {:?}",
            after.body, before.body
        );
    }
}
