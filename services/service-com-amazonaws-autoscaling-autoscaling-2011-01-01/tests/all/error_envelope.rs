// SPDX-License-Identifier: CC0-1.0

//! The awsQuery error envelope, over the wire: every failed call answers an
//! `<ErrorResponse>` carrying a `<RequestId>`, and a rejected `NextToken` is named in
//! the `InvalidNextToken` message.
//!
//! Two rules are under test, neither of them per operation:
//!
//! 1. an awsQuery error response is
//!    `<ErrorResponse><Error><Type/><Code/><Message/></Error><RequestId/></ErrorResponse>`,
//!    so whichever layer refused the call — the boundary validator, a handler, the
//!    scaffold's 501, the routing table — the answer carries a request id, which is what
//!    an SDK's error object exposes and what correlates a failed call against
//!    server-side records. The table below is therefore keyed on the CLASS of failure,
//!    not on the operation, and each row names the status and wire code it expects as
//!    well, so a row cannot pass by failing for a different reason.
//! 2. a paginated read handed a cursor it never issued answers `InvalidNextToken` with a
//!    message BUILT FROM THE REQUEST: it quotes the rejected token back, because that is
//!    the only thing that says which of a caller's cursors went stale. The second table
//!    spans the token forms a client can submit — every one `DescribeAutoScalingGroups`
//!    does not issue. (`DescribeLaunchConfigurations` is not in that table: its token is
//!    a start-after key, so no value is invalid and it answers a page instead of
//!    refusing — see `describe_launch_configurations.rs`.)

use crate::support::{new_engine, query, router_with, tag_value, tag_values, xml_unescape};

/// The launch source a group in this suite is created from — a launch TEMPLATE, because
/// no account can create a launch configuration (see
/// `launch_configurations_are_unavailable.rs`), so a group naming one could not be
/// created either.
const LAUNCH_SOURCE: (&str, &str) = ("LaunchTemplate.LaunchTemplateName", "lt-1");

/// One row of the error-envelope table: a request that must fail, and the status and
/// wire code it must fail with.
struct ErrorCase {
    /// What class of failure this row drives.
    case: &'static str,
    action: &'static str,
    params: &'static [(&'static str, &'static str)],
    status: u16,
    code: &'static str,
}

/// The classes of failure this service can answer, one row each.
///
/// The codes are the ones each layer names: a modelled error renders its
/// `awsQueryError` code (`InvalidNextToken`, `smithy_query(query="operation",
/// operation="DescribeAutoScalingGroups")`), a handler refusal the `ValidationError`
/// this service answers an unsatisfiable request with, the launch-configuration surface
/// its `UnsupportedOperation`, a body the codec could not read `MalformedInput`, a value
/// outside a declared set `ValidationError`, a scaffolded operation `NotImplemented`,
/// and an `Action` naming no modelled operation `InvalidAction`.
const ERROR_CASES: &[ErrorCase] = &[
    ErrorCase {
        case: "a modelled error: a NextToken the service never issued",
        action: "DescribeAutoScalingGroups",
        params: &[("NextToken", "garbage-token-not-issued-by-service")],
        status: 400,
        code: "InvalidNextToken",
    },
    ErrorCase {
        case: "a handler refusal: a create naming no launch source",
        action: "CreateAutoScalingGroup",
        params: &[
            ("AutoScalingGroupName", "no-launch-source"),
            ("MinSize", "0"),
            ("MaxSize", "1"),
            ("AvailabilityZones.member.1", "us-west-2a"),
        ],
        status: 400,
        code: "ValidationError",
    },
    ErrorCase {
        case: "a handler refusal: a delete naming no such launch configuration",
        action: "DeleteLaunchConfiguration",
        params: &[("LaunchConfigurationName", "never-created")],
        status: 400,
        code: "ValidationError",
    },
    ErrorCase {
        case: "a handler refusal: an update naming no such group",
        action: "UpdateAutoScalingGroup",
        params: &[("AutoScalingGroupName", "no-such-group"), ("MaxSize", "2")],
        status: 400,
        code: "ValidationError",
    },
    ErrorCase {
        case: "a handler refusal: a capacity change naming no such group",
        action: "SetDesiredCapacity",
        params: &[
            ("AutoScalingGroupName", "no-such-group"),
            ("DesiredCapacity", "1"),
        ],
        status: 400,
        code: "ValidationError",
    },
    ErrorCase {
        case: "a handler refusal: a delete naming no such group",
        action: "DeleteAutoScalingGroup",
        params: &[("AutoScalingGroupName", "no-such-group")],
        status: 400,
        code: "ValidationError",
    },
    ErrorCase {
        case: "a handler refusal: a page size outside the documented range",
        action: "DescribeAutoScalingGroups",
        params: &[("MaxRecords", "101")],
        status: 400,
        code: "ValidationError",
    },
    ErrorCase {
        case: "a handler refusal on the other paginated read",
        action: "DescribeLaunchConfigurations",
        params: &[("MaxRecords", "0")],
        status: 400,
        code: "ValidationError",
    },
    ErrorCase {
        case: "an operation no account can perform",
        action: "CreateLaunchConfiguration",
        params: &[
            ("LaunchConfigurationName", "lc-1"),
            ("ImageId", "ami-0123456789abcdef0"),
            ("InstanceType", "t3.micro"),
        ],
        status: 400,
        code: "UnsupportedOperation",
    },
    ErrorCase {
        case: "a value outside an enum's declared set",
        action: "CreateAutoScalingGroup",
        params: &[
            ("AutoScalingGroupName", "bad-protection"),
            ("MinSize", "0"),
            ("MaxSize", "0"),
            ("AvailabilityZones.member.1", "us-west-2a"),
            ("DeletionProtection", "not-a-level"),
        ],
        status: 400,
        code: "ValidationError",
    },
    ErrorCase {
        case: "an absent required member, which the codec refuses",
        action: "SetDesiredCapacity",
        params: &[],
        status: 400,
        code: "MalformedInput",
    },
    ErrorCase {
        case: "a scaffolded operation, which answers the 501 channel",
        action: "DescribeAutoScalingInstances",
        params: &[],
        status: 501,
        code: "NotImplemented",
    },
    ErrorCase {
        case: "an Action naming no modelled operation",
        action: "NotAnActionAtAll",
        params: &[],
        status: 400,
        code: "InvalidAction",
    },
];

