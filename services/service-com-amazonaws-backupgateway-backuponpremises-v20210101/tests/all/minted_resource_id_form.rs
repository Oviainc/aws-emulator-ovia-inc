// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the FORM of the resource id this service mints: the `<id>` behind
//! the `<type>/` of `arn:aws:backup-gateway:<region>:<account>:<type>/<id>`.
//!
//! Every case drives a real `POST /` awsJson1_0 request through the `MultiServiceRouter`
//! — the harness of `gateway_lifecycle` — so what is asserted is the wire behaviour.
//!
//! The rule under test: a minted id is rendered in the identifier form of the RESOURCE
//! KIND it names — that kind's id prefix followed by a body of that kind's own fixed width
//! in lowercase hexadecimal — and not in one form shared by every resource of the service.
//! So the id a client parses out of an identity this service hands it is recognisable as an
//! id of that resource and of no other, on every surface that reports the identity or the
//! bare id inside it.
//!
//! The model fixes only the ARN around the id — `ServerArn` and `VirtualMachine$ResourceArn`
//! are `@length(min: 50, max: 500)`, `GatewayArn` is `@length(min: 50, max: 180)`, all three
//! with pattern `^arn:(aws|aws-cn|aws-us-gov):backup-gateway(:[a-zA-Z-0-9]+){3}\/[a-zA-Z-0-9]+$`
//! (`smithy_query(query="shape",shape="ServerArn,GatewayArn,VirtualMachine")`) — so the
//! per-kind forms are tabled in [`MINTED_KINDS`], each row naming where its form comes from.
//!
//! The table spans the class the rule generalises over: every kind this service mints an
//! identity for (not only the hypervisor), several successive mints of each so a row differs
//! in the sequence behind its body rather than being the first of a run, in two regions, and
//! each minted id checked against EVERY kind's form — so an id of one kind never passes for
//! another kind's — then read back through every surface that names the record.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::SystemTime;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_backupgateway_backuponpremises_v20210101 as backup_gateway;

use backup_gateway::state::{service_state, VirtualMachineInventory};

use crate::gateway_lifecycle::{
    build_engine, build_router, call_in_region, create_body, issued_activation_key_for, Reply,
    REGION,
};

/// A second region to mint in, so a row differs in the region whose sequence an identity
/// comes from.
const OTHER_REGION: &str = "eu-central-1";

/// How many identities of each kind every region mints: enough that a row is not the first
/// mint of a run, so the body of an id is seen over successive sequences.
const MINTS_PER_REGION: usize = 3;

/// One resource kind this service mints an identity for, and the identifier form its id is
/// minted in.
#[derive(Debug, Clone, Copy)]
struct MintedKind {
    case: &'static str,
    /// The ARN resource type: the `hypervisor` of `hypervisor/hype-1a2b3c4d`.
    resource_type: &'static str,
    /// The prefix the id of that kind carries.
    id_prefix: &'static str,
    /// How many hexadecimal digits the id carries behind its prefix.
    id_digits: usize,
    /// The `@length` ceiling of the member that reports an identity of this kind
    /// (`GatewayArn` 180, `ServerArn` and `VirtualMachine$ResourceArn` 500).
    arn_max_len: usize,
}

/// A gateway's id form: `gateway/bgw-<16 hex>`. Nothing published pins the width of a
/// gateway id and no recorded response of this service diverges on one, so this is the form
/// the store has always minted.
const GATEWAY: MintedKind = MintedKind {
    case: "a gateway created by CreateGateway",
    resource_type: "gateway",
    id_prefix: "bgw-",
    id_digits: 16,
    arn_max_len: 180,
};

/// A hypervisor's id form: `hypervisor/hype-<8 hex>` — the identity form a hypervisor of
/// this service is served with on the wire, which the modelled `ServerArn` pattern permits
/// but does not state.
const HYPERVISOR: MintedKind = MintedKind {
    case: "a hypervisor imported by ImportHypervisorConfiguration",
    resource_type: "hypervisor",
    id_prefix: "hype-",
    id_digits: 8,
    arn_max_len: 500,
};

/// A virtual machine's id form: `vm/vm-<16 hex>` — a body of the same order as the one the
/// modelled `VirtualMachine$ResourceArn` doc illustrates
/// (`arn:aws:backup-gateway:us-west-1:0000000000000:vm/vm-0000ABCDEFGIJKL`, thirteen
/// characters behind the prefix), which is all that is published about a machine id.
const VIRTUAL_MACHINE: MintedKind = MintedKind {
    case: "a virtual machine recorded from a hypervisor's inventory",
    resource_type: "vm",
    id_prefix: "vm-",
    id_digits: 16,
    arn_max_len: 500,
};

