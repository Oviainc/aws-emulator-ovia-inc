// SPDX-License-Identifier: CC0-1.0

//! The awsQuery error envelope, over the wire: every error response carries a
//! `<RequestId>` in its `<ErrorResponse>`.
//!
//! The rule under test is envelope-wide, not per operation: an awsQuery error response is
//! `<ErrorResponse><Error><Type/><Code/><Message/></Error><RequestId/></ErrorResponse>`,
//! so whichever layer refused the call — the boundary validator, the handler, the routing
//! table — the answer carries a request id, which is what an SDK's error object exposes
//! and what correlates a failed call against server-side records.
//!
//! The table below is therefore keyed on the CLASS of failure, not on the operation: a
//! value outside a declared bound, an undeclared enum value, a string under its declared
//! length, an absent required member, a handler's own refusal, a modelled error, the
//! scaffold's 501 and an unknown `Action`. Each row names the status and the wire code it
//! expects as well, so a row cannot pass by failing for a different reason.

use crate::harness::{call, create_stack, member, router, xml_text, xml_texts};

/// One row of the error-envelope table: a request that must fail, and the status and wire
/// code it must fail with.
struct ErrorCase {
    /// What class of failure this row drives.
    case: &'static str,
    action: &'static str,
    members: &'static [(&'static str, &'static str)],
    status: u16,
    code: &'static str,
}

/// The classes of failure this service can answer, one row each.
///
/// The bounds and enum value sets come from the model
/// (`smithy_query(query="operation", operation="ListStackSets,GetTemplate,…")`):
/// `MaxResults` is `range 1..100` on every paginated read, `TemplateStage` is
/// `[Original, Processed]`, `RegistrationStatus` is `[COMPLETE, IN_PROGRESS, FAILED]`,
/// `NextToken` is `length 1..1024`, and `ListStackSetOperations$StackSetName` is
/// `@required`. The rows named "…bound…" cover each published bound and one past it.
const ERROR_CASES: &[ErrorCase] = &[
    ErrorCase {
        case: "integer under its declared minimum",
        action: "ListStackSets",
        members: &[("MaxResults", "0")],
        status: 400,
        code: "ValidationError",
    },
    ErrorCase {
        case: "integer over its declared maximum",
        action: "ListStackSets",
        members: &[("MaxResults", "101")],
        status: 400,
        code: "ValidationError",
    },
    ErrorCase {
        case: "integer out of bounds on another paginated read",
        action: "ListStackSetOperations",
        members: &[("StackSetName", "a-stack-set"), ("MaxResults", "0")],
        status: 400,
        code: "ValidationError",
    },
    ErrorCase {
        case: "integer out of bounds on a boxed MaxResults",
        action: "DescribeStackResourceDrifts",
        members: &[("StackName", "a-stack"), ("MaxResults", "1000")],
        status: 400,
        code: "ValidationError",
    },
    ErrorCase {
        case: "value outside an enum's declared set",
        action: "GetTemplate",
        members: &[("StackName", "a-stack"), ("TemplateStage", "Bogus")],
        status: 400,
        code: "ValidationError",
    },
    ErrorCase {
        case: "value outside a different enum's declared set",
        action: "ListTypeRegistrations",
        members: &[("RegistrationStatusFilter", "NOT_A_REAL_STATUS")],
        status: 400,
        code: "ValidationError",
    },
    ErrorCase {
        case: "string under its declared length",
        action: "DescribeStacks",
        members: &[("NextToken", "")],
        status: 400,
        code: "ValidationError",
    },
    ErrorCase {
        case: "absent required member",
        action: "ListStackSetOperations",
        members: &[],
        status: 400,
        code: "ValidationError",
    },
    ErrorCase {
        case: "handler refusal: a read naming no addressable stack",
        action: "DescribeStacks",
        members: &[("StackName", "no-such-stack")],
        status: 400,
        code: "ValidationError",
    },
    ErrorCase {
        case: "handler refusal on another operation",
        action: "GetTemplate",
        members: &[("StackName", "no-such-stack")],
        status: 400,
        code: "ValidationError",
    },
    ErrorCase {
        case: "handler refusal on a mutation",
        action: "UpdateStack",
        members: &[("StackName", "no-such-stack"), ("TemplateBody", "{}")],
        status: 400,
        code: "ValidationError",
    },
    ErrorCase {
        case: "scaffolded operation, which answers the 501 channel",
        action: "DescribeOrganizationsAccess",
        members: &[],
        status: 501,
        code: "NotImplemented",
    },
    ErrorCase {
        case: "unknown Action, which names no modelled operation",
        action: "NotAnActionAtAll",
        members: &[],
        status: 400,
        code: "InvalidAction",
    },
];

