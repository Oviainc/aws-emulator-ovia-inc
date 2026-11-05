// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `PutHypervisorPropertyMappings`.
//!
//! Every case drives a real `POST /` awsJson1_0 request through the
//! `MultiServiceRouter`, reusing the harness of `gateway_lifecycle` and the mapping
//! helpers of `hypervisor_property_mappings`, so what is asserted is the wire behaviour
//! and the only way a later request can see what an earlier one wrote is the shared store.
//!
//! The rule under test: a property mapping is applied to a hypervisor's entities by the
//! backup GATEWAY associated with it, so `PutHypervisorPropertyMappings` persists the
//! request's IAM role and tag mappings — in the order they were sent, replacing whatever
//! the record carried — on the hypervisor its `ServerArn` resolves to and answers with
//! that record's stored ARN ONLY when a gateway is associated with it; with none it
//! refuses with HTTP 400 `ValidationException` carrying the `ErrorCode` AWS states for an
//! out-of-date gateway (the `GATEWAY_OUT_OF_DATE` constant this suite asserts)
//! and writes nothing, however often the call is repeated. An ARN the store holds no
//! hypervisor under never reaches that gate: it is refused as this service's one missing
//! hypervisor (`put_property_mappings_one_answer_for_a_missing_hypervisor`).
//!
//! The tables below span that class rather than one recorded request: mapping sets
//! differing in count, in order, in whether a category or a key repeats, and at each
//! published bound of the modelled members; IAM roles differing in partition, path and
//! length; ARNs differing in resource type, partition, id and length, including each
//! published bound of `ServerArn` and one past it; and the association differing in
//! whether no gateway exists, one exists unassociated, or one is associated with a
//! different hypervisor.
//!
//! `AssociateGatewayToServer` — the operation that associates a gateway with a hypervisor
//! — has its own suite (`gateway_server_association`); the association is seeded here
//! through the very `StoredGateway` fields it writes and this handler reads
//! ([`associate`]), so this suite's scene stays independent of that operation. The
//! workflow driven with wire calls only is covered by `metadata_sync_over_the_wire`.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_backupgateway_backuponpremises_v20210101 as backup_gateway;

use backup_gateway::state::{
    arn_resource_id, service_state, VmwareToAwsTagMapping, GATEWAY_OUT_OF_DATE,
    GATEWAY_OUT_OF_DATE_MESSAGE, HYPERVISOR_NOT_FOUND, HYPERVISOR_NOT_FOUND_MESSAGE,
    MAPPINGS_NOT_FOUND,
};

use crate::gateway_lifecycle::{build_engine, build_router, call, create, Reply, REGION};
use crate::hypervisor_property_mappings::{
    arn_of_length, expected_mappings, get_mappings, hypervisor_arn_in, import, mapping,
    persisted_mappings,
};

/// Associate `gateway_arn` with the hypervisor `hypervisor_arn` names, straight on the
/// stored record.
///
/// This is what `AssociateGatewayToServer` persists over the wire (that operation has its
/// own suite, `gateway_server_association`): the gateway record's `hypervisor_arn` — the
/// field `PutHypervisorPropertyMappings` resolves the association through — and the bare
/// `HypervisorId` read off that ARN, so the two cannot name different hypervisors.
/// Seeding it here keeps this suite's scene independent of that operation.
fn associate(engine: &Arc<engine::InMemoryEngine>, gateway_arn: &str, hypervisor_arn: &str) {
    let hypervisor_id = arn_resource_id(hypervisor_arn).map(str::to_string);
    let state = service_state(&**engine).expect("the in-memory engine must expose this state");
    let mut store = state.lock();
    let gateway = store
        .gateway_mut(gateway_arn)
        .unwrap_or_else(|| panic!("no gateway stored under {gateway_arn}"));
    gateway.hypervisor_arn = Some(hypervisor_arn.to_string());
    gateway.hypervisor_id = hypervisor_id;
}

/// A hypervisor with a gateway associated with it: the scene a put can succeed in.
async fn served_hypervisor(
    engine: &Arc<engine::InMemoryEngine>,
    router: &MultiServiceRouter,
    name: &str,
    host: &str,
) -> String {
    let hypervisor = import(router, name, host).await;
    let gateway = create(router, &format!("gw-{name}")).await;
    associate(engine, &gateway, &hypervisor);
    hypervisor
}

