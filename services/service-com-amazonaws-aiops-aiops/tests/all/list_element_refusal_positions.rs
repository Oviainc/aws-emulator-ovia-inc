// SPDX-License-Identifier: CC0-1.0

//! Where a `ValidationException`'s member path points when the violation is inside a
//! LIST element.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 deserialization, the modelled-constraint walk and this service's refusal
//! renderer — so what is asserted is the message a signed AWS SDK request observes.
//!
//! The rule under test: a refusal of a member that lives inside a list element states
//! that element's own 1-based position and the step into the collection
//! (`<member>.<n>.member.<element member>`), and it states that path whether the
//! requirement came from a modelled constraint the request boundary applied or from a
//! rule the handler enforces itself — so the two cannot be told apart by their path
//! grammar. A request whose elements break one rule in several positions states one
//! clause per position, counted in the envelope, because each position is a separate
//! fact about the request.
//!
//! The rows differ along what the rule generalises over: the POSITION of the offending
//! element (first, middle, last), the SIZE of the list (one entry, a few, and the
//! modelled maximum of 25 — the member is a list of `@length` 0..25, per
//! `smithy_query(query="shape",shape="CrossAccountConfigurations")`), the NUMBER of
//! offending elements (one, two, every one of them), the SOURCE of the requirement (the
//! modelled `@length` minimum the boundary walk applies, and the same-account rule the
//! handlers enforce themselves) and the OPERATIONS that store the list
//! (`CreateInvestigationGroup` and `UpdateInvestigationGroup`), with all-foreign rows as
//! the positive control.
//!
//! An account holds one investigation group per Region, so each row signs for its own
//! Regions: one with an empty store for the create, one already holding a group for the
//! update.

use std::sync::Arc;

use serde_json::{json, Map, Value};
use service_com_amazonaws_aiops_aiops::state::{constraint_clause, validation_envelope};
use smithy_server_runtime::MultiServiceRouter;

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`): the investigation
/// account, which a cross-account source role may NOT name.
const ACCOUNT: &str = "000000000000";

/// An account OTHER than the investigation account, which a source role may name.
const FOREIGN_ACCOUNT: &str = "111122223333";

/// A role ARN that satisfies the modelled `RoleArn` (length 20..2048, `^arn:.*$`).
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/AIOpsAssistantRole";

/// The modelled error shape both writes refuse a bad list with
/// (`smithy_query(query="operation",operation="UpdateInvestigationGroup")`:
/// `ValidationException`, `@httpError 400`).
const VALIDATION: &str = "ValidationException";

/// The most entries `crossAccountConfigurations` admits, from the member's own modelled
/// length (`smithy_query(query="shape",shape="CrossAccountConfigurations")`: a list of
/// `@length` 0..25).
const MAX_CONFIGURATIONS: usize = 25;

/// The shortest `sourceRoleArn` the model admits, from the member's own `@length`
/// (`smithy_query(query="shape",shape="CrossAccountConfiguration")`: it targets
/// `RoleArn`, `@length` 20..2048).
const MIN_SOURCE_ROLE_ARN_LENGTH: usize = 20;

/// Real AWS Regions the rows sign for, in order; past the list the names are synthetic
/// ones inside what a SigV4 credential scope admits (`[a-zA-Z0-9-]`).
const REGIONS: &[&str] = &[
    "us-west-2",
    "us-east-1",
    "us-east-2",
    "eu-central-1",
    "eu-west-1",
    "eu-west-2",
    "ap-southeast-1",
    "ap-northeast-1",
    "sa-east-1",
    "ca-central-1",
    "ap-south-1",
    "eu-north-1",
    "me-south-1",
    "af-south-1",
    "ap-east-1",
    "us-gov-west-1",
];

/// The Region slot `n` signs for; each row needs two of its own.
fn region(n: usize) -> String {
    REGIONS
        .get(n)
        .map_or_else(|| format!("region-{n:04}"), |name| (*name).to_string())
}

/// One emulator with an empty store, mounted exactly as the router mounts it.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_aiops_aiops::EmulatedService::new(
        engine,
    ));
    router
}

/// A request signed for `aiops` in `region`, the signal the router routes on and the
/// handlers read the Region from.
fn request(
    region: &str,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> http::Request<bytes::Bytes> {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("aiops.{region}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDAIOPSTEST/20260101/{region}/aiops/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        );
    let payload = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            bytes::Bytes::from(serde_json::to_vec(value).expect("serialize the request body"))
        }
        None => bytes::Bytes::new(),
    };
    builder.body(payload).expect("a well-formed test request")
}

/// Status and parsed body of one call. An empty body reads as `Value::Null`.
async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let response = router.dispatch(request(region, method, uri, body)).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let mut parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    // restJson1 carries the error shape in `__type`, and the framework also sets the
    // `x-amzn-errortype` header; fold the header in so one accessor reads either.
    if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
        map.entry("__type")
            .or_insert_with(|| Value::String(error_type));
    }
    (status, parsed)
}

/// Percent-encode a value for use as a single `@httpLabel` path segment, which is what
/// an SDK does: a non-greedy label may not carry a raw `/`.
fn label(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            '/' => "%2F".to_string(),
            ' ' => "%20".to_string(),
            other => other.to_string(),
        })
        .collect()
}

/// The modelled error shape a failed call reported.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        // An error `__type` may be namespaced (`com.amazonaws.aiops#Foo`).
        .rsplit('#')
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

