// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the tagging operations over EVERY resource this service holds.
//!
//! Every case drives a real `POST /` awsJson1_0 request through the `MultiServiceRouter`,
//! reusing the harness of `gateway_lifecycle` and the sync workflow of
//! `metadata_sync_over_the_wire`, so what is asserted is the wire behaviour of
//! `TagResource`, `ListTagsForResource` and `UntagResource`, and the only way a later
//! request can see what an earlier one wrote is the shared store.
//!
//! The rule under test: the tagging operations carry ONE `ResourceARN` member, so they
//! resolve it against every resource this service reports — a gateway, a hypervisor and a
//! virtual machine of a hypervisor's inventory alike — and refuse only an ARN the service
//! reports no resource under; what they write and read is the tag set of the resolved
//! record, so a resource a sibling read (`GetGateway`, `GetHypervisor`,
//! `GetVirtualMachine`) resolves is one the tagging operations resolve too.
//!
//! The table below spans that class rather than one recorded request: each row is run
//! against all three resource kinds, and the rows differ in how many tags are written
//! (none, one, several), whether a key is set a second time, the character forms a
//! modelled `TagKey` admits (letters of any script, separators, digits and
//! `_ . : / = + - @`), each published bound of the modelled `TagKey` length, tag-key case,
//! and the value written (including an empty one) — and in which of those keys the
//! following `UntagResource` removes.
//!
//! Waiting here is a real wait ([`std::thread::sleep`]), as in the sibling
//! `virtual_machine_inventory_sync` suite: nothing but the clock carries a metadata sync
//! along, and the machines it discovered are reported from the instant it falls due.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_backupgateway_backuponpremises_v20210101 as backup_gateway;

use backup_gateway::state::{service_state, METADATA_SYNC_SUCCEEDS_AFTER};

use crate::gateway_lifecycle::{
    build_engine, build_router, call, create, persisted_client_tags, Reply, REGION,
};
use crate::hypervisor_property_mappings::import;
use crate::metadata_sync_over_the_wire::{start_sync, syncable_hypervisor};

/// Long enough past [`METADATA_SYNC_SUCCEEDS_AFTER`] that a sync started before the wait
/// has fallen due on a loaded machine too.
const PAST_THE_SYNC: Duration =
    METADATA_SYNC_SUCCEEDS_AFTER.saturating_add(Duration::from_millis(150));

/// The resource kinds this service's one `ResourceARN` member accepts.
const KINDS: [&str; 3] = ["gateway", "hypervisor", "virtual machine"];

