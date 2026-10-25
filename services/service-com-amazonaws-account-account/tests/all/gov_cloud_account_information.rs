// SPDX-License-Identifier: CC0-1.0

//! What `GetGovCloudAccountInformation` reports, over the real request path.
//!
//! RULE under test: the operation REPORTS the GovCloud account the store holds
//! linked to the standard account the request names — the linked account's
//! 12-digit id and the linked account's own `AwsAccountState`, exactly as
//! stored — defaulting to the caller's own account when the request names none;
//! a standard account the store holds with no GovCloud account linked to it has
//! no such resource to report (the modelled HTTP 404
//! `ResourceNotFoundException`), a standard account the store holds no record of
//! is the authorization refusal every reader of this service answers (HTTP 403
//! `AccessDeniedException`), and the report writes nothing, so a later caller
//! sees exactly what this one saw.
//!
//! No account comes up GovCloud-linked — nothing in this emulator creates a
//! GovCloud account — so the standalone call on a fresh store is that 404, and
//! becomes a report only once a link is recorded.
//!
//! The rows span what the rule generalises over: which account the call names
//! (none, the caller's own, several member ids differing in their digits, ids
//! with no record at all), whether a link is recorded for it, the linked
//! account's id (including the bounds of the 12-digit space) and its state
//! (every value `AwsAccountState` declares).
//!
//! Nothing Account Management binds CREATES a GovCloud account — that is an
//! Organizations operation — so each row that needs a link records one in the
//! store through the same seam such a creator writes through
//! (`Account::link_gov_cloud_account`), the own-account rows through the
//! minting constructor `GovCloudAccountLink::active_for` so the id in the
//! answer is one the store minted. Every case then goes through
//! `MultiServiceRouter::dispatch`, so routing, the boundary validator, the
//! handler, the persisted store and the response codec all take part.

use std::sync::Arc;
use std::time::SystemTime;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_account_account::state::{
    default_account_id, service_state, GovCloudAccountLink,
};
use service_com_amazonaws_account_account::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service: it is what routes a request to
/// the `account` service. Signatures are not verified here.
const SIGNED_FOR_US_EAST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/account/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// `AwsAccountState`'s values, from `smithy_query(query="shape",
/// shape="com.amazonaws.account#AwsAccountState")` — spelled out here, not
/// imported, so a wrong set in the implementation cannot make this test pass.
const AWS_ACCOUNT_STATES: [&str; 4] = ["PENDING_ACTIVATION", "ACTIVE", "SUSPENDED", "CLOSED"];

/// One recorded link: the standard account, the GovCloud account id linked to
/// it and that GovCloud account's own state.
///
/// The rows differ in the standard account's digits, in the linked id (both
/// bounds of the 12-digit space and values in between) and in the state — one
/// row per value `AwsAccountState` declares, so no row's answer can be the
/// state another row stored.
struct Link {
    case: &'static str,
    standard_account_id: &'static str,
    gov_cloud_account_id: &'static str,
    state: &'static str,
}

const RECORDED: &[Link] = &[
    Link {
        case: "a pending GovCloud account, linked id at the bottom of the space",
        standard_account_id: "210987654321",
        gov_cloud_account_id: "000000000001",
        state: "PENDING_ACTIVATION",
    },
    Link {
        case: "an active GovCloud account",
        standard_account_id: "123123123123",
        gov_cloud_account_id: "123456789012",
        state: "ACTIVE",
    },
    Link {
        case: "a suspended GovCloud account, linked id at the top of the space",
        standard_account_id: "000000000002",
        gov_cloud_account_id: "999999999999",
        state: "SUSPENDED",
    },
    Link {
        case: "a closed GovCloud account",
        standard_account_id: "999999999998",
        gov_cloud_account_id: "900000000000",
        state: "CLOSED",
    },
];

