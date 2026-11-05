// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the rule that a stored cross-account configuration
//! NAMES an account, and names one other than the investigation account.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers — so what is
//! asserted is what a signed AWS SDK request observes.
//!
//! The rule under test: an entry of `crossAccountConfigurations` is acceptable only
//! when its `sourceRoleArn` is present and is a complete ARN whose account id field
//! names an account other than the one the investigation group belongs to; anything
//! else is refused with the modelled `ValidationException` (400) and is never stored.
//! A source role that names no other account is refused with the service's own sentence
//! for that error class, which names the investigation account and echoes the ARN the
//! request sent, and names the first such entry; an entry carrying no source role at all
//! is the condition the clause grammar states, one clause per offending position inside
//! the envelope that counts them. The model admits both ways an entry can
//! name no account at all — `sourceRoleArn` is not `@required`
//! (`smithy_query(query="shape",shape="CrossAccountConfiguration")`), and its
//! constraints (`@length 20..2048`, `^arn:.*$`) admit text with fewer fields than an
//! ARN carrying an account id — so the rows below walk the FORMS a source role can
//! arrive in, not just one value.
//!
//! An account holds one investigation group per Region, so each row drives the create
//! in a Region of its own, and the update against a Region that already holds a group.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_aiops_aiops::state::{
    absent_member_clause, requirement_text, validation_envelope,
};
use smithy_server_runtime::{MultiServiceRouter, ViolationKind};

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`): the investigation
/// account, which a cross-account source role may NOT name.
const ACCOUNT: &str = "000000000000";

/// An account OTHER than the investigation account, which a source role may name.
const FOREIGN_ACCOUNT: &str = "111122223333";

/// A role ARN for the group itself, satisfying the modelled `RoleArn`
/// (`@length 20..2048`, `^arn:.*$`).
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/AIOpsAssistantRole";

/// The modelled error shape a refused write reports
/// (`smithy_query(query="operation",operation="CreateInvestigationGroup")`:
/// `ValidationException`, `@httpError 400`).
const VALIDATION: &str = "ValidationException";

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

/// The Region slot `n` signs for.
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
/// handlers read the Region and the caller principal from.
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

async fn create(router: &MultiServiceRouter, region: &str, body: &Value) -> (u16, Value) {
    call_in(router, region, "POST", "/investigationGroups", Some(body)).await
}

async fn update(
    router: &MultiServiceRouter,
    region: &str,
    arn: &str,
    body: &Value,
) -> (u16, Value) {
    call_in(
        router,
        region,
        "PATCH",
        &format!("/investigationGroups/{}", label(arn)),
        Some(body),
    )
    .await
}

async fn get(router: &MultiServiceRouter, region: &str, arn: &str) -> Value {
    let (status, body) = call_in(
        router,
        region,
        "GET",
        &format!("/investigationGroups/{}", label(arn)),
        None,
    )
    .await;
    assert_eq!(status, 200, "a read-back must answer 200, got {body:?}");
    body
}

/// Create a group that must succeed in `region`, and return its ARN.
async fn create_ok(router: &MultiServiceRouter, region: &str, body: &Value) -> String {
    let (status, created) = create(router, region, body).await;
    assert_eq!(status, 201, "create {body}: expected 201, got {created:?}");
    created
        .get("arn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("create {body}: response carried no arn, got {created:?}"))
        .to_string()
}

/// The ARNs of the groups the Region holds.
async fn listed(router: &MultiServiceRouter, region: &str) -> Vec<String> {
    let (status, body) = call_in(router, region, "GET", "/investigationGroups", None).await;
    assert_eq!(status, 200, "a list must answer 200, got {body:?}");
    body.get("investigationGroups")
        .and_then(Value::as_array)
        .map(|groups| {
            groups
                .iter()
                .filter_map(|group| group.get("arn").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// What a refused list must be refused with.
#[derive(Debug, Clone)]
enum Refusal {
    /// Every entry that carried no `sourceRoleArn`, by 1-based position: the clause
    /// grammar states one clause per position, and the envelope counts them.
    AbsentMembers(Vec<usize>),
    /// The FIRST entry whose source role names no account other than this one, by the
    /// ARN the refusal echoes: the service's sentence names one source role.
    NoOtherAccount(String),
}

/// A row: the `crossAccountConfigurations` list as sent, and the refusal it must draw
/// — `None` when both writes must accept and STORE it.
struct Row {
    case: &'static str,
    entries: Value,
    refused: Option<Refusal>,
}

/// The whole message a refused list must carry.
///
/// `AbsentMembers` is the clause grammar, matched in full: the envelope over one
/// absent-member clause per offending position, whose requirement is the framework's
/// own absent-member wording, so the test states none of it itself. `NoOtherAccount` is
/// the service's own sentence for that error class: fixed text around the investigation
/// account and the ARN the request sent.
fn expected_message(refused: &Refusal) -> String {
    match refused {
        Refusal::AbsentMembers(positions) => {
            let clauses: Vec<String> = positions
                .iter()
                .map(|position| {
                    let path =
                        format!("crossAccountConfigurations.{position}.member.sourceRoleArn");
                    absent_member_clause(
                        &path,
                        &requirement_text(ViolationKind::MissingRequiredMember),
                    )
                })
                .collect();
            validation_envelope(&clauses)
        }
        Refusal::NoOtherAccount(arn) => format!(
            "Source role ARN cannot belong to the investigation account {ACCOUNT}. \
             Found ARN: {arn}"
        ),
    }
}

/// One entry naming `arn` as its source role.
fn entry(arn: &str) -> Value {
    json!({ "sourceRoleArn": arn })
}

/// A refusal echoing the source role `arn` that names no other account.
fn no_other_account(arn: &str) -> Option<Refusal> {
    Some(Refusal::NoOtherAccount(arn.to_string()))
}

/// A refusal of every entry at the 1-based `positions` that carried no source role.
fn absent_members(positions: &[usize]) -> Option<Refusal> {
    Some(Refusal::AbsentMembers(positions.to_vec()))
}

/// The table. The rule generalises over the FORM a source role arrives in, so the rows
/// differ along: whether the member is there at all; how many colon-separated fields
/// the value carries and therefore whether an account id can be read from it; whether
/// that account is this one, a lookalike of this one, or another; the partition and the
/// resource shape of a well-formed ARN; the number of entries; and the position of the
/// offending entries inside the list.
fn rows() -> Vec<Row> {
    let good = format!("arn:aws:iam::{FOREIGN_ACCOUNT}:role/Good");
    let own = format!("arn:aws:iam::{ACCOUNT}:role/Own");
    let own_at_a_path = format!("arn:aws:iam::{ACCOUNT}:role/team/ops/Own");
    let no_account_field = "arn:aws:iam::role/CrossAccountInvestigation";
    let empty_account_field = "arn:aws:iam:::role/CrossAccountInvestigation";
    let stops_at_the_account = format!("arn:aws:iam::{FOREIGN_ACCOUNT}");
    let partition_and_service = "arn:aws:identitymanagement";
    vec![
        // --- accepted: a complete ARN naming another account, in several forms ---
        Row {
            case: "an empty list names no configuration at all",
            entries: json!([]),
            refused: None,
        },
        Row {
            case: "one foreign-account role",
            entries: json!([entry(&format!(
                "arn:aws:iam::{FOREIGN_ACCOUNT}:role/AIOpsCrossAccountRole"
            ))]),
            refused: None,
        },
        Row {
            case: "a foreign-account role in another partition",
            entries: json!([entry(&format!(
                "arn:aws-us-gov:iam::{FOREIGN_ACCOUNT}:role/AIOpsCrossAccountRole"
            ))]),
            refused: None,
        },
        Row {
            case: "a foreign-account role at a path, and one whose resource holds colons",
            entries: json!([
                entry(&format!("arn:aws:iam::{FOREIGN_ACCOUNT}:role/team/ops/Reader")),
                entry(&format!(
                    "arn:aws:iam::{FOREIGN_ACCOUNT}:role:scoped:AIOpsCrossAccountRole"
                )),
            ]),
            refused: None,
        },
        Row {
            case: "an account that merely looks like the investigation account",
            entries: json!([entry(&format!("arn:aws:iam::{ACCOUNT}1:role/Lookalike"))]),
            refused: None,
        },
        // --- refused: the entry names THIS account ---
        Row {
            case: "the investigation account itself",
            entries: json!([entry(&own)]),
            refused: no_other_account(&own),
        },
        Row {
            case: "the investigation account behind a resource path",
            entries: json!([entry(&own_at_a_path)]),
            refused: no_other_account(&own_at_a_path),
        },
        // --- refused: the entry names NO account ---
        Row {
            case: "an ARN with no account field",
            entries: json!([entry(no_account_field)]),
            refused: no_other_account(no_account_field),
        },
        Row {
            case: "an ARN whose account field is empty",
            entries: json!([entry(empty_account_field)]),
            refused: no_other_account(empty_account_field),
        },
        Row {
            case: "an ARN that stops at the account field, with no resource",
            entries: json!([entry(&stops_at_the_account)]),
            refused: no_other_account(&stops_at_the_account),
        },
        Row {
            case: "a value with nothing but a partition and a service",
            entries: json!([entry(partition_and_service)]),
            refused: no_other_account(partition_and_service),
        },
        // --- refused: the entry carries no source role at all ---
        Row {
            case: "an entry carrying no member whatsoever",
            entries: json!([{}]),
            refused: absent_members(&[1]),
        },
        // (An entry whose `sourceRoleArn` is an explicit JSON `null` never reaches this
        // rule: the restJson1 deserializer refuses a null where a string is modelled,
        // before any handler runs.)
        // --- positions and counts: every absent member reports, in list order, and the
        // sentence that names ONE source role reports the first offending entry ---
        Row {
            case: "an empty entry after a good one",
            entries: json!([entry(&good), json!({})]),
            refused: absent_members(&[2]),
        },
        Row {
            case: "every way of naming no other account, in one list",
            entries: json!([
                entry(&good),
                json!({}),
                entry(no_account_field),
                entry(&own),
            ]),
            refused: absent_members(&[2]),
        },
        Row {
            case: "a good entry, then two that name no other account",
            entries: json!([entry(&good), entry(&own), entry(no_account_field)]),
            refused: no_other_account(&own),
        },
        Row {
            case: "a list of nothing but empty entries",
            entries: json!([{}, {}, {}]),
            refused: absent_members(&[1, 2, 3]),
        },
        Row {
            case: "empty entries around a good one",
            entries: json!([json!({}), entry(&good), json!({})]),
            refused: absent_members(&[1, 3]),
        },
    ]
}

/// The members a write may replace, which a refused write must leave exactly as they
/// were.
const CONFIGURATION_MEMBERS: &[&str] = &[
    "crossAccountConfigurations",
    "encryptionConfiguration",
    "roleArn",
    "lastModifiedAt",
];

/// Assert the answer to one refused write.
fn assert_refused(operation: &str, case: &str, status: u16, body: &Value, refused: &Refusal) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected {operation} to refuse with 400, got {status} / {body:?}"
    );
    assert_eq!(
        error_code(body),
        VALIDATION,
        "case {case:?}: expected {operation} to report {VALIDATION}, got {body:?}"
    );
    let reported = message(body);
    let expected = expected_message(refused);
    assert_eq!(
        reported, expected,
        "case {case:?}: expected {operation} to answer {expected:?}, got {reported:?}"
    );
}

#[tokio::test]
async fn a_stored_cross_account_configuration_names_another_account() {
    let router = emulator();
    let table = rows();

    // The update path runs against a Region holding a group whose stored list is the
    // positive control, so an accepted row can be seen to REPLACE it and a refused one
    // to leave it alone.
    let control = json!([entry(&format!(
        "arn:aws:iam::{FOREIGN_ACCOUNT}:role/ControlRole"
    ))]);
    let update_region = region(2 * table.len());
    let target = create_ok(
        &router,
        &update_region,
        &json!({
            "name": "cross-account-update-target",
            "roleArn": ROLE_ARN,
            "crossAccountConfigurations": control,
        }),
    )
    .await;

    for (index, row) in table.iter().enumerate() {
        let case = row.case;
        let create_region = region(2 * index);
        let body = json!({
            "name": format!("cross-account-{index}"),
            "roleArn": ROLE_ARN,
            "crossAccountConfigurations": row.entries,
        });

        let before = get(&router, &update_region, &target).await;
        let (create_status, created) = create(&router, &create_region, &body).await;
        let patch = json!({ "crossAccountConfigurations": row.entries });
        let (update_status, updated) = update(&router, &update_region, &target, &patch).await;

        if row.refused.is_none() {
            assert_eq!(
                create_status, 201,
                "case {case:?}: expected the create to be accepted, \
                 got {create_status} / {created:?}"
            );
            assert_eq!(
                update_status, 200,
                "case {case:?}: expected the update to be accepted, \
                 got {update_status} / {updated:?}"
            );

            // Accepted means STORED: both writes' lists are what a later read reports,
            // and an empty list stores no configuration at all.
            let sent = row
                .entries
                .as_array()
                .expect("every row sends a list")
                .clone();
            let want = (!sent.is_empty()).then_some(Value::Array(sent));
            let created_arn = created
                .get("arn")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: the create reported no arn"))
                .to_string();
            for (operation, region, arn) in [
                (
                    "CreateInvestigationGroup",
                    create_region.as_str(),
                    created_arn.as_str(),
                ),
                (
                    "UpdateInvestigationGroup",
                    update_region.as_str(),
                    target.as_str(),
                ),
            ] {
                let stored = get(&router, region, arn).await;
                assert_eq!(
                    stored.get("crossAccountConfigurations"),
                    want.as_ref(),
                    "case {case:?}: expected {operation} to store {want:?}, got {:?}",
                    stored.get("crossAccountConfigurations")
                );
            }

            // Put the control back, so the next row's update starts from it.
            let (status, answer) = update(
                &router,
                &update_region,
                &target,
                &json!({ "crossAccountConfigurations": control }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: restoring the control must be accepted, got {answer:?}"
            );
        } else if let Some(refused) = &row.refused {
            assert_refused(
                "CreateInvestigationGroup",
                case,
                create_status,
                &created,
                refused,
            );
            assert_refused(
                "UpdateInvestigationGroup",
                case,
                update_status,
                &updated,
                refused,
            );

            // The two writes refuse it IDENTICALLY, which is what one shared rule buys.
            assert_eq!(
                message(&created),
                message(&updated),
                "case {case:?}: the two writes must word one refusal the same way, \
                 got {:?} and {:?}",
                message(&created),
                message(&updated)
            );

            // A refused create stores nothing at all...
            let left = listed(&router, &create_region).await;
            assert!(
                left.is_empty(),
                "case {case:?}: a refused create must store nothing, got {left:?}"
            );
            // ...and a refused update leaves the group it addressed exactly as it was.
            let after = get(&router, &update_region, &target).await;
            for member in CONFIGURATION_MEMBERS {
                assert_eq!(
                    after.get(member),
                    before.get(member),
                    "case {case:?}: a refused update must leave {member} alone, \
                     was {:?} now {:?}",
                    before.get(member),
                    after.get(member)
                );
            }
        }
    }
}
