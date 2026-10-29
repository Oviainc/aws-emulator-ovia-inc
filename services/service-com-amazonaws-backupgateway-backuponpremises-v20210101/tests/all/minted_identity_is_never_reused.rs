// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the IDENTITIES this service hands out: the `<kind>/<id>` ARNs
//! `CreateGateway`, `ImportHypervisorConfiguration` and the inventory channel a metadata
//! sync records a machine through all mint from one sequence per (resource kind, region).
//!
//! Every case drives a real `POST /` awsJson1_0 request through the `MultiServiceRouter`
//! — the harness of `gateway_lifecycle` — so what is asserted is the wire behaviour, and
//! the only way a later request can see what an earlier one minted is the shared store.
//!
//! The rule under test: an identity this service has handed out is handed out ONCE in a
//! run, whatever the store does afterwards to keep its bookkeeping bounded — so bounding
//! the mint registry frees its per-pair entries without letting any later mint reach a
//! sequence it already issued, and a page boundary the service issued stays one it
//! issued. Three consequences, each a test below: a re-mint after the bound ran never
//! repeats an ARN; a `NextToken` a client is still holding keeps resolving and the record
//! minted after the bound is reported rather than skipped past; and the server a gateway
//! still names after that server was deleted cannot be re-bound to a hypervisor imported
//! later.
//!
//! The table spans the class rather than one recorded request: every kind of resource
//! this service mints an ARN for, in several regions — the two halves of a mint
//! sequence's key — with the registry driven over its ceiling in between.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::SystemTime;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_backupgateway_backuponpremises_v20210101 as backup_gateway;

use backup_gateway::state::{
    arn_resource_id, service_state, HypervisorConfiguration, VirtualMachineInventory,
    GATEWAY_OUT_OF_DATE, MAX_MINT_SEQUENCES,
};

use crate::gateway_lifecycle::{
    build_engine, build_router, call_in_region, create_body, issued_activation_key_for, Reply,
    REGION,
};

/// The regions the table below mints in. The region half of a mint sequence's key comes
/// off the caller's credential scope, so which pairs exist is caller-driven: the rows
/// differ in region as well as in kind.
const REGIONS: [&str; 3] = [REGION, "eu-central-1", "ap-southeast-1"];

/// One kind of resource this service mints an ARN for — the three resource types of
/// `arn:aws:backup-gateway:<region>:<account>:<type>/<id>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Gateway,
    Hypervisor,
    VirtualMachine,
}

const KINDS: [Kind; 3] = [Kind::Gateway, Kind::Hypervisor, Kind::VirtualMachine];

impl Kind {
    /// The resource-type segment of an ARN of this kind.
    fn resource_type(self) -> &'static str {
        match self {
            Self::Gateway => "gateway",
            Self::Hypervisor => "hypervisor",
            Self::VirtualMachine => "vm",
        }
    }

    /// A name for this kind that satisfies the modelled `Name` pattern
    /// (`^[a-zA-Z0-9-]*$`), so it can go into a display name or a hypervisor name.
    fn label(self) -> &'static str {
        match self {
            Self::Gateway => "gw",
            Self::Hypervisor => "hv",
            Self::VirtualMachine => "vm",
        }
    }

    /// The read that resolves an ARN of this kind, and the pointer the record's own ARN
    /// arrives at in its reply.
    fn read(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Self::Gateway => ("GetGateway", "GatewayArn", "/Gateway/GatewayArn"),
            Self::Hypervisor => ("GetHypervisor", "HypervisorArn", "/Hypervisor/HypervisorArn"),
            Self::VirtualMachine => {
                ("GetVirtualMachine", "ResourceArn", "/VirtualMachine/ResourceArn")
            }
        }
    }
}

/// Import a hypervisor in `region` over the wire and hand back the ARN the import minted.
async fn import_in(router: &MultiServiceRouter, region: &str, label: &str) -> String {
    let reply = call_in_region(
        router,
        region,
        "ImportHypervisorConfiguration",
        &json!({ "Name": label, "Host": format!("{label}.example.internal") }),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "case {label}: ImportHypervisorConfiguration must answer 200, got {} {:?}",
        reply.status, reply.body
    );
    reply
        .str_at("/HypervisorArn")
        .unwrap_or_else(|| panic!("case {label}: no HypervisorArn in {:?}", reply.body))
        .to_string()
}

/// Delete the hypervisor `arn` names over the wire. The machines of its inventory go with
/// it, which is how a virtual-machine record leaves the store.
async fn delete_hypervisor(router: &MultiServiceRouter, region: &str, arn: &str) {
    let reply = call_in_region(
        router,
        region,
        "DeleteHypervisor",
        &json!({ "HypervisorArn": arn }),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "case delete {arn}: DeleteHypervisor must answer 200, got {} {:?}",
        reply.status, reply.body
    );
}