fn message(body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
        .to_string()
}

async fn create(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    list: &Value,
) -> (u16, Value) {
    let mut body = Map::new();
    body.insert("name".to_string(), Value::String(name.to_string()));
    body.insert("roleArn".to_string(), Value::String(ROLE_ARN.to_string()));
    body.insert("crossAccountConfigurations".to_string(), list.clone());
    call_in(
        router,
        region,
        "POST",
        "/investigationGroups",
        Some(&Value::Object(body)),
    )
    .await
}

async fn update(
    router: &MultiServiceRouter,
    region: &str,
    arn: &str,
    list: &Value,
) -> (u16, Value) {
    call_in(
        router,
        region,
        "PATCH",
        &format!("/investigationGroups/{}", label(arn)),
        Some(&json!({ "crossAccountConfigurations": list })),
    )
    .await
}

/// Create a group that must succeed in `region`, and return its ARN.
async fn create_ok(router: &MultiServiceRouter, region: &str, name: &str) -> String {
    let (status, created) = call_in(
        router,
        region,
        "POST",
        "/investigationGroups",
        Some(&json!({ "name": name, "roleArn": ROLE_ARN })),
    )
    .await;
    assert_eq!(
        status, 201,
        "create {name:?}: expected 201, got {status} / {created:?}"
    );
    created
        .get("arn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("create {name:?}: response carried no arn, got {created:?}"))
        .to_string()
}

/// What one element of the list is, which decides whether and why it is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Entry {
    /// A source role in another account: accepted.
    Foreign,
    /// A source role in the investigation account: refused by the rule the handlers
    /// enforce themselves, which no modelled constraint expresses.
    OwnAccount,
    /// A source role below the member's modelled length minimum: refused by the
    /// constraint walk at the request boundary.
    TooShort,
}

impl Entry {
    /// The `sourceRoleArn` this kind of element carries, distinguished by `n` so no two
    /// elements of one list are the same value.
    fn source_role_arn(self, n: usize) -> String {
        match self {
            Entry::Foreign => format!("arn:aws:iam::{FOREIGN_ACCOUNT}:role/Foreign{n}"),
            Entry::OwnAccount => format!("arn:aws:iam::{ACCOUNT}:role/Own{n}"),
            // One character short of the modelled minimum, and still inside the
            // modelled `^arn:.*$` pattern, so the length is the only requirement it
            // breaks.
            Entry::TooShort => {
                let stem = format!("arn:aws:iam:{n}");
                let pad = (MIN_SOURCE_ROLE_ARN_LENGTH - 1).saturating_sub(stem.len());
                format!("{stem}{}", "x".repeat(pad))
            }
        }
    }