/// Every kind this service mints an identity for.
const MINTED_KINDS: [MintedKind; 3] = [GATEWAY, HYPERVISOR, VIRTUAL_MACHINE];

/// The `<type>` and `<id>` of an ARN of this service, or `None` for anything that is not
/// the six colon-separated fields of an ARN with a `<type>/<id>` resource.
///
/// Spelled out here rather than taken from the crate, so every assertion below is
/// independent of the code that mints the ARN.
fn resource_of(arn: &str) -> Option<(&str, &str)> {
    let fields: Vec<&str> = arn.split(':').collect();
    let [literal, _partition, service, _region, _account, resource] = fields.as_slice() else {
        return None;
    };
    if *literal != "arn" || *service != "backup-gateway" {
        return None;
    }
    resource.split_once('/')
}

/// Whether `arn` satisfies the modelled form of an identity of this service, with the
/// `@length` ceiling of the member that carries it.
fn matches_modelled_arn(arn: &str, max_len: usize) -> bool {
    let Some((resource_type, id)) = resource_of(arn) else {
        return false;
    };
    let fields: Vec<&str> = arn.split(':').collect();
    (50..=max_len).contains(&arn.len())
        && matches!(fields[1], "aws" | "aws-cn" | "aws-us-gov")
        && [fields[3], fields[4], resource_type, id]
            .into_iter()
            .all(|segment| {
                !segment.is_empty()
                    && segment
                        .chars()
                        .all(|character| character.is_ascii_alphanumeric() || character == '-')
            })
}

/// Whether `id` is an id minted in `kind`'s form: that kind's prefix and exactly that
/// kind's number of lowercase hexadecimal digits behind it.
fn matches_id_form(kind: &MintedKind, id: &str) -> bool {
    id.strip_prefix(kind.id_prefix).is_some_and(|body| {
        body.len() == kind.id_digits
            && body
                .chars()
                .all(|digit| digit.is_ascii_digit() || ('a'..='f').contains(&digit))
    })
}

/// One minted identity: the kind it names and the ARN the service handed out for it.
#[derive(Debug, Clone)]
struct Minted {
    kind: MintedKind,
    region: &'static str,
    arn: String,
}

/// The `HypervisorArn` a successful import handed back.
fn hypervisor_arn(reply: &Reply) -> String {
    reply
        .str_at("/HypervisorArn")
        .unwrap_or_else(|| panic!("a successful import must carry a HypervisorArn: {:?}", reply.body))
        .to_string()
}

/// Import a hypervisor in `region`, over the wire.
async fn import(router: &MultiServiceRouter, region: &str, index: usize) -> String {
    let reply = call_in_region(
        router,
        region,
        "ImportHypervisorConfiguration",
        &json!({
            "Name": format!("hv-{index}"),
            // A record stands for the appliance at its host, so each gets its own.
            "Host": format!("{region}-hv-{index}.example.internal"),
        }),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "case import {region}/{index}: ImportHypervisorConfiguration must answer 200, got {} \
         {:?}",
        reply.status, reply.body
    );
    hypervisor_arn(&reply)
}

/// Create a gateway in `region`, over the wire, through the activation handshake a create
/// resolves its key against.
async fn create_gateway(router: &MultiServiceRouter, region: &str, index: usize) -> String {
    let activation_key = issued_activation_key_for(router, region).await;
    let reply = call_in_region(
        router,
        region,
        "CreateGateway",
        &create_body(&format!("gw-{index}"), &activation_key, &BTreeMap::new()),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "case create {region}/{index}: CreateGateway must answer 200, got {} {:?}",
        reply.status, reply.body
    );
    reply.gateway_arn().to_string()
}

