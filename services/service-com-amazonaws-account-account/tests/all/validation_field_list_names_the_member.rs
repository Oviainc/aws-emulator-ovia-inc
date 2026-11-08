// SPDX-License-Identifier: CC0-1.0

//! The one rule every refusal this service decides about a single input member
//! follows, driven end to end over HTTP.
//!
//! The rule: when this service refuses a request because ONE named input member
//! is at fault, the modelled 400 `ValidationException` reports that member in
//! `fieldList` — "The field where the invalid entry was detected"
//! (`smithy_query(query="shape", shape="ValidationException")`) — with a message
//! about it, and carries reason `fieldValidationFailed`, the
//! `ValidationExceptionReason` for a member whose value failed validation
//! (`smithy_query(query="shape", shape="ValidationExceptionReason")`).
//! `fieldList`
//! is left unset only where no single input member is at fault. Two refusals of
//! the same member are told apart by their message, not by whether the member is
//! named or by the reason reported.
//!
//! The table spans what the rule generalises over rather than one request: the
//! member at fault (`RegionName`, `CountryCode`, `StateOrRegion`, `NextToken`,
//! `PrimaryEmail`), the operation carrying it (six of them, including both
//! Region-opt calls and a reporting one), the kind of fault
//! (a value outside a catalog this service keeps, a value that is catalogued but
//! not a legal target, a member left out that its sibling made required, a token
//! this service never minted, a value outside an expression) and whether the
//! member is at the top level of the request or nested inside a structure. The
//! last test is the counter-case that keeps the rule from collapsing into "always
//! set `fieldList`": a refusal no single input member is at fault for names none.
//!
//! Only refusals THIS SERVICE decides are in scope: a value outside a modelled
//! constraint is refused at the request boundary, whose wording
//! `tests/all/pattern_validation_messages.rs` and
//! `tests/all/enum_input_validation.rs` pin.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_account_account::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service: it is what routes a request to
/// the `account` service (its sigv4 signing name).
const SIGNED_FOR_US_EAST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/account/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The wire error code and status of the modelled `ValidationException`, from
/// `smithy_query(query="shape", shape="ValidationException")` (`errorKind:
/// client`, `httpError: 400`).
const VALIDATION_EXCEPTION: &str = "ValidationException";
const VALIDATION_STATUS: u16 = 400;

/// The `ValidationExceptionReason` a refusal detected on one input member
/// reports, from
/// `smithy_query(query="shape", shape="ValidationExceptionReason")`.
const FIELD_VALIDATION_FAILED: &str = "fieldValidationFailed";

/// A Region Amazon Web Services turns on for every account, so it is not a
/// Region-opt target. SOURCE: the AWS General Reference Region table "Available
/// Regions" (<https://docs.aws.amazon.com/general/latest/gr/rande-manage.html>),
/// the rows marked "Enabled by default" — spelled out here rather than imported,
/// and neither row is the Region the issue behind this suite cited.
const DEFAULT_ENABLED: [&str; 2] = ["eu-north-1", "sa-east-1"];

/// A `NextToken` no `ListRegions` response ever carried: the hex of a string
/// that is not a Region code.
const UNISSUED_NEXT_TOKEN: &str = "6e6f742061207265616c20746f6b656e";

/// A member account id satisfying `AccountId`'s `^\d{12}$` which is not the
/// caller's own account, for the operation whose account-id member is required.
const MEMBER_ACCOUNT: &str = "111111111111";

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

    /// One member of this answer's body, as a string (empty when absent).
    fn member(&self, name: &str) -> &str {
        self.body[name].as_str().unwrap_or_default()
    }

    /// The members this refusal's `fieldList` names, in the order it lists them,
    /// or `None` when it carries no `fieldList` at all.
    fn invalid_members(&self) -> Option<Vec<&str>> {
        Some(
            self.body["fieldList"]
                .as_array()?
                .iter()
                .map(|field| field["name"].as_str().unwrap_or_default())
                .collect(),
        )
    }

    /// The `message` of the `fieldList` entry at `index` (empty when absent).
    fn field_message(&self, index: usize) -> &str {
        self.body["fieldList"][index]["message"]
            .as_str()
            .unwrap_or_default()
    }
}