    /// The requirement a clause states for an element of this kind, or `None` when the
    /// kind is accepted.
    fn requirement(self) -> Option<String> {
        match self {
            Entry::Foreign => None,
            Entry::OwnAccount => Some(format!(
                "Member must name an account other than the investigation account {ACCOUNT}"
            )),
            Entry::TooShort => Some(format!(
                "Member must have length greater than or equal to {MIN_SOURCE_ROLE_ARN_LENGTH}"
            )),
        }
    }
}

/// The `crossAccountConfigurations` list `kinds` describes.
fn list(kinds: &[Entry]) -> Value {
    Value::Array(
        kinds
            .iter()
            .enumerate()
            .map(|(index, kind)| json!({ "sourceRoleArn": kind.source_role_arn(index + 1) }))
            .collect(),
    )
}

/// The whole message `kinds` must be refused with, or `None` when every element is
/// accepted: one clause per offending POSITION, in list order, inside the envelope that
/// counts them.
fn expected_message(kinds: &[Entry]) -> Option<String> {
    let clauses: Vec<String> = kinds
        .iter()
        .enumerate()
        .filter_map(|(index, kind)| {
            kind.requirement().map(|requirement| {
                constraint_clause(
                    // The path grammar under test, spelled out rather than built
                    // through the service's own helper.
                    &format!(
                        "crossAccountConfigurations.{}.member.sourceRoleArn",
                        index + 1
                    ),
                    &requirement,
                )
            })
        })
        .collect();
    (!clauses.is_empty()).then(|| validation_envelope(&clauses))
}

/// A row: the case and the elements the list carries. Each row carries at most one KIND
/// of offending element, because the boundary walk refuses a request before any handler
/// rule runs — the order those two are applied in is pinned by
/// `tests/all/write_configuration_rejection_chain.rs`.
struct Row {
    case: &'static str,
    kinds: Vec<Entry>,
}

/// `count` elements, every one of them `kind`, except position `at` (1-based) which is
/// `offender`.
fn one_offender(count: usize, at: usize, offender: Entry) -> Vec<Entry> {
    (1..=count)
        .map(|position| {
            if position == at {
                offender
            } else {
                Entry::Foreign
            }
        })
        .collect()
}

fn rows() -> Vec<Row> {
    let mut rows = vec![
        Row {
            case: "an empty list",
            kinds: Vec::new(),
        },
        Row {
            case: "one accepted element",
            kinds: vec![Entry::Foreign],
        },
        Row {
            case: "a full list of accepted elements",
            kinds: vec![Entry::Foreign; MAX_CONFIGURATIONS],
        },
        Row {
            case: "the only element names the investigation account",
            kinds: vec![Entry::OwnAccount],
        },
        Row {
            case: "the only element is below the modelled minimum",
            kinds: vec![Entry::TooShort],
        },
        Row {
            case: "the investigation account in the middle of a short list",
            kinds: one_offender(3, 2, Entry::OwnAccount),
        },
        Row {
            case: "below the modelled minimum in the middle of a short list",
            kinds: one_offender(3, 2, Entry::TooShort),
        },
        Row {
            case: "two elements name the investigation account",
            kinds: vec![
                Entry::Foreign,
                Entry::OwnAccount,
                Entry::Foreign,
                Entry::OwnAccount,
            ],
        },
        Row {
            case: "two elements below the modelled minimum",
            kinds: vec![
                Entry::Foreign,
                Entry::TooShort,
                Entry::Foreign,
                Entry::TooShort,
            ],
        },
        Row {
            case: "every element names the investigation account",
            kinds: vec![Entry::OwnAccount; 4],
        },
    ];
    // The first and the last position of a FULL list: the positions a position-less
    // clause could not tell apart.
    for at in [1, MAX_CONFIGURATIONS] {
        rows.push(Row {
            case: if at == 1 {
                "the investigation account first in a full list"
            } else {
                "the investigation account last in a full list"
            },
            kinds: one_offender(MAX_CONFIGURATIONS, at, Entry::OwnAccount),
        });
        rows.push(Row {
            case: if at == 1 {
                "below the modelled minimum first in a full list"
            } else {
                "below the modelled minimum last in a full list"
            },
            kinds: one_offender(MAX_CONFIGURATIONS, at, Entry::TooShort),
        });
    }
    rows
}