/// Record `count` machines on `hypervisor_arn` and hand back the ARNs the store minted.
///
/// A virtual machine is minted by no operation of this model: it reaches the service as part
/// of the inventory of the hypervisor it runs on, which is the channel
/// `record_virtual_machine` stands for.
fn record_machines(
    engine: &Arc<engine::InMemoryEngine>,
    region: &str,
    hypervisor_arn: &str,
    count: usize,
) -> Vec<String> {
    let state = service_state(&**engine).expect("the in-memory engine must expose this state");
    let mut store = state.lock();
    (0..count)
        .map(|index| {
            store
                .record_virtual_machine(
                    region,
                    hypervisor_arn,
                    VirtualMachineInventory {
                        name: format!("vm-{index}"),
                        host_name: format!("vm-{index}"),
                        path: format!("/Datacenter/vm/{region}/{index}"),
                        last_backup_date: None,
                        vmware_tags: Vec::new(),
                    },
                    SystemTime::now(),
                )
                .unwrap_or_else(|| {
                    panic!("case machine {region}/{index}: the store must record a machine")
                })
                .arn
        })
        .collect()
}

/// Mint [`MINTS_PER_REGION`] identities of every kind in `region`.
async fn mint_every_kind_in(
    engine: &Arc<engine::InMemoryEngine>,
    router: &MultiServiceRouter,
    region: &'static str,
) -> Vec<Minted> {
    let mut minted = Vec::new();
    for index in 0..MINTS_PER_REGION {
        minted.push(Minted {
            kind: GATEWAY,
            region,
            arn: create_gateway(router, region, index).await,
        });
    }
    let mut hypervisors = Vec::new();
    for index in 0..MINTS_PER_REGION {
        let arn = import(router, region, index).await;
        hypervisors.push(arn.clone());
        minted.push(Minted {
            kind: HYPERVISOR,
            region,
            arn,
        });
    }
    let host = hypervisors
        .first()
        .expect("the mint imported a hypervisor")
        .clone();
    for arn in record_machines(engine, region, &host, MINTS_PER_REGION) {
        minted.push(Minted {
            kind: VIRTUAL_MACHINE,
            region,
            arn,
        });
    }
    minted
}

/// RULE: every identity this service mints carries the id form of the kind it names — that
/// kind's prefix and that kind's width of lowercase hex digits — inside an ARN the model
/// accepts; the id of one kind matches no other kind's form; and each mint of a region gets
/// its own id.
#[tokio::test]
async fn every_minted_identity_carries_the_id_form_of_its_resource_kind() {
    let engine = build_engine();
    let router = build_router(&engine);

    let mut minted = mint_every_kind_in(&engine, &router, REGION).await;
    minted.extend(mint_every_kind_in(&engine, &router, OTHER_REGION).await);
    assert_eq!(
        minted.len(),
        2 * MINTS_PER_REGION * MINTED_KINDS.len(),
        "case table: every kind must have been minted in both regions, got {minted:?}"
    );

    // Every (region, id) pair handed out, so a repeat inside a region shows up: a bare id is
    // scoped to the region of the identity that carries it.
    let mut regional_ids: BTreeSet<(&str, String)> = BTreeSet::new();
    // The bodies minted per (region, kind), which must differ from one another.
    let mut bodies: BTreeMap<(&str, &str), BTreeSet<String>> = BTreeMap::new();

    for entry in &minted {
        let Minted { kind, region, arn } = entry;
        let case = kind.case;
        assert!(
            matches_modelled_arn(arn, kind.arn_max_len),
            "case {case:?} in {region}: the minted identity must satisfy the modelled ARN \
             (50..={} characters, the ARN pattern), got {arn:?}",
            kind.arn_max_len
        );
        let (resource_type, id) = resource_of(arn).unwrap_or_else(|| {
            panic!("case {case:?} in {region}: {arn:?} is not an ARN of this service at all")
        });
        assert_eq!(
            resource_type, kind.resource_type,
            "case {case:?} in {region}: the identity must name its own resource type, got \
             {arn:?}"
        );
        assert!(
            matches_id_form(kind, id),
            "case {case:?} in {region}: the minted id must be the {:?} prefix and {} \
             lowercase hex digits, got {id:?}",
            kind.id_prefix,
            kind.id_digits
        );
        // The other half of the rule: a form belongs to ONE kind, so the id of this kind is
        // not an id any other kind would have been minted under.
        for other in MINTED_KINDS
            .iter()
            .filter(|other| other.resource_type != kind.resource_type)
        {
            assert!(
                !matches_id_form(other, id),
                "case {case:?} in {region}: the id {id:?} must not also be of the {:?} form \
                 ({:?} and {} digits)",
                other.case,
                other.id_prefix,
                other.id_digits
            );
        }
        assert!(
            regional_ids.insert((region, id.to_string())),
            "case {case:?} in {region}: the service minted the id {id:?} twice"
        );
        let body = id
            .strip_prefix(kind.id_prefix)
            .expect("the id carries its kind's prefix")
            .to_string();
        assert!(
            bodies
                .entry((region, kind.resource_type))
                .or_default()
                .insert(body.clone()),
            "case {case:?} in {region}: the body {body:?} was minted twice"
        );
    }

    for ((region, resource_type), minted_bodies) in &bodies {
        assert_eq!(
            minted_bodies.len(),
            MINTS_PER_REGION,
            "case {resource_type} in {region}: every mint must carry its own body, got \
             {minted_bodies:?}"
        );
    }
}