async fn call(router: &MultiServiceRouter, uri: &str, body: Value) -> Answer {
    let payload = body.to_string();
    let request = ::http::Request::builder()
        .method("POST")
        .uri(uri)
        .header("host", "account.us-east-1.amazonaws.com")
        .header("authorization", SIGNED_FOR_US_EAST_1)
        .header("content-type", "application/json")
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "POST {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

/// A `ContactInformation` whose every member but the two under test is inside
/// its modelled constraint, so a refusal of it is a refusal of the country or
/// the state alone.
fn contact_in(country_code: &str, state_or_region: Option<&str>) -> Value {
    let mut contact = json!({
        "FullName": "Grace Hopper",
        "AddressLine1": "1 Compiler Road",
        "City": "Example City",
        "PostalCode": "00002",
        "CountryCode": country_code,
        "PhoneNumber": "+1 206-555-0199",
    });
    if let Some(state) = state_or_region {
        contact["StateOrRegion"] = json!(state);
    }
    contact
}

/// One row of the class: a request this service refuses because one named input
/// member is at fault.
struct Row {
    /// How this row is reported when it fails: the operation and the fault.
    case: &'static str,
    uri: &'static str,
    request: Value,
    /// The `ValidationExceptionReason` the refusal reports.
    reason: &'static str,
    /// The input member at fault, which `fieldList` must name.
    member: &'static str,
}

/// Every refusal this service decides about one named input member, one row per
/// (member, operation, reason, kind of fault) the rule covers.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "EnableRegion on a Region enabled by default",
            uri: "/enableRegion",
            request: json!({ "RegionName": DEFAULT_ENABLED[0] }),
            reason: FIELD_VALIDATION_FAILED,
            member: "RegionName",
        },
        Row {
            case: "DisableRegion on a Region enabled by default",
            uri: "/disableRegion",
            request: json!({ "RegionName": DEFAULT_ENABLED[1] }),
            reason: FIELD_VALIDATION_FAILED,
            member: "RegionName",
        },
        Row {
            case: "EnableRegion on a value that is not a Region code",
            uri: "/enableRegion",
            request: json!({ "RegionName": "eu-west-9" }),
            reason: FIELD_VALIDATION_FAILED,
            member: "RegionName",
        },
        Row {
            case: "DisableRegion on a Region code of another partition",
            uri: "/disableRegion",
            request: json!({ "RegionName": "us-gov-west-1" }),
            reason: FIELD_VALIDATION_FAILED,
            member: "RegionName",
        },
        Row {
            case: "GetRegionOptStatus on a bare word",
            uri: "/getRegionOptStatus",
            request: json!({ "RegionName": "somewhere" }),
            reason: FIELD_VALIDATION_FAILED,
            member: "RegionName",
        },
        Row {
            case: "PutContactInformation with a country code no country carries",
            uri: "/putContactInformation",
            request: json!({ "ContactInformation": contact_in("ZZ", None) }),
            reason: FIELD_VALIDATION_FAILED,
            member: "CountryCode",
        },
        Row {
            case: "PutContactInformation in a country that requires a state, with none",
            uri: "/putContactInformation",
            request: json!({ "ContactInformation": contact_in("BR", None) }),
            reason: FIELD_VALIDATION_FAILED,
            member: "StateOrRegion",
        },
        Row {
            case: "ListRegions resuming from a token it never issued",
            uri: "/listRegions",
            request: json!({ "NextToken": UNISSUED_NEXT_TOKEN }),
            reason: FIELD_VALIDATION_FAILED,
            member: "NextToken",
        },
        Row {
            case: "StartPrimaryEmailUpdate towards a value that is not an address",
            uri: "/startPrimaryEmailUpdate",
            request: json!({ "AccountId": MEMBER_ACCOUNT, "PrimaryEmail": "no-mailbox-here" }),
            reason: FIELD_VALIDATION_FAILED,
            member: "PrimaryEmail",
        },
    ]
}

