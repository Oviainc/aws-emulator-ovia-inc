// SPDX-License-Identifier: CC0-1.0

//! Reading OpenSearch Serverless-managed interface VPC endpoints by identifier, end to end
//! over the wire.
//!
//! Every case drives the real request path through [`crate::collection_lifecycle::call`] —
//! `MultiServiceRouter::dispatch` with `POST /`, `X-Amz-Target:
//! OpenSearchServerless.<Operation>` and an `awsJson1_0` body — so the boundary validator,
//! the protocol codec and the handler all take part, exactly as they do for an SDK client.
//!
//! The rule under test: a batch read answers EVERY entry of its identifier list
//! independently and in request order, each one either as the stored record's detail or as
//! the not-found error detail, it reports no identifier the request did not carry, it
//! changes nothing a later caller sees, and a list outside its modelled bounds is refused
//! before anything is read. The table spans what that rule generalises over: the list
//! length at both published bounds, the mix of stored and unknown identifiers, the request
//! order against the order the endpoints were created in, and a repeated identifier.

use serde_json::{json, Value};

use crate::collection_lifecycle::{at, call, emulator, text, CLIENT_ERROR};
use crate::vpc_endpoint::{assert_members, await_active, create_endpoint, read_endpoints, ACTIVE};

/// The bounds of `BatchGetVpcEndpointRequest$ids`, from its target's own
/// `@length(min: 1, max: 100)` (`smithy_query(query="shape", shape="VpcEndpointIds")`).
const MIN_IDS: usize = 1;
const MAX_IDS: usize = 100;

/// `VpcEndpointId`'s own `@length(min: 1, max: 255)`
/// (`smithy_query(query="shape", shape="VpcEndpointIds")` -> `member`).
const MAX_ID_LEN: usize = 255;

/// The `errorCode` this service's batch readers report for an identifier that names
/// nothing.
const NOT_FOUND_CODE: &str = "NOT_FOUND";

/// The `errorMessage` that goes with it, as `BatchGetVpcEndpoint`'s sibling batch readers
/// word theirs for the same cause with this resource named instead — the kind spelled
/// `VpcEndpoint`, one word.
const NOT_FOUND_MESSAGE: &str = "The specified VpcEndpoint is not found.";

/// A well-formed `VpcEndpointId` that names no stored endpoint: `index` in hexadecimal,
/// padded to the 17-character suffix width, inside `^vpce-[0-9a-z]*$` and the shape's
/// length bound.
fn unknown_id(index: usize) -> String {
    format!("vpce-{index:017x}")
}

/// The `id` each entry of `field` carries, in the order the answer lists them.
fn reported_ids(answer: &Value, field: &str) -> Vec<String> {
    at(answer, field)
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| text(entry, "id"))
        .collect()
}