/// An ARN of this service naming `resource_type`/`id`, in the region the harness signs
/// for.
fn arn_of(resource_type: &str, id: &str) -> String {
    format!(
        "arn:aws:backup-gateway:{REGION}:{}:{resource_type}/{id}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

fn tag_map<'a>(
    pairs: impl IntoIterator<Item = &'a (&'a str, &'a str)>,
) -> BTreeMap<String, String> {
    pairs
        .into_iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

async fn tag(router: &MultiServiceRouter, arn: &str, tags: &[(&str, &str)]) -> Reply {
    call(
        router,
        "TagResource",
        &json!({
            "ResourceARN": arn,
            "Tags": tags
                .iter()
                .map(|(key, value)| json!({ "Key": key, "Value": value }))
                .collect::<Vec<Value>>(),
        }),
    )
    .await
}

async fn untag(router: &MultiServiceRouter, arn: &str, keys: &[&str]) -> Reply {
    call(
        router,
        "UntagResource",
        &json!({ "ResourceARN": arn, "TagKeys": keys }),
    )
    .await
}

async fn list_tags(router: &MultiServiceRouter, arn: &str) -> Reply {
    call(router, "ListTagsForResource", &json!({ "ResourceArn": arn })).await
}

/// The tags `ListTagsForResource` reports for `arn`, asserting the call was served and
/// that it echoed the ARN the store holds the resource under.
async fn listed_tags(
    router: &MultiServiceRouter,
    case: &str,
    arn: &str,
) -> BTreeMap<String, String> {
    let reply = list_tags(router, arn).await;
    assert_eq!(
        (reply.status, reply.str_at("/ResourceArn")),
        (200, Some(arn)),
        "case {case:?}: ListTagsForResource must answer 200 echoing {arn}, got {} {:?}",
        reply.status,
        reply.body
    );
    reply.tags()
}

async fn get_machine(router: &MultiServiceRouter, arn: &str) -> Reply {
    call(router, "GetVirtualMachine", &json!({ "ResourceArn": arn })).await
}

/// The ARNs of the machines of `hypervisor_arn` the STORE holds whose discovering sync has
/// not fallen due yet — the records no wire call reports, read with a clock far enough
/// ahead that the store resolves them.
fn pending_machines(engine: &Arc<engine::InMemoryEngine>, hypervisor_arn: &str) -> Vec<String> {
    let state = service_state(&**engine).expect("the in-memory engine must expose this state");
    let store = state.lock();
    let far_ahead = SystemTime::now() + PAST_THE_SYNC * 10;
    store
        .page_of_virtual_machines(REGION, Some(hypervisor_arn), None, 100, far_ahead)
        .items
        .iter()
        .map(|machine| machine.arn.clone())
        .collect()
}

/// The `ResourceArn`s `ListVirtualMachines` reports for `hypervisor_arn`, in the order it
/// reported them: the only way a client learns a machine's ARN.
async fn machines_of(router: &MultiServiceRouter, hypervisor_arn: &str) -> Vec<String> {
    let reply = call(
        router,
        "ListVirtualMachines",
        &json!({ "HypervisorArn": hypervisor_arn }),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "ListVirtualMachines must answer 200, got {} {:?}",
        reply.status, reply.body
    );
    reply
        .body
        .get("VirtualMachines")
        .and_then(Value::as_array)
        .map(|machines| {
            machines
                .iter()
                .filter_map(|machine| machine.get("ResourceArn").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Import a hypervisor the sync's precondition is met for, sync it, wait for the sync to
/// fall due, and hand back the ARNs of the machines its inventory now holds — the wire-only
/// path from "no machines" to a machine a client can name.
async fn synced_machines(
    router: &MultiServiceRouter,
    name: &str,
    host: &str,
) -> (String, Vec<String>) {
    let hypervisor = syncable_hypervisor(router, name, host).await;
    let started = start_sync(router, &hypervisor).await;
    assert_eq!(
        started.status, 200,
        "StartVirtualMachinesMetadataSync must answer 200, got {} {:?}",
        started.status, started.body
    );
    std::thread::sleep(PAST_THE_SYNC);
    let machines = machines_of(router, &hypervisor).await;
    assert!(
        !machines.is_empty(),
        "a succeeded sync of {name} must report machines, got none"
    );
    (hypervisor, machines)
}

/// One row of the tagging table: the tags written first, the tags written next (so a key
/// set twice is covered), what the resource must then carry, the keys removed, and what it
/// must be left carrying.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    first: &'static [(&'static str, &'static str)],
    second: &'static [(&'static str, &'static str)],
    tagged: &'static [(&'static str, &'static str)],
    remove: &'static [&'static str],
    left: &'static [(&'static str, &'static str)],
}

/// The tagging rows. The key forms come from the modelled `TagKey`:
/// `@length(min: 1, max: 128)` and `@pattern("^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$")`
/// (`smithy_query(query="shape",shape="Tag",member="Key")`), so letters of any script,
/// separators (spaces) and digits are all legal key characters beside `_ . : / = + - @`.
/// Values carry no space: the modelled `Tag` doc states "Spaces are not allowed in tag
/// values".
const ROWS: &[Row] = &[
    Row {
        case: "no tags at all",
        first: &[],
        second: &[],
        tagged: &[],
        remove: &["never-set"],
        left: &[],
    },
    Row {
        case: "one tag",
        first: &[("env", "test")],
        second: &[],
        tagged: &[("env", "test")],
        remove: &["env"],
        left: &[],
    },
    Row {
        case: "several tags in one request",
        first: &[("env", "test"), ("owner", "qa"), ("team", "core")],
        second: &[],
        tagged: &[("env", "test"), ("owner", "qa"), ("team", "core")],
        remove: &["owner"],
        left: &[("env", "test"), ("team", "core")],
    },
    Row {
        case: "a second request merges by key, overwriting one and adding another",
        first: &[("env", "test"), ("team", "core")],
        second: &[("env", "prod"), ("tier", "gold")],
        tagged: &[("env", "prod"), ("team", "core"), ("tier", "gold")],
        remove: &["env", "tier"],
        left: &[("team", "core")],
    },
    Row {
        case: "keys carrying separators and non-latin letters",
        first: &[
            ("cost center", "ops"),
            ("Ключ", "значение"),
            ("path/to:thing=1+2-3@x", "kept"),
        ],
        second: &[],
        tagged: &[
            ("cost center", "ops"),
            ("Ключ", "значение"),
            ("path/to:thing=1+2-3@x", "kept"),
        ],
        remove: &["cost center", "Ключ"],
        left: &[("path/to:thing=1+2-3@x", "kept")],
    },
    Row {
        case: "tag keys are case sensitive",
        first: &[("Env", "upper"), ("env", "lower")],
        second: &[],
        tagged: &[("Env", "upper"), ("env", "lower")],
        remove: &["env"],
        left: &[("Env", "upper")],
    },
    Row {
        case: "an empty value beside a non-empty one",
        first: &[("empty", ""), ("full", "a-non-empty_value:1")],
        second: &[],
        tagged: &[("empty", ""), ("full", "a-non-empty_value:1")],
        remove: &["empty"],
        left: &[("full", "a-non-empty_value:1")],
    },
    Row {
        case: "a 128-character key, the modelled TagKey maximum, and a one-character one",
        first: &[
            (
                "kkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkk\
                 kkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkk",
                "max",
            ),
            ("k", "min"),
        ],
        second: &[],
        tagged: &[
            (
                "kkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkk\
                 kkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkk",
                "max",
            ),
            ("k", "min"),
        ],
        remove: &[
            "kkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkk\
             kkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkk",
        ],
        left: &[("k", "min")],
    },
];

/// A pool of resources of one kind, each row of the table getting its own.
async fn seed_pool(router: &MultiServiceRouter, kind: &str, count: usize) -> Vec<String> {
    match kind {
        "gateway" => {
            let mut arns = Vec::new();
            for index in 0..count {
                arns.push(create(router, &format!("gw-tagging-{index}")).await);
            }
            arns
        }
        "hypervisor" => {
            let mut arns = Vec::new();
            for index in 0..count {
                arns.push(
                    import(
                        router,
                        &format!("hv-tagging-{index}"),
                        &format!("192.0.2.{}", 10 + index),
                    )
                    .await,
                );
            }
            arns
        }
        "virtual machine" => {
            // A machine is not created by any request: it reaches the service as part of
            // the inventory a metadata sync of its hypervisor reads, so the pool is filled
            // by syncing as many hypervisors as it takes to reach `count` machines.
            let mut arns = Vec::new();
            let mut index = 0;
            while arns.len() < count {
                let (_, machines) = synced_machines(
                    router,
                    &format!("hv-vm-tagging-{index}"),
                    &format!("192.0.2.{}", 100 + index),
                )
                .await;
                arns.extend(machines);
                index += 1;
            }
            arns.truncate(count);
            arns
        }
        other => panic!("unknown resource kind {other:?}"),
    }
}

/// The rule: one `ResourceARN` member resolves against every resource this service holds,
/// so the whole tag round trip — write, read back, remove — works the same on a gateway, a
/// hypervisor and a virtual machine, and what the wire reports is what the store holds.
#[tokio::test]
async fn tagging_spans_every_resource_this_service_holds() {
    let engine = build_engine();
    let router = build_router(&engine);

    for kind in KINDS {
        let pool = seed_pool(&router, kind, ROWS.len()).await;
        for (row, arn) in ROWS.iter().zip(&pool) {
            let case = format!("{kind} / {}", row.case);

            // An untagged resource reports an empty tag list, not a refusal: it exists and
            // carries no tags.
            let before = listed_tags(&router, &case, arn).await;
            assert!(
                before.is_empty(),
                "case {case:?}: a freshly seeded resource must carry no tags, got {before:?}"
            );

            for tags in [row.first, row.second] {
                if tags.is_empty() {
                    continue;
                }
                let written = tag(&router, arn, tags).await;
                assert_eq!(
                    (written.status, written.str_at("/ResourceARN")),
                    (200, Some(arn.as_str())),
                    "case {case:?}: TagResource must answer 200 echoing the stored ARN, \
                     got {} {:?}",
                    written.status,
                    written.body
                );
            }

            let expected = tag_map(row.tagged);
            let listed = listed_tags(&router, &case, arn).await;
            assert_eq!(
                listed, expected,
                "case {case:?}: after tagging, the resource must carry {expected:?}, \
                 ListTagsForResource reported {listed:?}"
            );
            let stored = persisted_client_tags(&engine, arn);
            assert_eq!(
                stored, expected,
                "case {case:?}: after tagging, the STORE must hold {expected:?}, it holds \
                 {stored:?}"
            );

            let removed = untag(&router, arn, row.remove).await;
            assert_eq!(
                (removed.status, removed.str_at("/ResourceARN")),
                (200, Some(arn.as_str())),
                "case {case:?}: UntagResource must answer 200 echoing the stored ARN, got \
                 {} {:?}",
                removed.status,
                removed.body
            );

            let left = tag_map(row.left);
            let after = listed_tags(&router, &case, arn).await;
            assert_eq!(
                after, left,
                "case {case:?}: after removing {:?} the resource must carry {left:?}, \
                 ListTagsForResource reported {after:?}",
                row.remove
            );
            let stored_after = persisted_client_tags(&engine, arn);
            assert_eq!(
                stored_after, left,
                "case {case:?}: after removing {:?} the STORE must hold {left:?}, it holds \
                 {stored_after:?}",
                row.remove
            );
        }

        // Every resource of the pool kept its OWN tags: one tag write reaches one record.
        for (row, arn) in ROWS.iter().zip(&pool) {
            let case = format!("{kind} / {} (isolated)", row.case);
            let listed = listed_tags(&router, &case, arn).await;
            assert_eq!(
                listed,
                tag_map(row.left),
                "case {case:?}: a tag write must reach only the resource it named, got \
                 {listed:?}"
            );
        }
    }
}

/// A read of tags is a read: it reports the same tags however often it is called, and
/// changes nothing a later caller — including the sibling read of the same resource —
/// sees.
#[tokio::test]
async fn listing_tags_changes_nothing_a_later_caller_sees() {
    let engine = build_engine();
    let router = build_router(&engine);

    let (_, machines) = synced_machines(&router, "hv-tag-reads", "192.0.2.60").await;
    let machine = machines[0].clone();
    let resources = [
        ("gateway", create(&router, "gw-tag-reads").await),
        (
            "hypervisor",
            import(&router, "hv-tag-reads-plain", "192.0.2.61").await,
        ),
        ("virtual machine", machine.clone()),
    ];

    for (kind, arn) in &resources {
        tag(&router, arn, &[("env", "test"), ("team", "core")]).await;
        let expected = tag_map(&[("env", "test"), ("team", "core")]);
        for attempt in 1..=3 {
            let case = format!("{kind} read {attempt}");
            let listed = listed_tags(&router, &case, arn).await;
            assert_eq!(
                listed, expected,
                "case {case:?}: a repeated read must report {expected:?}, got {listed:?}"
            );
            let stored = persisted_client_tags(&engine, arn);
            assert_eq!(
                stored, expected,
                "case {case:?}: a read must leave the store holding {expected:?}, it holds \
                 {stored:?}"
            );
        }
    }

    // The machine is still the machine the inventory reports: a tag write of this API
    // touches the AWS tags of the record and not the VMware tags its hypervisor's
    // inventory attached to it (`VirtualMachineDetails$VmwareTags`), and the machine stays
    // readable and listed.
    let read = get_machine(&router, &machine).await;
    assert_eq!(
        (
            read.status,
            read.body.pointer("/VirtualMachine/VmwareTags").cloned(),
            read.str_at("/VirtualMachine/ResourceArn"),
        ),
        (200, Some(json!([])), Some(machine.as_str())),
        "a tagged machine must still report the inventory's own VmwareTags, got {} {:?}",
        read.status,
        read.body
    );
}

/// The AWS tags a client wrote on a machine survive a re-sync of its hypervisor: a tag
/// belongs to the resource, where the rest of the record is what the inventory reported.
#[tokio::test]
async fn a_machines_tags_survive_a_resync_of_its_hypervisor() {
    let engine = build_engine();
    let router = build_router(&engine);

    let (hypervisor, machines) = synced_machines(&router, "hv-tag-resync", "192.0.2.70").await;
    let machine = machines[0].clone();
    tag(&router, &machine, &[("env", "test"), ("tier", "gold")]).await;

    let restarted = start_sync(&router, &hypervisor).await;
    assert_eq!(
        restarted.status, 200,
        "starting the sync again must answer 200, got {} {:?}",
        restarted.status, restarted.body
    );
    std::thread::sleep(PAST_THE_SYNC);

    let expected = tag_map(&[("env", "test"), ("tier", "gold")]);
    let listed = listed_tags(&router, "a re-synced machine", &machine).await;
    assert_eq!(
        listed, expected,
        "a re-sync must leave the machine's tags alone, got {listed:?}"
    );
    let stored = persisted_client_tags(&engine, &machine);
    assert_eq!(
        stored, expected,
        "a re-sync must leave the STORE holding {expected:?}, it holds {stored:?}"
    );
}

/// The tagging operations and the sibling read of a resource AGREE about which ARNs name a
/// resource: an ARN `GetVirtualMachine` serves is one all three tagging operations serve,
/// and an ARN it refuses is one they refuse too — a machine whose discovering sync has not
/// fallen due, a machine of a deleted hypervisor, and an ARN no sync ever minted.
#[tokio::test]
async fn the_tagging_operations_resolve_exactly_the_machines_the_service_reports() {
    let engine = build_engine();
    let router = build_router(&engine);

    let (served_hypervisor, served) = synced_machines(&router, "hv-agree-served", "192.0.2.80")
        .await;
    let (doomed_hypervisor, doomed) = synced_machines(&router, "hv-agree-doomed", "192.0.2.81")
        .await;

    // A sync still in flight: the store holds the records, but the service does not report
    // them yet — neither the listing that would hand a client the ARN, nor the read, nor
    // the tagging operations. Their ARNs are read out of the store (the only place they
    // exist before the sync falls due) with a clock far enough ahead to see them.
    let in_flight_hypervisor =
        syncable_hypervisor(&router, "hv-agree-in-flight", "192.0.2.82").await;
    start_sync(&router, &in_flight_hypervisor).await;
    let pending = pending_machines(&engine, &in_flight_hypervisor);
    assert!(
        !pending.is_empty(),
        "the sync must have recorded the machines it will report"
    );
    assert!(
        machines_of(&router, &in_flight_hypervisor).await.is_empty(),
        "a sync in flight must report no machine yet"
    );

    let removed = call(
        &router,
        "DeleteHypervisor",
        &json!({ "HypervisorArn": doomed_hypervisor }),
    )
    .await;
    assert_eq!(
        removed.status, 200,
        "DeleteHypervisor must answer 200, got {} {:?}",
        removed.status, removed.body
    );

    // (case, the ARN, whether the service reports a resource under it)
    let rows: Vec<(String, String, bool)> = vec![
        (
            "a machine a succeeded sync discovered".to_string(),
            served[0].clone(),
            true,
        ),
        (
            "another machine of the same inventory".to_string(),
            served[served.len() - 1].clone(),
            true,
        ),
        (
            "the hypervisor that machine runs on".to_string(),
            served_hypervisor.clone(),
            true,
        ),
        (
            "a machine of a deleted hypervisor".to_string(),
            doomed[0].clone(),
            false,
        ),
        (
            "a machine whose discovering sync has not fallen due".to_string(),
            pending[0].clone(),
            false,
        ),
        (
            "a vm ARN no sync ever minted".to_string(),
            arn_of("vm", "vm-1111111111111111"),
            false,
        ),
    ];

    for (case, arn, reported) in &rows {
        let read = get_machine(&router, arn).await;
        let read_serves = read.status == 200;
        for (operation, reply) in [
            ("ListTagsForResource", list_tags(&router, arn).await),
            ("TagResource", tag(&router, arn, &[("env", "test")]).await),
            ("UntagResource", untag(&router, arn, &["env"]).await),
        ] {
            let serves = reply.status == 200;
            assert_eq!(
                serves,
                *reported,
                "case {case:?}: {operation} must {} this ARN, got {} {:?}",
                if *reported { "serve" } else { "refuse" },
                reply.status,
                reply.body
            );
            if !*reported {
                // Every row that is refused here names a virtual machine, so the refusal
                // is the modelled `ResourceNotFoundException` at HTTP 400 carrying the
                // `ErrorCode` that names the machine.
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
                        Some("VirtualMachineNotFound"),
                        Some("The virtual machine is not found."),
                    ),
                    "case {case:?}: {operation} must refuse an ARN with no record 400 \
                     ResourceNotFoundException / VirtualMachineNotFound, got {} {:?}",
                    reply.status,
                    reply.body
                );
            }
        }
        // A machine ARN the tagging operations resolve is one the sibling read resolves:
        // the hypervisor row is the one ARN only the tagging operations take.
        if arn != &served_hypervisor {
            assert_eq!(
                read_serves, *reported,
                "case {case:?}: GetVirtualMachine and the tagging operations must agree \
                 about this ARN, the read answered {} {:?}",
                read.status, read.body
            );
        }
    }

    // The machine whose sync is still in flight becomes taggable at the same instant the
    // listing starts reporting it and the read starts serving it — nothing else changed.
    std::thread::sleep(PAST_THE_SYNC);
    let late = machines_of(&router, &in_flight_hypervisor).await;
    assert!(
        !late.is_empty(),
        "the sync must have fallen due and reported its machines"
    );
    for arn in &late {
        let read = get_machine(&router, arn).await;
        let listed = list_tags(&router, arn).await;
        assert_eq!(
            (read.status, listed.status),
            (200, 200),
            "a machine a fallen-due sync reports must be both readable and taggable, got \
             {} and {} {:?}",
            read.status,
            listed.status,
            listed.body
        );
        // The refused tag write above wrote nothing, so the machine arrives untagged.
        assert!(
            listed.tags().is_empty(),
            "a refused tag write must have written nothing, got {:?}",
            listed.body
        );
    }
}