/// One row of a table that drives one action: the case, the action, and the parameters
/// the request carries.
type ActionCase<'a> = (&'a str, &'a str, Vec<(&'a str, &'a str)>);

/// One row of a table whose action must FAIL: [`ActionCase`] plus the status and the wire
/// code it must fail with.
type RefusedCase<'a> = (&'a str, &'a str, Vec<(&'a str, &'a str)>, u16, &'a str);

/// The request id of an error response, checked to be where AWS puts it: exactly one
/// `<RequestId>`, a sibling AFTER `<Error>` and inside `<ErrorResponse>`.
fn error_envelope_request_id(case: &str, body: &str) -> String {
    assert!(
        body.contains("<ErrorResponse"),
        "case {case:?}: expected an <ErrorResponse> envelope, got {body:?}"
    );
    let ids = tag_values(body, "RequestId");
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
    let request_id = ids.into_iter().next().expect("one id");
    assert!(
        !request_id.is_empty(),
        "case {case:?}: expected a non-empty request id, got {body:?}"
    );
    request_id
}

/// A request id has the 8-4-4-4-12 lowercase-hex form AWS answers with, whether it came
/// from an error envelope or from a successful response's `ResponseMetadata`.
fn assert_request_id_shape(case: &str, request_id: &str) {
    let groups: Vec<&str> = request_id.split('-').collect();
    let widths = [8usize, 4, 4, 4, 12];
    assert_eq!(
        groups.len(),
        widths.len(),
        "case {case:?}: expected five hyphen-separated groups, got {request_id:?}"
    );
    for (group, width) in groups.iter().zip(widths) {
        assert!(
            group.len() == width
                && group
                    .chars()
                    .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)),
            "case {case:?}: expected a {width}-character lowercase-hex group, got {group:?} in \
             {request_id:?}"
        );
    }
}

/// Every class of failure answers an envelope carrying a request id — and still answers
/// the status and the wire code it did before.
#[tokio::test]
async fn every_error_response_carries_a_request_id() {
    let router = router_with(new_engine());

    for case in ERROR_CASES {
        let (status, body) = query(&router, case.action, case.params).await;
        assert_eq!(
            status, case.status,
            "case {:?}: expected HTTP {}, got {status} with {body:?}",
            case.case, case.status
        );
        assert_eq!(
            tag_value(&body, "Code"),
            case.code,
            "case {:?}: expected <Code>{}</Code>, got {body:?}",
            case.case,
            case.code
        );
        let request_id = error_envelope_request_id(case.case, &body);
        assert_request_id_shape(case.case, &request_id);
    }
}

/// A request id names ONE request: two failures, of the same class or of different ones,
/// never report the same id.
#[tokio::test]
async fn each_error_response_carries_its_own_request_id() {
    let router = router_with(new_engine());

    let mut seen: Vec<String> = Vec::new();
    for case in ERROR_CASES.iter().chain(ERROR_CASES.iter()) {
        let (_, body) = query(&router, case.action, case.params).await;
        let request_id = error_envelope_request_id(case.case, &body);
        assert!(
            !seen.contains(&request_id),
            "case {:?}: expected a fresh request id, got the already-reported {request_id:?}",
            case.case
        );
        seen.push(request_id);
    }
}

