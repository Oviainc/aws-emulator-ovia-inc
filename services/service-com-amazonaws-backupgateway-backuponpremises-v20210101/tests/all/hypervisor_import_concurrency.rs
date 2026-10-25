// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ImportHypervisorConfiguration` reached CONCURRENTLY.
//!
//! The rule under test: an import mints one persisted hypervisor record PER CALL, keyed
//! by the `HypervisorArn` it mints for it and by nothing else — so concurrent imports are
//! independent of one another whatever they share, each minting its own record under its
//! own ARN, whether their `Host`, `Name`, `KmsKeyArn` or `Tags` differ or are identical.
//! That holds however many callers arrive at the same instant: the store bound and the
//! insert are one atomic step, so N racing callers leave exactly N records and no caller
//! loses its work.
//!
//! The sibling suite `hypervisor_reads` pins the same rule for imports issued one after
//! another; these cases arrive at once, on a multi-threaded runtime, which is where an
//! insert that is not atomic drops a racer's record or lands it on another's. The rows
//! span the number of racing callers, the host form (IPv4, FQDN, the modelled 3-character
//! minimum), whether the racers share a `Name` or a `Host`, and whether they carry a
//! `KmsKeyArn` (in both forms its pattern accepts).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_backupgateway_backuponpremises_v20210101 as backup_gateway;

use backup_gateway::state::HYPERVISOR_PENDING;

use crate::gateway_lifecycle::{
    build_engine, build_router, call, hypervisor_arns_for_host, hypervisor_count, Reply,
};

/// The `Host` racer `index` imports, cycling the host forms the member accepts: an IPv4
/// address, an FQDN, and the modelled 3-character minimum
/// (`smithy_query(query="shape",shape="Host")`: `@length(min: 3, max: 128)`, pattern
/// `^.+$`).
fn host_of(index: usize) -> String {
    match index % 3 {
        0 => format!("198.51.100.{}", index + 10),
        1 => format!("hv-{index}.hypervisor.example.com"),
        _ => format!("a.{index}"),
    }
}

/// The `KmsKeyArn` racer `index` carries, or `None` for a racer that carries none. The
/// two forms are the alternatives of the member's own pattern
/// (`smithy_query(query="shape",shape="KmsKeyArn")`): a `key/` ARN and an `alias/` ARN,
/// both at least the modelled 50-character minimum.
fn kms_key_of(index: usize) -> Option<String> {
    match index % 3 {
        0 => None,
        1 => Some(format!(
            "arn:aws:kms:us-west-2:000000000000:key/1234abcd-12ab-34cd-56ef-1234567890{index:02}"
        )),
        _ => Some(format!(
            "arn:aws:kms:us-west-2:000000000000:alias/backup-gateway-racer-{index}"
        )),
    }
}

/// The `ImportHypervisorConfiguration` request body for `name` at `host`.
fn import_body(name: &str, host: &str, kms_key_arn: Option<&str>) -> Value {
    let mut body = json!({ "Name": name, "Host": host });
    if let Some(kms_key_arn) = kms_key_arn {
        body["KmsKeyArn"] = json!(kms_key_arn);
    }
    body
}

/// Dispatch `requests` (one `(name, host, kms_key_arn)` each) at the same time, and hand
/// back what each one answered, in the order the requests were given.
async fn race_imports(
    router: &Arc<MultiServiceRouter>,
    requests: Vec<(String, String, Option<String>)>,
) -> Vec<(usize, Reply)> {
    let mut tasks = Vec::new();
    for (index, (name, host, kms_key_arn)) in requests.into_iter().enumerate() {
        let router = Arc::clone(router);
        tasks.push(tokio::spawn(async move {
            let reply = call(
                &router,
                "ImportHypervisorConfiguration",
                &import_body(&name, &host, kms_key_arn.as_deref()),
            )
            .await;
            (index, reply)
        }));
    }

    let mut replies = Vec::new();
    for task in tasks {
        replies.push(task.await.expect("the racing import task must not panic"));
    }
    replies.sort_by_key(|(index, _)| *index);
    replies
}

async fn get_hypervisor(router: &MultiServiceRouter, arn: &str) -> Reply {
    call(router, "GetHypervisor", &json!({ "HypervisorArn": arn })).await
}

