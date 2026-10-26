// SPDX-License-Identifier: CC0-1.0

//! Every response this service sends names the request it answered.
//!
//! The rule: a Step Functions response carries the id of the request it answers in the
//! `x-amzn-RequestId` response header — on a SUCCESS exactly as on a failure — and that
//! id is per RESPONSE, so two calls are told apart by it. It is a property of the
//! response rather than of any modelled shape: no output and no error shape of this
//! model declares it, which is why it is asserted on the header here and not on a body
//! member. An SDK surfaces it as `ResponseMetadata.RequestId`, the join key correlation,
//! retry bookkeeping and audit workflows read.
//!
//! The table below spans the layers that can answer a request, since the rule holds over
//! all of them and not over one operation: a reporting operation's 200, a mutating
//! operation's 200, a 200 whose request carries no members at all, a handler-returned
//! modelled error, a constraint refusal raised before any handler ran, and a routing
//! failure no operation produced.

use std::collections::HashSet;

use serde_json::{json, Value};

use crate::state_machine_lifecycle::{
    create_machine, Emulator, ACCOUNT, HELLO_DEFINITION, REGION, ROLE_ARN,
};

/// The header AWS reports a response's request id in, lowercased as HTTP compares it.
const REQUEST_ID_HEADER: &str = "x-amzn-requestid";

/// The id a response states, which must be there.
fn request_id(response: &http::Response<bytes::Bytes>, case: &str) -> String {
    response
        .headers()
        .get(REQUEST_ID_HEADER)
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected a {REQUEST_ID_HEADER} header, got headers {:?}",
                response.headers()
            )
        })
        .to_str()
        .unwrap_or_else(|error| panic!("case {case:?}: expected an ASCII id, got {error}"))
        .to_string()
}

/// Whether `id` has the 8-4-4-4-12 lowercase-hex form an AWS request id carries.
fn is_request_id(id: &str) -> bool {
    let mut groups = id.split('-');
    for width in [8usize, 4, 4, 4, 12] {
        let Some(group) = groups.next() else {
            return false;
        };
        let well_formed = group.len() == width
            && group
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase());
        if !well_formed {
            return false;
        }
    }
    groups.next().is_none()
}

/// Every response names the request it answers, whatever the layer that answered it and
/// whatever the status — and no two responses name the same request.
#[tokio::test]
async fn every_response_names_the_request_it_answers() {
    let emulator = Emulator::new();
    let machine = create_machine(&emulator, "named-responses", HELLO_DEFINITION).await;
    let missing = format!("arn:aws:states:{REGION}:{ACCOUNT}:stateMachine:never-created");

    // The rows differ along the LAYER that answers and the status it answers with, which
    // is what the rule generalises over: a report, a mutation, a request with no members,
    // a modelled error from a handler, a refusal raised before any handler ran, and a
    // failure no operation produced at all.
    //
    // The status a row states is the one this rule is indifferent to: the id is owed
    // whatever it is. A row that answers a 2xx or a MODELLED error states it, since a
    // success body and a modelled code are what the operations below own; the row for a
    // request naming no modelled operation states `None`, because the status a routing
    // failure carries is the routing layer's own concern (and its own issue), not this
    // one's — pinning it here would make this suite fail when that is corrected.
    let cases: Vec<(&str, &str, Value, Option<u16>)> = vec![
        (
            "a reporting operation's success",
            "DescribeStateMachine",
            json!({ "stateMachineArn": machine }),
            Some(200),
        ),
        (
            "a mutating operation's success",
            "CreateStateMachine",
            json!({
                "name": "second-machine",
                "definition": HELLO_DEFINITION,
                "roleArn": ROLE_ARN,
            }),
            Some(200),
        ),
        (
            "a success whose request carries no members",
            "ListStateMachines",
            json!({}),
            Some(200),
        ),
        (
            "a success reporting an empty collection",
            "ListTagsForResource",
            json!({ "resourceArn": machine }),
            Some(200),
        ),
        (
            "a handler-returned modelled error",
            "DescribeStateMachine",
            json!({ "stateMachineArn": missing }),
            Some(400),
        ),
        (
            "a constraint refusal raised before any handler ran",
            "CreateStateMachine",
            json!({ "name": "no-definition", "roleArn": ROLE_ARN }),
            Some(400),
        ),
        (
            "a request naming no modelled operation",
            "NoSuchOperationInThisModel",
            json!({}),
            None,
        ),
    ];

    let mut seen: HashSet<String> = HashSet::new();
    for (case, operation, body, expected_status) in cases {
        let response = emulator.dispatch(operation, &body).await;
        let status = response.status().as_u16();
        if let Some(expected) = expected_status {
            assert_eq!(
                status, expected,
                "case {case:?}: expected HTTP {expected}, got {status} with body {:?}",
                String::from_utf8_lossy(response.body())
            );
        }
        let id = request_id(&response, case);
        assert!(
            is_request_id(&id),
            "case {case:?}: expected an 8-4-4-4-12 lowercase-hex request id, got {id:?}"
        );
        assert!(
            seen.insert(id.clone()),
            "case {case:?}: expected a request id of this response alone, got the already \
             reported {id:?}"
        );
    }
}

/// The SAME call, repeated, names each of its responses separately: the id belongs to
/// the response, not to the resource it reports or to the operation that answered.
#[tokio::test]
async fn repeating_one_call_names_each_response_separately() {
    let emulator = Emulator::new();
    let machine = create_machine(&emulator, "repeated-describe", HELLO_DEFINITION).await;
    let body = json!({ "stateMachineArn": machine });

    let mut seen: HashSet<String> = HashSet::new();
    for attempt in 1..=4 {
        let case = format!("DescribeStateMachine attempt {attempt}");
        let response = emulator.dispatch("DescribeStateMachine", &body).await;
        let id = request_id(&response, &case);
        assert!(
            is_request_id(&id),
            "case {case:?}: expected an 8-4-4-4-12 lowercase-hex request id, got {id:?}"
        );
        assert!(
            seen.insert(id.clone()),
            "case {case:?}: expected an id of this response alone, got the already reported \
             {id:?}"
        );
    }
}

/// One response states ONE id: the header is not repeated, so a client reading it sees a
/// single value whichever layer wrote it.
#[tokio::test]
async fn one_response_states_one_id() {
    let emulator = Emulator::new();
    let machine = create_machine(&emulator, "single-valued", HELLO_DEFINITION).await;

    for (case, operation, body) in [
        (
            "a success",
            "DescribeStateMachine",
            json!({ "stateMachineArn": machine }),
        ),
        (
            "a refusal",
            "CreateStateMachine",
            json!({ "roleArn": ROLE_ARN }),
        ),
    ] {
        let response = emulator.dispatch(operation, &body).await;
        let stated: Vec<&str> = response
            .headers()
            .get_all(REQUEST_ID_HEADER)
            .iter()
            .map(|value| value.to_str().expect("an ASCII id"))
            .collect();
        assert_eq!(
            stated.len(),
            1,
            "case {case:?}: expected exactly one {REQUEST_ID_HEADER}, got {stated:?}"
        );
    }
}