/// RULE (the reporting half): the id the mint rendered is what every surface of this service
/// reports — the identity a read echoes, and the BARE id a gateway and a virtual machine
/// name their hypervisor by — so a client parsing an id off any of them sees one form.
#[tokio::test]
async fn every_surface_reports_the_minted_id_in_its_kinds_form() {
    let engine = build_engine();
    let router = build_router(&engine);

    let hypervisor = import(&router, REGION, 0).await;
    let (_, hypervisor_id) =
        resource_of(&hypervisor).expect("a minted identity is an ARN of this service");
    let gateway = create_gateway(&router, REGION, 0).await;
    let machines = record_machines(&engine, REGION, &hypervisor, 1);

    let associated = call_in_region(
        &router,
        REGION,
        "AssociateGatewayToServer",
        &json!({ "GatewayArn": gateway, "ServerArn": hypervisor }),
    )
    .await;
    assert_eq!(
        associated.status, 200,
        "AssociateGatewayToServer must answer 200, got {} {:?}",
        associated.status, associated.body
    );

    // The hypervisor read reports the identity it was minted under, unchanged.
    let read = call_in_region(
        &router,
        REGION,
        "GetHypervisor",
        &json!({ "HypervisorArn": hypervisor }),
    )
    .await;
    assert_eq!(
        (read.status, read.str_at("/Hypervisor/HypervisorArn")),
        (200, Some(hypervisor.as_str())),
        "GetHypervisor must report the minted identity, got {} {:?}",
        read.status,
        read.body
    );

    // The gateway reports its hypervisor by the BARE id — the id segment of that identity,
    // in the hypervisor's own form.
    let fetched = call_in_region(
        &router,
        REGION,
        "GetGateway",
        &json!({ "GatewayArn": gateway }),
    )
    .await;
    assert_eq!(
        (fetched.status, fetched.str_at("/Gateway/HypervisorId")),
        (200, Some(hypervisor_id)),
        "GetGateway must report the hypervisor's minted id, got {} {:?}",
        fetched.status,
        fetched.body
    );
    assert!(
        matches_id_form(&HYPERVISOR, hypervisor_id),
        "the bare HypervisorId must carry the hypervisor id form, got {hypervisor_id:?}"
    );

    // And a machine of that hypervisor's inventory reports its own identity in the machine
    // form and its hypervisor by the same bare id.
    let listed = call_in_region(
        &router,
        REGION,
        "ListVirtualMachines",
        &json!({ "HypervisorArn": hypervisor }),
    )
    .await;
    assert_eq!(
        listed.status, 200,
        "ListVirtualMachines must answer 200, got {} {:?}",
        listed.status, listed.body
    );
    let records = listed
        .body
        .get("VirtualMachines")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        records.len(),
        machines.len(),
        "the listing must report the recorded machines, got {:?}",
        listed.body
    );
    for record in &records {
        let arn = record
            .get("ResourceArn")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            machines.iter().any(|minted| minted == arn),
            "the listing must report the identity the store minted, got {record:?}"
        );
        let (resource_type, id) = resource_of(arn)
            .unwrap_or_else(|| panic!("{arn:?} is not an ARN of this service at all"));
        assert_eq!(
            resource_type, VIRTUAL_MACHINE.resource_type,
            "a machine's identity must name the machine resource type, got {arn:?}"
        );
        assert!(
            matches_id_form(&VIRTUAL_MACHINE, id),
            "a machine's minted id must carry the machine id form, got {id:?}"
        );
        assert_eq!(
            record.get("HypervisorId").and_then(Value::as_str),
            Some(hypervisor_id),
            "a machine must name its hypervisor by the hypervisor's minted id, got {record:?}"
        );
    }
}
