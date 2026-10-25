// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the identity of a hypervisor record.
//!
//! Every case drives a real `POST /` awsJson1_0 request through the `MultiServiceRouter`,
//! reusing the harness of `gateway_lifecycle`, so what is asserted is the wire behaviour.
//!
//! The rule under test: a hypervisor is identified by the `HypervisorArn` minted for it
//! and by nothing else, so `Host` is metadata the record carries rather than a key — any
//! number of records may hold one host, and no call is refused because another record
//! already carries the host it asks for. That holds for whichever path writes the host:
//! `ImportHypervisorConfiguration` mints a record per call, and `UpdateHypervisor` writes
//! the host its request names onto the record its ARN names.
//!
//! The table below spans that class: hosts differing in form and in length (both published
//! bounds of the modelled member), the number of records that end up sharing one host, and
//! how each record got there — minted at the host by an import, or moved onto it by an
//! update.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_backupgateway_backuponpremises_v20210101 as backup_gateway;

use backup_gateway::state::service_state;

use crate::gateway_lifecycle::{build_engine, build_router, call, Reply};

/// `Host` is `@length(min: 3, max: 128)` with pattern `^.+$`
/// (`smithy_query(query="shape",shape="Host")`), so every one of these is a host the
/// member accepts: the pattern constrains nothing beyond non-emptiness, and the two
/// extremes are the published bounds.
fn hosts_spanning_the_member() -> Vec<(&'static str, String)> {
    vec![
        ("an IPv4 host", "198.51.100.5".to_string()),
        ("an FQDN host", "hypervisor.example.com".to_string()),
        ("the 3-character minimum", "a.b".to_string()),
        (
            "the 128-character maximum",
            format!("{}.example.com", "h".repeat(128 - ".example.com".len())),
        ),
        // `^.+$` accepts any non-empty run of characters, spaces included.
        ("a host carrying spaces", "the hypervisor box".to_string()),
    ]
}

/// `Name`, `Host` and `LogGroupArn` a read must report for a record.
type Reported<'a> = (&'a str, &'a str, Option<&'a str>);

/// One row: how many records end up on the shared host, and by which path each got there.
#[derive(Debug)]
struct Row {
    case: &'static str,
    /// How many records are MINTED at the shared host by an import.
    imported_at_host: usize,
    /// How many records are minted elsewhere and then MOVED onto the shared host by an
    /// update.
    updated_onto_host: usize,
}

async fn import(router: &MultiServiceRouter, name: &str, host: &str) -> Reply {
    call(
        router,
        "ImportHypervisorConfiguration",
        &json!({ "Name": name, "Host": host }),
    )
    .await
}

async fn get_hypervisor(router: &MultiServiceRouter, arn: &str) -> Reply {
    call(router, "GetHypervisor", &json!({ "HypervisorArn": arn })).await
}

/// The ARNs the STORE holds a record for `host` under, read out of the records.
fn persisted_arns_for_host(engine: &Arc<engine::InMemoryEngine>, host: &str) -> BTreeSet<String> {
    service_state(&**engine)
        .expect("the in-memory engine must expose this state")
        .lock()
        .hypervisors_for_host(host)
        .iter()
        .map(|record| record.arn.clone())
        .collect()
}

