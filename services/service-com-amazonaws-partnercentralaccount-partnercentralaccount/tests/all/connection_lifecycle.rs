// SPDX-License-Identifier: CC0-1.0

//! The connection-invitation -> connection lifecycle, end to end over the wire:
//! `CreateConnectionInvitation`, `GetConnectionInvitation`,
//! `ListConnectionInvitations`, `AcceptConnectionInvitation`, `GetConnection` and
//! `CancelConnection`.
//!
//! Every call goes through the real [`MultiServiceRouter`] the production emulator
//! mounts (`POST /` with `X-Amz-Target: PartnerCentralAccount.<Operation>`,
//! `awsJson1_0`), and only the HTTP status and the JSON body are inspected — so a
//! response that is not computed from persisted state cannot pass.
//!
//! The tests are tables: each row is one input along the axis the rule under test
//! generalises over (catalog, connection type, message and name form, receiver
//! identifier form, page size, filter, signing region), and the `partner-123` case
//! from the bootstrap issue is only one row.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_partnercentralaccount_partnercentralaccount::state::{
    ComAmazonawsPartnercentralaccountPartnercentralaccountState as PartnerCentralState,
    MAX_CONNECTIONS, MAX_FILTER_IDENTIFIERS, MAX_IDEMPOTENCY_TOKENS, MAX_INVITATIONS,
};
use service_com_amazonaws_partnercentralaccount_partnercentralaccount::EmulatedService;

/// The region every test signs with unless it is exercising the region rule itself.
const REGION: &str = "us-east-1";

/// The account id every emulated resource presents (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// `MaxResults` on both list operations is `range 1..100` with `@default 20`
/// (source: `smithy_query(query="operation",operation="ListConnectionInvitations")`).
const MAX_RESULTS_MIN: i64 = 1;
const MAX_RESULTS_MAX: i64 = 100;

/// An emulator with just this service mounted on a fresh engine, driven in-process
/// through the production router. The engine handle is kept so a test can move the
/// stored clock (there is no other way to reach an instant 30 days out).
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signatures are not what these tests are about; the router still routes by
        // the SigV4 credential scope below, which is also where an ARN's region comes
        // from.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// Call one operation in `region` and return `(http status, parsed body)`.
    async fn call_in(&self, region: &str, operation: &str, body: &Value) -> (u16, Value) {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/partnercentral-account/aws4_request, \
             SignedHeaders=host;x-amz-target, Signature=0000"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("authorization", authorization)
            .header("host", format!("partnercentral-account.{region}.amazonaws.com"))
            .header("content-type", "application/x-amz-json-1.0")
            .header("x-amz-target", format!("PartnerCentralAccount.{operation}"))
            .body(Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let header_error = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let bytes = response.into_body();
        let mut parsed = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                panic!(
                    "{operation} answered a body that is not JSON ({error}): {}",
                    String::from_utf8_lossy(&bytes)
                )
            })
        };
        // The awsJson error envelope carries the code in `__type`; the boundary
        // validator also sets `x-amzn-errortype`. Fold the header in so one helper
        // reads the code of either refusal.
        if let (Value::Object(map), false) = (&mut parsed, header_error.is_empty()) {
            map.entry("__type")
                .or_insert_with(|| Value::String(header_error));
        }
        (status, parsed)
    }

    async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        self.call_in(REGION, operation, body).await
    }

    /// Succeed-or-explain: the call must answer 200.
    async fn ok(&self, operation: &str, body: &Value) -> Value {
        let (status, parsed) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "{operation} expected 200, got {status}: {parsed}"
        );
        parsed
    }
}

/// The wire error code of an error body: the `__type` of the awsJson envelope (or
/// the `x-amzn-errortype` the boundary validator sets, folded in above).
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .to_string()
}

/// A failure's status and code, checked together: a client branches on both.
fn assert_error(case: &str, got: (u16, Value), status: u16, code: &str) {
    let (actual_status, body) = got;
    assert_eq!(
        actual_status, status,
        "case {case:?}: expected HTTP {status}, got {actual_status}: {body}"
    );
    let actual_code = error_code(&body);
    assert!(
        actual_code.contains(code),
        "case {case:?}: expected error code {code}, got {actual_code:?}: {body}"
    );
}

/// `ConnectionInvitationId`'s modelled `@pattern` `^pacinv-[A-Za-z0-9]{13}$` and
/// `ConnectionId`'s `^pac-[A-Za-z0-9]{13}$` (source:
/// `smithy_query(query="operation",operation="CreateConnectionInvitation")`),
/// checked by hand because this crate has no regex dependency.
fn has_id_shape(value: &str, prefix: &str) -> bool {
    value.strip_prefix(prefix).is_some_and(|body| {
        body.len() == 13 && body.chars().all(|c| c.is_ascii_alphanumeric())
    })
}

fn is_invitation_id(value: &str) -> bool {
    has_id_shape(value, "pacinv-")
}

fn is_connection_id(value: &str) -> bool {
    has_id_shape(value, "pac-")
}

/// The ARN partition a region belongs to, from AWS's published prefixes: `cn-*` is
/// `aws-cn`, `us-gov-*` is `aws-us-gov`, and every commercial region is `aws`. The
/// modelled ARN pattern's partition segment is `[a-zA-Z0-9-]+`, so it carries any of
/// them.
fn partition_for(region: &str) -> &'static str {
    if region.starts_with("cn-") {
        return "aws-cn";
    }
    if region.starts_with("us-gov-") {
        return "aws-us-gov";
    }
    "aws"
}

/// `ConnectionInvitationArn`'s modelled form
/// `arn:<partition>:partnercentral:<region>::catalog/<Catalog>/connection-invitation/<Id>`.
fn invitation_arn(region: &str, catalog: &str, id: &str) -> String {
    format!(
        "arn:{}:partnercentral:{region}::catalog/{catalog}/connection-invitation/{id}",
        partition_for(region)
    )
}

/// `ConnectionArn`'s modelled form
/// `arn:<partition>:partnercentral:<region>::catalog/<Catalog>/connection/<Id>`.
fn connection_arn(region: &str, catalog: &str, id: &str) -> String {
    format!(
        "arn:{}:partnercentral:{region}::catalog/{catalog}/connection/{id}",
        partition_for(region)
    )
}

/// A required string member of a response.
fn string(body: &Value, member: &str, case: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: {member} is not a string: {body}"))
        .to_string()
}