/// Every refusal of a single named input member reports that member in
/// `fieldList`, whichever reason it carries.
#[tokio::test]
async fn a_refusal_of_one_input_member_names_that_member() {
    let router = emulator();

    for row in rows() {
        let case = row.case;
        let answer = call(&router, row.uri, row.request.clone()).await;

        assert_eq!(
            answer.status, VALIDATION_STATUS,
            "case {case:?}: expected {VALIDATION_STATUS}, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            VALIDATION_EXCEPTION,
            "case {case:?}: expected {VALIDATION_EXCEPTION}, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.member("reason"),
            row.reason,
            "case {case:?}: expected reason {:?}, got {:?}",
            row.reason,
            answer.body
        );
        let named = answer.invalid_members().unwrap_or_else(|| {
            panic!(
                "case {case:?}: the refusal must name the member at fault in fieldList, got {:?}",
                answer.body
            )
        });
        assert_eq!(
            named,
            vec![row.member],
            "case {case:?}: fieldList must name {:?}, got {:?}",
            row.member,
            answer.body
        );
        assert!(
            !answer.field_message(0).is_empty(),
            "case {case:?}: the fieldList entry must carry a message about the member, got {:?}",
            answer.body
        );
    }
}

/// The two refusals of ONE member report it alike: the message differs, the named
/// member and the reason do not. Both Region-opt calls are checked, since they
/// decide by one shared rule.
#[tokio::test]
async fn the_two_refusals_of_the_region_member_name_it_alike() {
    let router = emulator();

    for uri in ["/enableRegion", "/disableRegion"] {
        let mut answers = Vec::new();
        for (fault, region_name) in [
            ("a Region enabled by default", DEFAULT_ENABLED[0]),
            ("a value that is not a Region code", "nowhere-central-9"),
        ] {
            let case = format!("{uri} for {fault}");
            let answer = call(&router, uri, json!({ "RegionName": region_name })).await;
            assert_eq!(
                answer.status, VALIDATION_STATUS,
                "case {case:?}: expected {VALIDATION_STATUS}, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.invalid_members(),
                Some(vec!["RegionName"]),
                "case {case:?}: fieldList must name RegionName, got {:?}",
                answer.body
            );
            answers.push((case, answer));
        }

        // The two refusals are told apart by their message, not by whether the
        // member at fault is reported or by the reason they carry.
        let (ref first_case, ref first) = answers[0];
        let (ref second_case, ref second) = answers[1];
        assert_ne!(
            first.member("message"),
            second.member("message"),
            "case {first_case:?} vs {second_case:?}: the two refusals must differ in message, \
             both reported {:?}",
            first.member("message")
        );
        for (case, answer) in [(first_case, first), (second_case, second)] {
            assert_eq!(
                answer.member("reason"),
                FIELD_VALIDATION_FAILED,
                "case {case:?}: expected reason {FIELD_VALIDATION_FAILED}, got {:?}",
                answer.body
            );
        }
        assert_eq!(
            first.invalid_members(),
            second.invalid_members(),
            "case {first_case:?} vs {second_case:?}: both refuse RegionName, so both must name \
             it; got {:?} and {:?}",
            first.body,
            second.body
        );
    }
}

/// The counter-case: a refusal no single input member is at fault for names
/// none. `GetPrimaryEmailUpdateStatus` with no update in flight is refused
/// because the request assumes a resource that is not there, not because a
/// member it carries is wrong — so `fieldList` (and `reason`, which neither
/// modelled value covers) stays unset.
#[tokio::test]
async fn a_refusal_no_input_member_is_at_fault_for_names_none() {
    let router = emulator();

    // A member account this emulator holds a record of, so the read reaches the
    // update that is not there rather than the account that is not reachable.
    let named = call(
        &router,
        "/putAccountName",
        json!({ "AccountId": MEMBER_ACCOUNT, "AccountName": "Example Member" }),
    )
    .await;
    assert_eq!(
        named.status, 200,
        "naming a member account must answer 200, got {} with {:?}",
        named.status, named.body
    );

    for (case, request) in [
        ("GetPrimaryEmailUpdateStatus with no update", json!({})),
        (
            "GetPrimaryEmailUpdateStatus for a member account with no update",
            json!({ "AccountId": MEMBER_ACCOUNT }),
        ),
    ] {
        let answer = call(&router, "/getPrimaryEmailUpdateStatus", request).await;
        assert_eq!(
            answer.status, VALIDATION_STATUS,
            "case {case:?}: expected {VALIDATION_STATUS}, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            VALIDATION_EXCEPTION,
            "case {case:?}: expected {VALIDATION_EXCEPTION}, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.invalid_members(),
            None,
            "case {case:?}: no input member is at fault, so fieldList must stay unset, got {:?}",
            answer.body
        );
    }
}