/// The request id of an error response, checked to be where AWS puts it: exactly one
/// `<RequestId>`, a sibling AFTER `<Error>` and inside `<ErrorResponse>`.
fn error_envelope_request_id(case: &str, body: &str) -> String {
    assert!(
        body.contains("<ErrorResponse"),
        "case {case:?}: expected an <ErrorResponse> envelope, got {body:?}"
    );
    let ids = xml_texts(body, "RequestId");
    assert_eq!(
        ids.len(),
        1,
        "case {case:?}: expected exactly one <RequestId> in the envelope, got {ids:?} from \
         {body:?}"
    );
    let error_close = body
        .rfind("</Error>")
        .unwrap_or_else(|| panic!("case {case:?}: expected an <Error> element, got {body:?}"));
    let request_id_open = body
        .find("<RequestId>")
        .unwrap_or_else(|| panic!("case {case:?}: expected a <RequestId> element, got {body:?}"));
    let envelope_close = body.rfind("</ErrorResponse>").unwrap_or_else(|| {
        panic!("case {case:?}: expected a closed <ErrorResponse>, got {body:?}")
    });
    assert!(
        error_close < request_id_open && request_id_open < envelope_close,
        "case {case:?}: expected <RequestId> between </Error> and </ErrorResponse>, got {body:?}"
    );
    ids.into_iter().next().expect("one id")
}

/// An identifier has the 8-4-4-4-12 lowercase-hex form AWS answers with — a request id,
/// whether it came from an error envelope or from a successful response's `ResponseMetadata`,
/// and every other id this service mints in that form.
///
/// `what` names the kind of id under assertion, so one helper serves every caller: this is
/// the ONE spelling of that check in this test crate, reused by `stack_refactors.rs` for the
/// `StackRefactorId` a create hands out.
pub(crate) fn assert_identifier_shape(case: &str, what: &str, request_id: &str) {
    let groups: Vec<&str> = request_id.split('-').collect();
    let widths = [8usize, 4, 4, 4, 12];
    assert_eq!(
        groups.len(),
        widths.len(),
        "case {case:?}: expected a {what} of five hyphen-separated groups, got {request_id:?}"
    );
    for (group, width) in groups.iter().zip(widths) {
        assert!(
            group.len() == width
                && group
                    .chars()
                    .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)),
            "case {case:?}: expected a {width}-character lowercase-hex group in a {what}, got \
             {group:?} in {request_id:?}"
        );
    }
}

/// [`assert_identifier_shape`] for a request id, which is what this suite asserts.
fn assert_request_id_shape(case: &str, request_id: &str) {
    assert_identifier_shape(case, "request id", request_id);
}

fn members_of(case: &ErrorCase) -> Vec<(String, String)> {
    case.members
        .iter()
        .map(|(key, value)| member(key, value))
        .collect()
}

/// Every class of failure answers an envelope carrying a request id — and still answers
/// the status and the wire code it did before.
#[tokio::test]
async fn every_error_response_carries_a_request_id() {
    let router = router();

    for case in ERROR_CASES {
        let (status, body) = call(&router, case.action, &members_of(case)).await;
        assert_eq!(
            status, case.status,
            "case {:?}: expected HTTP {}, got {status} with {body:?}",
            case.case, case.status
        );
        assert_eq!(
            xml_text(&body, "Code").as_deref(),
            Some(case.code),
            "case {:?}: expected <Code>{}</Code>, got {body:?}",
            case.case,
            case.code
        );
        let request_id = error_envelope_request_id(case.case, &body);
        assert_request_id_shape(case.case, &request_id);
    }
}