/// A timestamp member as epoch seconds, accepting either wire form a `DateTime`
/// takes (an epoch-seconds number, or the RFC 3339 string the model's
/// `@timestampFormat("date-time")` asks for).
fn timestamp(body: &Value, member: &str, case: &str) -> f64 {
    match body.get(member) {
        Some(Value::Number(number)) => number
            .as_f64()
            .unwrap_or_else(|| panic!("case {case:?}: {member} is not a number: {body}")),
        Some(Value::String(text)) => DateTime::from_str(text, Format::DateTime)
            .unwrap_or_else(|error| {
                panic!("case {case:?}: {member} is not an RFC 3339 instant ({error}): {body}")
            })
            .as_secs_f64(),
        _ => panic!("case {case:?}: {member} is absent or not a timestamp: {body}"),
    }
}

/// One `CreateConnectionInvitation` request.
#[derive(Debug, Clone)]
struct InvitationCase {
    label: &'static str,
    catalog: &'static str,
    connection_type: &'static str,
    email: &'static str,
    message: String,
    name: &'static str,
    receiver: String,
}

impl InvitationCase {
    fn request(&self, client_token: &str) -> Value {
        json!({
            "Catalog": self.catalog,
            "ClientToken": client_token,
            "ConnectionType": self.connection_type,
            "Email": self.email,
            "Message": self.message,
            "Name": self.name,
            "ReceiverIdentifier": self.receiver,
        })
    }
}

/// The create table: rows differ along every axis the create rule generalises over —
/// catalog, connection type (both `ConnectionType` values), email and name form,
/// message length and content (including the newline the modelled pattern allows and
/// the 80-character maximum the compiled schema enforces), and receiver-identifier
/// form.
fn invitation_cases() -> Vec<InvitationCase> {
    vec![
        InvitationCase {
            label: "the issue's recorded case",
            catalog: "AWS",
            connection_type: "OPPORTUNITY_COLLABORATION",
            email: "inviter@example.com",
            message: "hello".to_string(),
            name: "Inviter",
            receiver: "partner-123".to_string(),
        },
        InvitationCase {
            label: "subsidiary connection in the Sandbox catalog",
            catalog: "Sandbox",
            connection_type: "SUBSIDIARY",
            email: "ops.team+subsidiary@partner.example.co.uk",
            message: "first line\nsecond line".to_string(),
            name: "Ops Team",
            receiver: "pprofile-ABCDEFGHIJKLM".to_string(),
        },
        InvitationCase {
            label: "longest message the compiled schema allows",
            catalog: "AWS",
            connection_type: "OPPORTUNITY_COLLABORATION",
            // `Message` is `UnicodeStringIncludingNewLine`; the compiled schema
            // enforces length 1..80 (the operation header's "len 1..80").
            message: "m".repeat(80),
            email: "a@b.co",
            name: "Ünicode Nâme",
            receiver: "r".repeat(100),
        },
        InvitationCase {
            label: "single-character receiver and shortest message",
            catalog: "AWS",
            connection_type: "SUBSIDIARY",
            email: "x@y.io",
            message: "m".to_string(),
            name: "A",
            receiver: "r".to_string(),
        },
    ]
}

/// The members a create response and the matching get response must agree on.
const SHARED_MEMBERS: &[&str] = &[
    "Catalog",
    "Id",
    "Arn",
    "ConnectionType",
    "OtherParticipantIdentifier",
    "ParticipantType",
    "Status",
    "InvitationMessage",
    "InviterEmail",
    "InviterName",
];

/// CreateConnectionInvitation mints and stores an invitation, and
/// GetConnectionInvitation reads that record back — over the whole create table.
#[tokio::test]
async fn create_then_get_reads_back_the_stored_invitation() {
    let emulator = Emulator::new();
    let mut seen_ids: BTreeSet<String> = BTreeSet::new();

    for (index, case) in invitation_cases().into_iter().enumerate() {
        let label = case.label;
        let created = emulator
            .ok(
                "CreateConnectionInvitation",
                &case.request(&format!("token-{index}")),
            )
            .await;

        let id = string(&created, "Id", label);
        assert!(
            is_invitation_id(&id),
            "case {label:?}: Id must match ^pacinv-[A-Za-z0-9]{{13}}$, got {id:?}"
        );
        assert!(
            seen_ids.insert(id.clone()),
            "case {label:?}: Id {id} was already minted for another invitation"
        );
        assert_eq!(
            string(&created, "Arn", label),
            invitation_arn(REGION, case.catalog, &id),
            "case {label:?}: Arn must name the catalog and the minted id"
        );
        assert_eq!(
            string(&created, "Status", label),
            "PENDING",
            "case {label:?}: a new invitation is PENDING"
        );
        assert_eq!(
            string(&created, "ParticipantType", label),
            "SENDER",
            "case {label:?}: the creator of an invitation is its SENDER"
        );
        assert_eq!(
            string(&created, "OtherParticipantIdentifier", label),
            case.receiver,
            "case {label:?}: OtherParticipantIdentifier is the invited participant"
        );
        assert_eq!(
            string(&created, "InvitationMessage", label),
            case.message,
            "case {label:?}: InvitationMessage is the message the invitation carries"
        );
        assert_eq!(
            string(&created, "InviterEmail", label),
            case.email,
            "case {label:?}: InviterEmail is the sender's address"
        );
        assert_eq!(
            string(&created, "InviterName", label),
            case.name,
            "case {label:?}: InviterName is the sender's name"
        );
        assert_eq!(
            string(&created, "ConnectionType", label),
            case.connection_type,
            "case {label:?}: ConnectionType is the type that was requested"
        );
        assert!(
            created.get("ConnectionId").is_none_or(Value::is_null),
            "case {label:?}: an unanswered invitation has no ConnectionId: {created}"
        );

        let created_at = timestamp(&created, "CreatedAt", label);
        let updated_at = timestamp(&created, "UpdatedAt", label);
        let expires_at = timestamp(&created, "ExpiresAt", label);
        assert!(
            (created_at - updated_at).abs() < f64::EPSILON,
            "case {label:?}: an invitation nothing has changed has CreatedAt == UpdatedAt, got {created_at} and {updated_at}"
        );
        assert!(
            expires_at > created_at,
            "case {label:?}: ExpiresAt {expires_at} must follow CreatedAt {created_at}"
        );

        let fetched = emulator
            .ok(
                "GetConnectionInvitation",
                &json!({ "Catalog": case.catalog, "Identifier": id }),
            )
            .await;
        for member in SHARED_MEMBERS {
            assert_eq!(
                fetched.get(*member),
                created.get(*member),
                "case {label:?}: {member} read back must equal what create stored"
            );
        }
        for member in ["CreatedAt", "UpdatedAt", "ExpiresAt"] {
            assert!(
                (timestamp(&fetched, member, label) - timestamp(&created, member, label)).abs()
                    < 1e-3,
                "case {label:?}: {member} read back must equal what create stored: {fetched}"
            );
        }
    }
}

