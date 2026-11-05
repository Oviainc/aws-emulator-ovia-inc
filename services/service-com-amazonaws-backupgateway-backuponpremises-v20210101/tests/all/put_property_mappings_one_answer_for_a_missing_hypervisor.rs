// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for ONE rule: `PutHypervisorPropertyMappings` resolves the hypervisor
//! its `HypervisorArn` names BEFORE it asks which appliance would apply the mappings, so an
//! ARN this service holds no hypervisor under is refused as this service's one missing
//! hypervisor — HTTP 400 `ResourceNotFoundException` with `ErrorCode` `HypervisorNotFound`
//! and the sentence every operation that resolves a hypervisor states — however it came to
//! name nothing and whatever OTHER resource still points at it.
//!
//! Every case drives a real `POST /` awsJson1_0 request through the `MultiServiceRouter`,
//! reusing the harness of `gateway_lifecycle` and the mapping helpers of
//! `hypervisor_property_mappings`, so what is asserted is the wire behaviour and the only
//! way a later request can see what an earlier one wrote is the shared store.
//!
//! The table spans the class the rule generalises over: ARNs differing in HOW they came to
//! name no hypervisor (never imported; imported and deleted with no gateway ever associated;
//! imported, associated with a gateway and deleted, so the gateway record still names the
//! deleted server; the same with two gateways; associated, disassociated and deleted), in
//! partition (`aws`, `aws-cn`, `aws-us-gov`), in the resource type the ARN spells (a gateway
//! that really exists, a virtual machine) and in length, at each published bound of the
//! modelled `ServerArn` (`@length(min: 50, max: 500)`,
//! `smithy_query(query="shape",shape="ServerArn")`).
//!
//! The counterpart is asserted too, so resolving the record first did not flatten the
//! refusal it precedes: a hypervisor this store HOLDS that no gateway is associated with
//! keeps the gateway refusal (`ValidationException` with the `ErrorCode` AWS states for an
//! out-of-date gateway), and associating
//! a gateway with that very record makes the same request succeed.

use std::collections::BTreeSet;

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_backupgateway_backuponpremises_v20210101 as backup_gateway;

use backup_gateway::state::{
    GATEWAY_OUT_OF_DATE, GATEWAY_OUT_OF_DATE_MESSAGE, HYPERVISOR_NOT_FOUND,
    HYPERVISOR_NOT_FOUND_MESSAGE,
};

use crate::gateway_lifecycle::{
    build_engine, build_router, call, create, hypervisor_count, Reply, REGION,
};
use crate::hypervisor_property_mappings::{
    arn_of_length, expected_mappings, hypervisor_arn_in, import, mapping,
};

/// What one refusal said, reduced to the parts a client branches on.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Answer {
    status: u16,
    /// The awsJson error discriminator, namespace stripped.
    shape: Option<String>,
    /// The `ErrorCode` member of the modelled error shape.
    error_code: Option<String>,
    message: Option<String>,
}

fn answer_of(reply: &Reply) -> Answer {
    Answer {
        status: reply.status,
        shape: reply.code.clone(),
        error_code: reply.str_at("/ErrorCode").map(str::to_string),
        message: reply.str_at("/Message").map(str::to_string),
    }
}

/// The one answer this service gives for a `HypervisorArn` it holds no record under, spelled
/// out here rather than taken from the handler, so a drift in any of its three parts fails.
fn missing_hypervisor() -> Answer {
    Answer {
        status: 400,
        shape: Some("ResourceNotFoundException".to_string()),
        error_code: Some("HypervisorNotFound".to_string()),
        message: Some("The hypervisor is not found.".to_string()),
    }
}

/// A put of one well-formed mapping on `arn`: every modelled constraint satisfied, so the
/// only thing left for the handler to answer is what the store holds.
async fn put(router: &MultiServiceRouter, arn: &str) -> Reply {
    call(
        router,
        "PutHypervisorPropertyMappings",
        &json!({
            "HypervisorArn": arn,
            "IamRoleArn": format!(
                "arn:aws:iam::{}:role/backup-gateway-mappings",
                engine::EMULATOR_ACCOUNT_ID
            ),
            "VmwareToAwsTagMappings": expected_mappings(&[mapping(
                "department",
                "finance",
                "Department",
                "Finance",
            )]),
        }),
    )
    .await
}

async fn associate(router: &MultiServiceRouter, gateway_arn: &str, server_arn: &str) {
    let reply = call(
        router,
        "AssociateGatewayToServer",
        &json!({ "GatewayArn": gateway_arn, "ServerArn": server_arn }),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "AssociateGatewayToServer must answer 200, got {} {:?}",
        reply.status, reply.body
    );
}

