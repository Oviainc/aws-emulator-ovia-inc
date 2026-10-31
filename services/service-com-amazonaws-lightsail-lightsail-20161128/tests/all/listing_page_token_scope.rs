// SPDX-License-Identifier: CC0-1.0

//! What a name-keyed Lightsail listing answers a `pageToken` with — driven over
//! the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: a page token NAMES the record a page opens at, so a
//! listing answers a token only when the record it names is one this request
//! reports — the page is then that record and the records after it, bounded by
//! the page size — and refuses every other token with the modelled
//! `InvalidInputException`. A token naming nothing is therefore never answered
//! with a page the caller cannot tell from a complete one, and a token one
//! listing minted is no cursor into another's records. Only the request that asks
//! for the FIRST page — carrying no token, or an empty one — is answered without
//! naming a record.
//!
//! The rows span what the rule generalises over: the KIND of collection (records
//! the caller names, and a recordset named by a dotted domain name), and where
//! the token falls against the names a listing holds — below every one of them,
//! strictly between two, past every one, a string no name of that shape could be,
//! and a name another collection of the same account holds. The names a listing
//! does hold are the rows of the positive half, asserted the same way.

use serde_json::{json, Value};

use crate::instance_lifecycle::{error_code, Emulator};

/// The Availability Zone every resource this suite creates is created in.
const ZONE: &str = "us-east-1a";

/// The blueprint and bundle the instances of this suite run: not what any case
/// here is about.
const BLUEPRINT: &str = "amazon_linux_2023";
const INSTANCE_BUNDLE: &str = "nano_3_0";

/// The kind of record a row's collection holds — what it takes to create one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Instance,
    Disk,
    StaticIp,
    Domain,
}

/// One collection: the operation that lists it, the output member that carries a
/// page, and the names this suite stocks it with.
///
/// The names of the four collections are disjoint, so a name of one is a token
/// that names nothing of the others.
#[derive(Debug, Clone, Copy)]
struct Row {
    kind: Kind,
    operation: &'static str,
    member: &'static str,
    /// In the store's own total order, which is the order a listing reports them
    /// in — and NOT the order this suite creates them in.
    names: &'static [&'static str],
}

/// The collections this suite drives. Fewer records than a page holds, so a row
/// is about the token alone.
const ROWS: &[Row] = &[
    Row {
        kind: Kind::Instance,
        operation: "GetInstances",
        member: "instances",
        names: &["pt-instance-a", "pt-instance-m", "pt-instance-z"],
    },
    Row {
        kind: Kind::Disk,
        operation: "GetDisks",
        member: "disks",
        names: &["pt-disk-a", "pt-disk-m", "pt-disk-z"],
    },
    Row {
        kind: Kind::StaticIp,
        operation: "GetStaticIps",
        member: "staticIps",
        names: &["pt-ip-a", "pt-ip-m", "pt-ip-z"],
    },
    // A DNS zone is the one kind of record named by a dotted name, so the token
    // positions are the same ones over a different name shape.
    Row {
        kind: Kind::Domain,
        operation: "GetDomains",
        member: "domains",
        names: &[
            "pt-zone-a.example",
            "pt-zone-m.example",
            "pt-zone-z.example",
        ],
    },
];

/// Create the record `name` of `kind`.
async fn create(emulator: &Emulator, kind: Kind, name: &str) {
    match kind {
        Kind::Instance => {
            emulator
                .ok(
                    "CreateInstances",
                    json!({
                        "instanceNames": [name],
                        "availabilityZone": ZONE,
                        "blueprintId": BLUEPRINT,
                        "bundleId": INSTANCE_BUNDLE,
                    }),
                )
                .await;
        }
        Kind::Disk => {
            emulator
                .ok(
                    "CreateDisk",
                    json!({"diskName": name, "availabilityZone": ZONE, "sizeInGb": 8}),
                )
                .await;
        }
        Kind::StaticIp => {
            emulator
                .ok("AllocateStaticIp", json!({"staticIpName": name}))
                .await;
        }
        Kind::Domain => {
            emulator
                .ok("CreateDomain", json!({"domainName": name}))
                .await;
        }
    }
}

/// Stock every collection, each IN REVERSE NAME ORDER, so a listing that echoed
/// the order its records were created in would fail every row.
async fn stock(emulator: &Emulator) {
    for row in ROWS {
        for name in row.names.iter().rev() {
            create(emulator, row.kind, name).await;
        }
    }
}

/// Call `row`'s listing with `token` when one is given: the status and the parsed
/// body.
async fn call(emulator: &Emulator, row: &Row, token: Option<&str>) -> (u16, Value) {
    let request = match token {
        Some(token) => json!({"pageToken": token}),
        None => json!({}),
    };
    emulator.call(row.operation, request).await
}

/// The names `row`'s listing reports for `token`, and the token it names for the
/// page after it — requiring that it answered a page at all.
async fn page(
    emulator: &Emulator,
    row: &Row,
    case: &str,
    token: Option<&str>,
) -> (Vec<String>, Option<String>) {
    let (status, body) = call(emulator, row, token).await;
    assert_eq!(
        status, 200,
        "case {case}: expected the listing to answer a page, got {status} {body:?}"
    );
    let names = body[row.member]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: expected a {} array, got {body:?}",
                row.member
            )
        })
        .iter()
        .map(|record| {
            record["name"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case}: expected every record to carry its name, got {record:?}")
                })
                .to_string()
        })
        .collect();
    (names, body["nextPageToken"].as_str().map(str::to_string))
}