#[tokio::test]
async fn a_refusal_inside_a_list_element_names_that_element() {
    let router = emulator();

    for (index, row) in rows().iter().enumerate() {
        let case = row.case;
        let sent = list(&row.kinds);
        let expected = expected_message(&row.kinds);

        // Two Regions per row: one with an empty store for the create, one already
        // holding a group for the update.
        let create_region = region(2 * index);
        let update_region = region(2 * index + 1);
        let arn = create_ok(&router, &update_region, &format!("list-paths-{index}")).await;

        let (create_status, created) =
            create(&router, &create_region, &format!("list-source-{index}"), &sent).await;
        let (update_status, updated) = update(&router, &update_region, &arn, &sent).await;

        for (operation, status, body) in [
            ("CreateInvestigationGroup", create_status, &created),
            ("UpdateInvestigationGroup", update_status, &updated),
        ] {
            match &expected {
                Some(expected) => {
                    assert_eq!(
                        status, 400,
                        "case {case:?}: expected {operation} to refuse with 400, \
                         got {status} / {body:?}"
                    );
                    assert_eq!(
                        error_code(body),
                        VALIDATION,
                        "case {case:?}: expected {operation} to report {VALIDATION}, \
                         got {body:?}"
                    );
                    assert_eq!(
                        &message(body),
                        expected,
                        "case {case:?}: expected {operation} to answer {expected:?}, got {:?}",
                        message(body)
                    );
                }
                None => {
                    let accepted = if operation == "CreateInvestigationGroup" {
                        201
                    } else {
                        200
                    };
                    assert_eq!(
                        status, accepted,
                        "case {case:?}: expected {operation} to accept the list with \
                         {accepted}, got {status} / {body:?}"
                    );
                }
            }
        }
    }
}

/// The subject of every clause a message states: the member path between `at '` and the
/// quote that closes it.
fn clause_subjects(message: &str) -> Vec<String> {
    message
        .split("at '")
        .skip(1)
        .filter_map(|rest| rest.split_once('\''))
        .map(|(subject, _)| subject.to_string())
        .collect()
}

/// The two kinds of rule refuse the SAME element at the SAME path: one grammar, so a
/// client cannot tell a handler-enforced refusal from a boundary refusal by where it
/// points.
///
/// The cases differ along the position of the offending element and the size of the list
/// it sits in, which is exactly what the path has to carry.
#[tokio::test]
async fn both_rules_refuse_one_element_at_one_path() {
    let router = emulator();

    // (case, list size, 1-based position of the offending element)
    let cases: &[(&str, usize, usize)] = &[
        ("the only element", 1, 1),
        ("the first of two", 2, 1),
        ("the last of two", 2, 2),
        ("the middle of a longer list", 7, 4),
        ("the first of a full list", MAX_CONFIGURATIONS, 1),
        (
            "the last of a full list",
            MAX_CONFIGURATIONS,
            MAX_CONFIGURATIONS,
        ),
    ];

    for (index, (case, size, at)) in cases.iter().enumerate() {
        let region = region(index);
        let arn = create_ok(&router, &region, &format!("one-grammar-{index}")).await;

        let mut subjects = Vec::new();
        for offender in [Entry::OwnAccount, Entry::TooShort] {
            let sent = list(&one_offender(*size, *at, offender));
            let (status, body) = update(&router, &region, &arn, &sent).await;
            assert_eq!(
                status, 400,
                "case {case:?} with {offender:?}: expected 400, got {status} / {body:?}"
            );
            subjects.push(clause_subjects(&message(&body)));
        }

        let want = vec![format!(
            "crossAccountConfigurations.{at}.member.sourceRoleArn"
        )];
        for (offender, got) in [Entry::OwnAccount, Entry::TooShort].iter().zip(&subjects) {
            assert_eq!(
                got, &want,
                "case {case:?} with {offender:?}: expected the clause subjects {want:?}, \
                 got {got:?}"
            );
        }
    }
}