/// Account ids inside `AccountId`'s `^\d{12}$` that no request and no seed ever
/// wrote, so the store holds no record of them.
const UNRECORDED_ACCOUNT_IDS: &[&str] = &[
    "111111111111",
    "000000000003",
    "999999999997",
    "321321321321",
];

/// Values OUTSIDE `StandardAccountId`'s modelled `^\d{12}$`: too short, too
/// long, non-digits, empty, and digits with separators.
const MALFORMED_ACCOUNT_IDS: &[(&str, &str)] = &[
    ("one digit short", "12345678901"),
    ("one digit long", "1234567890123"),
    ("letters", "abcdefghijkl"),
    ("empty", ""),
    ("digits with a hyphen", "1234-5678901"),
];

/// One emulator, plus the shared engine behind it so a case can record a link
/// in the store the handler reads.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// Record `gov_cloud_account_id` as the GovCloud account of `standard_account_id`,
/// the way whatever creates a GovCloud account records it: the store the handler
/// reads is the one written here.
fn record_link(engine: &engine::InMemoryEngine, link: &Link) {
    let state = service_state(engine).expect("the account store is reachable");
    let mut store = state.lock();
    let account = store
        .account_mut(link.standard_account_id, SystemTime::now())
        .expect("the store admits this account record");
    account.link_gov_cloud_account(GovCloudAccountLink {
        gov_cloud_account_id: link.gov_cloud_account_id.to_string(),
        state: link.state.to_string(),
    });
}

/// Record a MINTED GovCloud account for `standard_account_id` — the whole
/// write a creator of a GovCloud account performs, id and all — and answer the
/// pair the store now holds for it.
///
/// The id comes from `GovCloudAccountLink::active_for`, so nothing in this test
/// chooses it: the assertions that follow are about what the store minted and
/// stored, not about a value this file wrote into the response.
fn record_minted_link(engine: &engine::InMemoryEngine, standard_account_id: &str) -> (String, String) {
    let state = service_state(engine).expect("the account store is reachable");
    {
        let mut store = state.lock();
        let account = store
            .account_mut(standard_account_id, SystemTime::now())
            .expect("the store admits this account record");
        account.link_gov_cloud_account(GovCloudAccountLink::active_for(standard_account_id));
    }
    stored_link(engine, standard_account_id).expect("the link just recorded is held")
}

/// The GovCloud account id and state the store holds for `account_id`, or `None`
/// when the record carries no link.
fn stored_link(engine: &engine::InMemoryEngine, account_id: &str) -> Option<(String, String)> {
    let state = service_state(engine).expect("the account store is reachable");
    let store = state.lock();
    store.account(account_id).and_then(|account| {
        account
            .gov_cloud_account
            .as_ref()
            .map(|link| (link.gov_cloud_account_id.clone(), link.state.clone()))
    })
}

/// Whether `value` satisfies `AccountId`'s modelled `^\d{12}$`. Spelled out, not
/// imported.
fn is_account_id(value: &str) -> bool {
    value.len() == 12 && value.bytes().all(|byte| byte.is_ascii_digit())
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

    /// One member of the body, as a string (empty when absent).
    fn member(&self, name: &str) -> &str {
        self.body[name].as_str().unwrap_or_default()
    }
}