/// The entries of `field`, in the order the answer lists them.
fn entries(answer: &Value, field: &str) -> Vec<Value> {
    at(answer, field).as_array().cloned().unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Every entry of `ids` answered, in request order, over a table that spans the list
// length at both published bounds, the mix of stored and unknown ids, the request order
// and a repeated id.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_requested_id_is_answered_once_in_request_order() {
    let router = emulator();

    // Three endpoints differing in the members a detail reports — the subnet count and
    // width, the presence of security groups, the `VpcEndpointName` length — so a detail
    // answered for the wrong id cannot look right.
    let requests: Vec<Value> = vec![
        json!({
            "name": "batch-read-one",
            "vpcId": "vpc-1a2b3c4d",
            "subnetIds": ["subnet-1a2b3c4d"],
        }),
        json!({
            "name": "btr",
            "vpcId": "vpc-0123456789abcdef0",
            "subnetIds": ["subnet-0123456789abcdef0", "subnet-abcdef01234567890"],
            "securityGroupIds": ["sg-0123456789abcdef0", "sg-2"],
        }),
        json!({
            "name": format!("c{}", "d".repeat(31)),
            "vpcId": "vpc-9f8e7d6c",
            "subnetIds": ["subnet-11111111"],
            "securityGroupIds": ["sg-only"],
        }),
    ];

    // The id each request's endpoint was minted, in creation order, with the request that
    // created it beside it.
    let mut stored: Vec<(String, Value)> = Vec::new();
    for request in &requests {
        let case = text(request, "name");
        let detail = create_endpoint(&router, &case, request.clone()).await;
        let id = text(&detail, "id");
        // Settled first, so every row below reads one terminal status and the answers are
        // comparable across rows.
        await_active(&router, &case, &id).await;
        stored.push((id, request.clone()));
    }
    let created: Vec<String> = stored.iter().map(|(id, _)| id.clone()).collect();

    // Rows 1 and 2 sit on the minimum list length of one, for a stored and for an unknown
    // id; rows 3 and 4 request every stored id in the creation order and in its reverse,
    // so a request order the store does not share is still the answer's order; row 5
    // interleaves stored and unknown ids; row 6 repeats both kinds, which the sibling
    // batch readers answer once per ENTRY rather than once per distinct id; row 7 sits on
    // the maximum list length of 100; row 8 carries no stored id at all.
    let reversed: Vec<String> = created.iter().rev().cloned().collect();
    let mut at_maximum: Vec<String> = created.clone();
    at_maximum.extend((0..MAX_IDS - created.len()).map(unknown_id));
    let cases: Vec<(String, Vec<String>)> = vec![
        (
            format!("one stored id — the minimum list length of {MIN_IDS}"),
            vec![created[0].clone()],
        ),
        (
            format!("one unknown id — the minimum list length of {MIN_IDS}"),
            vec![unknown_id(901)],
        ),
        (
            "every stored id, in creation order".to_string(),
            created.clone(),
        ),
        (
            "every stored id, in the reverse of creation order".to_string(),
            reversed,
        ),
        (
            "stored and unknown ids interleaved".to_string(),
            vec![
                created[2].clone(),
                unknown_id(902),
                created[0].clone(),
                unknown_id(903),
                created[1].clone(),
            ],
        ),
        (
            "a stored id and an unknown id each repeated".to_string(),
            vec![
                created[1].clone(),
                unknown_id(904),
                created[1].clone(),
                created[1].clone(),
                unknown_id(904),
            ],
        ),
        (
            format!("the maximum list length of {MAX_IDS}"),
            at_maximum,
        ),
        (
            "only unknown ids".to_string(),
            (905..910).map(unknown_id).collect(),
        ),
    ];

    for (case, requested) in cases {
        let borrowed: Vec<&str> = requested.iter().map(String::as_str).collect();
        let answer = read_endpoints(&router, &case, &borrowed).await;

        // The rule, applied to this row: an entry naming a stored endpoint is a detail,
        // every other entry is an error detail, and both keep the request's order.
        let expected_details: Vec<String> = requested
            .iter()
            .filter(|id| created.contains(id))
            .cloned()
            .collect();
        let expected_errors: Vec<String> = requested
            .iter()
            .filter(|id| !created.contains(id))
            .cloned()
            .collect();
        let details = reported_ids(&answer, "vpcEndpointDetails");
        let errors = reported_ids(&answer, "vpcEndpointErrorDetails");
        assert_eq!(
            details, expected_details,
            "case {case}: expected one detail per stored id in request order, got {details:?}"
        );
        assert_eq!(
            errors, expected_errors,
            "case {case}: expected one error detail per unknown id in request order, got {errors:?}"
        );

        // Each detail is the record the create stored, read back whole.
        for detail in entries(&answer, "vpcEndpointDetails") {
            let id = text(&detail, "id");
            let (_, request) = stored
                .iter()
                .find(|(stored_id, _)| *stored_id == id)
                .unwrap_or_else(|| panic!("case {case}: {id} is one of the created endpoints"));
            assert_members(&case, &id, request, &detail);
            assert_eq!(
                text(&detail, "status"),
                ACTIVE,
                "case {case}: expected the settled status, got {detail}"
            );
        }

        // Each error detail names the id the request carried and the not-found cause.
        for error in entries(&answer, "vpcEndpointErrorDetails") {
            assert_eq!(
                (
                    text(&error, "errorCode").as_str(),
                    text(&error, "errorMessage").as_str()
                ),
                (NOT_FOUND_CODE, NOT_FOUND_MESSAGE),
                "case {case}: expected the not-found cause, got {error}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// A list outside the modelled bounds, one past each published bound.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_batch_read_outside_the_modelled_ids_bounds_is_refused() {
    let router = emulator();

    let request = json!({
        "name": "bounds-endpoint",
        "vpcId": "vpc-1a2b3c4d",
        "subnetIds": ["subnet-1a2b3c4d"],
    });
    let detail = create_endpoint(&router, "the endpoint under the bounds", request.clone()).await;
    let id = text(&detail, "id");

    let over_maximum: Vec<String> = (0..=MAX_IDS).map(unknown_id).collect();
    let over_width = format!("vpce-{}", "a".repeat(MAX_ID_LEN - "vpce-".len() + 1));
    let cases: Vec<(String, Value)> = vec![
        ("no ids member at all".to_string(), json!({})),
        (
            format!("an empty list, one entry under the minimum of {MIN_IDS}"),
            json!({ "ids": [] }),
        ),
        (
            format!("one entry over the maximum of {MAX_IDS}"),
            json!({ "ids": over_maximum }),
        ),
        (
            "an id carrying another resource's prefix".to_string(),
            json!({ "ids": ["vpc-1a2b3c4d"] }),
        ),
        (
            "an id whose suffix is outside the modelled pattern".to_string(),
            json!({ "ids": ["vpce-ABCDEF"] }),
        ),
        (
            format!("an id one character over the maximum width of {MAX_ID_LEN}"),
            json!({ "ids": [over_width] }),
        ),
    ];

    for (case, body) in cases {
        let (status, code, answer) = call(&router, "BatchGetVpcEndpoint", body).await;
        assert_eq!(
            (status, code.as_str()),
            (CLIENT_ERROR, "ValidationException"),
            "case {case}: expected {CLIENT_ERROR} ValidationException, got {status} {code} with {answer}"
        );
    }

    // Every refusal above left the store as it was: the stored endpoint still reads back.
    let answer = read_endpoints(&router, "after every refusal", &[id.as_str()]).await;
    assert_eq!(
        reported_ids(&answer, "vpcEndpointDetails"),
        vec![id.clone()],
        "case after every refusal: expected the stored endpoint to still read back, got {answer}"
    );
}

// ---------------------------------------------------------------------------
// The read reports, it does not change what a later caller sees.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn repeated_batch_reads_answer_the_same_thing() {
    let router = emulator();

    let request = json!({
        "name": "repeat-read",
        "vpcId": "vpc-1a2b3c4d",
        "subnetIds": ["subnet-1a2b3c4d", "subnet-22222222"],
        "securityGroupIds": ["sg-repeat"],
    });
    let detail = create_endpoint(&router, "the endpoint read repeatedly", request.clone()).await;
    let id = text(&detail, "id");
    await_active(&router, "the endpoint read repeatedly", &id).await;

    let unknown = unknown_id(911);
    let requested = [id.as_str(), unknown.as_str()];
    let first = read_endpoints(&router, "the first read", &requested).await;
    for case in ["the second read", "the third read"] {
        let again = read_endpoints(&router, case, &requested).await;
        assert_eq!(
            again, first,
            "case {case}: expected the same answer as the first read, got {again}"
        );
    }

    // Reading an id that names nothing never makes it name something.
    let answer = read_endpoints(&router, "the unknown id alone", &[unknown.as_str()]).await;
    assert!(
        reported_ids(&answer, "vpcEndpointDetails").is_empty(),
        "case the unknown id alone: expected the earlier reads to have stored nothing, got {answer}"
    );
    assert_eq!(
        reported_ids(&answer, "vpcEndpointErrorDetails"),
        vec![unknown.clone()],
        "case the unknown id alone: expected the not-found error detail, got {answer}"
    );
}
