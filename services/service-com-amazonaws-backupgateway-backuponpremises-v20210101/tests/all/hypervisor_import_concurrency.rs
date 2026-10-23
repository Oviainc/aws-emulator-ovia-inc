// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ImportHypervisorConfiguration` reached CONCURRENTLY.
//!
//! The rule under test: an import mints one persisted hypervisor record per HOST and
//! nothing else — the record stands for the machine reachable at `Host`, so imports of
//! DISTINCT hosts are independent of one another (each mints its own record and its own
//! `HypervisorArn`, whatever `Name`, `KmsKeyArn` or `Tags` they share), while imports of
//! ONE host can only ever leave one record behind, so exactly one of them succeeds and
//! every other is refused with the modelled `ConflictException`. That holds however many
//! callers arrive at the same instant: the host lookup, the store bound and the insert
//! are one atomic step, not three a second caller can interleave with.
//!
//! The sibling suite `hypervisor_reads` pins the same rule for imports issued one after
//! another; these cases arrive at once, on a multi-threaded runtime, which is where a
//! lookup-then-insert that is not atomic reports two records for one host (or two
//! winners). The rows span the number of racing callers, the host form (IPv4, FQDN, the
//! modelled 3-character minimum), whether the racers share a `Name`, and whether they
//! carry a `KmsKeyArn` (in both forms its pattern accepts).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_backupgateway_backuponpremises_v20210101 as backup_gateway;

use backup_gateway::state::{service_state, HYPERVISOR_PENDING};

use crate::gateway_lifecycle::{build_engine, build_router, call, Reply};

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

/// How many hypervisors the store holds.
fn stored_hypervisors(engine: &Arc<engine::InMemoryEngine>) -> usize {
    service_state(&**engine)
        .expect("the in-memory engine must expose this state")
        .lock()
        .hypervisor_count()
}

/// The ARN the store holds the record for `host` under, if it holds one.
fn stored_arn_for_host(engine: &Arc<engine::InMemoryEngine>, host: &str) -> Option<String> {
    service_state(&**engine)
        .expect("the in-memory engine must expose this state")
        .lock()
        .hypervisor_by_host(host)
        .map(|record| record.arn.clone())
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
            stored_hypervisors(&engine),
            racers,
            "case {case:?}: the store must hold one record per host, got {} for {arns:?}",
            stored_hypervisors(&engine)
        );
        for (index, (_, host, _)) in requests.iter().enumerate() {
            assert_eq!(
                stored_arn_for_host(&engine, host).as_ref(),
                Some(&arns[index]),
                "case {case:?}: host {host:?} must be held under racer {index}'s ARN {:?}, \
                 got {:?}",
                arns[index],
                stored_arn_for_host(&engine, host)
            );
        }
    }
}

/// RULE: one host, one record — concurrent imports of the SAME host have exactly one
/// winner, and every loser is refused with the modelled `ConflictException` (HTTP 409)
/// while writing nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_imports_of_one_host_leave_exactly_one_record() {
    // (case, the contested host, how many callers race, do they all share one Name?)
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

        let mut winners = Vec::new();
        for (index, reply) in &replies {
            if reply.status == 200 {
                winners.push((
                    *index,
                    reply
                        .str_at("/HypervisorArn")
                        .unwrap_or_else(|| {
                            panic!("case {case:?}: winner {index}: no HypervisorArn in {:?}", reply.body)
                        })
                        .to_string(),
                ));
                continue;
            }
            assert_eq!(
                (
                    reply.status,
                    reply.code.as_deref(),
                    reply.str_at("/ErrorCode"),
                ),
                (409, Some("ConflictException"), Some("HypervisorAlreadyExists")),
                "case {case:?}: loser {index} must be refused with 409 ConflictException, \
                 got {} {:?}",
                reply.status,
                reply.body
            );
            assert!(
                reply.str_at("/HypervisorArn").is_none(),
                "case {case:?}: loser {index} must hand back no HypervisorArn, got {:?}",
                reply.body
            );
        }

        assert_eq!(
            winners.len(),
            1,
            "case {case:?}: {racers} callers racing for one host must have exactly one \
             winner, got {winners:?}"
        );
        let (winner_index, winner_arn) = &winners[0];
        assert_eq!(
            stored_hypervisors(&engine),
            1,
            "case {case:?}: one host must leave exactly one record, got {}",
            stored_hypervisors(&engine)
        );
        assert_eq!(
            stored_arn_for_host(&engine, host).as_ref(),
            Some(winner_arn),
            "case {case:?}: the store must hold the host under the winner's ARN \
             {winner_arn:?}, got {:?}",
            stored_arn_for_host(&engine, host)
        );

        // The record the host is held under is the WINNER's request, not a loser's: it
        // reads back with the name and KMS key that caller asked for.
        let (name, _, kms_key_arn) = &requests[*winner_index];
        let read = get_hypervisor(&router, winner_arn).await;
        assert_eq!(
            (
                read.status,
                read.str_at("/Hypervisor/Name"),
                read.str_at("/Hypervisor/Host"),
                read.str_at("/Hypervisor/KmsKeyArn"),
            ),
            (
                200,
                Some(name.as_str()),
                Some(host),
                kms_key_arn.as_deref(),
            ),
            "case {case:?}: the surviving record must be racer {winner_index}'s, got {} {:?}",
            read.status,
            read.body
        );
    }
}

/// RULE (the refusal is about the host alone, and it survives the race): once one caller
/// holds a host, EVERY later import of that host is refused however many arrive at once,
/// and deleting the record makes the host importable again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_held_host_refuses_every_racing_import_until_it_is_deleted() {
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
        "the first import of a free host must answer 200, got {} {:?}",
        first.status, first.body
    );
    let held = first
        .str_at("/HypervisorArn")
        .unwrap_or_else(|| panic!("no HypervisorArn in {:?}", first.body))
        .to_string();

    // Rows: how many callers race for the host that is already held. None of them may
    // win, whatever their count.
    for racers in [1usize, 2, 5] {
        let requests: Vec<(String, String, Option<String>)> = (0..racers)
            .map(|index| (format!("hv-late-{index}"), host.to_string(), kms_key_of(index)))
            .collect();
        for (index, reply) in race_imports(&router, requests).await {
            assert_eq!(
                (reply.status, reply.code.as_deref()),
                (409, Some("ConflictException")),
                "case {racers} late callers: racer {index} must be refused with 409 \
                 ConflictException, got {} {:?}",
                reply.status,
                reply.body
            );
        }
        assert_eq!(
            stored_hypervisors(&engine),
            1,
            "case {racers} late callers: the refused imports must have written nothing, \
             got {} records",
            stored_hypervisors(&engine)
        );
    }

    // The conflict is keyed on a record the store HOLDS, so removing the record frees
    // the host: the next import of it succeeds and mints a new ARN.
    let deleted = call(&router, "DeleteHypervisor", &json!({ "HypervisorArn": held })).await;
    assert_eq!(
        (deleted.status, deleted.str_at("/HypervisorArn")),
        (200, Some(held.as_str())),
        "deleting the holder must answer 200 with its ARN, got {} {:?}",
        deleted.status,
        deleted.body
    );

    let again = call(
        &router,
        "ImportHypervisorConfiguration",
        &import_body("hv-reimported", host, None),
    )
    .await;
    let reimported = again
        .str_at("/HypervisorArn")
        .unwrap_or_else(|| panic!("no HypervisorArn in {:?}", again.body))
        .to_string();
    assert_eq!(
        again.status, 200,
        "a host whose record was deleted must be importable again, got {} {:?}",
        again.status, again.body
    );
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