async fn call(router: &MultiServiceRouter, uri: &str, body: Value) -> Answer {
    let request = ::http::Request::builder()
        .method("POST")
        .uri(uri)
        .header("host", "account.us-east-1.amazonaws.com")
        .header("authorization", SIGNED_FOR_US_EAST_1)
        .header("content-type", "application/json")
        .body(Bytes::from(body.to_string()))
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

async fn get_gov_cloud_account_information(router: &MultiServiceRouter, body: Value) -> Answer {
    call(router, "/getGovCloudAccountInformation", body).await
}

/// The standalone call — the one that names no account, which the model
/// documents as defaulting to the account of the calling identity — is the
/// modelled 404 until a GovCloud account is linked to that account, and reports
/// the link once one is recorded; naming that same account explicitly is the
/// authorization refusal instead, whether a link is recorded or not.
///
/// Nothing in this emulator comes up GovCloud-linked: the link is recorded
/// through the same minting constructor whatever creates a GovCloud account
/// would use, so the id in the answer is one the store minted and never one the
/// test or the request supplied.
#[tokio::test]
async fn the_standalone_call_reports_the_callers_own_linked_account() {
    let (router, engine) = emulator();
    let own = default_account_id().to_string();

    assert!(
        stored_link(&engine, &own).is_none(),
        "case a fresh store: the caller's own account must come up with no GovCloud link"
    );
    for (case, request) in [
        ("the standalone call, unlinked", json!({})),
        ("the standalone call, unlinked, repeated", json!({})),
    ] {
        let answer = get_gov_cloud_account_information(&router, request).await;
        assert_eq!(
            (answer.status, answer.error_code()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected the modelled 404, got {} with {:?}",
            answer.status,
            answer.body
        );
    }

    // Naming the caller's own account in `StandardAccountId` is refused: the
    // member names a member account of the organization.
    let named = get_gov_cloud_account_information(&router, json!({"StandardAccountId": own})).await;
    assert_eq!(
        (named.status, named.error_code()),
        (403, "AccessDeniedException"),
        "case \"the caller's own account named explicitly, unlinked\": expected the 403 \
         refusal, got {} with {:?}",
        named.status,
        named.body
    );

    // The link arrives the way a creator of a GovCloud account records one.
    let (stored_id, stored_state) = record_minted_link(&engine, &own);

    for (case, request) in [
        ("the standalone call", json!({})),
        ("the standalone call, repeated", json!({})),
    ] {
        let answer = get_gov_cloud_account_information(&router, request).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.member("GovCloudAccountId"),
            stored_id,
            "case {case:?}: expected the stored linked id {stored_id:?}, got {:?}",
            answer.member("GovCloudAccountId")
        );
        assert_eq!(
            answer.member("AccountState"),
            stored_state,
            "case {case:?}: expected the stored linked state {stored_state:?}, got {:?}",
            answer.member("AccountState")
        );
        assert!(
            is_account_id(answer.member("GovCloudAccountId")),
            "case {case:?}: the reported id must satisfy ^\\d{{12}}$, got {:?}",
            answer.member("GovCloudAccountId")
        );
        assert_ne!(
            answer.member("GovCloudAccountId"),
            own,
            "case {case:?}: the GovCloud account is a separate account from the standard one"
        );
        assert!(
            AWS_ACCOUNT_STATES.contains(&answer.member("AccountState")),
            "case {case:?}: expected one of {AWS_ACCOUNT_STATES:?}, got {:?}",
            answer.member("AccountState")
        );
    }

    // The recorded link does not make the self-naming request answerable either:
    // the refusal is about which account the member may name, not about the link.
    let named = get_gov_cloud_account_information(&router, json!({"StandardAccountId": own})).await;
    assert_eq!(
        (named.status, named.error_code()),
        (403, "AccessDeniedException"),
        "case \"the caller's own account named explicitly, linked\": expected the 403 \
         refusal, got {} with {:?}",
        named.status,
        named.body
    );
}

/// A link the store holds is reported exactly as stored — the linked account's
/// id and the linked account's OWN state — for every row of the class, and never
/// the standard account's own state or id.
#[tokio::test]
async fn a_recorded_link_is_reported_as_stored() {
    let (router, engine) = emulator();
    for link in RECORDED {
        record_link(&engine, link);
    }

    for link in RECORDED {
        let case = link.case;
        let answer = get_gov_cloud_account_information(
            &router,
            json!({"StandardAccountId": link.standard_account_id}),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.member("GovCloudAccountId"),
            link.gov_cloud_account_id,
            "case {case:?}: expected the stored id {:?}, got {:?}",
            link.gov_cloud_account_id,
            answer.member("GovCloudAccountId")
        );
        assert_eq!(
            answer.member("AccountState"),
            link.state,
            "case {case:?}: expected the linked account's stored state {:?}, got {:?}",
            link.state,
            answer.member("AccountState")
        );

        // The standard account itself is ACTIVE in this store, so a row whose
        // linked account is in any other state proves the reported state is the
        // LINKED account's and not the standard account's.
        let standard = call(
            &router,
            "/getAccountInformation",
            json!({"AccountId": link.standard_account_id}),
        )
        .await;
        assert_eq!(
            (standard.status, standard.member("AccountState")),
            (200, "ACTIVE"),
            "case {case:?}: the standard account must read back ACTIVE, got {:?}",
            standard.body
        );
        if link.state != "ACTIVE" {
            assert_ne!(
                answer.member("AccountState"),
                standard.member("AccountState"),
                "case {case:?}: the reported state must be the LINKED account's, not the \
                 standard account's"
            );
        }
    }
}

/// A standard account the store HOLDS, with no GovCloud account linked to it,
/// has no such resource to report: HTTP 404 `ResourceNotFoundException`, for
/// every row — and the refusal is not the authorization one, which is about the
/// standard account rather than about its GovCloud account.
#[tokio::test]
async fn an_account_with_no_link_is_not_found() {
    let (router, engine) = emulator();

    for (index, standard_account_id) in UNRECORDED_ACCOUNT_IDS.iter().enumerate() {
        // The write is what brings the record into existence, exactly as a
        // client would: the account is then readable and simply carries no link.
        let created = call(
            &router,
            "/putAccountName",
            json!({"AccountName": format!("Member {index}"), "AccountId": standard_account_id}),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {standard_account_id:?}: the member record must be created, got {} with {:?}",
            created.status, created.body
        );
        assert!(
            stored_link(&engine, standard_account_id).is_none(),
            "case {standard_account_id:?}: a member record must carry no link until one is recorded"
        );

        let answer = get_gov_cloud_account_information(
            &router,
            json!({"StandardAccountId": standard_account_id}),
        )
        .await;
        assert_eq!(
            answer.status, 404,
            "case {standard_account_id:?}: expected HTTP 404, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ResourceNotFoundException",
            "case {standard_account_id:?}: expected ResourceNotFoundException, got {:?}",
            answer.body
        );

        // Recording a link for that same account turns the SAME request into a
        // report: the 404 is about the missing link, nothing else.
        let link = Link {
            case: "a link recorded after the refusal",
            standard_account_id: "000000000000",
            gov_cloud_account_id: "555555555555",
            state: AWS_ACCOUNT_STATES[index % AWS_ACCOUNT_STATES.len()],
        };
        record_link(
            &engine,
            &Link {
                standard_account_id,
                ..link
            },
        );
        let after = get_gov_cloud_account_information(
            &router,
            json!({"StandardAccountId": standard_account_id}),
        )
        .await;
        assert_eq!(
            (after.status, after.member("GovCloudAccountId"), after.member("AccountState")),
            (200, link.gov_cloud_account_id, link.state),
            "case {standard_account_id:?}: expected the recorded link reported, got {:?}",
            after.body
        );
    }
}

/// A standard account the store holds NO record of is the authorization refusal
/// — HTTP 403 `AccessDeniedException` — and not the 404, which would claim the
/// account is readable and merely unlinked.
#[tokio::test]
async fn an_account_with_no_record_is_an_authorization_refusal() {
    let (router, _engine) = emulator();

    for standard_account_id in UNRECORDED_ACCOUNT_IDS {
        let answer = get_gov_cloud_account_information(
            &router,
            json!({"StandardAccountId": standard_account_id}),
        )
        .await;
        assert_eq!(
            answer.status, 403,
            "case {standard_account_id:?}: expected HTTP 403, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "AccessDeniedException",
            "case {standard_account_id:?}: expected AccessDeniedException, got {:?}",
            answer.body
        );
    }
}

/// A `StandardAccountId` outside the member's modelled `^\d{12}$` is refused at
/// the boundary with HTTP 400 `ValidationException`, whatever the store holds.
#[tokio::test]
async fn a_malformed_standard_account_id_is_a_validation_refusal() {
    let (router, _engine) = emulator();

    for (case, value) in MALFORMED_ACCOUNT_IDS {
        let answer =
            get_gov_cloud_account_information(&router, json!({"StandardAccountId": value})).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            answer.body
        );
    }
}