async fn delete_hypervisor(router: &MultiServiceRouter, arn: &str) {
    let reply = call(router, "DeleteHypervisor", &json!({ "HypervisorArn": arn })).await;
    assert_eq!(
        reply.status, 200,
        "DeleteHypervisor must answer 200, got {} {:?}",
        reply.status, reply.body
    );
}

/// The `HypervisorId` a gateway reports, so a stale link is shown to be still there.
async fn reported_hypervisor_id(router: &MultiServiceRouter, gateway_arn: &str) -> Option<String> {
    let reply = call(router, "GetGateway", &json!({ "GatewayArn": gateway_arn })).await;
    assert_eq!(
        reply.status, 200,
        "GetGateway must answer 200, got {} {:?}",
        reply.status, reply.body
    );
    reply
        .str_at("/Gateway/HypervisorId")
        .map(str::to_string)
}

/// Every way an ARN can name no hypervisor is answered with the one refusal, twice over, and
/// writes nothing.
#[tokio::test]
async fn an_arn_naming_no_hypervisor_is_refused_one_way_however_it_came_to_name_nothing() {
    let engine = build_engine();
    let router = build_router(&engine);

    // A hypervisor that really exists, served by its own gateway, so a refusal below is
    // about the ARN that was asked for and not about an empty store.
    let live = import(&router, "hv-live", "198.51.100.10").await;
    let live_gateway = create(&router, "gw-live").await;
    associate(&router, &live_gateway, &live).await;

    // Imported and deleted with no gateway ever associated with it.
    let deleted_unserved = import(&router, "hv-deleted-unserved", "198.51.100.11").await;
    delete_hypervisor(&router, &deleted_unserved).await;

    // Imported, a gateway associated with it, then deleted: the gateway record still names
    // the deleted server (the association is the GATEWAY's own fact), which is the state the
    // ARN below is asked in.
    let stale = import(&router, "hv-stale", "198.51.100.12").await;
    let stale_gateway = create(&router, "gw-stale").await;
    associate(&router, &stale_gateway, &stale).await;
    delete_hypervisor(&router, &stale).await;

    // The same with TWO gateways left naming the deleted server, so the answer does not
    // depend on how many stale links there are.
    let stale_twice = import(&router, "hv-stale-twice", "198.51.100.13").await;
    let stale_gateways = [
        create(&router, "gw-stale-a").await,
        create(&router, "gw-stale-b").await,
    ];
    for gateway in &stale_gateways {
        associate(&router, gateway, &stale_twice).await;
    }
    delete_hypervisor(&router, &stale_twice).await;

    // Associated, disassociated and then deleted: no link is left at all.
    let disassociated = import(&router, "hv-disassociated", "198.51.100.14").await;
    let disassociated_gateway = create(&router, "gw-disassociated").await;
    associate(&router, &disassociated_gateway, &disassociated).await;
    let cleared = call(
        &router,
        "DisassociateGatewayFromServer",
        &json!({ "GatewayArn": disassociated_gateway }),
    )
    .await;
    assert_eq!(
        cleared.status, 200,
        "DisassociateGatewayFromServer must answer 200, got {} {:?}",
        cleared.status, cleared.body
    );
    delete_hypervisor(&router, &disassociated).await;

    // A gateway that really exists: its ARN names a record of this service, just not a
    // hypervisor.
    let gateway_arn = create(&router, "gw-not-a-hypervisor").await;

    // (case, the HypervisorArn the put names)
    let rows: Vec<(&str, String)> = vec![
        (
            "never imported",
            hypervisor_arn_in("aws", "hv-ffffffffffffff11"),
        ),
        (
            "never imported, the aws-cn partition",
            hypervisor_arn_in("aws-cn", "hv-ffffffffffffff12"),
        ),
        (
            "never imported, the aws-us-gov partition",
            hypervisor_arn_in("aws-us-gov", "hv-ffffffffffffff13"),
        ),
        ("imported and deleted, never served", deleted_unserved),
        (
            "deleted while a gateway was associated with it",
            stale.clone(),
        ),
        (
            "deleted while two gateways were associated with it",
            stale_twice,
        ),
        (
            "associated, disassociated and then deleted",
            disassociated,
        ),
        ("the ARN of a gateway that really exists", gateway_arn),
        (
            "an ARN naming a virtual machine",
            format!(
                "arn:aws:backup-gateway:{REGION}:{}:vm/vm-1111111111111111",
                engine::EMULATOR_ACCOUNT_ID
            ),
        ),
        (
            "the 50-character minimum of the modelled ServerArn",
            arn_of_length(50),
        ),
        (
            "the 500-character maximum of the modelled ServerArn",
            arn_of_length(500),
        ),
    ];

    let held = hypervisor_count(&engine);
    let mut distinct: BTreeSet<Answer> = BTreeSet::new();

    for (case, arn) in &rows {
        let mut answers = Vec::new();
        for attempt in 0..2 {
            let reply = put(&router, arn).await;
            let answer = answer_of(&reply);
            assert_eq!(
                answer,
                missing_hypervisor(),
                "case {case:?}, attempt {attempt}: an ARN naming no hypervisor must be \
                 refused with {:?}, got {answer:?}",
                missing_hypervisor()
            );
            answers.push(answer);
        }
        assert!(
            answers.windows(2).all(|pair| pair[0] == pair[1]),
            "case {case:?}: repeated puts must be refused identically, got {answers:?}"
        );
        distinct.extend(answers);

        // The very same condition, asked through the operation that only READS the record:
        // one condition, one answer, whichever operation resolved it.
        let read = call(
            &router,
            "GetHypervisor",
            &json!({ "HypervisorArn": arn }),
        )
        .await;
        assert_eq!(
            answer_of(&read),
            missing_hypervisor(),
            "case {case:?}: GetHypervisor must answer the same condition the same way, got \
             {} {:?}",
            read.status,
            read.body
        );
    }

    assert_eq!(
        distinct.len(),
        1,
        "one condition must be answered one way across every row, got {distinct:?}"
    );

    // Nothing above wrote: no record appeared or vanished, the stale link is still the
    // gateway's own fact, and the hypervisor that exists still takes the put.
    assert_eq!(
        hypervisor_count(&engine),
        held,
        "a refused put must neither add nor resurrect a hypervisor record"
    );
    assert_eq!(
        reported_hypervisor_id(&router, &stale_gateway).await,
        stale.rsplit('/').next().map(str::to_string),
        "the gateway must still report the id of the server it was associated with"
    );
    let accepted = put(&router, &live).await;
    assert_eq!(
        (accepted.status, accepted.str_at("/HypervisorArn")),
        (200, Some(live.as_str())),
        "the served hypervisor must still accept the request the missing ones were refused, \
         got {} {:?}",
        accepted.status,
        accepted.body
    );
}