/// Every `HypervisorArn` `ListHypervisors` reports.
fn listed_arns(reply: &Reply) -> BTreeSet<String> {
    reply
        .body
        .get("Hypervisors")
        .and_then(Value::as_array)
        .map(|hypervisors| {
            hypervisors
                .iter()
                .filter_map(|hypervisor| {
                    hypervisor
                        .get("HypervisorArn")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// RULE: any number of hypervisor records may carry one `Host`, whichever path wrote it,
/// and each record keeps its own members — so neither an import nor an update is refused
/// for the host it asks for.
#[tokio::test]
async fn any_number_of_hypervisors_may_share_one_host() {
    let rows = [
        Row {
            case: "two records, both imported at the host",
            imported_at_host: 2,
            updated_onto_host: 0,
        },
        Row {
            case: "two records, one imported at the host and one moved onto it",
            imported_at_host: 1,
            updated_onto_host: 1,
        },
        Row {
            case: "three records, all moved onto the host",
            imported_at_host: 0,
            updated_onto_host: 3,
        },
        Row {
            case: "five records, by both paths",
            imported_at_host: 2,
            updated_onto_host: 3,
        },
    ];

    for row in &rows {
        for (host_case, host) in hosts_spanning_the_member() {
            // A store of its own per case, so the record count asserted below is exactly
            // what this case wrote.
            let engine = build_engine();
            let router = build_router(&engine);
            let case = format!("{} at {host_case}", row.case);

            // (the ARN, the Name the record must report)
            let mut records: Vec<(String, String)> = Vec::new();

            for index in 0..row.imported_at_host {
                let name = format!("hv-imported-{index}");
                let reply = import(&router, &name, &host).await;
                assert_eq!(
                    reply.status, 200,
                    "case {case:?}: import {index} at a host {} other record(s) already \
                     hold must answer 200, got {} {:?}",
                    records.len(),
                    reply.status,
                    reply.body
                );
                let arn = reply
                    .str_at("/HypervisorArn")
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: no HypervisorArn in {:?}", reply.body)
                    })
                    .to_string();
                records.push((arn, name));
            }

            for index in 0..row.updated_onto_host {
                // Minted somewhere else, so the update is what puts it on the shared host.
                let seed = import(&router, &format!("hv-seed-{index}"), &format!("10.0.0.{index}"))
                    .await;
                assert_eq!(
                    seed.status, 200,
                    "case {case:?}: the record to move must import, got {} {:?}",
                    seed.status, seed.body
                );
                let arn = seed
                    .str_at("/HypervisorArn")
                    .unwrap_or_else(|| panic!("case {case:?}: no HypervisorArn in {:?}", seed.body))
                    .to_string();

                let name = format!("hv-moved-{index}");
                let moved = call(
                    &router,
                    "UpdateHypervisor",
                    &json!({ "HypervisorArn": arn, "Host": host, "Name": name }),
                )
                .await;
                assert_eq!(
                    (moved.status, moved.str_at("/HypervisorArn")),
                    (200, Some(arn.as_str())),
                    "case {case:?}: moving a record onto a host {} other record(s) hold \
                     must answer 200 with its ARN, got {} {:?}",
                    records.len(),
                    moved.status,
                    moved.body
                );
                records.push((arn, name));
            }

            let arns: BTreeSet<String> = records.iter().map(|(arn, _)| arn.clone()).collect();
            assert_eq!(
                arns.len(),
                records.len(),
                "case {case:?}: every record must have an ARN of its own, got {records:?}"
            );

            // Each record reports its OWN members, and the host every one of them carries.
            for (arn, name) in &records {
                let reply = get_hypervisor(&router, arn).await;
                assert_eq!(
                    (
                        reply.status,
                        reply.str_at("/Hypervisor/Name"),
                        reply.str_at("/Hypervisor/Host"),
                    ),
                    (200, Some(name.as_str()), Some(host.as_str())),
                    "case {case:?}: {arn} must report its own Name and the shared Host, \
                     got {} {:?}",
                    reply.status,
                    reply.body
                );
            }

            // The store holds them all, and so does a listing.
            assert_eq!(
                persisted_arns_for_host(&engine, &host),
                arns,
                "case {case:?}: every record must be stored for the shared host, got {:?}",
                persisted_arns_for_host(&engine, &host)
            );
            let listing = call(&router, "ListHypervisors", &json!({})).await;
            assert_eq!(
                listing.status, 200,
                "case {case:?}: ListHypervisors must answer 200, got {} {:?}",
                listing.status, listing.body
            );
            assert!(
                listed_arns(&listing).is_superset(&arns),
                "case {case:?}: a listing must report every record, got {:?}",
                listed_arns(&listing)
            );
        }
    }
}

/// RULE (the same rule, for the OTHER members an update carries): `Name` and
/// `LogGroupArn` are metadata too, so an update that gives two records one name or one log
/// group is written exactly like any other.
#[tokio::test]
async fn an_update_may_duplicate_any_member_of_another_record() {
    let engine = build_engine();
    let router = build_router(&engine);

    let log_group = format!(
        "arn:aws:logs:us-west-2:{}:log-group:/aws/backup-gateway/shared:*",
        engine::EMULATOR_ACCOUNT_ID
    );

    let first = import(&router, "hv-original", "198.51.100.60").await;
    let first_arn = first
        .str_at("/HypervisorArn")
        .unwrap_or_else(|| panic!("no HypervisorArn in {:?}", first.body))
        .to_string();
    let set_log_group = call(
        &router,
        "UpdateHypervisor",
        &json!({ "HypervisorArn": first_arn, "LogGroupArn": log_group }),
    )
    .await;
    assert_eq!(
        set_log_group.status, 200,
        "the first record must take its log group, got {} {:?}",
        set_log_group.status, set_log_group.body
    );

    let second = import(&router, "hv-other", "198.51.100.61").await;
    let second_arn = second
        .str_at("/HypervisorArn")
        .unwrap_or_else(|| panic!("no HypervisorArn in {:?}", second.body))
        .to_string();

    // (case, the members the second record's update carries, what it must then report)
    let rows: Vec<(&str, Value, Reported<'_>)> = vec![
        (
            "the name another record holds",
            json!({ "Name": "hv-original" }),
            ("hv-original", "198.51.100.61", None),
        ),
        (
            "the log group another record holds",
            json!({ "LogGroupArn": log_group.clone() }),
            ("hv-original", "198.51.100.61", Some(log_group.as_str())),
        ),
        (
            "the name, host and log group another record holds, all at once",
            json!({
                "Name": "hv-original",
                "Host": "198.51.100.60",
                "LogGroupArn": log_group.clone(),
            }),
            ("hv-original", "198.51.100.60", Some(log_group.as_str())),
        ),
    ];

    for (case, members, expected) in &rows {
        let mut body = json!({ "HypervisorArn": second_arn });
        for (member, value) in members.as_object().expect("the members are an object") {
            body[member] = value.clone();
        }
        let reply = call(&router, "UpdateHypervisor", &body).await;
        assert_eq!(
            (reply.status, reply.str_at("/HypervisorArn")),
            (200, Some(second_arn.as_str())),
            "case {case:?}: an update carrying a value another record holds must answer \
             200 with its ARN, got {} {:?}",
            reply.status,
            reply.body
        );

        let read = get_hypervisor(&router, &second_arn).await;
        let actual = (
            read.str_at("/Hypervisor/Name"),
            read.str_at("/Hypervisor/Host"),
            read.str_at("/Hypervisor/LogGroupArn"),
        );
        assert_eq!(
            actual,
            (Some(expected.0), Some(expected.1), expected.2),
            "case {case:?}: expected (Name, Host, LogGroupArn) {expected:?}, got {actual:?}"
        );

        // The record it copied from is untouched.
        let other = get_hypervisor(&router, &first_arn).await;
        let other_actual = (
            other.str_at("/Hypervisor/Name"),
            other.str_at("/Hypervisor/Host"),
            other.str_at("/Hypervisor/LogGroupArn"),
        );
        assert_eq!(
            other_actual,
            (
                Some("hv-original"),
                Some("198.51.100.60"),
                Some(log_group.as_str())
            ),
            "case {case:?}: the record copied from must be unchanged, got {other_actual:?}"
        );
    }
}