/// The operation is a REPORT: repeating it answers the same thing, it creates no
/// record for an account the store does not hold, it creates no link for an
/// account that has none, and it leaves the standard account's own record
/// untouched.
#[tokio::test]
async fn a_read_changes_nothing_a_later_caller_sees() {
    let (router, engine) = emulator();
    for link in RECORDED {
        record_link(&engine, link);
    }
    record_minted_link(&engine, default_account_id());

    // A linked account: two reads, byte for byte the same answer.
    for link in RECORDED {
        let case = link.case;
        let first = get_gov_cloud_account_information(
            &router,
            json!({"StandardAccountId": link.standard_account_id}),
        )
        .await;
        let second = get_gov_cloud_account_information(
            &router,
            json!({"StandardAccountId": link.standard_account_id}),
        )
        .await;
        assert_eq!(
            (first.status, &first.body),
            (second.status, &second.body),
            "case {case:?}: a repeated read must answer the same, got {:?} then {:?}",
            first.body,
            second.body
        );
    }

    // An account with no record: the read leaves it with none, so a later
    // reader still gets the authorization refusal rather than a report.
    for standard_account_id in UNRECORDED_ACCOUNT_IDS {
        get_gov_cloud_account_information(
            &router,
            json!({"StandardAccountId": standard_account_id}),
        )
        .await;
        assert!(
            stored_link(&engine, standard_account_id).is_none(),
            "case {standard_account_id:?}: the read must create neither a record nor a link"
        );
        let later = call(
            &router,
            "/getAccountInformation",
            json!({"AccountId": standard_account_id}),
        )
        .await;
        assert_eq!(
            later.status, 403,
            "case {standard_account_id:?}: the read must leave the account unknown, got {} with {:?}",
            later.status, later.body
        );
    }

    // An account the store holds with no link: the read leaves it unlinked, so
    // the next read is the same 404 and not a report.
    let unlinked = "444444444444";
    let created = call(
        &router,
        "/putAccountName",
        json!({"AccountName": "Unlinked", "AccountId": unlinked}),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "the unlinked member record must be created, got {} with {:?}",
        created.status, created.body
    );
    for attempt in 1..=2 {
        let answer =
            get_gov_cloud_account_information(&router, json!({"StandardAccountId": unlinked}))
                .await;
        assert_eq!(
            (answer.status, answer.error_code()),
            (404, "ResourceNotFoundException"),
            "case read {attempt} of an unlinked account: expected the same 404, got {:?}",
            answer.body
        );
    }
    assert!(
        stored_link(&engine, unlinked).is_none(),
        "case an unlinked account: the read must not record a link"
    );

    // And the caller's own account, linked before any of these reads, still
    // reports exactly the link that was recorded for it.
    let own = default_account_id().to_string();
    let (stored_id, stored_state) =
        stored_link(&engine, &own).expect("the link recorded for the own account is still held");
    let answer = get_gov_cloud_account_information(&router, json!({})).await;
    assert_eq!(
        (
            answer.status,
            answer.member("GovCloudAccountId"),
            answer.member("AccountState")
        ),
        (200, stored_id.as_str(), stored_state.as_str()),
        "case the caller's own account after every other read: expected the stored link, got {:?}",
        answer.body
    );
}