/// The tokens `row`'s listing must REFUSE, each with the position it holds
/// against the names that listing reports.
fn refused_tokens(row: &Row) -> Vec<(String, String)> {
    let first = row.names[0];
    let inside = row.names[row.names.len() / 2];
    let last = row.names[row.names.len() - 1];
    let mut tokens = vec![
        // `!` is the lowest printable character, so it sorts below every name.
        ("a token below every name".to_string(), "!".to_string()),
        (
            "a token just below the first name".to_string(),
            first[..first.len() - 1].to_string(),
        ),
        (
            "a token strictly between two names".to_string(),
            format!("{inside}x"),
        ),
        (
            "a token past every name".to_string(),
            format!("{last}~"),
        ),
        (
            "a token no name of this account's could be".to_string(),
            "zzz".to_string(),
        ),
    ];
    // A name another collection of the SAME account holds: a token is scoped to
    // the collection that minted it, so one of these names no record HERE.
    for other in ROWS {
        if other.operation == row.operation {
            continue;
        }
        tokens.push((
            format!("a name {} reports", other.operation),
            other.names[0].to_string(),
        ));
    }
    tokens
}

/// RULE: a listing answers a `pageToken` only when it names a record this request
/// reports, with the page that opens at it; every other token is refused with the
/// modelled 400 `InvalidInputException` rather than resumed at the next name after
/// it.
#[tokio::test(flavor = "current_thread")]
async fn a_page_token_is_only_answered_by_the_record_it_names() {
    let emulator = Emulator::new();
    stock(&emulator).await;

    for row in ROWS {
        let held: Vec<String> = row.names.iter().map(|name| (*name).to_string()).collect();

        // The request that asks for the first page names no record: no token at
        // all, and the empty token, are the same request.
        for token in [None, Some("")] {
            let case = format!("{} with the token {token:?}", row.operation);
            let (names, next) = page(&emulator, row, &case, token).await;
            assert_eq!(
                names, held,
                "case {case}: expected every record this suite created, got {names:?}"
            );
            assert_eq!(
                next, None,
                "case {case}: expected no next page token, got {next:?}"
            );
        }

        // A token that NAMES a record opens the page at it — every held name is
        // a row of this half.
        for (index, token) in held.iter().enumerate() {
            let case = format!("{} resuming from {token}", row.operation);
            let (names, next) = page(&emulator, row, &case, Some(token)).await;
            assert_eq!(
                names,
                held[index..].to_vec(),
                "case {case}: expected the records from that name onward, got {names:?}"
            );
            assert_eq!(
                next, None,
                "case {case}: expected no next page token, got {next:?}"
            );
        }

        // Every other token names no record of this listing, so there is no page
        // to answer.
        for (token_case, token) in refused_tokens(row) {
            let case = format!("{} with {token_case}", row.operation);
            let (status, body) = call(&emulator, row, Some(&token)).await;
            assert_eq!(
                status, 400,
                "case {case}: expected the invalid-input status, got {status} {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidInputException",
                "case {case}: expected the modelled InvalidInputException, got {body:?}"
            );
            let message = body["message"].as_str().unwrap_or_default();
            assert!(
                message.contains(&token),
                "case {case}: expected the refusal to name the token sent, got {body:?}"
            );
            assert!(
                message.contains("pageToken"),
                "case {case}: expected the refusal to name the member it is about, got {body:?}"
            );
        }

        // The refusals changed nothing: the listing still reports every record,
        // so no earlier record was dropped from a page on the way.
        let case = format!("{} after the refusals", row.operation);
        let (names, next) = page(&emulator, row, &case, None).await;
        assert_eq!(
            names, held,
            "case {case}: expected the listing to be unchanged, got {names:?}"
        );
        assert_eq!(
            next, None,
            "case {case}: expected no next page token, got {next:?}"
        );
    }
}

/// RULE (the same one, where the collection is EMPTY): the request for the first
/// page is answered with an empty list, because the caller asked what it holds;
/// a token names a record it does not hold, so that request is refused.
#[tokio::test(flavor = "current_thread")]
async fn an_empty_collection_answers_the_first_page_and_refuses_a_token() {
    let emulator = Emulator::new();

    for row in ROWS {
        for token in [None, Some("")] {
            let case = format!("the empty {} with the token {token:?}", row.operation);
            let (names, next) = page(&emulator, row, &case, token).await;
            assert!(
                names.is_empty(),
                "case {case}: expected an empty list, got {names:?}"
            );
            assert_eq!(
                next, None,
                "case {case}: expected no next page token, got {next:?}"
            );
        }

        for token in ["!", "pt-anything", "zzz"] {
            let case = format!("the empty {} with the token {token:?}", row.operation);
            let (status, body) = call(&emulator, row, Some(token)).await;
            assert_eq!(
                status, 400,
                "case {case}: expected the invalid-input status, got {status} {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidInputException",
                "case {case}: expected the modelled InvalidInputException, got {body:?}"
            );
        }
    }
}
