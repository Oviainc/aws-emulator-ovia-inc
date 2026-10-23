// SPDX-License-Identifier: CC0-1.0

//! The awsQuery error envelope, over the wire: every error response this service answers
//! carries a `<RequestId>` beside its `<Error>`.
//!
//! The rule under test is envelope-wide, not per operation. An awsQuery error response is
//! `<ErrorResponse><Error><Type/><Code/><Message/></Error><RequestId/></ErrorResponse>`
//! (`smithy_query(query="service_info")` on this crate's model: protocol `awsQuery`,
//! version `2014-10-31`, xmlNamespace `http://rds.amazonaws.com/doc/2014-10-31/`), so
//! whichever layer refused the call — a handler, the boundary validator, the scaffold's
//! 501 channel, the routing table, the API-version gate — the answer carries a request
//! id: that element is what an SDK's error object exposes and what correlates a failed
//! call against server-side records, and an XML consumer binding the whole `ErrorResponse`
//! shape fails to deserialize a fault without it.
//!
//! The table below is therefore keyed on the CLASS of failure, not on one operation: a
//! modelled 404 on each of the three reads that declare one, a modelled 400 conflict, a
//! handler refusal of members that cannot hold together, a value outside a documented
//! bound, an absent required member, a scaffolded operation, an `Action` that names no
//! modelled operation, and an envelope naming an API version this service does not serve.
//! Each row names the status and the wire `<Code>` it expects as well, so a row cannot
//! pass by failing for a different reason — and the success path is checked too, to pin
//! that it keeps its single `ResponseMetadata/RequestId` and grows no error envelope.
//!
//! The router and the XML element readers come from the `db_instances` harness, the one
//! wire harness this test target has. Only the request builder is local: this suite needs
//! the `Version` field of the Query envelope as an axis (one row names a version the
//! service does not serve) and the `content-type` beside the body, neither of which the
//! shared `call` exposes.
//!
//! The sibling suite `all/docdb_query_surface.rs` pins the same rule over the LAYERS that
//! can raise an error on the DB instance operations; the rows here span the classes it
//! does not reach — the option-group and snapshot reads' own modelled 404s, a modelled
//! 400 conflict, the refusals `DescribeOptionGroups`'s selection members raise, a
//! scaffolded operation's 501, and the envelope's root namespace.

use service_com_amazonaws_rds_amazonrdsv19::service::api_version::API_VERSION;
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{emulator, text, texts};

/// The region every call below is signed for.
const REGION: &str = "us-west-2";

/// One answer from the emulator: its HTTP status, its `content-type` and its body.
struct Answer {
    status: u16,
    content_type: String,
    body: String,
}

