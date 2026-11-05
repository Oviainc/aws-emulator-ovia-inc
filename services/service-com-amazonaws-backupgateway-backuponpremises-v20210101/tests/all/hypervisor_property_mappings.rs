// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetHypervisorPropertyMappings`.
//!
//! Every case drives a real `POST /` awsJson1_0 request through the
//! `MultiServiceRouter`, reusing the harness of `gateway_lifecycle`, so what is asserted
//! is the wire behaviour and the only way a later request can see what an earlier one
//! wrote is the shared store.
//!
//! The rule under test: `GetHypervisorPropertyMappings` reports the property mappings
//! PERSISTED on the hypervisor its `ServerArn` resolves to — that record's ARN, the IAM
//! role the mappings were set with and every VMware-to-Amazon tag mapping in the stored
//! order — and refuses with the modelled `ResourceNotFoundException` when the store holds
//! no such resource: a missing hypervisor (`ErrorCode` `HypervisorNotFound`) or a
//! hypervisor carrying no mappings (`ErrorCode` `HypervisorPropertyMappingsNotFound`).
//! Being a read, it
//! changes nothing a later caller sees.
//!
//! The tables below span that class — mapping sets differing in count, in order, in
//! whether a category or a key repeats, and at each published bound of the modelled
//! member lengths; IAM roles differing in partition, path and length; and ARNs differing
//! in resource type, partition, id and length, including each published bound of
//! `ServerArn` and one past it.
//!
//! `PutHypervisorPropertyMappings` is the operation that SETS mappings; the cases here
//! seed them straight into the `property_mappings` field it writes, so a read is asserted
//! against a known store whatever that handler does, and the write side is covered
//! end-to-end by the sibling `put_hypervisor_property_mappings` suite. The helpers below
//! are shared with it.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_backupgateway_backuponpremises_v20210101 as backup_gateway;

use backup_gateway::state::{
    service_state, StoredPropertyMappings, VmwareToAwsTagMapping, HYPERVISOR_NOT_FOUND,
    HYPERVISOR_ONLINE, MAPPINGS_NOT_FOUND, MAPPINGS_NOT_FOUND_MESSAGE,
};

use crate::gateway_lifecycle::{build_engine, build_router, call, create, Reply, REGION};

