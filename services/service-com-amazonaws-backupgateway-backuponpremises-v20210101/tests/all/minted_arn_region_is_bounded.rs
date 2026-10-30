// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the REGION segment of the ARNs this service mints, whatever region
//! the request was signed for.
//!
//! Every case drives real requests through the `MultiServiceRouter` — the harness of
//! `gateway_lifecycle` — so what is asserted is the wire behaviour, and the region a request
//! carries is the one its SigV4 credential scope names, exactly as a client addressing a
//! regional endpoint would.
//!
//! The rule under test: an ARN this service mints is one its OWN operations can resolve, so
//! every minted ARN satisfies the modelled constraints of the shapes that carry it — the
//! character class AND the length of `GatewayArn` (len 50..180) and of
//! `ServerArn`/`ResourceArn` (len 50..500)
//! (`smithy_query(query="shape",shape="GatewayArn,ServerArn,ResourceArn")`). The one segment
//! a request can grow is the region, which arrives caller-supplied on the credential scope
//! and is bounded nowhere else, so a region that cannot sit inside a modelled ARN is not
//! minted into one: the ARN carries the emulator's fallback region instead, and the record
//! stays readable, taggable, updatable and — the part that matters — deletable through the
//! operations that take its ARN.
//!
//! The tables below span the class the rule generalises over: regions differing in LENGTH
//! (one character, an ordinary name, exactly the bound, one past it, far past it) and in
//! spelling (a character no ARN segment may carry), asserted on all three resource types
//! this service mints an ARN for — the gateway the activation handshake leads to, the
//! hypervisor an import mints, and the virtual machine an inventory report mints.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::SystemTime;

use serde_json::{json, Value};

use service_com_amazonaws_backupgateway_backuponpremises_v20210101 as backup_gateway;

use backup_gateway::state::{
    service_state, VirtualMachineInventory, DEFAULT_REGION, MAX_ARN_REGION_LENGTH,
};

use crate::gateway_lifecycle::{
    activation_handshake, build_engine, build_router, call_in_region, create_body, gateway_count,
    hypervisor_count, Reply, REGION,
};

/// The modelled length bounds of the ARN shapes this service's operations take, spelled out
/// here rather than read off the crate, so an assertion is about the MODEL and not about the
/// code that mints the ARN: `GatewayArn` `@length(min: 50, max: 180)` and
/// `ServerArn`/`ResourceArn` `@length(min: 50, max: 500)`
/// (`smithy_query(query="shape",shape="GatewayArn,ServerArn,ResourceArn")`).
const ARN_MIN_LENGTH: usize = 50;
const GATEWAY_ARN_MAX_LENGTH: usize = 180;
const SERVER_ARN_MAX_LENGTH: usize = 500;

/// One region a request is signed for.
#[derive(Debug, Clone)]
struct Row {
    case: &'static str,
    /// The region named by the request's SigV4 credential scope.
    signed_region: String,
    /// Whether an ARN minted for such a request may carry that region VERBATIM: a region
    /// that cannot sit inside a modelled ARN cannot, so the mint falls back.
    kept_verbatim: bool,
}

impl Row {
    /// The region segment an ARN minted for this row must carry.
    fn minted_region(&self) -> String {
        if self.kept_verbatim {
            self.signed_region.clone()
        } else {
            DEFAULT_REGION.to_string()
        }
    }
}

/// The regions under test: the lengths the rule generalises over (one character, an ordinary
/// name, the bound itself, one past it, far past it) and a spelling no ARN segment accepts.
///
/// The rows past the bound are what the modelled maximum is at stake for: an ARN built
/// around a region of 434 characters or more cannot satisfy `ServerArn`'s 500 at all, and one
/// around 144 characters already cannot satisfy `GatewayArn`'s 180.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "an ordinary region name",
            signed_region: REGION.to_string(),
            kept_verbatim: true,
        },
        Row {
            case: "the emulator's own fallback region, asked for by name",
            signed_region: DEFAULT_REGION.to_string(),
            kept_verbatim: true,
        },
        Row {
            case: "a region of a single character",
            signed_region: "a".to_string(),
            kept_verbatim: true,
        },
        Row {
            case: "a region of exactly the bound",
            signed_region: "r".repeat(MAX_ARN_REGION_LENGTH),
            kept_verbatim: true,
        },
        Row {
            case: "a region one character past the bound",
            signed_region: "s".repeat(MAX_ARN_REGION_LENGTH + 1),
            kept_verbatim: false,
        },
        Row {
            case: "a region long enough to push a gateway ARN past the modelled 180",
            signed_region: "t".repeat(200),
            kept_verbatim: false,
        },
        Row {
            case: "a region long enough to push a hypervisor ARN past the modelled 500",
            signed_region: "u".repeat(512),
            kept_verbatim: false,
        },
        Row {
            case: "a region far longer than any endpoint name",
            signed_region: "v".repeat(4_096),
            kept_verbatim: false,
        },
        Row {
            case: "a region spelled with a character no ARN segment may carry",
            signed_region: "us_west_2".to_string(),
            kept_verbatim: false,
        },
    ]
}