/// RULE: imports of DISTINCT hosts are independent — concurrent callers each mint their
/// own record under their own `HypervisorArn`, carrying the members their own request
/// named, even when they all ask for the same `Name`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_imports_of_distinct_hosts_are_all_independent() {
    // (case, how many callers race, do they all share one Name?)
    let cases: [(&str, usize, bool); 4] = [
        ("two callers, distinct names", 2, false),
        ("two callers sharing one name", 2, true),
        ("three callers sharing one name", 3, true),
        ("six callers, distinct names", 6, false),
    ];

    for (case, racers, share_name) in cases {
        let engine = build_engine();
        let router = Arc::new(build_router(&engine));

        let requests: Vec<(String, String, Option<String>)> = (0..racers)
            .map(|index| {
                let name = if share_name {
                    "hv-shared".to_string()
                } else {
                    format!("hv-racer-{index}")
                };
                (name, host_of(index), kms_key_of(index))
            })
            .collect();

        let replies = race_imports(&router, requests.clone()).await;

        let mut arns = Vec::new();
        for ((index, reply), (name, host, kms_key_arn)) in replies.iter().zip(&requests) {
            assert_eq!(
                reply.status, 200,
                "case {case:?}: racer {index} imported its own host {host:?} and must \
                 answer 200, got {} {:?}",
                reply.status, reply.body
            );
            let arn = reply
                .str_at("/HypervisorArn")
                .unwrap_or_else(|| {
                    panic!("case {case:?}: racer {index}: no HypervisorArn in {:?}", reply.body)
                })
                .to_string();
            // Each record reports the members ITS request carried, so no racer's write
            // landed on another's record.
            let read = get_hypervisor(&router, &arn).await;
            assert_eq!(
                (
                    read.status,
                    read.str_at("/Hypervisor/Name"),
                    read.str_at("/Hypervisor/Host"),
                    read.str_at("/Hypervisor/KmsKeyArn"),
                    read.str_at("/Hypervisor/State"),
                ),
                (
                    200,
                    Some(name.as_str()),
                    Some(host.as_str()),
                    kms_key_arn.as_deref(),
                    Some(HYPERVISOR_PENDING),
                ),
                "case {case:?}: racer {index} must read back its own record, got {} {:?}",
                read.status,
                read.body
            );
            arns.push(arn);
        }

        let unique: BTreeSet<&String> = arns.iter().collect();
        assert_eq!(
            unique.len(),
            racers,
            "case {case:?}: {racers} distinct hosts must mint {racers} distinct ARNs, got \
             {arns:?}"
        );
        assert_eq!(
            hypervisor_count(&engine),
            racers,
            "case {case:?}: the store must hold one record per host, got {} for {arns:?}",
            hypervisor_count(&engine)
        );
        for (index, (_, host, _)) in requests.iter().enumerate() {
            assert_eq!(
                hypervisor_arns_for_host(&engine, host),
                BTreeSet::from([arns[index].clone()]),
                "case {case:?}: host {host:?} must be held under racer {index}'s ARN {:?}, \
                 got {:?}",
                arns[index],
                hypervisor_arns_for_host(&engine, host)
            );
        }
    }
}

/// RULE: one call, one record — concurrent imports of the SAME host all succeed, each
/// minting its own `HypervisorArn`, because the ARN is a record's only identity and
/// `Host` constrains nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_imports_of_one_host_each_mint_their_own_record() {
    // (case, the shared host, how many callers race, do they all share one Name?)
    let cases: [(&str, &str, usize, bool); 5] = [
        ("two callers, an IPv4 host", "198.51.100.10", 2, false),
        (
            "three callers, an FQDN host",
            "hypervisor.example.com",
            3,
            false,
        ),
        (
            "three callers sharing one name, an FQDN host",
            "shared.hypervisor.example.com",
            3,
            true,
        ),
        ("eight callers, the shortest host", "a.b", 8, false),
        ("two callers sharing one name", "203.0.113.7", 2, true),
    ];

    for (case, host, racers, share_name) in cases {
        let engine = build_engine();
        let router = Arc::new(build_router(&engine));

        let requests: Vec<(String, String, Option<String>)> = (0..racers)
            .map(|index| {
                let name = if share_name {
                    "hv-contested".to_string()
                } else {
                    format!("hv-contender-{index}")
                };
                (name, host.to_string(), kms_key_of(index))
            })
            .collect();

        let replies = race_imports(&router, requests.clone()).await;

        let mut arns = Vec::new();
        for ((index, reply), (name, _, kms_key_arn)) in replies.iter().zip(&requests) {
            assert_eq!(
                reply.status, 200,
                "case {case:?}: racer {index} must answer 200 — one host constrains no \
                 import — got {} {:?}",
                reply.status, reply.body
            );
            let arn = reply
                .str_at("/HypervisorArn")
                .unwrap_or_else(|| {
                    panic!("case {case:?}: racer {index}: no HypervisorArn in {:?}", reply.body)
                })
                .to_string();
            // Each record reports the members ITS request carried, so no racer's write
            // landed on another's record.
            let read = get_hypervisor(&router, &arn).await;
            assert_eq!(
                (
                    read.status,
                    read.str_at("/Hypervisor/Name"),
                    read.str_at("/Hypervisor/Host"),
                    read.str_at("/Hypervisor/KmsKeyArn"),
                    read.str_at("/Hypervisor/State"),
                ),
                (
                    200,
                    Some(name.as_str()),
                    Some(host),
                    kms_key_arn.as_deref(),
                    Some(HYPERVISOR_PENDING),
                ),
                "case {case:?}: racer {index} must read back its own record, got {} {:?}",
                read.status,
                read.body
            );
            arns.push(arn);
        }

        let unique: BTreeSet<String> = arns.iter().cloned().collect();
        assert_eq!(
            unique.len(),
            racers,
            "case {case:?}: {racers} callers must mint {racers} distinct ARNs, got {arns:?}"
        );
        assert_eq!(
            hypervisor_count(&engine),
            racers,
            "case {case:?}: {racers} imports must leave {racers} records, got {}",
            hypervisor_count(&engine)
        );
        assert_eq!(
            hypervisor_arns_for_host(&engine, host),
            unique,
            "case {case:?}: every racer's record must be stored for host {host:?}, got {:?}",
            hypervisor_arns_for_host(&engine, host)
        );
    }
}

