// SPDX-License-Identifier: CC0-1.0

//! One service, ONE refusal for a reference that names no stored resource.
//!
//! Wherever this service refuses a request because a reference it carries resolves to no
//! stored resource and the request is not addressed AT that resource — a reporting
//! operation reached through it, a create or another store hanging a new record off it —
//! the refusal is the same shape whichever operation answered
//! it: HTTP 400 with `x-amzn-errortype: ValidationException`, `Reason` =
//! `FieldValidationFailed`, and one `Fields` entry whose `Name` is the offending request
//! member in this service's own lowerCamelCase field naming and whose `Message` is the
//! top-level `Message` — the constant "<Kind> not found." for the kind the member
//! references. A client keying on `Fields[].Name` therefore reads one name for one member,
//! not one per operation.
//!
//! The rows span the ways a request reaches that refusal: the operation (a reporting
//! operation addressed through the reference, and the creates that hang a new resource off
//! it), the binding the reference travels in (a URI path label, a query string beside it,
//! a JSON body member), and the form of the id that resolves to nothing (one never minted,
//! one minted and then deleted, and a string that is not an id of this service at all).
//! `unknown_parent_reference` pins the create half over every reference member; this suite
//! pins that the reporting half reports the identical member and prose.

use serde_json::{json, Value};

use crate::global_network_lifecycle::{error_code, Emulator};
use crate::network_resource_counts::SUPPORTED_TYPES;

/// The field name this service reports for an unresolvable `GlobalNetworkId`, in the
/// lowerCamelCase spelling of the request member (the spelling the recorded `CreateSite`
/// refusal under an unknown global network answers, pinned by
/// `crate::unknown_parent_reference`).
const FIELD_GLOBAL_NETWORK_ID: &str = "globalNetworkId";

/// The prose a global-network reference that resolves to nothing carries, as recorded.
const GLOBAL_NETWORK_NOT_FOUND: &str = "Global network not found.";

/// One row: a request whose global-network reference names nothing, and how it travels.
struct Row {
    case: String,
    method: &'static str,
    uri: String,
    body: Option<Value>,
}

/// The `(Message, Fields)` pair of a refusal, which is what the rule fixes.
fn refusal_of(body: &Value) -> (Option<&str>, &Value) {
    (body["Message"].as_str(), &body["Fields"])
}

/// Drive `row` and assert the one refusal the rule allows, returning the answer so two
/// operations' refusals can be compared with each other.
async fn assert_unresolvable_reference(emulator: &Emulator, row: &Row) -> Value {
    let Row {
        case,
        method,
        uri,
        body,
    } = row;
    let (status, answer) = emulator.call(method, uri, body.clone()).await;
    assert_eq!(
        status, 400,
        "case {case}: expected 400, got {status} with {answer}"
    );
    assert_eq!(
        error_code(&answer),
        "ValidationException",
        "case {case}: expected ValidationException, got {answer}"
    );
    assert_eq!(
        answer["Reason"].as_str(),
        Some("FieldValidationFailed"),
        "case {case}: expected Reason FieldValidationFailed, got {answer}"
    );
    assert_eq!(
        answer["Message"].as_str(),
        Some(GLOBAL_NETWORK_NOT_FOUND),
        "case {case}: expected Message {GLOBAL_NETWORK_NOT_FOUND:?}, got {answer}"
    );
    assert_eq!(
        answer["Fields"],
        json!([{ "Name": FIELD_GLOBAL_NETWORK_ID, "Message": GLOBAL_NETWORK_NOT_FOUND }]),
        "case {case}: expected one Fields entry naming {FIELD_GLOBAL_NETWORK_ID:?}, got {answer}"
    );
    answer
}