/// Record one machine on the hypervisor `hypervisor_arn` names, in `region`.
///
/// A virtual machine is minted by no operation of this model — it reaches the service as
/// part of the inventory of the hypervisor it runs on — so this kind is minted through
/// the channel that inventory arrives on, as the sibling `page_token_provenance` suite
/// seeds it.
fn record_machine(
    engine: &Arc<engine::InMemoryEngine>,
    region: &str,
    hypervisor_arn: &str,
    label: &str,
) -> String {
    let state = service_state(&**engine).expect("the in-memory engine must expose this state");
    let mut store = state.lock();
    store
        .record_virtual_machine(
            region,
            hypervisor_arn,
            VirtualMachineInventory {
                name: label.to_string(),
                host_name: label.to_string(),
                path: format!("/Datacenter/vm/{label}"),
                last_backup_date: None,
                vmware_tags: Vec::new(),
            },
            SystemTime::now(),
        )
        .unwrap_or_else(|| panic!("case {label}: the store must record a machine"))
        .arn
}

/// Mint one record of `kind` in `region` over the wire, and hand back the ARN the service
/// minted for it. The record STAYS stored, so it can be read back.
async fn mint(
    engine: &Arc<engine::InMemoryEngine>,
    router: &MultiServiceRouter,
    kind: Kind,
    region: &str,
    label: &str,
) -> String {
    match kind {
        Kind::Gateway => {
            let activation_key = issued_activation_key_for(router, region).await;
            let reply = call_in_region(
                router,
                region,
                "CreateGateway",
                &create_body(label, &activation_key, &BTreeMap::new()),
            )
            .await;
            assert_eq!(
                reply.status, 200,
                "case {label}: CreateGateway must answer 200, got {} {:?}",
                reply.status, reply.body
            );
            reply.gateway_arn().to_string()
        }
        Kind::Hypervisor => import_in(router, region, label).await,
        Kind::VirtualMachine => {
            let hypervisor = import_in(router, region, &format!("{label}-host")).await;
            record_machine(engine, region, &hypervisor, label)
        }
    }
}

/// Mint one record of `kind` in `region` and then DELETE it, handing back the ARN that
/// was minted: a (kind, region) pair whose sequence is spent and whose records are all
/// gone, which is the shape of pair the registry's bound is entitled to drop.
async fn mint_then_delete(
    engine: &Arc<engine::InMemoryEngine>,
    router: &MultiServiceRouter,
    kind: Kind,
    region: &str,
    label: &str,
) -> String {
    match kind {
        Kind::Gateway => {
            let arn = mint(engine, router, kind, region, label).await;
            let reply = call_in_region(
                router,
                region,
                "DeleteGateway",
                &json!({ "GatewayArn": arn }),
            )
            .await;
            assert_eq!(
                reply.status, 200,
                "case {label}: DeleteGateway must answer 200, got {} {:?}",
                reply.status, reply.body
            );
            arn
        }
        Kind::Hypervisor => {
            let arn = import_in(router, region, label).await;
            delete_hypervisor(router, region, &arn).await;
            arn
        }
        Kind::VirtualMachine => {
            let hypervisor = import_in(router, region, &format!("{label}-host")).await;
            let machine = record_machine(engine, region, &hypervisor, label);
            // The machine goes with its hypervisor, so both pairs of this region are left
            // holding a spent sequence and no record.
            delete_hypervisor(router, region, &hypervisor).await;
            machine
        }
    }
}

/// Read the record `arn` names through the operation that resolves an ARN of `kind`.
async fn read(router: &MultiServiceRouter, kind: Kind, region: &str, arn: &str) -> Reply {
    let (operation, member, _) = kind.read();
    call_in_region(router, region, operation, &json!({ member: arn })).await
}

/// Drive the mint registry over its ceiling with (kind, region) pairs the store holds no
/// record for, until its bound runs, and hand back (the size the registry had reached,
/// the size the bound left it at).
///
/// The filling goes through the state API rather than tens of thousands of requests — the
/// way the sibling `gateway_activation` suite fills the activation registry — because
/// what is under test is the behaviour AT the ceiling. Each import is removed again
/// immediately, so every pair it adds is one with a spent sequence and no record.
fn fill_registry_until_bounded(engine: &Arc<engine::InMemoryEngine>) -> (usize, usize) {
    let state = service_state(&**engine).expect("the in-memory engine must expose this state");
    let mut store = state.lock();
    for index in 0..=MAX_MINT_SEQUENCES {
        let before = store.mint_sequence_count();
        let region = format!("fill-{index}");
        let imported = store.import_hypervisor(
            &region,
            HypervisorConfiguration {
                name: format!("hv-fill-{index}"),
                host: format!("fill-{index}.example.internal"),
                ..HypervisorConfiguration::default()
            },
            SystemTime::now(),
        );
        store.remove_hypervisor(&imported.arn);
        let after = store.mint_sequence_count();
        if after <= before {
            return (before, after);
        }
    }
    panic!(
        "case fill: the registry's bound must run inside {MAX_MINT_SEQUENCES} record-less \
         pairs, and the registry still holds {}",
        store.mint_sequence_count()
    );
}