/// Whether `arn` satisfies the modelled ARN form of this service — the pattern
/// `^arn:(aws|aws-cn|aws-us-gov):backup-gateway(:[a-zA-Z-0-9]+){3}\/[a-zA-Z-0-9]+$` with
/// `@length(min: 50, max: max_len)` — spelled out here rather than reused from the crate, so
/// the assertion is independent of the code that mints the ARN.
fn satisfies_modelled_arn(arn: &str, max_len: usize) -> bool {
    if arn.len() < ARN_MIN_LENGTH || arn.len() > max_len {
        return false;
    }
    let segment = |s: &str| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-')
    };
    let fields: Vec<&str> = arn.split(':').collect();
    let [literal, partition, service, region, account, resource] = fields.as_slice() else {
        return false;
    };
    let Some((resource_type, id)) = resource.split_once('/') else {
        return false;
    };
    *literal == "arn"
        && matches!(*partition, "aws" | "aws-cn" | "aws-us-gov")
        && *service == "backup-gateway"
        && [*region, *account, resource_type, id]
            .into_iter()
            .all(segment)
}

/// The region segment of an ARN of this service.
fn region_of(arn: &str) -> &str {
    arn.split(':').nth(3).unwrap_or_default()
}

/// The ARNs a listing reply carries under `list_member`/`arn_member`.
fn listed_arns(reply: &Reply, list_member: &str, arn_member: &str) -> Vec<String> {
    reply
        .body
        .get(list_member)
        .and_then(Value::as_array)
        .map(|records| {
            records
                .iter()
                .map(|record| {
                    record
                        .get(arn_member)
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Report one machine on `hypervisor_arn` as the hypervisor's inventory would, in `region` —
/// the out-of-band channel a virtual machine reaches this service through — and hand back the
/// ARN the store minted for it.
fn record_machine(
    engine: &Arc<engine::InMemoryEngine>,
    case: &str,
    region: &str,
    hypervisor_arn: &str,
) -> String {
    let state = service_state(&**engine).expect("the in-memory engine must expose this state");
    let mut store = state.lock();
    store
        .record_virtual_machine(
            region,
            hypervisor_arn,
            VirtualMachineInventory {
                name: "vm-minted".to_string(),
                host_name: "vm-minted".to_string(),
                path: "/Datacenter/vm/vm-minted".to_string(),
                last_backup_date: None,
                vmware_tags: Vec::new(),
            },
            SystemTime::now(),
        )
        .unwrap_or_else(|| panic!("case {case:?}: the store must record a machine"))
        .arn
}

/// Every hypervisor an import mints is minted under an ARN the operations that TAKE one can
/// resolve: it reads back, lists, tags, updates and deletes, whatever region the import was
/// signed for — and the virtual machines of that hypervisor are minted the same way.
#[tokio::test]
async fn an_imported_hypervisor_is_resolvable_whatever_region_was_signed_for() {
    let engine = build_engine();
    let router = build_router(&engine);

    let mut minted = BTreeSet::new();
    for row in rows() {
        let case = row.case;
        let signed = row.signed_region.as_str();
        let expected_region = row.minted_region();

        let imported = call_in_region(
            &router,
            signed,
            "ImportHypervisorConfiguration",
            &json!({ "Name": "hv-minted", "Host": "198.51.100.7" }),
        )
        .await;
        assert_eq!(
            imported.status, 200,
            "case {case:?}: ImportHypervisorConfiguration must answer 200, got {} {:?}",
            imported.status, imported.body
        );
        let arn = imported
            .str_at("/HypervisorArn")
            .unwrap_or_else(|| panic!("case {case:?}: no HypervisorArn in {:?}", imported.body))
            .to_string();
        assert!(
            satisfies_modelled_arn(&arn, SERVER_ARN_MAX_LENGTH),
            "case {case:?}: the minted ARN must satisfy the modelled ServerArn constraints \
             (len {ARN_MIN_LENGTH}..{SERVER_ARN_MAX_LENGTH}), got {} characters: {arn:?}",
            arn.len()
        );
        assert_eq!(
            region_of(&arn),
            expected_region,
            "case {case:?}: the minted ARN must carry {expected_region:?} as its region, got \
             {arn:?}"
        );
        assert!(
            minted.insert(arn.clone()),
            "case {case:?}: the ARN {arn:?} was already minted for another import"
        );

        // The record is really in the store, and this endpoint's listing reports it.
        assert_eq!(
            hypervisor_count(&engine),
            1,
            "case {case:?}: the import must store exactly one hypervisor, got {}",
            hypervisor_count(&engine)
        );
        let listed = call_in_region(&router, signed, "ListHypervisors", &json!({})).await;
        assert_eq!(
            (
                listed.status,
                listed_arns(&listed, "Hypervisors", "HypervisorArn")
            ),
            (200, vec![arn.clone()]),
            "case {case:?}: ListHypervisors must report the imported hypervisor, got {} {:?}",
            listed.status,
            listed.body
        );

        // Every operation that TAKES the ARN resolves it: a read, a tag write, a tag read
        // and an update, all signed for the same region the import was.
        let fetched = call_in_region(
            &router,
            signed,
            "GetHypervisor",
            &json!({ "HypervisorArn": arn }),
        )
        .await;
        assert_eq!(
            (
                fetched.status,
                fetched.str_at("/Hypervisor/HypervisorArn"),
                fetched.str_at("/Hypervisor/Host"),
                fetched.str_at("/Hypervisor/Name"),
            ),
            (
                200,
                Some(arn.as_str()),
                Some("198.51.100.7"),
                Some("hv-minted")
            ),
            "case {case:?}: GetHypervisor must report the imported record, got {} {:?}",
            fetched.status,
            fetched.body
        );

        let tagged = call_in_region(
            &router,
            signed,
            "TagResource",
            &json!({
                "ResourceARN": arn,
                "Tags": [{ "Key": "env", "Value": "test" }],
            }),
        )
        .await;
        assert_eq!(
            tagged.status, 200,
            "case {case:?}: TagResource must answer 200 for a minted ARN, got {} {:?}",
            tagged.status, tagged.body
        );
        let tag_read = call_in_region(
            &router,
            signed,
            "ListTagsForResource",
            &json!({ "ResourceArn": arn }),
        )
        .await;
        assert_eq!(
            (tag_read.status, tag_read.tags()),
            (
                200,
                [("env".to_string(), "test".to_string())]
                    .into_iter()
                    .collect::<BTreeMap<String, String>>()
            ),
            "case {case:?}: ListTagsForResource must report the written tag, got {} {:?}",
            tag_read.status,
            tag_read.body
        );

        let updated = call_in_region(
            &router,
            signed,
            "UpdateHypervisor",
            &json!({ "HypervisorArn": arn, "Name": "hv-renamed" }),
        )
        .await;
        assert_eq!(
            (updated.status, updated.str_at("/HypervisorArn")),
            (200, Some(arn.as_str())),
            "case {case:?}: UpdateHypervisor must answer 200 for a minted ARN, got {} {:?}",
            updated.status,
            updated.body
        );

        // A machine of that hypervisor is minted under the same rule, and resolves too.
        let machine_arn = record_machine(&engine, case, signed, &arn);
        assert!(
            satisfies_modelled_arn(&machine_arn, SERVER_ARN_MAX_LENGTH),
            "case {case:?}: a minted virtual-machine ARN must satisfy the modelled \
             ResourceArn constraints (len {ARN_MIN_LENGTH}..{SERVER_ARN_MAX_LENGTH}), got {} \
             characters: {machine_arn:?}",
            machine_arn.len()
        );
        assert_eq!(
            region_of(&machine_arn),
            expected_region,
            "case {case:?}: a minted virtual-machine ARN must carry {expected_region:?} as \
             its region, got {machine_arn:?}"
        );
        let machine = call_in_region(
            &router,
            signed,
            "GetVirtualMachine",
            &json!({ "ResourceArn": machine_arn }),
        )
        .await;
        assert_eq!(
            (
                machine.status,
                machine.str_at("/VirtualMachine/ResourceArn")
            ),
            (200, Some(machine_arn.as_str())),
            "case {case:?}: GetVirtualMachine must report the recorded machine, got {} {:?}",
            machine.status,
            machine.body
        );

        // The part that matters: the record can be DELETED again, so it does not occupy a
        // store slot for the life of the process.
        let deleted = call_in_region(
            &router,
            signed,
            "DeleteHypervisor",
            &json!({ "HypervisorArn": arn }),
        )
        .await;
        assert_eq!(
            (deleted.status, deleted.str_at("/HypervisorArn")),
            (200, Some(arn.as_str())),
            "case {case:?}: DeleteHypervisor must remove the imported record, got {} {:?}",
            deleted.status,
            deleted.body
        );
        assert_eq!(
            hypervisor_count(&engine),
            0,
            "case {case:?}: the delete must leave the store empty, got {}",
            hypervisor_count(&engine)
        );
        let gone = call_in_region(
            &router,
            signed,
            "GetHypervisor",
            &json!({ "HypervisorArn": arn }),
        )
        .await;
        assert_ne!(
            gone.status, 200,
            "case {case:?}: a deleted hypervisor must not read back, got {} {:?}",
            gone.status, gone.body
        );
    }
}

/// The gateway path mints under the same rule: the activation handshake issues a key only
/// for a region an ARN can carry, so every `GatewayArn` a create mints stays inside the
/// modelled 180 and resolves through the operations that take one.
#[tokio::test]
async fn a_created_gateway_is_resolvable_and_an_unusable_region_issues_no_key() {
    let engine = build_engine();
    let router = build_router(&engine);

    let mut created = 0usize;
    for row in rows() {
        let case = row.case;
        let signed = row.signed_region.as_str();
        let (status, body) =
            activation_handshake(&router, &format!("activationRegion={signed}")).await;

        if !row.kept_verbatim {
            // No key, so no gateway can be minted under an ARN its own API would refuse.
            assert_eq!(
                status, 400,
                "case {case:?}: a handshake for a region no ARN may carry must be refused, \
                 got {status} {body:?}"
            );
            assert!(
                body.contains(&MAX_ARN_REGION_LENGTH.to_string())
                    && body.contains("letters, digits and hyphens"),
                "case {case:?}: the refusal must state what a region looks like — the bound \
                 and the characters a region is spelled with — got {body:?}"
            );
            continue;
        }

        assert_eq!(
            status, 200,
            "case {case:?}: the handshake must issue a key, got {status} {body:?}"
        );
        let reply = call_in_region(
            &router,
            signed,
            "CreateGateway",
            &create_body("gw-minted", &body, &BTreeMap::new()),
        )
        .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: CreateGateway must answer 200, got {} {:?}",
            reply.status, reply.body
        );
        let arn = reply.gateway_arn().to_string();
        assert!(
            satisfies_modelled_arn(&arn, GATEWAY_ARN_MAX_LENGTH),
            "case {case:?}: the minted ARN must satisfy the modelled GatewayArn constraints \
             (len {ARN_MIN_LENGTH}..{GATEWAY_ARN_MAX_LENGTH}), got {} characters: {arn:?}",
            arn.len()
        );
        assert_eq!(
            region_of(&arn),
            row.minted_region(),
            "case {case:?}: the minted ARN must carry the region the create was addressed \
             to, got {arn:?}"
        );

        let fetched = call_in_region(&router, signed, "GetGateway", &json!({ "GatewayArn": arn }))
            .await;
        assert_eq!(
            (fetched.status, fetched.str_at("/Gateway/GatewayArn")),
            (200, Some(arn.as_str())),
            "case {case:?}: GetGateway must report the created gateway, got {} {:?}",
            fetched.status,
            fetched.body
        );

        let deleted =
            call_in_region(&router, signed, "DeleteGateway", &json!({ "GatewayArn": arn })).await;
        assert_eq!(
            (deleted.status, deleted.str_at("/GatewayArn")),
            (200, Some(arn.as_str())),
            "case {case:?}: DeleteGateway must remove the created gateway, got {} {:?}",
            deleted.status,
            deleted.body
        );
        created += 1;
        assert_eq!(
            gateway_count(&engine),
            0,
            "case {case:?}: the delete must leave no gateway in the store, got {}",
            gateway_count(&engine)
        );
    }

    assert!(
        created > 0,
        "the table must create at least one gateway for this test to mean anything, got \
         {created}"
    );
}