/// The same `ClientToken` with the same request replays the stored invitation; the
/// same token with a different request is the conflict the model names
/// `CONFLICT_CLIENT_TOKEN`. One row per member a client could have changed.
#[tokio::test]
async fn create_is_idempotent_per_client_token() {
    let emulator = Emulator::new();
    let base = InvitationCase {
        label: "idempotency base",
        catalog: "AWS",
        connection_type: "OPPORTUNITY_COLLABORATION",
        email: "inviter@example.com",
        message: "hello".to_string(),
        name: "Inviter",
        receiver: "partner-123".to_string(),
    };

    let first = emulator
        .ok("CreateConnectionInvitation", &base.request("tok-1"))
        .await;
    let id = string(&first, "Id", base.label);

    // Replaying the identical request answers with the identical invitation...
    for attempt in 0..3 {
        let replay = emulator
            .ok("CreateConnectionInvitation", &base.request("tok-1"))
            .await;
        assert_eq!(
            replay, first,
            "case replay {attempt}: a replayed ClientToken answers with the stored invitation"
        );
    }

    // ... and does not create a second one.
    let listed = emulator
        .ok(
            "ListConnectionInvitations",
            &json!({ "Catalog": "AWS", "MaxResults": MAX_RESULTS_MAX }),
        )
        .await;
    let summaries = listed["ConnectionInvitationSummaries"]
        .as_array()
        .expect("ConnectionInvitationSummaries is a list");
    assert_eq!(
        summaries.len(),
        1,
        "a replayed ClientToken must not store a second invitation: {listed}"
    );
    assert_eq!(
        summaries[0]["Id"].as_str(),
        Some(id.as_str()),
        "the single stored invitation is the one the first call minted: {listed}"
    );

    // Each row changes exactly one member the token was first used with.
    let mut changed = vec![
        ("Email", base.request("tok-1")),
        ("Message", base.request("tok-1")),
        ("ConnectionType", base.request("tok-1")),
        ("Name", base.request("tok-1")),
        ("ReceiverIdentifier", base.request("tok-1")),
        ("Catalog", base.request("tok-1")),
    ];
    changed[0].1["Email"] = json!("someone.else@example.com");
    changed[1].1["Message"] = json!("a different message");
    changed[2].1["ConnectionType"] = json!("SUBSIDIARY");
    changed[3].1["Name"] = json!("Another Inviter");
    changed[4].1["ReceiverIdentifier"] = json!("partner-456");
    changed[5].1["Catalog"] = json!("Sandbox");

    for (member, request) in changed {
        let answer = emulator.call("CreateConnectionInvitation", &request).await;
        assert_error(member, answer, 409, "ConflictException");
    }
}