/// A hypervisor ARN in `partition` naming `id`, so the rows span every partition the
/// modelled `ServerArn` pattern accepts (`aws`, `aws-cn`, `aws-us-gov`).
pub(crate) fn hypervisor_arn_in(partition: &str, id: &str) -> String {
    format!(
        "arn:{partition}:backup-gateway:{REGION}:{}:hypervisor/{id}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// A `ServerArn`-shaped ARN of exactly `len` characters, for the published length bounds
/// of the modelled shape: `@length(min: 50, max: 500)`
/// (`smithy_query(query="shape",shape="ServerArn")`). The id segment absorbs the length,
/// so every character stays inside the pattern's `[a-zA-Z-0-9]+`, and the resource type
/// is kept short enough that the 50-character minimum fits.
pub(crate) fn arn_of_length(len: usize) -> String {
    let prefix = format!(
        "arn:aws:backup-gateway:{REGION}:{}:h/",
        engine::EMULATOR_ACCOUNT_ID
    );
    assert!(
        len > prefix.len(),
        "a {len}-character ARN cannot hold the {}-character prefix",
        prefix.len()
    );
    format!("{prefix}{}", "x".repeat(len - prefix.len()))
}

/// One modelled `VmwareToAwsTagMapping`
/// (`smithy_query(query="shape",shape="VmwareToAwsTagMapping")`: every member required).
pub(crate) fn mapping(
    category: &str,
    tag_name: &str,
    key: &str,
    value: &str,
) -> VmwareToAwsTagMapping {
    VmwareToAwsTagMapping {
        vmware_category: category.to_string(),
        vmware_tag_name: tag_name.to_string(),
        aws_tag_key: key.to_string(),
        aws_tag_value: value.to_string(),
    }
}

/// The `VmwareToAwsTagMappings` list as it must appear in the response body, in the
/// stored order.
pub(crate) fn expected_mappings(mappings: &[VmwareToAwsTagMapping]) -> Value {
    Value::Array(
        mappings
            .iter()
            .map(|mapping| {
                json!({
                    "VmwareCategory": mapping.vmware_category,
                    "VmwareTagName": mapping.vmware_tag_name,
                    "AwsTagKey": mapping.aws_tag_key,
                    "AwsTagValue": mapping.aws_tag_value,
                })
            })
            .collect(),
    )
}

/// Import a hypervisor over the wire and hand back the ARN the import minted.
pub(crate) async fn import(router: &MultiServiceRouter, name: &str, host: &str) -> String {
    let reply = call(
        router,
        "ImportHypervisorConfiguration",
        &json!({ "Name": name, "Host": host }),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "ImportHypervisorConfiguration must answer 200, got {} {:?}",
        reply.status, reply.body
    );
    reply
        .str_at("/HypervisorArn")
        .unwrap_or_else(|| panic!("no HypervisorArn in {:?}", reply.body))
        .to_string()
}

pub(crate) async fn get_mappings(router: &MultiServiceRouter, arn: &str) -> Reply {
    call(
        router,
        "GetHypervisorPropertyMappings",
        &json!({ "HypervisorArn": arn }),
    )
    .await
}

/// Set `mappings` on the stored hypervisor — the one field
/// `PutHypervisorPropertyMappings` will write (see the module docs).
pub(crate) fn set_mappings(
    engine: &Arc<engine::InMemoryEngine>,
    arn: &str,
    mappings: Option<StoredPropertyMappings>,
) {
    let state = service_state(&**engine).expect("the in-memory engine must expose this state");
    state
        .lock()
        .hypervisor_mut(arn)
        .unwrap_or_else(|| panic!("no hypervisor stored under {arn}"))
        .property_mappings = mappings;
}

/// The mappings PERSISTED on the record under `arn`, rendered for comparison: what a read
/// must not have changed.
pub(crate) fn persisted_mappings(engine: &Arc<engine::InMemoryEngine>, arn: &str) -> String {
    let state = service_state(&**engine).expect("the in-memory engine must expose this state");
    let store = state.lock();
    let record = store
        .hypervisor(arn)
        .unwrap_or_else(|| panic!("no hypervisor stored under {arn}"));
    format!("{:?}", record.property_mappings)
}

/// Every member the response carries comes off the stored record, for mapping sets
/// spanning count, order, repeated categories and keys, each published bound of the
/// modelled member lengths, and IAM roles spanning partition, path and length.
#[tokio::test]
async fn get_hypervisor_property_mappings_reports_the_stored_mappings() {
    let engine = build_engine();
    let router = build_router(&engine);

    // The published bounds of the mapping members
    // (`smithy_query(query="shape",shape="VmwareToAwsTagMapping")`): `VmwareCategory` and
    // `VmwareTagName` `@length(min: 1, max: 80)`, `AwsTagKey` 1..128, `AwsTagValue`
    // 0..256.
    let longest_category = "c".repeat(80);
    let longest_tag_name = "t".repeat(80);
    let longest_key = "k".repeat(128);
    let longest_value = "v".repeat(256);

    // (case, Host, IamRoleArn, the mappings stored for it)
    let rows: Vec<(&str, &str, String, Vec<VmwareToAwsTagMapping>)> = vec![
        (
            "one mapping, a plain role",
            "198.51.100.10",
            format!(
                "arn:aws:iam::{}:role/backup-gateway-mappings",
                engine::EMULATOR_ACCOUNT_ID
            ),
            vec![mapping("department", "finance", "Department", "Finance")],
        ),
        (
            "three mappings, order preserved, one category repeated",
            "198.51.100.11",
            format!(
                "arn:aws:iam::{}:role/service-role/backup-gateway",
                engine::EMULATOR_ACCOUNT_ID
            ),
            vec![
                mapping("department", "finance", "Department", "Finance"),
                mapping("department", "legal", "Department", "Legal"),
                mapping("tier", "gold", "Tier", "Gold"),
            ],
        ),
        (
            "the minimum of every mapping member, including an empty AwsTagValue",
            "hypervisor.example.com",
            // `IamRoleArn` is `@length(min: 20, max: 2048)`; this ARN is the shortest the
            // pattern `^arn:(aws|aws-cn|aws-us-gov):iam::([0-9]+):role/(\S+)$` allows at
            // that bound.
            "arn:aws:iam::1:role/r".to_string(),
            vec![mapping("c", "t", "k", "")],
        ),
        (
            "the maximum of every mapping member, in the aws-cn partition",
            "203.0.113.7",
            format!(
                "arn:aws-cn:iam::{}:role/{}",
                engine::EMULATOR_ACCOUNT_ID,
                "r".repeat(64)
            ),
            vec![mapping(
                &longest_category,
                &longest_tag_name,
                &longest_key,
                &longest_value,
            )],
        ),
        (
            "an empty mapping list set in the aws-us-gov partition",
            "203.0.113.8",
            format!(
                "arn:aws-us-gov:iam::{}:role/gov-mappings",
                engine::EMULATOR_ACCOUNT_ID
            ),
            vec![],
        ),
    ];

    for (index, (case, host, iam_role_arn, mappings)) in rows.iter().enumerate() {
        let arn = import(&router, &format!("hv-{index}"), host).await;
        set_mappings(
            &engine,
            &arn,
            Some(StoredPropertyMappings {
                iam_role_arn: iam_role_arn.clone(),
                vmware_to_aws_tag_mappings: mappings.clone(),
            }),
        );

        let reply = get_mappings(&router, &arn).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: a hypervisor carrying mappings must answer 200, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.str_at("/HypervisorArn"),
            Some(arn.as_str()),
            "case {case:?}: the reported ARN must be the record's own, got {:?}",
            reply.body
        );
        assert_eq!(
            reply.str_at("/IamRoleArn"),
            Some(iam_role_arn.as_str()),
            "case {case:?}: the reported role must be the stored one, got {:?}",
            reply.body
        );
        assert_eq!(
            reply
                .body
                .get("VmwareToAwsTagMappings")
                .cloned()
                .unwrap_or(Value::Null),
            expected_mappings(mappings),
            "case {case:?}: the reported mappings must be the stored list in order, got {:?}",
            reply.body
        );
    }

    // Each record answers from its OWN mappings: re-reading every ARN after all of them
    // were stored still reports that row's role, so no read is served from another
    // record.
    for (index, (case, host, iam_role_arn, _)) in rows.iter().enumerate() {
        let arn = service_state(&*engine)
            .expect("the in-memory engine must expose this state")
            .lock()
            .page_of_hypervisors(REGION, None, 100)
            .items
            .into_iter()
            .find(|record| record.host == *host)
            .map(|record| record.arn)
            .unwrap_or_else(|| panic!("case {case:?}: row {index} must be in the store"));
        let reply = get_mappings(&router, &arn).await;
        assert_eq!(
            (reply.status, reply.str_at("/IamRoleArn")),
            (200, Some(iam_role_arn.as_str())),
            "case {case:?}: each ARN must report its own mappings, got {} {:?}",
            reply.status,
            reply.body
        );
    }
}

/// A hypervisor the store really holds, carrying NO mappings, is refused as a missing
/// MAPPINGS resource: the modelled `ResourceNotFoundException` with `ErrorCode`
/// `HypervisorPropertyMappingsNotFound`, whatever else the record carries — and whether it
/// never had
/// mappings or no longer does.
#[tokio::test]
async fn a_hypervisor_with_no_mappings_is_refused_as_missing_mappings() {
    let engine = build_engine();
    let router = build_router(&engine);

    // (case, Host, whether mappings were set and then cleared, whether the activation is
    // already due). The rows differ in what the record carries BESIDE its mappings, which
    // the refusal keys on none of.
    let rows: [(&str, &str, bool, bool); 4] = [
        ("freshly imported, activation pending", "198.51.100.20", false, false),
        ("freshly imported, activation already due", "198.51.100.21", false, true),
        ("mappings set and then cleared", "198.51.100.22", true, false),
        ("an FQDN host", "hv.example.org", false, true),
    ];

    for (index, (case, host, cleared, online, )) in rows.iter().enumerate() {
        let arn = import(&router, &format!("hv-nomap-{index}"), host).await;
        if *cleared {
            set_mappings(
                &engine,
                &arn,
                Some(StoredPropertyMappings {
                    iam_role_arn: format!(
                        "arn:aws:iam::{}:role/temporary",
                        engine::EMULATOR_ACCOUNT_ID
                    ),
                    vmware_to_aws_tag_mappings: vec![mapping("c", "t", "k", "v")],
                }),
            );
            set_mappings(&engine, &arn, None);
        }
        if *online {
            service_state(&*engine)
                .expect("the in-memory engine must expose this state")
                .lock()
                .hypervisor_mut(&arn)
                .expect("the imported record must be in the store")
                .online_due = Some(SystemTime::now() - Duration::from_millis(1));
        }

        let reply = get_mappings(&router, &arn).await;
        assert_eq!(
            (
                reply.status,
                reply.code.as_deref(),
                reply.str_at("/ErrorCode"),
                reply.str_at("/Message"),
            ),
            (
                400,
                Some("ResourceNotFoundException"),
                Some(MAPPINGS_NOT_FOUND),
                Some(MAPPINGS_NOT_FOUND_MESSAGE),
            ),
            "case {case:?}: a hypervisor with no mappings must be refused as missing \
             mappings, got {} {:?}",
            reply.status,
            reply.body
        );

        // The refused read is still a read: it must not have invented mappings, and the
        // hypervisor itself still reads back.
        assert_eq!(
            persisted_mappings(&engine, &arn),
            "None",
            "case {case:?}: a refused read must not write mappings"
        );
        let hypervisor = call(&router, "GetHypervisor", &json!({ "HypervisorArn": arn })).await;
        assert_eq!(
            hypervisor.status, 200,
            "case {case:?}: the hypervisor itself must still read back, got {} {:?}",
            hypervisor.status, hypervisor.body
        );
        if *online {
            assert_eq!(
                hypervisor.str_at("/Hypervisor/State"),
                Some(HYPERVISOR_ONLINE),
                "case {case:?}: the record's own state must be untouched, got {:?}",
                hypervisor.body
            );
        }
    }
}

/// A well-formed `ServerArn` the store holds no hypervisor under is refused as a missing
/// HYPERVISOR — the modelled `ResourceNotFoundException` answered at HTTP 400 with
/// `ErrorCode` `HypervisorNotFound`, the same way every other hypervisor operation of
/// this service refuses one — whatever the ARN's resource type, partition, id or length.
#[tokio::test]
async fn an_arn_with_no_stored_hypervisor_is_refused_as_a_missing_hypervisor() {
    let engine = build_engine();
    let router = build_router(&engine);

    // A gateway and a hypervisor that really exist, so a refusal below is about the ARN
    // asked for and not about an empty store.
    let live_gateway = create(&router, "gw-live").await;
    let live_hypervisor = import(&router, "hv-live", "198.51.100.30").await;
    set_mappings(
        &engine,
        &live_hypervisor,
        Some(StoredPropertyMappings {
            iam_role_arn: format!("arn:aws:iam::{}:role/live", engine::EMULATOR_ACCOUNT_ID),
            vmware_to_aws_tag_mappings: vec![mapping("department", "finance", "Dept", "Fin")],
        }),
    );

    let rows: Vec<(&str, String)> = vec![
        (
            "a hypervisor ARN that was never imported",
            hypervisor_arn_in("aws", "hv-ffffffffffffff01"),
        ),
        (
            "a hypervisor ARN in the aws-cn partition",
            hypervisor_arn_in("aws-cn", "hv-ffffffffffffff02"),
        ),
        (
            "a hypervisor ARN in the aws-us-gov partition",
            hypervisor_arn_in("aws-us-gov", "hv-ffffffffffffff03"),
        ),
        (
            "a gateway ARN that really names a gateway",
            live_gateway.clone(),
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

    for (case, arn) in &rows {
        let reply = get_mappings(&router, arn).await;
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
            "case {case:?}: an ARN with no stored hypervisor must be refused as a missing \
             hypervisor, got {} {:?}",
            reply.status,
            reply.body
        );
    }

    // The record that does exist still reports its mappings, and a record the store no
    // longer holds is refused exactly like one it never held.
    let found = get_mappings(&router, &live_hypervisor).await;
    assert_eq!(
        (found.status, found.str_at("/HypervisorArn")),
        (200, Some(live_hypervisor.as_str())),
        "the stored hypervisor must still report its mappings, got {} {:?}",
        found.status,
        found.body
    );
    service_state(&*engine)
        .expect("the in-memory engine must expose this state")
        .lock()
        .remove_hypervisor(&live_hypervisor)
        .expect("the record must have been in the store");
    let gone = get_mappings(&router, &live_hypervisor).await;
    assert_eq!(
        (
            gone.status,
            gone.code.as_deref(),
            gone.str_at("/ErrorCode")
        ),
        (
            400,
            Some("ResourceNotFoundException"),
            Some(HYPERVISOR_NOT_FOUND)
        ),
        "a removed record must be refused like one that never existed, got {} {:?}",
        gone.status,
        gone.body
    );
}

/// A `HypervisorArn` outside the modelled constraints never reaches the lookup: it is
/// refused with HTTP 400 `ValidationException`, one past each published bound of
/// `ServerArn` (`@length(min: 50, max: 500)`, pattern
/// `^arn:(aws|aws-cn|aws-us-gov):backup-gateway(:[a-zA-Z-0-9]+){3}\/[a-zA-Z-0-9]+$`).
#[tokio::test]
async fn get_hypervisor_property_mappings_refuses_an_arn_outside_the_modelled_constraints() {
    let engine = build_engine();
    let router = build_router(&engine);

    let rows: Vec<(&str, Value)> = vec![
        (
            "one character under the 50-character minimum",
            json!({ "HypervisorArn": arn_of_length(49) }),
        ),
        (
            "one character over the 500-character maximum",
            json!({ "HypervisorArn": arn_of_length(501) }),
        ),
        (
            "a partition the pattern does not accept",
            json!({ "HypervisorArn": hypervisor_arn_in("aws-iso", "hv-0000000000000003") }),
        ),
        (
            "another service's ARN",
            json!({ "HypervisorArn": format!(
                "arn:aws:backup:{REGION}:{}:hypervisor/hv-0000000000000004",
                engine::EMULATOR_ACCOUNT_ID
            ) }),
        ),
        ("no HypervisorArn at all", json!({})),
    ];

    for (case, body) in &rows {
        let reply = call(&router, "GetHypervisorPropertyMappings", body).await;
        assert_eq!(
            (reply.status, reply.code.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: an ARN outside the modelled constraints must be refused with \
             400 ValidationException, got {} {:?}",
            reply.status,
            reply.body
        );
    }

    assert_eq!(
        service_state(&*engine)
            .expect("the in-memory engine must expose this state")
            .lock()
            .hypervisor_count(),
        0,
        "a refused request must not have created anything"
    );
}

/// Reading the mappings changes nothing a later caller sees: repeated reads agree, the
/// persisted record is untouched, and the read neither moves the hypervisor's state nor
/// adds or drops a record.
#[tokio::test]
async fn reading_the_mappings_never_changes_what_a_later_caller_sees() {
    let engine = build_engine();
    let router = build_router(&engine);

    let arn = import(&router, "hv-stable", "198.51.100.40").await;
    let stored = StoredPropertyMappings {
        iam_role_arn: format!(
            "arn:aws:iam::{}:role/stable-mappings",
            engine::EMULATOR_ACCOUNT_ID
        ),
        vmware_to_aws_tag_mappings: vec![
            mapping("department", "finance", "Department", "Finance"),
            mapping("tier", "gold", "Tier", "Gold"),
        ],
    };
    set_mappings(&engine, &arn, Some(stored.clone()));
    let before = persisted_mappings(&engine, &arn);

    let mut bodies = Vec::new();
    for read in 0..3 {
        let reply = get_mappings(&router, &arn).await;
        assert_eq!(
            reply.status, 200,
            "read {read}: the stored mappings must read back, got {} {:?}",
            reply.status, reply.body
        );
        bodies.push(reply.body);
    }
    assert!(
        bodies.windows(2).all(|pair| pair[0] == pair[1]),
        "repeated reads must agree, got {bodies:?}"
    );
    assert_eq!(
        persisted_mappings(&engine, &arn),
        before,
        "a read must leave the persisted mappings exactly as they were"
    );
    assert_eq!(
        service_state(&*engine)
            .expect("the in-memory engine must expose this state")
            .lock()
            .hypervisor_count(),
        1,
        "a read must not add or drop a record"
    );

    // And the mappings a later write replaces are what every later read reports: the
    // response follows the store, not the first answer.
    let replacement = StoredPropertyMappings {
        iam_role_arn: format!(
            "arn:aws:iam::{}:role/replacement",
            engine::EMULATOR_ACCOUNT_ID
        ),
        vmware_to_aws_tag_mappings: vec![mapping("tier", "silver", "Tier", "Silver")],
    };
    set_mappings(&engine, &arn, Some(replacement.clone()));
    let reply = get_mappings(&router, &arn).await;
    assert_eq!(
        (reply.status, reply.str_at("/IamRoleArn")),
        (200, Some(replacement.iam_role_arn.as_str())),
        "a read must report the mappings the store now holds, got {} {:?}",
        reply.status,
        reply.body
    );
    assert_eq!(
        reply
            .body
            .get("VmwareToAwsTagMappings")
            .cloned()
            .unwrap_or(Value::Null),
        expected_mappings(&replacement.vmware_to_aws_tag_mappings),
        "a read must report the mappings the store now holds, got {:?}",
        reply.body
    );
}