/// Fill the registry, asserting the bound really ran: the registry reached its ceiling and
/// was left smaller than that.
fn bound_the_mint_registry(engine: &Arc<engine::InMemoryEngine>) {
    let (peak, left) = fill_registry_until_bounded(engine);
    assert!(
        peak >= MAX_MINT_SEQUENCES,
        "case fill: the bound must run only once the registry reached {MAX_MINT_SEQUENCES} \
         pairs, got {peak}"
    );
    assert!(
        left < peak && left <= MAX_MINT_SEQUENCES,
        "case fill: the bound must leave the registry inside {MAX_MINT_SEQUENCES} pairs and \
         smaller than the {peak} it reached, got {left}"
    );
}

/// The `HypervisorArn`s a `ListHypervisors` reply reports, in the order it reported them.
fn listed_hypervisors(reply: &Reply) -> Vec<String> {
    reply
        .body
        .get("Hypervisors")
        .and_then(Value::as_array)
        .map(|records| {
            records
                .iter()
                .map(|record| {
                    record
                        .get("HypervisorArn")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// An identity this service handed out is never handed out again, however the store bounds
/// its mint registry: after the bound has dropped the registry entries of the pairs that
/// hold no record, a fresh mint in such a pair carries an ARN no earlier mint of the run
/// did, and that ARN names the NEW record.
#[tokio::test]
async fn an_identity_this_service_minted_is_never_minted_again_after_the_bound_runs() {
    let engine = build_engine();
    let router = build_router(&engine);

    // Every ARN this run has seen, so a repeat is caught whichever pair it comes from.
    let mut handed_out: BTreeSet<String> = BTreeSet::new();
    // (kind, region, the ARN the now-record-less pair handed out)
    let mut drained: Vec<(Kind, &str, String)> = Vec::new();
    for region in REGIONS {
        for kind in KINDS {
            let label = format!("{}-{region}", kind.label());
            let arn = mint_then_delete(&engine, &router, kind, region, &label).await;
            assert!(
                arn.contains(&format!(":{region}:")) && arn.contains(kind.resource_type()),
                "case {label}: a minted ARN must name the addressed region and the resource \
                 type, got {arn:?}"
            );
            assert!(
                handed_out.insert(arn.clone()),
                "case {label}: the service handed out {arn:?} twice"
            );
            drained.push((kind, region, arn));
        }
    }

    bound_the_mint_registry(&engine);

    for (kind, region, spent) in &drained {
        let label = format!("{}-{region}-again", kind.label());
        let minted = mint(&engine, &router, *kind, region, &label).await;
        assert_ne!(
            &minted, spent,
            "case {label}: a mint after the registry's bound must not re-hand-out the ARN \
             the pair already issued, got {minted:?}"
        );
        assert!(
            handed_out.insert(minted.clone()),
            "case {label}: the service handed out {minted:?} twice"
        );

        // The ARN names the record the store now holds, read back through the operation
        // that resolves an ARN of this kind.
        let (_, _, pointer) = kind.read();
        let fresh = read(&router, *kind, region, &minted).await;
        assert_eq!(
            (fresh.status, fresh.str_at(pointer)),
            (200, Some(minted.as_str())),
            "case {label}: the minted ARN must read back as the record the store holds, got \
             {} {:?}",
            fresh.status,
            fresh.body
        );

        // And the spent ARN still names nothing: the new record did not resurrect it.
        let gone = read(&router, *kind, region, spent).await;
        assert_ne!(
            gone.status, 200,
            "case {label}: the deleted record's ARN {spent:?} must still resolve to nothing, \
             got {} {:?}",
            gone.status, gone.body
        );
    }

    assert_eq!(
        handed_out.len(),
        REGIONS.len() * KINDS.len() * 2,
        "case table: every mint of this run must have handed out its own ARN, got {}",
        handed_out.len()
    );
}

/// A `NextToken` this service issued keeps resolving after the registry's bound has run,
/// and the record minted in that region AFTER the bound is REPORTED by the resumed
/// listing rather than skipped: a new record carries a new identity, which sorts above
/// every boundary already handed out.
#[tokio::test]
async fn a_token_the_service_issued_still_resumes_after_the_registry_is_bounded() {
    let engine = build_engine();
    let router = build_router(&engine);

    // Two records, so this region's listing really cuts a page and hands out a boundary.
    let first = import_in(&router, REGION, "hv-token-0").await;
    let second = import_in(&router, REGION, "hv-token-1").await;
    let page_one = call_in_region(
        &router,
        REGION,
        "ListHypervisors",
        &json!({ "MaxResults": 1 }),
    )
    .await;
    assert_eq!(
        (page_one.status, listed_hypervisors(&page_one)),
        (200, vec![first.clone()]),
        "case page one: the first page must hold the first hypervisor in ARN order, got {} {:?}",
        page_one.status,
        page_one.body
    );
    let token = page_one.next_token().unwrap_or_else(|| {
        panic!(
            "case page one: a truncated page must carry a NextToken, got {:?}",
            page_one.body
        )
    });

    // The region is drained, so its pair holds spent sequences and no record at all.
    delete_hypervisor(&router, REGION, &first).await;
    delete_hypervisor(&router, REGION, &second).await;
    bound_the_mint_registry(&engine);

    let drained_page = call_in_region(
        &router,
        REGION,
        "ListHypervisors",
        &json!({ "NextToken": token.clone() }),
    )
    .await;
    assert_eq!(
        (
            drained_page.status,
            listed_hypervisors(&drained_page),
            drained_page.next_token()
        ),
        (200, Vec::new(), None),
        "case drained: a token this listing issued must still resume it, over an empty \
         region, got {} {:?}",
        drained_page.status,
        drained_page.body
    );

    let third = import_in(&router, REGION, "hv-token-2").await;
    for (case, earlier) in [("the first", &first), ("the second", &second)] {
        assert_ne!(
            &third, earlier,
            "case {case}: the import after the bound must not re-hand-out {earlier:?}"
        );
    }
    let resumed = call_in_region(
        &router,
        REGION,
        "ListHypervisors",
        &json!({ "NextToken": token }),
    )
    .await;
    assert_eq!(
        (resumed.status, listed_hypervisors(&resumed)),
        (200, vec![third.clone()]),
        "case resumed: the page after a boundary this listing issued must report the record \
         minted since, got {} {:?}",
        resumed.status,
        resumed.body
    );
}

/// The server a gateway still names after that server was DELETED is not re-bound to a
/// hypervisor imported later: the later import carries its own identity, so it is a
/// hypervisor no gateway is associated with and a put of its property mappings is still
/// refused with the gateway's own refusal.
#[tokio::test]
async fn a_stale_server_link_does_not_rebind_to_a_hypervisor_minted_later() {
    let engine = build_engine();
    let router = build_router(&engine);

    let gateway = mint(&engine, &router, Kind::Gateway, REGION, "gw-stale").await;
    let hypervisor = import_in(&router, REGION, "hv-stale").await;
    let associated = call_in_region(
        &router,
        REGION,
        "AssociateGatewayToServer",
        &json!({ "GatewayArn": gateway, "ServerArn": hypervisor }),
    )
    .await;
    assert_eq!(
        associated.status, 200,
        "case associate: AssociateGatewayToServer must answer 200, got {} {:?}",
        associated.status, associated.body
    );
    delete_hypervisor(&router, REGION, &hypervisor).await;
    let stale_id = arn_resource_id(&hypervisor)
        .expect("a minted ARN carries a resource id")
        .to_string();

    bound_the_mint_registry(&engine);

    let fresh = import_in(&router, REGION, "hv-fresh").await;
    assert_ne!(
        fresh, hypervisor,
        "case fresh import: the import after the bound must not re-hand-out the deleted \
         hypervisor's ARN {hypervisor:?}"
    );

    let refused = call_in_region(
        &router,
        REGION,
        "PutHypervisorPropertyMappings",
        &json!({
            "HypervisorArn": fresh,
            "IamRoleArn": format!("arn:aws:iam::{}:role/fresh", engine::EMULATOR_ACCOUNT_ID),
            "VmwareToAwsTagMappings": [{
                "VmwareCategory": "department",
                "VmwareTagName": "finance",
                "AwsTagKey": "Department",
                "AwsTagValue": "Finance",
            }],
        }),
    )
    .await;
    assert_eq!(
        (
            refused.status,
            refused.code.as_deref(),
            refused.str_at("/ErrorCode")
        ),
        (400, Some("ValidationException"), Some(GATEWAY_OUT_OF_DATE)),
        "case fresh import: a put for a hypervisor no gateway is associated with must be \
         refused, got {} {:?}",
        refused.status,
        refused.body
    );

    let fetched = call_in_region(
        &router,
        REGION,
        "GetGateway",
        &json!({ "GatewayArn": gateway }),
    )
    .await;
    assert_eq!(
        (fetched.status, fetched.str_at("/Gateway/HypervisorId")),
        (200, Some(stale_id.as_str())),
        "case fresh import: the gateway must still name the server it was associated with, \
         got {} {:?}",
        fetched.status,
        fetched.body
    );
}