/// RULE (a stored record constrains no later call): once one caller holds a host, every
/// later import of that host mints its own record however many arrive at once, and
/// deleting one of them leaves the others exactly as they were.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_held_host_does_not_refuse_a_later_import() {
    let engine = build_engine();
    let router = Arc::new(build_router(&engine));
    let host = "198.51.100.42";

    let first = call(
        &router,
        "ImportHypervisorConfiguration",
        &import_body("hv-holder", host, None),
    )
    .await;
    assert_eq!(
        first.status, 200,
        "the first import of a host must answer 200, got {} {:?}",
        first.status, first.body
    );
    let held = first
        .str_at("/HypervisorArn")
        .unwrap_or_else(|| panic!("no HypervisorArn in {:?}", first.body))
        .to_string();

    // Rows: how many callers race for the host that is already held. Every one of them
    // mints its own record, whatever their count.
    let mut minted: BTreeSet<String> = BTreeSet::from([held.clone()]);
    for racers in [1usize, 2, 5] {
        let requests: Vec<(String, String, Option<String>)> = (0..racers)
            .map(|index| (format!("hv-late-{index}"), host.to_string(), kms_key_of(index)))
            .collect();
        for (index, reply) in race_imports(&router, requests).await {
            assert_eq!(
                reply.status, 200,
                "case {racers} late callers: racer {index} must answer 200, got {} {:?}",
                reply.status, reply.body
            );
            let arn = reply
                .str_at("/HypervisorArn")
                .unwrap_or_else(|| {
                    panic!("case {racers} late callers: racer {index}: no HypervisorArn in {:?}", reply.body)
                })
                .to_string();
            assert!(
                minted.insert(arn.clone()),
                "case {racers} late callers: racer {index} must mint an ARN of its own, got \
                 {arn:?} again"
            );
        }
        assert_eq!(
            hypervisor_count(&engine),
            minted.len(),
            "case {racers} late callers: every import must have left a record, got {} for \
             {} ARNs",
            hypervisor_count(&engine),
            minted.len()
        );
        assert_eq!(
            hypervisor_arns_for_host(&engine, host),
            minted,
            "case {racers} late callers: every record must be stored for host {host:?}, \
             got {:?}",
            hypervisor_arns_for_host(&engine, host)
        );
    }

    // Deleting ONE of the records that share the host removes that record and no other.
    let deleted = call(&router, "DeleteHypervisor", &json!({ "HypervisorArn": held })).await;
    assert_eq!(
        (deleted.status, deleted.str_at("/HypervisorArn")),
        (200, Some(held.as_str())),
        "deleting one of the records must answer 200 with its ARN, got {} {:?}",
        deleted.status,
        deleted.body
    );
    minted.remove(&held);
    assert_eq!(
        hypervisor_arns_for_host(&engine, host),
        minted,
        "the records that were not deleted must still be stored for host {host:?}, got {:?}",
        hypervisor_arns_for_host(&engine, host)
    );

    let again = call(
        &router,
        "ImportHypervisorConfiguration",
        &import_body("hv-reimported", host, None),
    )
    .await;
    assert_eq!(
        again.status, 200,
        "the host must still be importable, got {} {:?}",
        again.status, again.body
    );
    let reimported = again
        .str_at("/HypervisorArn")
        .unwrap_or_else(|| panic!("no HypervisorArn in {:?}", again.body))
        .to_string();
    assert_ne!(
        reimported, held,
        "the re-import must mint its own record, got the deleted ARN {held:?} again"
    );
    let read = get_hypervisor(&router, &reimported).await;
    assert_eq!(
        (read.status, read.str_at("/Hypervisor/Name")),
        (200, Some("hv-reimported")),
        "the re-imported record must read back, got {} {:?}",
        read.status,
        read.body
    );
    // The tags map below is only here to keep the assertion honest about what an import
    // with no tags persists: nothing.
    let listed = call(
        &router,
        "ListTagsForResource",
        &json!({ "ResourceArn": reimported }),
    )
    .await;
    assert_eq!(
        (listed.status, listed.tags()),
        (200, BTreeMap::new()),
        "an import carrying no tags must persist none, got {} {:?}",
        listed.status,
        listed.body
    );
}
