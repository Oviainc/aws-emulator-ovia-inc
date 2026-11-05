// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for ONE rule: a well-formed ARN of this service that names no record
//! the store holds is refused for the KIND of resource that is missing, and that refusal —
//! the HTTP status, the `ErrorCode` member and the sentence — is the same whichever
//! operation resolved it, because the refusal is chosen for the condition and not per
//! operation.
//!
//! The kind of resource that is missing is decided by what the operation resolves: an
//! operation that resolves exactly one resource type is missing THAT resource whatever
//! resource type the ARN happens to spell, and the tagging operations, which take any of
//! this service's resources in one member, read the kind off the ARN. Every kind this
//! service names is answered HTTP 400 with the `ErrorCode` naming it and a fixed sentence
//! that carries no part of the request; an ARN of a resource type this service does not
//! name at all keeps the status the modelled shape declares.
//!
//! Every case drives a real `POST /` awsJson1_0 request through the `MultiServiceRouter`,
//! reusing the harness of `gateway_lifecycle`, so what is asserted is the wire behaviour.
//!
//! The tables below span the class the rule generalises over: every operation of this
//! service that resolves a `gateway/<id>` ARN crossed with ARNs that differ in how they
//! came to name nothing (never created, created and deleted), in partition, in id, in
//! resource-type segment and in length, including each published bound of the modelled
//! `GatewayArn` (`@length(min: 50, max: 180)`,
//! `smithy_query(query="shape",shape="GatewayArn")`); then the tagging operations crossed
//! with every resource type this service's ARNs can name, which is the one member that has
//! to tell them apart.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_backupgateway_backuponpremises_v20210101 as backup_gateway;

use backup_gateway::state::{
    GATEWAY_NOT_FOUND, GATEWAY_NOT_FOUND_MESSAGE, HYPERVISOR_NOT_FOUND,
    HYPERVISOR_NOT_FOUND_MESSAGE, VIRTUAL_MACHINE_NOT_FOUND, VIRTUAL_MACHINE_NOT_FOUND_MESSAGE,
};

use crate::gateway_lifecycle::{
    build_engine, build_router, call, create, gateway_count, never_created_arn, Reply, REGION,
};
use crate::gateway_server_association::{arn_of, arn_of_length, import, message_of};

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

/// The refusal this service states for a missing resource of one kind: the modelled
/// `ResourceNotFoundException` at HTTP 400, the `ErrorCode` naming that kind, and the fixed
/// sentence for it.
fn not_found(error_code: &str, message: &str) -> Refusal {
    Refusal {
        status: 400,
        shape: Some("ResourceNotFoundException".to_string()),
        error_code: Some(error_code.to_string()),
        message: message.to_string(),
    }
}

/// Every operation of this service that resolves a `gateway/<id>` ARN on its own, with a
/// request body that satisfies every OTHER modelled constraint — so the only thing left for
/// the handler to refuse is that the ARN names no gateway.
fn gateway_requests(arn: &str) -> Vec<(&'static str, Value)> {
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
            "DisassociateGatewayFromServer",
            json!({ "GatewayArn": arn }),
        ),
    ]
}