/// An identifier this service never minted, or one minted in another catalog, is a
/// 404 — never a 501 and never an empty success.
#[tokio::test]
async fn get_connection_invitation_refuses_an_unknown_identifier() {
    let emulator = Emulator::new();
    let created = emulator
        .ok(
            "CreateConnectionInvitation",
            &json!({
                "Catalog": "AWS",
                "ClientToken": "tok-known",
                "ConnectionType": "OPPORTUNITY_COLLABORATION",
                "Email": "inviter@example.com",
                "Message": "hello",
                "Name": "Inviter",
                "ReceiverIdentifier": "partner-123",
            }),
        )
        .await;
    let id = string(&created, "Id", "known invitation");

    // Rows: a well-formed id nobody minted, the stored id read in another catalog,
    // and ids the modelled `@pattern` itself refuses (too short, wrong prefix).
    let cases: Vec<(&str, Value, u16, &str)> = vec![
        (
            "never minted",
            json!({ "Catalog": "AWS", "Identifier": "pacinv-0000000000000" }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "minted in another catalog",
            json!({ "Catalog": "Sandbox", "Identifier": id }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "body shorter than the pattern",
            json!({ "Catalog": "AWS", "Identifier": "pacinv-short" }),
            400,
            "ValidationException",
        ),
        (
            "a connection id, not an invitation id",
            json!({ "Catalog": "AWS", "Identifier": "pac-ABCDEFGHIJKLM" }),
            400,
            "ValidationException",
        ),
    ];

    for (label, request, status, code) in cases {
        let answer = emulator.call("GetConnectionInvitation", &request).await;
        if status == 404 {
            assert_eq!(
                answer.1.get("Reason").and_then(Value::as_str),
                Some("CONNECTION_INVITATION_NOT_FOUND"),
                "case {label:?}: a missing invitation names its reason: {}",
                answer.1
            );
        }
        assert_error(label, answer, status, code);
    }
}

/// `ListConnectionInvitations` reports the stored invitations of one catalog, honours
/// every declared filter, caps the page at `MaxResults` and resumes exactly where the
/// page stopped.
#[tokio::test]
async fn list_connection_invitations_filters_and_pages() {
    let emulator = Emulator::new();

    // Five invitations in the AWS catalog differing in type and receiver, plus one in
    // another catalog that must never appear.
    let seeds: Vec<(&str, &str)> = vec![
        ("OPPORTUNITY_COLLABORATION", "partner-1"),
        ("SUBSIDIARY", "partner-2"),
        ("OPPORTUNITY_COLLABORATION", "partner-3"),
        ("SUBSIDIARY", "partner-1"),
        ("OPPORTUNITY_COLLABORATION", "partner-4"),
    ];
    let mut created_order: Vec<String> = Vec::new();
    for (index, (connection_type, receiver)) in seeds.iter().enumerate() {
        let created = emulator
            .ok(
                "CreateConnectionInvitation",
                &json!({
                    "Catalog": "AWS",
                    "ClientToken": format!("seed-{index}"),
                    "ConnectionType": connection_type,
                    "Email": "inviter@example.com",
                    "Message": format!("invitation {index}"),
                    "Name": "Inviter",
                    "ReceiverIdentifier": receiver,
                }),
            )
            .await;
        created_order.push(string(&created, "Id", "seed"));
    }
    emulator
        .ok(
            "CreateConnectionInvitation",
            &json!({
                "Catalog": "Sandbox",
                "ClientToken": "seed-other-catalog",
                "ConnectionType": "SUBSIDIARY",
                "Email": "inviter@example.com",
                "Message": "other catalog",
                "Name": "Inviter",
                "ReceiverIdentifier": "partner-9",
            }),
        )
        .await;

    /// One page's ids, and whether the page carried a `NextToken`.
    async fn page(emulator: &Emulator, request: &Value) -> (Vec<String>, Option<String>) {
        let body = emulator.ok("ListConnectionInvitations", request).await;
        let ids = body["ConnectionInvitationSummaries"]
            .as_array()
            .expect("ConnectionInvitationSummaries is a list")
            .iter()
            .map(|summary| {
                summary["Id"]
                    .as_str()
                    .expect("every summary carries an Id")
                    .to_string()
            })
            .collect();
        let token = body
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        (ids, token)
    }

    // Filter rows: each names the invitations it must select, by their index in
    // `seeds`.
    let filter_cases: Vec<(&str, Value, Vec<usize>)> = vec![
        ("no filter", json!({}), vec![0, 1, 2, 3, 4]),
        (
            "ConnectionType=OPPORTUNITY_COLLABORATION",
            json!({ "ConnectionType": "OPPORTUNITY_COLLABORATION" }),
            vec![0, 2, 4],
        ),
        (
            "ConnectionType=SUBSIDIARY",
            json!({ "ConnectionType": "SUBSIDIARY" }),
            vec![1, 3],
        ),
        (
            "Status=PENDING",
            json!({ "Status": "PENDING" }),
            vec![0, 1, 2, 3, 4],
        ),
        ("Status=ACCEPTED", json!({ "Status": "ACCEPTED" }), vec![]),
        (
            "ParticipantType=SENDER",
            json!({ "ParticipantType": "SENDER" }),
            vec![0, 1, 2, 3, 4],
        ),
        (
            "ParticipantType=RECEIVER",
            json!({ "ParticipantType": "RECEIVER" }),
            vec![],
        ),
        (
            "one OtherParticipantIdentifier",
            json!({ "OtherParticipantIdentifiers": ["partner-3"] }),
            vec![2],
        ),
        (
            "several OtherParticipantIdentifiers",
            json!({ "OtherParticipantIdentifiers": ["partner-1", "partner-4"] }),
            vec![0, 3, 4],
        ),
        (
            "an identifier nothing was sent to",
            json!({ "OtherParticipantIdentifiers": ["partner-absent"] }),
            vec![],
        ),
        (
            "two filters together",
            json!({ "ConnectionType": "SUBSIDIARY", "OtherParticipantIdentifiers": ["partner-1"] }),
            vec![3],
        ),
    ];

    for (label, filter, expected) in filter_cases {
        let mut request = json!({ "Catalog": "AWS", "MaxResults": MAX_RESULTS_MAX });
        for (key, value) in filter.as_object().expect("a filter object") {
            request[key] = value.clone();
        }
        let (ids, token) = page(&emulator, &request).await;
        let wanted: Vec<String> = expected
            .iter()
            .map(|index| created_order[*index].clone())
            .collect();
        assert_eq!(
            ids, wanted,
            "case {label:?}: the page must report exactly the matching invitations, in creation order"
        );
        assert!(
            token.is_none(),
            "case {label:?}: a page that reported every match carries no NextToken, got {token:?}"
        );
    }

    // Paging rows: every page size from the smallest the model allows up to more than
    // the store holds. Following the tokens must report every invitation exactly once,
    // in creation order, with no page larger than `MaxResults`.
    for page_size in [MAX_RESULTS_MIN, 2, 3, 5, 6, MAX_RESULTS_MAX] {
        let label = format!("MaxResults={page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut next: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({ "Catalog": "AWS", "MaxResults": page_size });
            if let Some(token) = &next {
                request["NextToken"] = json!(token);
            }
            let (ids, token) = page(&emulator, &request).await;
            assert!(
                ids.len() <= usize::try_from(page_size).expect("a positive page size"),
                "case {label:?}: a page carried {} invitations, more than MaxResults",
                ids.len()
            );
            walked.extend(ids);
            pages += 1;
            assert!(
                pages <= 10,
                "case {label:?}: paging did not terminate after {pages} pages"
            );
            match token {
                None => break,
                Some(token) => next = Some(token),
            }
        }
        assert_eq!(
            walked, created_order,
            "case {label:?}: following the tokens must report every invitation exactly once, in creation order"
        );
    }

    // `MaxResults` outside its published range is refused, a `NextToken` this
    // service never minted names no resume point, and a filter list longer than the
    // member's modelled maximum is refused before any invitation is scanned.
    let at_the_filter_maximum: Vec<String> = (0..MAX_FILTER_IDENTIFIERS - 1)
        .map(|index| format!("absent-{index}"))
        .chain(std::iter::once("partner-3".to_string()))
        .collect();
    let past_the_filter_maximum: Vec<String> = (0..=MAX_FILTER_IDENTIFIERS)
        .map(|index| format!("absent-{index}"))
        .collect();

    // The longest filter the member allows still selects what it names.
    let (ids, _) = page(
        &emulator,
        &json!({ "Catalog": "AWS", "OtherParticipantIdentifiers": at_the_filter_maximum }),
    )
    .await;
    assert_eq!(
        ids,
        vec![created_order[2].clone()],
        "case \"the longest filter list the member allows\": the filter still selects the invitation it names"
    );

    let refused: Vec<(&str, Value, u16, &str)> = vec![
        (
            "MaxResults below the minimum",
            json!({ "Catalog": "AWS", "MaxResults": MAX_RESULTS_MIN - 1 }),
            400,
            "ValidationException",
        ),
        (
            "MaxResults above the maximum",
            json!({ "Catalog": "AWS", "MaxResults": MAX_RESULTS_MAX + 1 }),
            400,
            "ValidationException",
        ),
        (
            "a NextToken this service did not mint",
            json!({ "Catalog": "AWS", "NextToken": "not-a-cursor" }),
            400,
            "ValidationException",
        ),
        (
            "one identifier past the filter maximum",
            json!({ "Catalog": "AWS", "OtherParticipantIdentifiers": past_the_filter_maximum }),
            400,
            "ValidationException",
        ),
    ];
    for (label, request, status, code) in refused {
        let answer = emulator.call("ListConnectionInvitations", &request).await;
        assert_error(label, answer, status, code);
    }
}

/// Accepting a stored invitation creates and stores the connection it produces,
/// links it back onto the invitation, and is the only call that can: a second
/// acceptance conflicts. The rows differ in connection type and in the form of the
/// receiver identifier, which decides which `Participant` variant reports the
/// counterparty.
#[tokio::test]
async fn accept_connection_invitation_creates_and_links_the_connection() {
    let emulator = Emulator::new();

    // The `Participant` variant each receiver-identifier form must be reported as,
    // from the model's own id constraints: `PartnerProfileSummary$Id` is
    // `^pprofile-[A-Za-z0-9]{13}$`, `SellerProfileSummary$Id` is `^[a-zA-Z0-9-]+$`
    // with length 1..50, and a longer identifier fits neither.
    let cases: Vec<(&str, &str, &str, &str)> = vec![
        (
            "a partner profile id",
            "OPPORTUNITY_COLLABORATION",
            "pprofile-ABCDEFGHIJKLM",
            "PartnerProfile",
        ),
        (
            "a plain participant identifier",
            "SUBSIDIARY",
            "partner-123",
            "SellerProfile",
        ),
        (
            "the longest identifier a profile id can carry",
            "OPPORTUNITY_COLLABORATION",
            "0123456789012345678901234567890123456789012345678",
            "SellerProfile",
        ),
        (
            "one character past that",
            "SUBSIDIARY",
            "01234567890123456789012345678901234567890123456789a",
            "Account",
        ),
    ];

    let mut seen_connection_ids: BTreeSet<String> = BTreeSet::new();
    for (index, (label, connection_type, receiver, variant)) in cases.into_iter().enumerate() {
        let created = emulator
            .ok(
                "CreateConnectionInvitation",
                &json!({
                    "Catalog": "AWS",
                    "ClientToken": format!("invite-{index}"),
                    "ConnectionType": connection_type,
                    "Email": "inviter@example.com",
                    "Message": "please connect",
                    "Name": "Inviter",
                    "ReceiverIdentifier": receiver,
                }),
            )
            .await;
        let invitation_id = string(&created, "Id", label);

        let accepted = emulator
            .ok(
                "AcceptConnectionInvitation",
                &json!({
                    "Catalog": "AWS",
                    "Identifier": invitation_id,
                    "ClientToken": format!("accept-{index}"),
                }),
            )
            .await;
        let connection = accepted["Connection"].clone();
        let connection_id = string(&connection, "Id", label);
        assert!(
            is_connection_id(&connection_id),
            "case {label:?}: Id must match ^pac-[A-Za-z0-9]{{13}}$, got {connection_id:?}"
        );
        assert!(
            seen_connection_ids.insert(connection_id.clone()),
            "case {label:?}: connection id {connection_id} was already minted"
        );
        assert_eq!(
            string(&connection, "Arn", label),
            connection_arn(REGION, "AWS", &connection_id),
            "case {label:?}: Arn must name the catalog and the minted connection id"
        );
        assert_eq!(
            string(&connection, "OtherParticipantAccountId", label),
            ACCOUNT,
            "case {label:?}: OtherParticipantAccountId is a 12-digit account id"
        );

        let detail = connection["ConnectionTypes"][connection_type].clone();
        assert!(
            detail.is_object(),
            "case {label:?}: ConnectionTypes must carry the invitation's type: {connection}"
        );
        assert_eq!(
            string(&detail, "Status", label),
            "ACTIVE",
            "case {label:?}: a new connection type is ACTIVE"
        );
        assert_eq!(
            string(&detail, "InviterEmail", label),
            "inviter@example.com",
            "case {label:?}: InviterEmail comes from the invitation"
        );
        assert_eq!(
            string(&detail, "InviterName", label),
            "Inviter",
            "case {label:?}: InviterName comes from the invitation"
        );
        assert!(
            detail.get("CanceledAt").is_none_or(Value::is_null)
                && detail.get("CanceledBy").is_none_or(Value::is_null),
            "case {label:?}: an active connection type records no cancellation: {detail}"
        );
        let participant = detail["OtherParticipant"][variant].clone();
        assert!(
            participant.is_object(),
            "case {label:?}: OtherParticipant must be reported as {variant}: {detail}"
        );
        if variant == "Account" {
            assert_eq!(
                string(&participant, "Name", label),
                ACCOUNT,
                "case {label:?}: an account participant is named by the account the connection carries"
            );
        } else {
            assert_eq!(
                string(&participant, "Id", label),
                receiver,
                "case {label:?}: the participant is identified by the invitation's receiver"
            );
        }

        // The invitation now records the answer and the connection it produced.
        let invitation = emulator
            .ok(
                "GetConnectionInvitation",
                &json!({ "Catalog": "AWS", "Identifier": invitation_id }),
            )
            .await;
        assert_eq!(
            string(&invitation, "Status", label),
            "ACCEPTED",
            "case {label:?}: an accepted invitation reports ACCEPTED"
        );
        assert_eq!(
            invitation.get("ConnectionId").and_then(Value::as_str),
            Some(connection_id.as_str()),
            "case {label:?}: the invitation links the connection it produced: {invitation}"
        );
        assert!(
            timestamp(&invitation, "UpdatedAt", label)
                > timestamp(&created, "UpdatedAt", label),
            "case {label:?}: accepting moves the invitation's UpdatedAt forward"
        );

        // GetConnection reads back exactly the connection acceptance stored.
        let fetched = emulator
            .ok(
                "GetConnection",
                &json!({ "Catalog": "AWS", "Identifier": connection_id }),
            )
            .await;
        for member in [
            "Catalog",
            "Id",
            "Arn",
            "OtherParticipantAccountId",
            "ConnectionTypes",
        ] {
            assert_eq!(
                fetched.get(member),
                connection.get(member),
                "case {label:?}: {member} read back must equal what acceptance stored"
            );
        }

        // Replaying the acceptance token answers with the same connection...
        let replay = emulator
            .ok(
                "AcceptConnectionInvitation",
                &json!({
                    "Catalog": "AWS",
                    "Identifier": invitation_id,
                    "ClientToken": format!("accept-{index}"),
                }),
            )
            .await;
        assert_eq!(
            replay, accepted,
            "case {label:?}: a replayed ClientToken answers with the connection it created"
        );

        // ... while a fresh token on an invitation that is no longer PENDING is a
        // conflict.
        let answer = emulator
            .call(
                "AcceptConnectionInvitation",
                &json!({
                    "Catalog": "AWS",
                    "Identifier": invitation_id,
                    "ClientToken": format!("accept-again-{index}"),
                }),
            )
            .await;
        assert_eq!(
            answer.1.get("Reason").and_then(Value::as_str),
            Some("INCOMPATIBLE_CONNECTION_INVITATION_STATE"),
            "case {label:?}: a non-PENDING invitation names its conflict reason: {}",
            answer.1
        );
        assert_error(label, answer, 409, "ConflictException");
    }

    // An invitation nobody created cannot be accepted, and neither can one stored in
    // another catalog.
    let refused: Vec<(&str, Value)> = vec![
        (
            "never minted",
            json!({
                "Catalog": "AWS",
                "Identifier": "pacinv-0000000000000",
                "ClientToken": "accept-absent",
            }),
        ),
        (
            "another catalog",
            json!({
                "Catalog": "Sandbox",
                "Identifier": "pacinv-0000000000001",
                "ClientToken": "accept-other-catalog",
            }),
        ),
    ];
    for (label, request) in refused {
        let answer = emulator.call("AcceptConnectionInvitation", &request).await;
        assert_error(label, answer, 404, "ResourceNotFoundException");
    }
}

/// `CancelConnection` mutates the stored connection: the named type's detail becomes
/// `CANCELED` with the instant and the account that cancelled it, the connection's
/// `UpdatedAt` moves forward, and a later `GetConnection` observes all of it.
#[tokio::test]
async fn cancel_connection_records_the_cancellation() {
    let emulator = Emulator::new();

    for (index, connection_type) in ["OPPORTUNITY_COLLABORATION", "SUBSIDIARY"]
        .into_iter()
        .enumerate()
    {
        let label = connection_type;
        let created = emulator
            .ok(
                "CreateConnectionInvitation",
                &json!({
                    "Catalog": "AWS",
                    "ClientToken": format!("invite-{index}"),
                    "ConnectionType": connection_type,
                    "Email": "inviter@example.com",
                    "Message": "please connect",
                    "Name": "Inviter",
                    "ReceiverIdentifier": "partner-123",
                }),
            )
            .await;
        let invitation_id = string(&created, "Id", label);
        let accepted = emulator
            .ok(
                "AcceptConnectionInvitation",
                &json!({
                    "Catalog": "AWS",
                    "Identifier": invitation_id,
                    "ClientToken": format!("accept-{index}"),
                }),
            )
            .await;
        let connection_id = string(&accepted["Connection"], "Id", label);
        let before = timestamp(&accepted["Connection"], "UpdatedAt", label);

        // The other connection type is not on this connection, so cancelling it is a
        // request about a value the resource does not carry.
        let other_type = if connection_type == "SUBSIDIARY" {
            "OPPORTUNITY_COLLABORATION"
        } else {
            "SUBSIDIARY"
        };
        let answer = emulator
            .call(
                "CancelConnection",
                &json!({
                    "Catalog": "AWS",
                    "Identifier": connection_id,
                    "ConnectionType": other_type,
                    "Reason": "wrong type",
                    "ClientToken": format!("cancel-wrong-{index}"),
                }),
            )
            .await;
        assert_error(label, answer, 400, "ValidationException");

        // The clock must move on, so `UpdatedAt` is comparable.
        tokio::time::sleep(Duration::from_millis(20)).await;

        let canceled = emulator
            .ok(
                "CancelConnection",
                &json!({
                    "Catalog": "AWS",
                    "Identifier": connection_id,
                    "ConnectionType": connection_type,
                    "Reason": "done",
                    "ClientToken": format!("cancel-{index}"),
                }),
            )
            .await;
        let detail = canceled["ConnectionTypes"][connection_type].clone();
        assert_eq!(
            string(&detail, "Status", label),
            "CANCELED",
            "case {label:?}: the cancelled connection type reports CANCELED"
        );
        assert_eq!(
            string(&detail, "CanceledBy", label),
            ACCOUNT,
            "case {label:?}: CanceledBy is the account that cancelled it"
        );
        let canceled_at = timestamp(&detail, "CanceledAt", label);
        assert!(
            canceled_at >= timestamp(&detail, "CreatedAt", label),
            "case {label:?}: CanceledAt {canceled_at} cannot precede the type's CreatedAt"
        );
        let after = timestamp(&canceled, "UpdatedAt", label);
        assert!(
            after > before,
            "case {label:?}: cancelling moves the connection's UpdatedAt forward: {before} -> {after}"
        );

        // A later read observes the cancelled state, not the state acceptance left.
        let fetched = emulator
            .ok(
                "GetConnection",
                &json!({ "Catalog": "AWS", "Identifier": connection_id }),
            )
            .await;
        assert_eq!(
            fetched["ConnectionTypes"], canceled["ConnectionTypes"],
            "case {label:?}: GetConnection must observe the cancellation"
        );
        assert!(
            (timestamp(&fetched, "UpdatedAt", label) - after).abs() < 1e-3,
            "case {label:?}: GetConnection reports the UpdatedAt the cancellation stored"
        );

        // Replaying the cancellation token reports the connection as that
        // cancellation left it...
        let replay = emulator
            .ok(
                "CancelConnection",
                &json!({
                    "Catalog": "AWS",
                    "Identifier": connection_id,
                    "ConnectionType": connection_type,
                    "Reason": "done",
                    "ClientToken": format!("cancel-{index}"),
                }),
            )
            .await;
        assert_eq!(
            replay, canceled,
            "case {label:?}: a replayed ClientToken answers with the cancellation it performed"
        );

        // ... while cancelling again under a fresh token conflicts with the state the
        // first cancellation left.
        let answer = emulator
            .call(
                "CancelConnection",
                &json!({
                    "Catalog": "AWS",
                    "Identifier": connection_id,
                    "ConnectionType": connection_type,
                    "Reason": "done again",
                    "ClientToken": format!("cancel-again-{index}"),
                }),
            )
            .await;
        assert_eq!(
            answer.1.get("Reason").and_then(Value::as_str),
            Some("INCOMPATIBLE_CONNECTION_STATE"),
            "case {label:?}: an already-cancelled type names its conflict reason: {}",
            answer.1
        );
        assert_error(label, answer, 409, "ConflictException");
    }

    // A connection nobody created, and a connection read in another catalog.
    let refused: Vec<(&str, Value)> = vec![
        (
            "never minted",
            json!({
                "Catalog": "AWS",
                "Identifier": "pac-0000000000000",
                "ConnectionType": "SUBSIDIARY",
                "Reason": "whatever",
                "ClientToken": "cancel-absent",
            }),
        ),
        (
            "another catalog",
            json!({
                "Catalog": "Sandbox",
                "Identifier": "pac-0000000000001",
                "ConnectionType": "SUBSIDIARY",
                "Reason": "whatever",
                "ClientToken": "cancel-other-catalog",
            }),
        ),
    ];
    for (label, request) in refused {
        let answer = emulator.call("CancelConnection", &request).await;
        assert_error(label, answer, 404, "ResourceNotFoundException");
    }

    let answer = emulator
        .call(
            "GetConnection",
            &json!({ "Catalog": "AWS", "Identifier": "pac-0000000000000" }),
        )
        .await;
    assert_eq!(
        answer.1.get("Reason").and_then(Value::as_str),
        Some("CONNECTION_NOT_FOUND"),
        "a missing connection names its reason: {}",
        answer.1
    );
    assert_error("get a connection nobody created", answer, 404, "ResourceNotFoundException");
}

/// An invitation nobody answered before its `ExpiresAt` is `EXPIRED` from that
/// instant on, and can no longer be accepted. The stored expiry is moved into the
/// past because the invitation lifetime is 30 days and a test cannot wait for it.
#[tokio::test]
async fn an_invitation_expires_when_its_expiry_falls_due() {
    let emulator = Emulator::new();
    let created = emulator
        .ok(
            "CreateConnectionInvitation",
            &json!({
                "Catalog": "AWS",
                "ClientToken": "tok-expiring",
                "ConnectionType": "OPPORTUNITY_COLLABORATION",
                "Email": "inviter@example.com",
                "Message": "please connect",
                "Name": "Inviter",
                "ReceiverIdentifier": "partner-123",
            }),
        )
        .await;
    let id = string(&created, "Id", "expiring invitation");

    // A whole second so the instant survives either timestamp wire form exactly.
    let due = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    emulator
        .engine
        .state::<PartnerCentralState>()
        .lock()
        .invitations
        .get_mut(&("AWS".to_string(), id.clone()))
        .expect("the invitation this test created is stored")
        .expires_at = due;

    let fetched = emulator
        .ok(
            "GetConnectionInvitation",
            &json!({ "Catalog": "AWS", "Identifier": id }),
        )
        .await;
    assert_eq!(
        string(&fetched, "Status", "expired"),
        "EXPIRED",
        "an invitation past its ExpiresAt is EXPIRED: {fetched}"
    );
    assert!(
        (timestamp(&fetched, "UpdatedAt", "expired") - 1_700_000_000.0).abs() < 1e-3,
        "the expiry moved UpdatedAt to the instant it fell due: {fetched}"
    );
    assert!(
        (timestamp(&fetched, "ExpiresAt", "expired") - 1_700_000_000.0).abs() < 1e-3,
        "ExpiresAt still reports the instant the invitation expired at: {fetched}"
    );

    // The status filter selects on the status the invitation actually has.
    for (label, status, expected) in [("expired", "EXPIRED", 1), ("pending", "PENDING", 0)] {
        let listed = emulator
            .ok(
                "ListConnectionInvitations",
                &json!({ "Catalog": "AWS", "Status": status }),
            )
            .await;
        assert_eq!(
            listed["ConnectionInvitationSummaries"]
                .as_array()
                .expect("ConnectionInvitationSummaries is a list")
                .len(),
            expected,
            "case {label:?}: the Status filter selects on the invitation's current status: {listed}"
        );
    }

    let answer = emulator
        .call(
            "AcceptConnectionInvitation",
            &json!({
                "Catalog": "AWS",
                "Identifier": id,
                "ClientToken": "accept-expired",
            }),
        )
        .await;
    assert_error("accept an expired invitation", answer, 409, "ConflictException");
}

/// A resource's ARN carries the region the request was signed for, and nothing else
/// about the resource changes with it.
#[tokio::test]
async fn the_arn_region_follows_the_signing_region() {
    let emulator = Emulator::new();

    for (index, region) in [
        "us-east-1",
        "eu-west-1",
        "ap-southeast-2",
        // A region outside the commercial partition: the ARN's partition segment
        // follows the region, it is not fixed at `aws`.
        "cn-north-1",
        "us-gov-west-1",
    ]
    .into_iter()
    .enumerate()
    {
        let (status, created) = emulator
            .call_in(
                region,
                "CreateConnectionInvitation",
                &json!({
                    "Catalog": "AWS",
                    "ClientToken": format!("tok-region-{index}"),
                    "ConnectionType": "OPPORTUNITY_COLLABORATION",
                    "Email": "inviter@example.com",
                    "Message": "please connect",
                    "Name": "Inviter",
                    "ReceiverIdentifier": "partner-123",
                }),
            )
            .await;
        assert_eq!(status, 200, "case {region:?}: expected 200, got {created}");
        let id = string(&created, "Id", region);
        assert_eq!(
            string(&created, "Arn", region),
            invitation_arn(region, "AWS", &id),
            "case {region:?}: the ARN carries the region the request was signed for"
        );

        // The connection the invitation produces carries the same rule.
        let accepted = emulator
            .call_in(
                region,
                "AcceptConnectionInvitation",
                &json!({
                    "Catalog": "AWS",
                    "Identifier": id,
                    "ClientToken": format!("accept-region-{index}"),
                }),
            )
            .await
            .1;
        let connection = accepted["Connection"].clone();
        let connection_id = string(&connection, "Id", region);
        assert_eq!(
            string(&connection, "Arn", region),
            connection_arn(region, "AWS", &connection_id),
            "case {region:?}: a connection's ARN carries the signing region too"
        );
    }
}

/// A catalog this service could mint no ARN for is refused on create: both resource
/// ARNs spell the catalog segment `catalog/[a-zA-Z]+`, while `Catalog` itself is
/// `^[a-zA-Z0-9-]+$`.
#[tokio::test]
async fn a_catalog_no_arn_can_spell_is_refused() {
    let emulator = Emulator::new();

    let cases: Vec<(&str, &str, u16)> = vec![
        ("AWS", "the production catalog", 200),
        ("Sandbox", "the sandbox catalog", 200),
        ("aws", "lowercase letters", 200),
        ("AWS-1", "a dash", 400),
        ("catalog2", "a digit", 400),
    ];

    for (index, (catalog, label, expected)) in cases.into_iter().enumerate() {
        let (status, body) = emulator
            .call(
                "CreateConnectionInvitation",
                &json!({
                    "Catalog": catalog,
                    "ClientToken": format!("tok-catalog-{index}"),
                    "ConnectionType": "OPPORTUNITY_COLLABORATION",
                    "Email": "inviter@example.com",
                    "Message": "please connect",
                    "Name": "Inviter",
                    "ReceiverIdentifier": "partner-123",
                }),
            )
            .await;
        assert_eq!(
            status, expected,
            "case {label:?}: catalog {catalog:?} expected HTTP {expected}, got {status}: {body}"
        );
        if expected == 200 {
            let id = string(&body, "Id", label);
            assert_eq!(
                string(&body, "Arn", label),
                invitation_arn(REGION, catalog, &id),
                "case {label:?}: the ARN carries the catalog the invitation was created in"
            );
        } else {
            assert_error(label, (status, body), 400, "ValidationException");
        }
    }
}

/// Neither store a caller can grow is unbounded: past its ceiling the create or the
/// acceptance is refused with the modelled quota error, and nothing is stored.
#[tokio::test]
async fn the_resource_stores_are_bounded() {
    let emulator = Emulator::new();

    /// One create request, with the token and receiver of row `index`.
    fn invite(index: usize) -> Value {
        json!({
            "Catalog": "AWS",
            "ClientToken": format!("fill-{index}"),
            "ConnectionType": "OPPORTUNITY_COLLABORATION",
            "Email": "inviter@example.com",
            "Message": "please connect",
            "Name": "Inviter",
            "ReceiverIdentifier": format!("partner-{index}"),
        })
    }

    let mut invitation_ids: Vec<String> = Vec::new();
    for index in 0..MAX_INVITATIONS {
        let created = emulator.ok("CreateConnectionInvitation", &invite(index)).await;
        invitation_ids.push(string(&created, "Id", "filling the invitation store"));
    }

    let answer = emulator
        .call("CreateConnectionInvitation", &invite(MAX_INVITATIONS))
        .await;
    assert_eq!(
        answer.1.get("Reason").and_then(Value::as_str),
        Some("LIMIT_EXCEEDED_NUMBER_OF_OPEN_CONNECTION_INVITATION"),
        "a full invitation store names its quota reason: {}",
        answer.1
    );
    assert_error(
        "one invitation past the ceiling",
        answer,
        402,
        "ServiceQuotaExceededException",
    );

    // The refusal stored nothing: the store still holds exactly the invitations that
    // were accepted into it.
    assert_eq!(
        emulator
            .engine
            .state::<PartnerCentralState>()
            .lock()
            .invitations
            .len(),
        MAX_INVITATIONS,
        "a refused create must not grow the store"
    );

    for (index, invitation_id) in invitation_ids.iter().take(MAX_CONNECTIONS).enumerate() {
        emulator
            .ok(
                "AcceptConnectionInvitation",
                &json!({
                    "Catalog": "AWS",
                    "Identifier": invitation_id,
                    "ClientToken": format!("accept-fill-{index}"),
                }),
            )
            .await;
    }

    let next = invitation_ids[MAX_CONNECTIONS].clone();
    let answer = emulator
        .call(
            "AcceptConnectionInvitation",
            &json!({
                "Catalog": "AWS",
                "Identifier": next,
                "ClientToken": "accept-past-the-ceiling",
            }),
        )
        .await;
    assert_eq!(
        answer.1.get("Reason").and_then(Value::as_str),
        Some("LIMIT_EXCEEDED_NUMBER_OF_ACTIVE_CONNECTION"),
        "a full connection store names its quota reason: {}",
        answer.1
    );
    assert_error(
        "one connection past the ceiling",
        answer,
        402,
        "ServiceQuotaExceededException",
    );

    // The refused acceptance left the invitation answerable.
    let invitation = emulator
        .ok(
            "GetConnectionInvitation",
            &json!({ "Catalog": "AWS", "Identifier": next }),
        )
        .await;
    assert_eq!(
        string(&invitation, "Status", "refused acceptance"),
        "PENDING",
        "a refused acceptance must not move the invitation: {invitation}"
    );
}

/// The idempotency-token store is bounded too: it holds at most
/// `MAX_IDEMPOTENCY_TOKENS` records, and a token it has forgotten is served as a new
/// request rather than remembered forever.
#[tokio::test]
async fn the_idempotency_store_evicts_its_oldest_record() {
    let emulator = Emulator::new();

    fn invite(token: &str) -> Value {
        json!({
            "Catalog": "AWS",
            "ClientToken": token,
            "ConnectionType": "OPPORTUNITY_COLLABORATION",
            "Email": "inviter@example.com",
            "Message": "please connect",
            "Name": "Inviter",
            "ReceiverIdentifier": "partner-123",
        })
    }

    let first = emulator
        .ok("CreateConnectionInvitation", &invite("token-oldest"))
        .await;
    let first_id = string(&first, "Id", "oldest token");

    // Fill the token store past its ceiling with distinct tokens.
    for index in 0..MAX_IDEMPOTENCY_TOKENS {
        emulator
            .ok(
                "CreateConnectionInvitation",
                &invite(&format!("token-{index}")),
            )
            .await;
    }

    let held = emulator
        .engine
        .state::<PartnerCentralState>()
        .lock()
        .idempotency
        .len();
    assert!(
        held <= MAX_IDEMPOTENCY_TOKENS,
        "the token store holds {held} records, more than the {MAX_IDEMPOTENCY_TOKENS} ceiling"
    );

    // The oldest token was evicted, so the identical request is served as a new one.
    let again = emulator
        .ok("CreateConnectionInvitation", &invite("token-oldest"))
        .await;
    assert_ne!(
        string(&again, "Id", "evicted token"),
        first_id,
        "a forgotten ClientToken is served as a new request, not replayed"
    );
}