/// One awsQuery call, with the envelope's `Version` as an explicit argument.
async fn call_at_version(
    router: &MultiServiceRouter,
    version: &str,
    action: &str,
    params: &[(&str, &str)],
) -> Answer {
    let mut body = format!("Action={action}&Version={version}");
    for (name, value) in params {
        assert!(
            !value.contains(['&', '=', '+', '%', ' ']),
            "test parameter {name} would need form escaping: {value:?}"
        );
        body.push('&');
        body.push_str(name);
        body.push('=');
        body.push_str(value);
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("https://rds.amazonaws.com/")
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEMULATOR/20260101/{REGION}/rds/aws4_request, \
                 SignedHeaders=host, Signature=unverified"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get("content-type")
        .map(|value| String::from_utf8_lossy(value.as_bytes()).to_string())
        .unwrap_or_default();
    Answer {
        status,
        content_type,
        body: String::from_utf8(response.body().to_vec()).expect("a UTF-8 response body"),
    }
}

/// One awsQuery call at the version this service serves.
async fn call(router: &MultiServiceRouter, action: &str, params: &[(&str, &str)]) -> Answer {
    call_at_version(router, API_VERSION, action, params).await
}

/// One row of the error-envelope table: a request that must fail, and the status and wire
/// code it must fail with.
struct ErrorCase {
    /// What class of failure this row drives.
    case: &'static str,
    /// The envelope's `Version` field — the service's own [`API_VERSION`] for every row
    /// but the one that names a version this service does not serve.
    version: &'static str,
    action: &'static str,
    params: &'static [(&'static str, &'static str)],
    status: u16,
    code: &'static str,
}

/// The classes of failure this service can answer, one row each.
///
/// The modelled codes and statuses are the model's own
/// (`smithy_query(query="operation", operation="DescribeOptionGroups,DescribeDBInstances,
/// DescribeDBSnapshots")`: `OptionGroupNotFoundFault` 404, `DBInstanceNotFound` 404,
/// `DBSnapshotNotFound` 404); `MaxRecords` is documented "Minimum 20, maximum 100", so 19
/// is one past its published bound; `DescribeDBClusters` is one of the operations still
/// carrying the scaffold header, so it answers the 501 channel.
///
/// The codes of the rows that are NOT modelled errors are the ones this service already
/// answers with — `ValidationException` from the scaffold's refusal channel,
/// `MalformedInput` from the boundary validator, `InvalidAction` from the routing
/// and API-version gates. They are pinned here so this change cannot alter them: whether
/// a parameter refusal should instead carry the Query API's `InvalidParameterValue` code
/// is a different defect with its own issue, and this suite is about the envelope.
const ERROR_CASES: &[ErrorCase] = &[
    ErrorCase {
        case: "modelled 404: a read naming no stored option group",
        version: API_VERSION,
        action: "DescribeOptionGroups",
        params: &[("OptionGroupName", "no-such-option-group")],
        status: 404,
        code: "OptionGroupNotFoundFault",
    },
    ErrorCase {
        case: "modelled 404 on another read",
        version: API_VERSION,
        action: "DescribeDBInstances",
        params: &[("DBInstanceIdentifier", "no-such-instance")],
        status: 404,
        code: "DBInstanceNotFound",
    },
    ErrorCase {
        case: "modelled 404 on a third read",
        version: API_VERSION,
        action: "DescribeDBSnapshots",
        params: &[("DBSnapshotIdentifier", "no-such-snapshot")],
        status: 404,
        code: "DBSnapshotNotFound",
    },
    ErrorCase {
        case: "handler refusal: selection members that cannot hold together",
        version: API_VERSION,
        action: "DescribeOptionGroups",
        params: &[("OptionGroupName", "a-group"), ("EngineName", "mysql")],
        status: 400,
        code: "ValidationException",
    },
    ErrorCase {
        case: "handler refusal: a value one past a documented bound",
        version: API_VERSION,
        action: "DescribeOptionGroups",
        params: &[("MaxRecords", "19")],
        status: 400,
        code: "ValidationException",
    },
    ErrorCase {
        case: "handler refusal on a mutation: an engine the service does not host",
        version: API_VERSION,
        action: "CreateOptionGroup",
        params: &[
            ("OptionGroupName", "envelope-engine"),
            ("EngineName", "not-an-engine"),
            ("MajorEngineVersion", "8.0"),
            ("OptionGroupDescription", "no"),
        ],
        status: 400,
        code: "ValidationException",
    },
    ErrorCase {
        case: "absent required member, refused at the boundary",
        version: API_VERSION,
        action: "CreateOptionGroup",
        params: &[],
        status: 400,
        code: "MalformedInput",
    },
    ErrorCase {
        case: "scaffolded operation, which answers the 501 channel",
        version: API_VERSION,
        // `DescribeDBClusterEndpoints` is still a stub (`DescribeDBClusters`, which stood
        // here before, reports the clusters `CreateDBCluster` stores).
        action: "DescribeDBClusterEndpoints",
        params: &[],
        status: 501,
        code: "NotImplemented",
    },
    ErrorCase {
        case: "an Action naming no modelled operation",
        version: API_VERSION,
        action: "NotAnActionAtAll",
        params: &[],
        status: 400,
        code: "InvalidAction",
    },
    ErrorCase {
        case: "an envelope naming an API version this service does not serve",
        version: "2099-01-01",
        action: "DescribeOptionGroups",
        params: &[],
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
    let ids = texts(body, "RequestId");
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

/// A request id has the 8-4-4-4-12 lowercase-hex form AWS answers with, whether it came
/// from an error envelope or from a successful response's `ResponseMetadata`.
fn assert_request_id_shape(case: &str, request_id: &str) {
    let widths = [8usize, 4, 4, 4, 12];
    let groups: Vec<&str> = request_id.split('-').collect();
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
/// the status, the XML content type and the wire code it did before.
#[tokio::test]
async fn every_error_response_carries_a_request_id() {
    let router = emulator();

    for row in ERROR_CASES {
        let answer = call_at_version(&router, row.version, row.action, row.params).await;
        assert_eq!(
            answer.status, row.status,
            "case {:?}: expected HTTP {}, got {} with body {}",
            row.case, row.status, answer.status, answer.body
        );
        assert_eq!(
            answer.content_type, "text/xml",
            "case {:?}: expected an XML error document, got content-type {:?} with body {}",
            row.case, answer.content_type, answer.body
        );
        let code = text(&answer.body, "Code");
        assert_eq!(
            code, row.code,
            "case {:?}: expected error code {}, got {code:?} in {}",
            row.case, row.code, answer.body
        );
        let request_id = error_envelope_request_id(row.case, &answer.body);
        assert_request_id_shape(row.case, &request_id);
    }
}

/// A modelled 400 conflict — one the model declares a shape and an `awsQueryError` code
/// for — is answered in the same envelope. `OptionGroupAlreadyExistsFault` is reached by
/// creating the same option group twice.
#[tokio::test]
async fn a_modelled_conflict_carries_a_request_id() {
    let router = emulator();
    let create = [
        ("OptionGroupName", "envelope-twice"),
        ("EngineName", "mysql"),
        ("MajorEngineVersion", "8.0"),
        ("OptionGroupDescription", "first"),
    ];
    let first = call(&router, "CreateOptionGroup", &create).await;
    assert_eq!(
        first.status, 200,
        "the first create must succeed, got {} with body {}",
        first.status, first.body
    );

    let again = call(&router, "CreateOptionGroup", &create).await;
    assert_eq!(
        again.status, 400,
        "case \"duplicate option group\": expected HTTP 400, got {} with body {}",
        again.status, again.body
    );
    let code = text(&again.body, "Code");
    assert_eq!(
        code, "OptionGroupAlreadyExistsFault",
        "case \"duplicate option group\": expected the modelled awsQuery code, got {code:?} in {}",
        again.body
    );
    let request_id = error_envelope_request_id("duplicate option group", &again.body);
    assert_request_id_shape("duplicate option group", &request_id);
}

/// A request id names ONE request: two failures, of the same class or of different ones,
/// never report the same id.
#[tokio::test]
async fn each_error_response_carries_its_own_request_id() {
    let router = emulator();

    let mut seen: Vec<String> = Vec::new();
    for row in ERROR_CASES.iter().chain(ERROR_CASES.iter()) {
        let answer = call_at_version(&router, row.version, row.action, row.params).await;
        let request_id = error_envelope_request_id(row.case, &answer.body);
        assert!(
            !seen.contains(&request_id),
            "case {:?}: expected a fresh request id, got the already-reported {request_id:?}",
            row.case
        );
        seen.push(request_id);
    }
}

/// The success path is untouched: a successful response keeps its single
/// `ResponseMetadata/RequestId` and grows no error envelope.
#[tokio::test]
async fn a_successful_response_keeps_its_response_metadata_request_id() {
    let router = emulator();
    let created = call(
        &router,
        "CreateOptionGroup",
        &[
            ("OptionGroupName", "envelope-success"),
            ("EngineName", "mysql"),
            ("MajorEngineVersion", "8.0"),
            ("OptionGroupDescription", "ok"),
        ],
    )
    .await;
    assert_eq!(
        created.status, 200,
        "the seed create must succeed, got {} with body {}",
        created.status, created.body
    );

    // (case, action, parameters): a create, a read of one resource, a read of a collection
    // and a catalog read — the four shapes of success this service answers.
    type SuccessRow = (&'static str, &'static str, &'static [(&'static str, &'static str)]);
    let rows: [SuccessRow; 4] = [
        ("the create itself", "CreateOptionGroup", &[
            ("OptionGroupName", "envelope-success-2"),
            ("EngineName", "postgres"),
            ("MajorEngineVersion", "16"),
            ("OptionGroupDescription", "ok"),
        ]),
        ("a read naming one group", "DescribeOptionGroups", &[(
            "OptionGroupName",
            "envelope-success",
        )]),
        ("a read of the collection", "DescribeOptionGroups", &[]),
        ("a catalog read", "DescribeEventCategories", &[]),
    ];

    for (case, action, params) in rows {
        let answer = call(&router, action, params).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {} with body {}",
            answer.status, answer.body
        );
        assert!(
            !answer.body.contains("<ErrorResponse"),
            "case {case:?}: a success must carry no error envelope, got {}",
            answer.body
        );
        let ids = texts(&answer.body, "RequestId");
        assert_eq!(
            ids.len(),
            1,
            "case {case:?}: expected one ResponseMetadata/RequestId, got {ids:?} from {}",
            answer.body
        );
        assert!(
            answer.body.contains(&format!(
                "<ResponseMetadata><RequestId>{}</RequestId></ResponseMetadata>",
                ids[0]
            )),
            "case {case:?}: expected the id inside <ResponseMetadata>, got {}",
            answer.body
        );
    }
}

/// The service's `@xmlNamespace` URI, read out of the shipped model rather than restated
/// here (`smithy_query(query="service_info")` -> `xmlNamespace.uri`).
fn modelled_xml_namespace() -> String {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/smithy-model/com-amazonaws-rds-amazonrdsv19.json"
    );
    let model: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).expect("the shipped model"))
            .expect("the model parses as JSON");
    model["shapes"]["com.amazonaws.rds#AmazonRDSv19"]["traits"]["smithy.api#xmlNamespace"]["uri"]
        .as_str()
        .expect("the service shape declares an @xmlNamespace uri")
        .to_string()
}

/// The envelope a modelled error is serialized into is rooted at the model's
/// `<ErrorResponse xmlns="...">`, and the added `<RequestId>` sits inside that root.
#[tokio::test]
async fn the_error_envelope_carries_the_service_namespace() {
    let router = emulator();
    let namespace = modelled_xml_namespace();
    let answer = call(
        &router,
        "DescribeOptionGroups",
        &[("OptionGroupName", "no-such-option-group")],
    )
    .await;
    assert_eq!(
        answer.status, 404,
        "the read must be refused as not found, got {} with body {}",
        answer.status, answer.body
    );
    assert!(
        answer
            .body
            .contains(&format!("<ErrorResponse xmlns=\"{namespace}\">")),
        "case \"modelled 404\": expected the model's namespace on the envelope root, got {}",
        answer.body
    );
    let request_id = error_envelope_request_id("modelled 404", &answer.body);
    assert_request_id_shape("modelled 404", &request_id);
}