/// An `IamRoleArn` of this account in `partition`, naming `path_and_name`.
fn role_arn(partition: &str, path_and_name: &str) -> String {
    format!(
        "arn:{partition}:iam::{}:role/{path_and_name}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

async fn put(
    router: &MultiServiceRouter,
    arn: &str,
    iam_role_arn: &str,
    mappings: &[VmwareToAwsTagMapping],
) -> Reply {
    put_body(
        router,
        &json!({
            "HypervisorArn": arn,
            "IamRoleArn": iam_role_arn,
            "VmwareToAwsTagMappings": expected_mappings(mappings),
        }),
    )
    .await
}

async fn put_body(router: &MultiServiceRouter, body: &Value) -> Reply {
    call(router, "PutHypervisorPropertyMappings", body).await
}

/// The published bounds of the modelled mapping members
/// (`smithy_query(query="shape",shape="VmwareToAwsTagMapping")`): `VmwareCategory` and
/// `VmwareTagName` `@length(min: 1, max: 80)`, `AwsTagKey` 1..128, `AwsTagValue` 0..256.
const LONGEST_CATEGORY: usize = 80;
const LONGEST_TAG_NAME: usize = 80;
const LONGEST_TAG_KEY: usize = 128;
const LONGEST_TAG_VALUE: usize = 256;

/// A put persists what it was sent and every member of a later read comes off that
/// record: the ARN is the store's own, the role and the mappings are the ones that were
/// set, in the order they were set.
#[tokio::test]
async fn putting_mappings_persists_them_and_they_read_back() {
    let engine = build_engine();
    let router = build_router(&engine);

    let longest_category = "c".repeat(LONGEST_CATEGORY);
    let longest_tag_name = "t".repeat(LONGEST_TAG_NAME);
    let longest_key = "k".repeat(LONGEST_TAG_KEY);
    let longest_value = "v".repeat(LONGEST_TAG_VALUE);

    // (case, Host, IamRoleArn, the mappings the put carries)
    let rows: Vec<(&str, &str, String, Vec<VmwareToAwsTagMapping>)> = vec![
        (
            "one mapping, a plain role",
            "198.51.100.60",
            role_arn("aws", "backup-gateway-mappings"),
            vec![mapping("department", "finance", "Department", "Finance")],
        ),
        (
            "four mappings, order preserved, a repeated category and a repeated key",
            "198.51.100.61",
            role_arn("aws", "service-role/backup-gateway"),
            vec![
                mapping("tier", "gold", "Tier", "Gold"),
                mapping("department", "legal", "Department", "Legal"),
                mapping("department", "finance", "Department", "Finance"),
                mapping("tier", "silver", "Tier", "Silver"),
            ],
        ),
        (
            "the minimum of every mapping member, including an empty AwsTagValue",
            "hv-put.example.com",
            // `IamRoleArn` is `@length(min: 20, max: 2048)` with pattern
            // `^arn:(aws|aws-cn|aws-us-gov):iam::([0-9]+):role/(\S+)$`; this is the
            // shortest ARN that pattern admits at that bound.
            "arn:aws:iam::1:role/r".to_string(),
            vec![mapping("c", "t", "k", "")],
        ),
        (
            "the maximum of every mapping member, in the aws-cn partition",
            "203.0.113.60",
            format!("arn:aws-cn:iam::{}:role/{}", 1, "r".repeat(64)),
            vec![mapping(
                &longest_category,
                &longest_tag_name,
                &longest_key,
                &longest_value,
            )],
        ),
        (
            "an empty mapping list in the aws-us-gov partition",
            "203.0.113.61",
            role_arn("aws-us-gov", "gov-mappings"),
            vec![],
        ),
    ];

    for (index, (case, host, iam_role_arn, mappings)) in rows.iter().enumerate() {
        let hypervisor = served_hypervisor(&engine, &router, &format!("hvput{index}"), host).await;

        let reply = put(&router, &hypervisor, iam_role_arn, mappings).await;
        assert_eq!(
            (reply.status, reply.str_at("/HypervisorArn")),
            (200, Some(hypervisor.as_str())),
            "case {case:?}: a put on a served hypervisor must answer 200 with the stored \
             ARN, got {} {:?}",
            reply.status,
            reply.body
        );

        // The round trip: what a later read reports is what the put persisted.
        let read = get_mappings(&router, &hypervisor).await;
        assert_eq!(
            (read.status, read.str_at("/IamRoleArn")),
            (200, Some(iam_role_arn.as_str())),
            "case {case:?}: the stored role must read back, got {} {:?}",
            read.status,
            read.body
        );
        assert_eq!(
            read.body
                .get("VmwareToAwsTagMappings")
                .cloned()
                .unwrap_or(Value::Null),
            expected_mappings(mappings),
            "case {case:?}: the mappings must read back in the order they were put, got {:?}",
            read.body
        );

        // A put of mappings is not a metadata sync: it touches nothing else on the record.
        let hypervisor_read = call(
            &router,
            "GetHypervisor",
            &json!({ "HypervisorArn": hypervisor }),
        )
        .await;
        assert_eq!(
            (
                hypervisor_read.status,
                hypervisor_read.str_at("/Hypervisor/LatestMetadataSyncStatus")
            ),
            (200, None),
            "case {case:?}: a put must not start a metadata sync, got {} {:?}",
            hypervisor_read.status,
            hypervisor_read.body
        );
    }

    // Each record answers from its OWN mappings after every row was written.
    for (index, (case, host, iam_role_arn, _)) in rows.iter().enumerate() {
        let hypervisor = service_state(&*engine)
            .expect("the in-memory engine must expose this state")
            .lock()
            .page_of_hypervisors(REGION, None, 100)
            .items
            .into_iter()
            .find(|record| record.host == *host)
            .map(|record| record.arn)
            .unwrap_or_else(|| panic!("case {case:?}: row {index} must be in the store"));
        let read = get_mappings(&router, &hypervisor).await;
        assert_eq!(
            (read.status, read.str_at("/IamRoleArn")),
            (200, Some(iam_role_arn.as_str())),
            "case {case:?}: each hypervisor must report the role put for it, got {} {:?}",
            read.status,
            read.body
        );
    }
}

/// The request states the mappings the hypervisor is to have: repeating one leaves the
/// record exactly as it was, and a later put REPLACES the previous mappings rather than
/// merging with them.
#[tokio::test]
async fn a_later_put_replaces_the_mappings_and_repeating_one_changes_nothing() {
    let engine = build_engine();
    let router = build_router(&engine);
    let hypervisor = served_hypervisor(&engine, &router, "hvreplace", "198.51.100.70").await;

    // (case, IamRoleArn, mappings) — applied in order; each row differs from the one
    // before in count, order, or both.
    let rows: Vec<(&str, String, Vec<VmwareToAwsTagMapping>)> = vec![
        (
            "the first put",
            role_arn("aws", "first"),
            vec![
                mapping("department", "finance", "Department", "Finance"),
                mapping("tier", "gold", "Tier", "Gold"),
            ],
        ),
        (
            "the same request again",
            role_arn("aws", "first"),
            vec![
                mapping("department", "finance", "Department", "Finance"),
                mapping("tier", "gold", "Tier", "Gold"),
            ],
        ),
        (
            "the same mappings in the opposite order, under another role",
            role_arn("aws", "second"),
            vec![
                mapping("tier", "gold", "Tier", "Gold"),
                mapping("department", "finance", "Department", "Finance"),
            ],
        ),
        (
            "a single mapping, replacing two",
            role_arn("aws-cn", "third"),
            vec![mapping("owner", "platform", "Owner", "Platform")],
        ),
        (
            "an empty mapping list, replacing one",
            role_arn("aws", "fourth"),
            vec![],
        ),
    ];

    // What the hypervisor read back as after each row, so the first two rows — the same
    // request twice — can be compared.
    let mut reads: Vec<Value> = Vec::new();
    for (case, iam_role_arn, mappings) in &rows {
        let reply = put(&router, &hypervisor, iam_role_arn, mappings).await;
        assert_eq!(
            (reply.status, reply.str_at("/HypervisorArn")),
            (200, Some(hypervisor.as_str())),
            "case {case:?}: the put must answer 200 with the stored ARN, got {} {:?}",
            reply.status,
            reply.body
        );

        let read = get_mappings(&router, &hypervisor).await;
        assert_eq!(
            (read.status, read.str_at("/IamRoleArn")),
            (200, Some(iam_role_arn.as_str())),
            "case {case:?}: the role of the LAST put must be the one reported, got {} {:?}",
            read.status,
            read.body
        );
        assert_eq!(
            read.body
                .get("VmwareToAwsTagMappings")
                .cloned()
                .unwrap_or(Value::Null),
            expected_mappings(mappings),
            "case {case:?}: only the mappings of the LAST put may be reported, got {:?}",
            read.body
        );

        let stored = persisted_mappings(&engine, &hypervisor);
        assert!(
            stored.contains(iam_role_arn),
            "case {case:?}: the persisted record must hold the role just put, got {stored}"
        );
        reads.push(read.body);
    }

    // Rows 0 and 1 are the IDENTICAL request: repeating it left exactly what the first
    // one did.
    assert_eq!(
        reads[0], reads[1],
        "repeating a put must leave exactly what the first one left, got {:?} then {:?}",
        reads[0], reads[1]
    );

    assert_eq!(
        service_state(&*engine)
            .expect("the in-memory engine must expose this state")
            .lock()
            .hypervisor_count(),
        1,
        "a put must not add or drop a hypervisor record"
    );
}

/// With no gateway associated with a hypervisor the store HOLDS there is no appliance that
/// can apply the mappings, so the put is refused with HTTP 400 `ValidationException` /
/// the `ErrorCode` AWS states for an out-of-date gateway — the same answer however often
/// it is repeated — and
/// nothing is written.
///
/// An ARN the store holds no hypervisor under is a different condition and takes this
/// service's one answer for it instead (HTTP 400 `ResourceNotFoundException` / `ErrorCode`
/// `HypervisorNotFound`), whatever a gateway record still names: the rows below carry both,
/// and `put_property_mappings_one_answer_for_a_missing_hypervisor` spans that class.
#[tokio::test]
async fn a_put_with_no_gateway_associated_is_refused_deterministically() {
    let engine = build_engine();
    let router = build_router(&engine);

    // The wire contract spelled out here rather than taken from the handler's own
    // constants, so a drift in either the `ErrorCode` clients match on or the sentence
    // AWS is recorded stating fails this test.
    assert_eq!(
        (GATEWAY_OUT_OF_DATE, GATEWAY_OUT_OF_DATE_MESSAGE),
        (
            " OutdatedGateway",
            "The specified gateway is out of date."
        ),
        "the refusal's wire ErrorCode and sentence must not drift"
    );

    // A hypervisor nothing is associated with, a gateway associated with nothing, and a
    // hypervisor that IS served — so a refusal below is about the association and not
    // about an empty store.
    let unserved = import(&router, "hvunserved", "198.51.100.80").await;
    let unassociated_gateway = create(&router, "gw-unassociated").await;
    let served = served_hypervisor(&engine, &router, "hvserved", "198.51.100.81").await;
    // A second gateway, associated with the SERVED hypervisor: a gateway exists for
    // another hypervisor, which does nothing for the rows below.
    let elsewhere = create(&router, "gw-elsewhere").await;
    associate(&engine, &elsewhere, &served);

    // (case, the HypervisorArn the put names, whether the store holds a hypervisor
    // under it)
    let rows: Vec<(&str, String, bool)> = vec![
        (
            "a hypervisor no gateway is associated with",
            unserved.clone(),
            true,
        ),
        (
            "an ARN that names no hypervisor at all",
            hypervisor_arn_in("aws", "hv-ffffffffffffff11"),
            false,
        ),
        (
            "an ARN in the aws-cn partition that names no hypervisor",
            hypervisor_arn_in("aws-cn", "hv-ffffffffffffff12"),
            false,
        ),
        (
            "an ARN in the aws-us-gov partition that names no hypervisor",
            hypervisor_arn_in("aws-us-gov", "hv-ffffffffffffff13"),
            false,
        ),
        (
            "the ARN of a gateway that really exists",
            unassociated_gateway.clone(),
            false,
        ),
        (
            "the 50-character minimum of the modelled ServerArn",
            arn_of_length(50),
            false,
        ),
        (
            "the 500-character maximum of the modelled ServerArn",
            arn_of_length(500),
            false,
        ),
    ];

    for (case, arn, stored) in &rows {
        let mappings = vec![mapping("department", "finance", "Department", "Finance")];
        let role = role_arn("aws", "refused");

        // Which condition the row states decides the answer: a record the store holds and
        // no appliance serves is the gateway refusal; an ARN naming no record is the
        // missing hypervisor every operation of this service refuses the one way.
        let expected = if *stored {
            (
                400,
                Some("ValidationException"),
                Some(GATEWAY_OUT_OF_DATE),
                Some(GATEWAY_OUT_OF_DATE_MESSAGE),
            )
        } else {
            (
                400,
                Some("ResourceNotFoundException"),
                Some(HYPERVISOR_NOT_FOUND),
                Some(HYPERVISOR_NOT_FOUND_MESSAGE),
            )
        };

        let mut answers = Vec::new();
        for attempt in 0..2 {
            let reply = put(&router, arn, &role, &mappings).await;
            assert_eq!(
                (
                    reply.status,
                    reply.code.as_deref(),
                    reply.str_at("/ErrorCode"),
                    reply.str_at("/Message")
                ),
                expected,
                "case {case:?}, attempt {attempt}: a put no appliance can carry out must be \
                 refused with {expected:?}, got {} {:?}",
                reply.status,
                reply.body
            );
            answers.push(reply.body);
        }
        assert!(
            answers.windows(2).all(|pair| pair[0] == pair[1]),
            "case {case:?}: repeated puts must be refused identically, got {answers:?}"
        );

        if *stored {
            assert_eq!(
                persisted_mappings(&engine, arn),
                "None",
                "case {case:?}: a refused put must write no mappings"
            );
            let read = get_mappings(&router, arn).await;
            assert_eq!(
                (read.status, read.str_at("/ErrorCode")),
                (400, Some(MAPPINGS_NOT_FOUND)),
                "case {case:?}: the hypervisor must still carry no mappings, got {} {:?}",
                read.status,
                read.body
            );
        }
    }

    // The gate keys on the association for a record the store holds: the hypervisor a
    // gateway IS associated with takes the very same request.
    let accepted = put(
        &router,
        &served,
        &role_arn("aws", "accepted"),
        &[mapping("department", "finance", "Department", "Finance")],
    )
    .await;
    assert_eq!(
        (accepted.status, accepted.str_at("/HypervisorArn")),
        (200, Some(served.as_str())),
        "a served hypervisor must accept the request the unserved ones were refused, got {} {:?}",
        accepted.status,
        accepted.body
    );
}

/// An input outside the modelled constraints never reaches the store: it is refused with
/// HTTP 400 `ValidationException` at the boundary, one past each published bound of
/// `ServerArn` (50..500), `IamRoleArn` (20..2048) and the `VmwareToAwsTagMapping` members
/// (`VmwareCategory`/`VmwareTagName` 1..80, `AwsTagKey` 1..128, `AwsTagValue` 0..256), and
/// for a missing required member.
#[tokio::test]
async fn a_put_outside_the_modelled_constraints_is_refused() {
    let engine = build_engine();
    let router = build_router(&engine);
    let hypervisor = served_hypervisor(&engine, &router, "hvbounds", "198.51.100.90").await;
    let role = role_arn("aws", "bounds");
    let one_mapping = expected_mappings(&[mapping("department", "finance", "Department", "Finance")]);

    let rows: Vec<(&str, Value)> = vec![
        (
            "one character under the 50-character ServerArn minimum",
            json!({
                "HypervisorArn": arn_of_length(49),
                "IamRoleArn": role,
                "VmwareToAwsTagMappings": one_mapping,
            }),
        ),
        (
            "one character over the 500-character ServerArn maximum",
            json!({
                "HypervisorArn": arn_of_length(501),
                "IamRoleArn": role,
                "VmwareToAwsTagMappings": one_mapping,
            }),
        ),
        (
            // The pattern admits nothing shorter than 21 characters, so the shortest
            // refusable role is under the 20-character minimum AND outside the pattern.
            "a role shorter than the 20-character minimum",
            json!({
                "HypervisorArn": hypervisor,
                "IamRoleArn": "arn:aws:iam::1:r/r",
                "VmwareToAwsTagMappings": one_mapping,
            }),
        ),
        (
            "a role one character over the 2048-character maximum",
            json!({
                "HypervisorArn": hypervisor,
                "IamRoleArn": format!("arn:aws:iam::1:role/{}", "r".repeat(2029)),
                "VmwareToAwsTagMappings": one_mapping,
            }),
        ),
        (
            "a role the IamRoleArn pattern does not accept",
            json!({
                "HypervisorArn": hypervisor,
                "IamRoleArn": role_arn("aws", "x").replace(":role/", ":user/"),
                "VmwareToAwsTagMappings": one_mapping,
            }),
        ),
        (
            "an empty VmwareCategory, under its 1-character minimum",
            json!({
                "HypervisorArn": hypervisor,
                "IamRoleArn": role,
                "VmwareToAwsTagMappings": expected_mappings(&[mapping("", "t", "k", "v")]),
            }),
        ),
        (
            "a VmwareTagName one character over its 80-character maximum",
            json!({
                "HypervisorArn": hypervisor,
                "IamRoleArn": role,
                "VmwareToAwsTagMappings": expected_mappings(&[mapping(
                    "c",
                    &"t".repeat(LONGEST_TAG_NAME + 1),
                    "k",
                    "v",
                )]),
            }),
        ),
        (
            "an AwsTagKey one character over its 128-character maximum",
            json!({
                "HypervisorArn": hypervisor,
                "IamRoleArn": role,
                "VmwareToAwsTagMappings": expected_mappings(&[mapping(
                    "c",
                    "t",
                    &"k".repeat(LONGEST_TAG_KEY + 1),
                    "v",
                )]),
            }),
        ),
        (
            "an AwsTagValue one character over its 256-character maximum",
            json!({
                "HypervisorArn": hypervisor,
                "IamRoleArn": role,
                "VmwareToAwsTagMappings": expected_mappings(&[mapping(
                    "c",
                    "t",
                    "k",
                    &"v".repeat(LONGEST_TAG_VALUE + 1),
                )]),
            }),
        ),
        (
            "no IamRoleArn at all",
            json!({
                "HypervisorArn": hypervisor,
                "VmwareToAwsTagMappings": one_mapping,
            }),
        ),
        (
            "no VmwareToAwsTagMappings at all",
            json!({ "HypervisorArn": hypervisor, "IamRoleArn": role }),
        ),
        (
            "no HypervisorArn at all",
            json!({ "IamRoleArn": role, "VmwareToAwsTagMappings": one_mapping }),
        ),
    ];

    for (case, body) in &rows {
        let reply = put_body(&router, body).await;
        assert_eq!(
            (reply.status, reply.code.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: an input outside the modelled constraints must be refused with \
             400 ValidationException, got {} {:?}",
            reply.status,
            reply.body
        );
        assert_eq!(
            persisted_mappings(&engine, &hypervisor),
            "None",
            "case {case:?}: a refused put must write no mappings"
        );
    }
}

/// A put whose gateway is associated with a hypervisor record the store NO LONGER holds
/// is refused as a missing hypervisor, the way every other hypervisor operation of this
/// service refuses one: the modelled `ResourceNotFoundException` at HTTP 400 with
/// `ErrorCode` `HypervisorNotFound`.
#[tokio::test]
async fn a_put_for_a_record_the_store_no_longer_holds_is_refused_as_a_missing_hypervisor() {
    let engine = build_engine();
    let router = build_router(&engine);
    let hypervisor = served_hypervisor(&engine, &router, "hvgone", "198.51.100.95").await;

    // The association outlives the record: `DeleteHypervisor` removes the hypervisor and
    // the gateway still names it.
    let deleted = call(
        &router,
        "DeleteHypervisor",
        &json!({ "HypervisorArn": hypervisor }),
    )
    .await;
    assert_eq!(
        deleted.status, 200,
        "DeleteHypervisor must answer 200, got {} {:?}",
        deleted.status, deleted.body
    );

    let reply = put(
        &router,
        &hypervisor,
        &role_arn("aws", "gone"),
        &[mapping("department", "finance", "Department", "Finance")],
    )
    .await;
    assert_eq!(
        (
            reply.status,
            reply.code.as_deref(),
            reply.str_at("/ErrorCode")
        ),
        (
            400,
            Some("ResourceNotFoundException"),
            Some(HYPERVISOR_NOT_FOUND)
        ),
        "a put for a record the store no longer holds must be refused as a missing \
         hypervisor, got {} {:?}",
        reply.status,
        reply.body
    );
    assert_eq!(
        service_state(&*engine)
            .expect("the in-memory engine must expose this state")
            .lock()
            .hypervisor_count(),
        0,
        "a refused put must not resurrect the record"
    );

    // And a fresh hypervisor in the same region, served by its own gateway, still works:
    // the refusal above was about that one record.
    let replacement = served_hypervisor(&engine, &router, "hvfresh", "198.51.100.96").await;
    assert!(
        replacement.contains(REGION),
        "the replacement ARN must carry the signed region, got {replacement}"
    );
    let reply = put(
        &router,
        &replacement,
        &role_arn("aws", "fresh"),
        &[mapping("tier", "gold", "Tier", "Gold")],
    )
    .await;
    assert_eq!(
        (reply.status, reply.str_at("/HypervisorArn")),
        (200, Some(replacement.as_str())),
        "a served hypervisor must still accept a put, got {} {:?}",
        reply.status,
        reply.body
    );
}