#[tokio::test]
async fn an_unresolvable_global_network_is_refused_under_one_field_name_whatever_the_operation() {
    let emulator = Emulator::new();

    // A global network minted and then deleted: a well-formed id that named a record and
    // names none now.
    let deleted = emulator.create_global_network("to be deleted").await;
    let (status, answer) = emulator
        .delete(&format!("/global-networks/{deleted}"))
        .await;
    assert_eq!(
        status, 200,
        "DeleteGlobalNetwork: expected 200, got {status} with {answer}"
    );

    // A live global network, so every refusal below is the reference check and nothing
    // wider: the same requests against this id are answered.
    let live = emulator.create_global_network("kept").await;

    let unresolvable: [(&str, String); 3] = [
        ("never minted", "global-network-00000000000000099".to_string()),
        ("minted and deleted", deleted.clone()),
        ("not an id of this service", "nope".to_string()),
    ];

    let mut rows: Vec<Row> = Vec::new();
    for (form, id) in &unresolvable {
        // The reporting operation, addressed through the reference in the URI path. The
        // query string beside it varies over no `ResourceType` at all, one documented type
        // (from the catalogue in `crate::network_resource_counts::SUPPORTED_TYPES`, whose
        // source is the member's own documentation) and a page size, because the
        // unresolvable reference is refused whatever else the request asks for.
        for (query_case, query) in [
            ("no query string", String::new()),
            (
                "a documented resourceType",
                format!("resourceType={}", SUPPORTED_TYPES[0]),
            ),
            (
                "a documented resourceType the issue did not cite",
                format!("resourceType={}", SUPPORTED_TYPES[SUPPORTED_TYPES.len() - 1]),
            ),
            ("a page size", "maxResults=1".to_string()),
        ] {
            let path = format!("/global-networks/{id}/network-resource-count");
            rows.push(Row {
                case: format!("GetNetworkResourceCounts on a global network {form}, {query_case}"),
                method: "GET",
                uri: if query.is_empty() {
                    path
                } else {
                    format!("{path}?{query}")
                },
                body: None,
            });
        }
        // The creates that hang a new resource off the same reference: one carrying it as
        // a path label, one carrying it in the body.
        rows.push(Row {
            case: format!("CreateSite under a global network {form}"),
            method: "POST",
            uri: format!("/global-networks/{id}/sites"),
            body: Some(json!({ "Description": "orphan" })),
        });
        rows.push(Row {
            case: format!("CreateDevice under a global network {form}"),
            method: "POST",
            uri: format!("/global-networks/{id}/devices"),
            body: Some(json!({ "Description": "orphan" })),
        });
        rows.push(Row {
            case: format!("CreateCoreNetwork under a global network {form}"),
            method: "POST",
            uri: "/core-networks".to_string(),
            body: Some(json!({ "GlobalNetworkId": id })),
        });
    }

    let mut answers: Vec<(String, Value)> = Vec::new();
    for row in &rows {
        let answer = assert_unresolvable_reference(&emulator, row).await;
        answers.push((row.case.clone(), answer));
    }

    // And every one of them is the SAME refusal: the reporting operation and the creates
    // agree on the member they name and the prose they carry.
    let (first_case, first) = &answers[0];
    for (case, answer) in &answers[1..] {
        assert_eq!(
            refusal_of(answer),
            refusal_of(first),
            "case {case}: its Message and Fields must match {first_case}, got {answer}"
        );
    }

    // The live global network answers the same requests, so the refusals above turn on the
    // reference resolving to nothing and nothing else.
    let (status, counted) = emulator
        .get(&format!(
            "/global-networks/{live}/network-resource-count?resourceType={}",
            SUPPORTED_TYPES[0]
        ))
        .await;
    assert_eq!(
        status, 200,
        "GetNetworkResourceCounts on a live global network: expected 200, got {status} with {counted}"
    );
    let (status, created) = emulator
        .post(
            &format!("/global-networks/{live}/sites"),
            json!({ "Description": "kept" }),
        )
        .await;
    assert_eq!(
        status, 200,
        "CreateSite under a live global network: expected 200, got {status} with {created}"
    );
}