/// A refusal that depends on STORED state answers the same envelope: a duplicate create
/// (the modelled `AlreadyExistsFault`, whose `awsQueryError` code is `AlreadyExists`) and
/// a delete the group's deletion protection forbids.
#[tokio::test]
async fn a_state_dependent_refusal_carries_a_request_id() {
    let router = router_with(new_engine());

    let create = vec![
        ("AutoScalingGroupName", "protected"),
        LAUNCH_SOURCE,
        ("MinSize", "0"),
        ("MaxSize", "0"),
        ("AvailabilityZones.member.1", "us-west-2a"),
        ("DeletionProtection", "prevent-all-deletion"),
    ];
    let (status, body) = query(&router, "CreateAutoScalingGroup", &create).await;
    assert_eq!(status, 200, "the first create must succeed, got {body:?}");

    let cases: [RefusedCase<'_>; 2] = [
        (
            "a duplicate create",
            "CreateAutoScalingGroup",
            create.clone(),
            400,
            "AlreadyExists",
        ),
        (
            "a delete the group's deletion protection forbids",
            "DeleteAutoScalingGroup",
            vec![("AutoScalingGroupName", "protected")],
            400,
            "ValidationError",
        ),
    ];

    for (case, action, params, expected_status, expected_code) in cases {
        let (status, body) = query(&router, action, &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with {body:?}"
        );
        assert_eq!(
            tag_value(&body, "Code"),
            expected_code,
            "case {case:?}: expected <Code>{expected_code}</Code>, got {body:?}"
        );
        let request_id = error_envelope_request_id(case, &body);
        assert_request_id_shape(case, &request_id);
    }
}

/// The success path is untouched: a successful response keeps its one
/// `ResponseMetadata/RequestId` and grows no error envelope.
#[tokio::test]
async fn a_successful_response_keeps_its_response_metadata_request_id() {
    let router = router_with(new_engine());

    let cases: [ActionCase<'_>; 4] = [
        ("an empty describe", "DescribeAutoScalingGroups", vec![]),
        (
            "a describe of the launch configurations",
            "DescribeLaunchConfigurations",
            vec![],
        ),
        (
            "a create",
            "CreateAutoScalingGroup",
            vec![
                ("AutoScalingGroupName", "asg-1"),
                LAUNCH_SOURCE,
                ("MinSize", "0"),
                ("MaxSize", "0"),
                ("AvailabilityZones.member.1", "us-west-2a"),
            ],
        ),
        (
            "a describe that reads it back",
            "DescribeAutoScalingGroups",
            vec![("AutoScalingGroupNames.member.1", "asg-1")],
        ),
    ];

    for (case, action, params) in cases {
        let (status, body) = query(&router, action, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {body:?}"
        );
        assert!(
            !body.contains("<ErrorResponse"),
            "case {case:?}: a success must carry no error envelope, got {body:?}"
        );
        let ids = tag_values(&body, "RequestId");
        assert_eq!(
            ids.len(),
            1,
            "case {case:?}: expected one ResponseMetadata/RequestId, got {ids:?} from {body:?}"
        );
        assert_request_id_shape(case, &ids[0]);
    }
}

/// The error envelope a handler's error is serialized into is rooted at the model's
/// `<ErrorResponse xmlns="...">`, and the added `<RequestId>` sits inside that root.
#[tokio::test]
async fn the_error_envelope_carries_the_service_namespace() {
    let router = router_with(new_engine());
    let namespace = modelled_xml_namespace();

    let (status, body) = query(
        &router,
        "DescribeAutoScalingGroups",
        &[("NextToken", "garbage-token-not-issued-by-service")],
    )
    .await;
    assert_eq!(status, 400, "the read must be refused, got {body:?}");
    assert!(
        body.contains(&format!("<ErrorResponse xmlns=\"{namespace}\">")),
        "case \"a modelled error\": expected the model's namespace on the envelope root, got \
         {body:?}"
    );
    let request_id = error_envelope_request_id("a modelled error", &body);
    assert_request_id_shape("a modelled error", &request_id);
}

/// The service's `@xmlNamespace` URI, read out of the shipped model rather than restated
/// here (`smithy_query(query="service_info")` -> `xmlNamespace.uri`).
fn modelled_xml_namespace() -> String {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/smithy-model/com-amazonaws-autoscaling-autoscaling-2011-01-01.json"
    );
    let model: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).expect("the shipped model"))
            .expect("the model parses as JSON");
    model["shapes"]["com.amazonaws.autoscaling#AutoScaling_2011_01_01"]["traits"]
        ["smithy.api#xmlNamespace"]["uri"]
        .as_str()
        .expect("the service shape declares an @xmlNamespace uri")
        .to_string()
}