/// Resolving the record first leaves the refusal it precedes in place: a hypervisor this
/// store HOLDS and no gateway serves is still refused as the gateway being out of date, and
/// the same ARN takes the put once a gateway is associated with it.
#[tokio::test]
async fn a_stored_hypervisor_no_gateway_serves_keeps_the_gateway_refusal() {
    let engine = build_engine();
    let router = build_router(&engine);

    // (case, the gateways that exist but are associated with nothing of this row)
    let rows: [(&str, usize); 3] = [
        ("no gateway exists at all", 0),
        ("one gateway exists, associated with nothing", 1),
        ("two gateways exist, associated with nothing", 2),
    ];

    for (index, (case, gateways)) in rows.iter().enumerate() {
        let unserved = import(
            &router,
            &format!("hv-unserved-{index}"),
            &format!("198.51.100.{}", 20 + index),
        )
        .await;
        let mut created = Vec::new();
        for spare in 0..*gateways {
            created.push(create(&router, &format!("gw-spare-{index}-{spare}")).await);
        }

        let refused = put(&router, &unserved).await;
        assert_eq!(
            answer_of(&refused),
            Answer {
                status: 400,
                shape: Some("ValidationException".to_string()),
                error_code: Some(GATEWAY_OUT_OF_DATE.to_string()),
                message: Some(GATEWAY_OUT_OF_DATE_MESSAGE.to_string()),
            },
            "case {case:?}: a stored hypervisor no gateway serves must be refused as out of \
             date, got {} {:?}",
            refused.status,
            refused.body
        );
        assert_ne!(
            answer_of(&refused),
            missing_hypervisor(),
            "case {case:?}: a record the store holds must not be refused as a missing \
             hypervisor"
        );

        // Associating a gateway with that very record makes the identical request succeed,
        // so the refusal above was about the appliance and nothing else.
        let gateway = match created.first() {
            Some(existing) => existing.clone(),
            None => create(&router, &format!("gw-serving-{index}")).await,
        };
        associate(&router, &gateway, &unserved).await;
        let accepted = put(&router, &unserved).await;
        assert_eq!(
            (accepted.status, accepted.str_at("/HypervisorArn")),
            (200, Some(unserved.as_str())),
            "case {case:?}: the served hypervisor must accept the identical request, got {} \
             {:?}",
            accepted.status,
            accepted.body
        );
    }

    // And the two refusals really are different answers, as the two conditions are.
    assert_ne!(
        (
            GATEWAY_OUT_OF_DATE,
            GATEWAY_OUT_OF_DATE_MESSAGE,
            "ValidationException"
        ),
        (
            HYPERVISOR_NOT_FOUND,
            HYPERVISOR_NOT_FOUND_MESSAGE,
            "ResourceNotFoundException"
        ),
        "the two conditions must not collapse onto one wire answer"
    );
    assert_eq!(
        hypervisor_count(&engine),
        rows.len(),
        "every row's record must still be held"
    );
}