/// A modelled error — one the model declares a shape and an `awsQueryError` code for — is
/// answered in the same envelope. `AlreadyExistsException` is reached by creating the same
/// stack twice.
#[tokio::test]
async fn a_modelled_error_carries_a_request_id() {
    let router = router();
    let (created, _) = create_stack(&router, "twice", "{}", &[]).await;
    assert_eq!(created, 200, "the first create must succeed");

    let (status, body) = create_stack(&router, "twice", "{}", &[]).await;
    assert_eq!(
        status, 400,
        "case \"duplicate stack\": expected HTTP 400, got {status} with {body:?}"
    );
    assert_eq!(
        xml_text(&body, "Code").as_deref(),
        Some("AlreadyExistsException"),
        "case \"duplicate stack\": expected the modelled awsQuery code, got {body:?}"
    );
    let request_id = error_envelope_request_id("duplicate stack", &body);
    assert_request_id_shape("duplicate stack", &request_id);
}

/// A request id names ONE request: two failures, of the same class or of different ones,
/// never report the same id.
#[tokio::test]
async fn each_error_response_carries_its_own_request_id() {
    let router = router();

    let mut seen: Vec<String> = Vec::new();
    for case in ERROR_CASES.iter().chain(ERROR_CASES.iter()) {
        let (_, body) = call(&router, case.action, &members_of(case)).await;
        let request_id = error_envelope_request_id(case.case, &body);
        assert!(
            !seen.contains(&request_id),
            "case {:?}: expected a fresh request id, got the already-reported {request_id:?}",
            case.case
        );
        seen.push(request_id);
    }
}

/// The success path is untouched: a successful response keeps its
/// `ResponseMetadata/RequestId` and grows no error envelope.
#[tokio::test]
async fn a_successful_response_keeps_its_response_metadata_request_id() {
    let router = router();

    for (case, action, members) in [
        ("CreateStack", "CreateStack", vec![member("StackName", "ok"), member("TemplateBody", "{}")]),
        ("DescribeStacks", "DescribeStacks", vec![]),
        ("ListStacks", "ListStacks", vec![]),
    ] {
        let (status, body) = call(&router, action, &members).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {body:?}"
        );
        assert!(
            !body.contains("<ErrorResponse"),
            "case {case:?}: a success must carry no error envelope, got {body:?}"
        );
        let ids = xml_texts(&body, "RequestId");
        assert_eq!(
            ids.len(),
            1,
            "case {case:?}: expected one ResponseMetadata/RequestId, got {ids:?} from {body:?}"
        );
        assert_request_id_shape(case, &ids[0]);
    }
}

/// The service's `@xmlNamespace` URI, read out of the shipped model rather than restated
/// here (`smithy_query(query="service_info")` -> `xmlNamespace.uri`).
fn modelled_xml_namespace() -> String {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/smithy-model/com-amazonaws-cloudformation-cloudformation.json"
    );
    let model: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).expect("the shipped model"))
            .expect("the model parses as JSON");
    model["shapes"]["com.amazonaws.cloudformation#CloudFormation"]["traits"]
        ["smithy.api#xmlNamespace"]["uri"]
        .as_str()
        .expect("the service shape declares an @xmlNamespace uri")
        .to_string()
}

/// The envelope a handler's error is serialized into is rooted at the model's
/// `<ErrorResponse xmlns="...">`, and the added `<RequestId>` sits inside that root.
#[tokio::test]
async fn the_error_envelope_carries_the_service_namespace() {
    let router = router();
    let namespace = modelled_xml_namespace();
    let (status, body) = call(
        &router,
        "DescribeStacks",
        &[member("StackName", "no-such-stack")],
    )
    .await;
    assert_eq!(status, 400, "the read must be refused, got {body:?}");
    assert!(
        body.contains(&format!("<ErrorResponse xmlns=\"{namespace}\">")),
        "case \"handler refusal\": expected the model's namespace on the envelope root, got \
         {body:?}"
    );
    let request_id = error_envelope_request_id("handler refusal", &body);
    assert_request_id_shape("handler refusal", &request_id);
}