/// The token forms a client can submit that `DescribeAutoScalingGroups` never issued — it
/// issues `next-<offset>` for an offset inside the list it paged — plus the one the
/// recorded AWS call used. A rejected token is quoted back whatever it holds, including
/// XML metacharacters, which the envelope escapes rather than letting them reshape it.
const REJECTED_TOKENS: &[(&str, &str)] = &[
    (
        "the token the recorded call submitted",
        "garbage-token-not-issued-by-service",
    ),
    ("a token that is not a number", "not-a-token"),
    (
        "a bare offset, without the marker an issued token carries",
        "9999",
    ),
    ("a bare zero", "0"),
    ("the first page, for which no token is ever issued", "next-0"),
    (
        "a token that points past the end of the list",
        "next-9999",
    ),
    ("a negative offset", "next--1"),
    ("an empty token", ""),
    ("a token carrying XML metacharacters", "next-<&'\"token>"),
    ("a token carrying a space", "next 1"),
];

/// Every token form this service never issued is refused with a message that NAMES the
/// token — the request-derived rule, not one wording.
#[tokio::test]
async fn an_invalid_next_token_message_quotes_the_rejected_token() {
    let router = router_with(new_engine());
    // One stored group, so a token that points past the end of a NON-empty list is
    // refused on the same rule as one that never parsed.
    let (status, body) = query(
        &router,
        "CreateAutoScalingGroup",
        &[
            ("AutoScalingGroupName", "asg-1"),
            LAUNCH_SOURCE,
            ("MinSize", "0"),
            ("MaxSize", "0"),
            ("AvailabilityZones.member.1", "us-west-2a"),
        ],
    )
    .await;
    assert_eq!(status, 200, "the create must succeed, got {body:?}");

    let mut messages: Vec<(&str, String)> = Vec::new();
    for (case, token) in REJECTED_TOKENS {
        let (status, body) = query(
            &router,
            "DescribeAutoScalingGroups",
            &[("NextToken", token)],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body:?}"
        );
        assert_eq!(
            tag_value(&body, "Code"),
            "InvalidNextToken",
            "case {case:?}: expected the InvalidNextToken code, got {body:?}"
        );
        let message = xml_unescape(&tag_value(&body, "Message"));
        assert!(
            message.contains(&format!("'{token}'")),
            "case {case:?}: expected the message to quote the submitted token {token:?}, got \
             {message:?}"
        );
        // The envelope stays well formed: an escaped metacharacter travels as an entity,
        // never as the raw character that would reshape the document.
        assert!(
            !tag_value(&body, "Message").contains('<'),
            "case {case:?}: expected the message's metacharacters escaped, got {body:?}"
        );
        error_envelope_request_id(case, &body);
        messages.push((case, message));
    }

    // A message built from the request distinguishes the cursors: no two different
    // tokens report the same text, which a constant message could not do.
    for (index, (case, message)) in messages.iter().enumerate() {
        for (other_case, other) in messages.iter().skip(index + 1) {
            assert_ne!(
                message, other,
                "case {case:?} and case {other_case:?} report the same message {message:?}: a \
                 rejected token must be identifiable from the response alone"
            );
        }
    }
}

/// A token the service DID issue still pages: naming the rejected token did not turn a
/// valid cursor into a refusal.
#[tokio::test]
async fn an_issued_next_token_still_pages() {
    let router = router_with(new_engine());
    for index in 0..3 {
        let name = format!("page-{index}");
        let (status, body) = query(
            &router,
            "CreateAutoScalingGroup",
            &[
                ("AutoScalingGroupName", name.as_str()),
                LAUNCH_SOURCE,
                ("MinSize", "0"),
                ("MaxSize", "0"),
                ("AvailabilityZones.member.1", "us-west-2a"),
            ],
        )
        .await;
        assert_eq!(status, 200, "creating {name}: got {status} with {body:?}");
    }

    let (status, body) = query(
        &router,
        "DescribeAutoScalingGroups",
        &[("MaxRecords", "2")],
    )
    .await;
    assert_eq!(status, 200, "the first page: got {status} with {body:?}");
    let token = tag_value(&body, "NextToken");
    assert!(
        !token.is_empty(),
        "a bounded page must carry a NextToken, got {body:?}"
    );

    let (status, body) = query(
        &router,
        "DescribeAutoScalingGroups",
        &[("MaxRecords", "2"), ("NextToken", token.as_str())],
    )
    .await;
    assert_eq!(status, 200, "the second page: got {status} with {body:?}");
    assert_eq!(
        tag_values(&body, "AutoScalingGroupName"),
        vec!["page-2".to_string()],
        "the issued token must resume the list, got {body:?}"
    );
}