/// The operations that take ANY resource of this service in one member.
///
/// `TagResource` and `UntagResource` spell the member `ResourceARN` and
/// `ListTagsForResource` spells it `ResourceArn`, as this service's model does.
fn tagging_requests(arn: &str) -> Vec<(&'static str, Value)> {
    vec![
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

/// The rule: for one ARN that resolves to no gateway, every operation that resolves a
/// gateway answers the very same refusal — status, `ErrorCode` member and sentence alike —
/// and the sentence carries no part of the request.
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
        for (operation, body) in gateway_requests(&arn) {
            let reply = call(&router, operation, &body).await;
            answers.push((operation, refusal_of(&reply)));
        }

        let expected = not_found(GATEWAY_NOT_FOUND, GATEWAY_NOT_FOUND_MESSAGE);
        for (operation, answer) in &answers {
            assert_eq!(
                answer, &expected,
                "case {case:?}: {operation} on an ARN with no gateway must answer \
                 {expected:?}, got {answer:?}"
            );
            assert!(
                !answer.message.contains(arn.as_str()),
                "case {case:?}: {operation} must not echo the ARN the request carried, got \
                 {answer:?}"
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

/// The same condition inside the operation whose model declares no
/// `ResourceNotFoundException` at all (`smithy_query(query="operation",
/// operation="AssociateGatewayToServer")`): an ARN that resolves to nothing is a not-found
/// condition, so it is answered in the same shape, with the same status, code and sentence
/// every operation that DOES declare it answers with.
#[tokio::test]
async fn the_operation_that_models_no_resource_not_found_states_the_same_refusal() {
    let engine = build_engine();
    let router = build_router(&engine);
    let server = import(&router, "hv-assoc-refusal", "198.51.100.90").await;

    for (case, arn) in arns_naming_nothing(&router).await {
        let refused = call(
            &router,
            "AssociateGatewayToServer",
            &json!({ "GatewayArn": arn, "ServerArn": server }),
        )
        .await;
        assert_eq!(
            refusal_of(&refused),
            not_found(GATEWAY_NOT_FOUND, GATEWAY_NOT_FOUND_MESSAGE),
            "case {case:?}: AssociateGatewayToServer must answer the service's one refusal \
             for a missing gateway, got {:?}",
            refused.body
        );

        // And it really is the same answer the operations that declare the shape give for
        // the same ARN.
        let read = call(&router, "GetGateway", &json!({ "GatewayArn": arn })).await;
        assert_eq!(
            refusal_of(&refused),
            refusal_of(&read),
            "case {case:?}: two operations must answer one condition one way, got {:?} and \
             {:?}",
            refused.body,
            read.body
        );
    }
}

/// The other half of the rule: the operations that take ANY resource of this service in one
/// member answer for the resource TYPE the ARN names, so unifying the answer per condition
/// did not flatten refusals that differ on the wire. A type this service does not name at
/// all has no `ErrorCode` of its own and keeps the status the modelled shape declares.
#[tokio::test]
async fn the_tagging_operations_answer_for_the_resource_type_the_arn_names() {
    let engine = build_engine();
    let router = build_router(&engine);

    // (case, an ARN this store holds no record under, the refusal it must take)
    let rows: [(&str, String, Refusal); 7] = [
        (
            "a hypervisor that was never imported",
            arn_of("hypervisor", "hv-1111111111111111"),
            not_found(HYPERVISOR_NOT_FOUND, HYPERVISOR_NOT_FOUND_MESSAGE),
        ),
        (
            "another hypervisor id",
            arn_of("hypervisor", "hv-2222222222nonexistent"),
            not_found(HYPERVISOR_NOT_FOUND, HYPERVISOR_NOT_FOUND_MESSAGE),
        ),
        (
            "a gateway that was never created",
            never_created_arn(),
            not_found(GATEWAY_NOT_FOUND, GATEWAY_NOT_FOUND_MESSAGE),
        ),
        (
            "another gateway id",
            arn_of("gateway", "bgw-3333333333nonexistent"),
            not_found(GATEWAY_NOT_FOUND, GATEWAY_NOT_FOUND_MESSAGE),
        ),
        (
            "a virtual machine no sync ever reported",
            arn_of("vm", "vm-1111111111111111"),
            not_found(
                VIRTUAL_MACHINE_NOT_FOUND,
                VIRTUAL_MACHINE_NOT_FOUND_MESSAGE,
            ),
        ),
        (
            "another virtual machine id",
            arn_of("vm", "vm-2222222222nonexistent"),
            not_found(
                VIRTUAL_MACHINE_NOT_FOUND,
                VIRTUAL_MACHINE_NOT_FOUND_MESSAGE,
            ),
        ),
        (
            "a resource type this service does not name",
            arn_of("appliance", "ap-1111111111111111"),
            Refusal {
                status: 404,
                shape: Some("ResourceNotFoundException".to_string()),
                error_code: None,
                message: format!(
                    "The resource 'arn:aws:backup-gateway:{REGION}:{}:appliance/ap-1111111111111111' was not found.",
                    engine::EMULATOR_ACCOUNT_ID
                ),
            },
        ),
    ];

    for (case, arn, expected) in &rows {
        for (operation, body) in tagging_requests(arn) {
            let refused = call(&router, operation, &body).await;
            assert_eq!(
                &refusal_of(&refused),
                expected,
                "case {case:?}: {operation} must refuse the ARN as the resource type it \
                 names with {expected:?}, got {:?}",
                refused.body
            );
        }
    }
}
