// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for ONE rule: a well-formed ARN of this service that names no record
//! the store holds is refused with the SAME answer whichever operation resolved it — the
//! same status, the same `ErrorCode` member and the same sentence — because the refusal is
//! chosen for the condition and not per operation.
//!
//! Every case drives a real `POST /` awsJson1_0 request through the `MultiServiceRouter`,
//! reusing the harness of `gateway_lifecycle`, so what is asserted is the wire behaviour.
//!
//! The table below spans the class the rule generalises over: every operation of this
//! service that resolves a `gateway/<id>` ARN — the ones whose model carries
//! `ResourceNotFoundException` and the tagging ones that take any resource of this service
//! in a single member — crossed with ARNs that differ in how they came to name nothing
//! (never created, created and deleted), in partition, in id, in resource-type segment and
//! in length, including each published bound of the modelled `GatewayArn`
//! (`@length(min: 50, max: 180)`, `smithy_query(query="shape",shape="GatewayArn")`). The
//! operation whose model declares no `ResourceNotFoundException` at all
//! (`AssociateGatewayToServer`, `smithy_query(query="operation",
//! operation="AssociateGatewayToServer")`) carries the condition in the modelled shape it
//! does declare, and is asserted to state the same sentence inside it.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::gateway_lifecycle::{
    build_engine, build_router, call, create, gateway_count, never_created_arn, Reply, REGION,
};
use crate::gateway_server_association::{arn_of, arn_of_length, import, message_of};

/// A `Host` the modelled `@length(min: 3, max: 128)` accepts, for the one operation that
/// resolves a gateway ARN and also takes a host.
const HOST: &str = "198.51.100.90";

/// What one refusal said, reduced to the three parts a client branches on.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Refusal {
    status: u16,
    /// The awsJson error discriminator, namespace stripped.
    shape: Option<String>,
    /// The `ErrorCode` member of the modelled error shape, absent for a refusal that
    /// states none.
    error_code: Option<String>,
    message: String,
}

fn refusal_of(reply: &Reply) -> Refusal {
    Refusal {
        status: reply.status,
        shape: reply.code.clone(),
        error_code: reply.str_at("/ErrorCode").map(str::to_string),
        message: message_of(reply),
    }
}

/// Every operation of this service that resolves a `gateway/<id>` ARN, with a request body
/// that satisfies every OTHER modelled constraint — so the only thing left for the handler
/// to refuse is that the ARN names no record.
///
/// `TagResource` and `UntagResource` spell the member `ResourceARN` and
/// `ListTagsForResource` spells it `ResourceArn`, as this service's model does.
fn requests(arn: &str) -> Vec<(&'static str, Value)> {
    vec![
        ("GetGateway", json!({ "GatewayArn": arn })),
        ("DeleteGateway", json!({ "GatewayArn": arn })),
        (
            "UpdateGatewayInformation",
            json!({ "GatewayArn": arn, "GatewayDisplayName": "renamed" }),
        ),
        ("UpdateGatewaySoftwareNow", json!({ "GatewayArn": arn })),
        (
            "PutMaintenanceStartTime",
            json!({ "GatewayArn": arn, "HourOfDay": 3, "MinuteOfHour": 30 }),
        ),
        (
            "PutBandwidthRateLimitSchedule",
            json!({ "GatewayArn": arn, "BandwidthRateLimitIntervals": [] }),
        ),
        (
            "GetBandwidthRateLimitSchedule",
            json!({ "GatewayArn": arn }),
        ),
        (
            "TestHypervisorConfiguration",
            json!({ "GatewayArn": arn, "Host": HOST }),
        ),
        (
            "DisassociateGatewayFromServer",
            json!({ "GatewayArn": arn }),
        ),
        (
            "TagResource",
            json!({ "ResourceARN": arn, "Tags": [{ "Key": "env", "Value": "test" }] }),
        ),
        ("ListTagsForResource", json!({ "ResourceArn": arn })),
        (
            "UntagResource",
            json!({ "ResourceARN": arn, "TagKeys": ["env"] }),
        ),
    ]
}

/// The ARNs that name no record, each well formed enough to reach a handler rather than the
/// boundary validator.
async fn arns_naming_nothing(router: &MultiServiceRouter) -> Vec<(String, String)> {
    // A gateway that really existed and was deleted: its ARN was minted by this service and
    // served, so the refusal cannot be about the ARN's shape.
    let deleted = create(router, "gw-refusal-deleted").await;
    let removed = call(
        router,
        "DeleteGateway",
        &json!({ "GatewayArn": deleted.clone() }),
    )
    .await;
    assert_eq!(
        removed.status, 200,
        "DeleteGateway must answer 200 before the ARN is reused as a missing one, got {} {:?}",
        removed.status, removed.body
    );

    vec![
        (
            "a gateway that was never created".to_string(),
            never_created_arn(),
        ),
        ("a gateway that was deleted".to_string(), deleted),
        (
            "another gateway id".to_string(),
            arn_of("gateway", "bgw-2222222222nonexistent"),
        ),
        (
            "an ARN naming a virtual machine".to_string(),
            arn_of("vm", "vm-1111111111111111"),
        ),
        (
            "a resource type this service does not name".to_string(),
            arn_of("appliance", "ap-1111111111111111"),
        ),
        (
            "the china partition".to_string(),
            format!(
                "arn:aws-cn:backup-gateway:{REGION}:{}:gateway/bgw-0000000000000888",
                engine::EMULATOR_ACCOUNT_ID
            ),
        ),
        (
            "the govcloud partition".to_string(),
            format!(
                "arn:aws-us-gov:backup-gateway:{REGION}:{}:gateway/bgw-0000000000000777",
                engine::EMULATOR_ACCOUNT_ID
            ),
        ),
        (
            "at the modelled 50-character minimum length".to_string(),
            arn_of_length(50),
        ),
        (
            "at the modelled 180-character maximum length".to_string(),
            arn_of_length(180),
        ),
    ]
}

/// The rule: for one ARN naming no record, every operation that resolves it answers the
/// very same refusal — status, `ErrorCode` member and sentence alike.
#[tokio::test]
async fn one_condition_is_answered_with_one_refusal_whichever_operation_asked() {
    let engine = build_engine();
    let router = build_router(&engine);
    // A gateway that really exists, so a refusal below is about the ARN that was asked for
    // and not about an empty store.
    let live = create(&router, "gw-live").await;
    let held = gateway_count(&engine);

    for (case, arn) in arns_naming_nothing(&router).await {
        let mut answers: Vec<(&str, Refusal)> = Vec::new();
        for (operation, body) in requests(&arn) {
            let reply = call(&router, operation, &body).await;
            answers.push((operation, refusal_of(&reply)));
        }

        // The sentence this service states for an ARN it holds no record under names the
        // ARN that was asked for and nothing else — one wording, so no operation may word
        // the condition in terms of its own resource.
        let expected = Refusal {
            status: 404,
            shape: Some("ResourceNotFoundException".to_string()),
            error_code: None,
            message: format!("The resource '{arn}' was not found."),
        };
        for (operation, answer) in &answers {
            assert_eq!(
                answer, &expected,
                "case {case:?}: {operation} on an ARN with no record must answer \
                 {expected:?}, got {answer:?}"
            );
        }

        // Stated as the invariant too, so a future wording change that drifts in only SOME
        // of the operations fails here even if the row above is updated with it.
        let distinct: BTreeSet<&Refusal> = answers.iter().map(|(_, answer)| answer).collect();
        assert_eq!(
            distinct.len(),
            1,
            "case {case:?}: one condition must be answered one way, got {distinct:?}"
        );
    }

    // Nothing above wrote: the gateway that exists still reads back, and the store holds
    // exactly the records it held before the refusals.
    let fetched = call(&router, "GetGateway", &json!({ "GatewayArn": live })).await;
    assert_eq!(
        (fetched.status, fetched.str_at("/Gateway/GatewayDisplayName")),
        (200, Some("gw-live")),
        "the gateway that exists must be unchanged after the refusals, got {} {:?}",
        fetched.status,
        fetched.body
    );
    assert_eq!(
        gateway_count(&engine),
        held,
        "case refusals: no refusal may add or remove a stored gateway"
    );
}

/// The same condition inside the one modelled shape the operation that declares no
/// `ResourceNotFoundException` can carry it in: the status and the `ErrorCode` are that
/// shape's — `ValidationException` (HTTP 400) with a code naming which end of the link is
/// missing — and the SENTENCE is the one every other operation states for the condition.
#[tokio::test]
async fn the_operation_that_models_no_resource_not_found_states_the_same_sentence() {
    let engine = build_engine();
    let router = build_router(&engine);
    let server = import(&router, "hv-assoc-refusal", HOST).await;

    for (case, arn) in arns_naming_nothing(&router).await {
        let refused = call(
            &router,
            "AssociateGatewayToServer",
            &json!({ "GatewayArn": arn, "ServerArn": server }),
        )
        .await;
        assert_eq!(
            refusal_of(&refused),
            Refusal {
                status: 400,
                shape: Some("ValidationException".to_string()),
                error_code: Some("GatewayNotFound".to_string()),
                message: format!("The resource '{arn}' was not found."),
            },
            "case {case:?}: AssociateGatewayToServer must carry the condition in the shape \
             it models and state the service's one sentence for it, got {:?}",
            refused.body
        );

        // And the sentence really is the same string the operations that DO model
        // `ResourceNotFoundException` state for the same ARN.
        let read = call(&router, "GetGateway", &json!({ "GatewayArn": arn })).await;
        assert_eq!(
            message_of(&refused),
            message_of(&read),
            "case {case:?}: the two shapes must state one sentence for one condition, got \
             {:?} and {:?}",
            refused.body,
            read.body
        );
    }
}

/// The counterpart the rule does NOT cover: a resource type this service refuses in its own
/// words keeps them, so unifying the sentence did not flatten a refusal that differs on the
/// wire. A missing hypervisor is answered at HTTP 400 with `ErrorCode` `HypervisorNotFound`.
#[tokio::test]
async fn a_resource_type_with_its_own_refusal_keeps_it() {
    let engine = build_engine();
    let router = build_router(&engine);

    // (case, the ARN of a hypervisor this store holds no record under)
    let rows: [(&str, String); 3] = [
        (
            "a hypervisor that was never imported",
            arn_of("hypervisor", "hv-1111111111111111"),
        ),
        (
            "another hypervisor id",
            arn_of("hypervisor", "hv-2222222222nonexistent"),
        ),
        (
            "a hypervisor id of the id form a listing reports",
            arn_of("hypervisor", "hv-0000000000009999"),
        ),
    ];

    for (case, arn) in &rows {
        for (operation, body) in [
            ("ListTagsForResource", json!({ "ResourceArn": arn })),
            (
                "TagResource",
                json!({ "ResourceARN": arn, "Tags": [{ "Key": "env", "Value": "test" }] }),
            ),
        ] {
            let refused = call(&router, operation, &body).await;
            assert_eq!(
                refusal_of(&refused),
                Refusal {
                    status: 400,
                    shape: Some("ResourceNotFoundException".to_string()),
                    error_code: Some("HypervisorNotFound".to_string()),
                    message: "The hypervisor is not found.".to_string(),
                },
                "case {case:?}: {operation} must keep this service's own refusal for a \
                 missing hypervisor, got {:?}",
                refused.body
            );
        }
    }
}
