// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateHypervisor`.
//!
//! Every case drives a real `POST /` awsJson1_0 request through the
//! `MultiServiceRouter`, reusing the harness of `gateway_lifecycle`, so what is asserted
//! is the wire behaviour, and the only way a later request can see what an earlier one
//! wrote is the shared store.
//!
//! The rule under test: `UpdateHypervisor` resolves the hypervisor its `ServerArn` names
//! out of the persisted store and writes ONLY the members the request carries, so every
//! later read reports the record the last write left and a member the request omitted
//! keeps the value it had; an ARN the store holds no hypervisor under is refused as a
//! missing hypervisor and nothing is written; and because the ARN is a record's only
//! identity, the VALUES an update carries never decide whether it may be written — a
//! `Host` another record already holds included.
//!
//! The tables below span that class — updates differing in which members they carry, in
//! their values and lengths (each published bound of the modelled members, and one past
//! it), and ARNs differing in resource type, partition, id and length — rather than one
//! recorded request.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_backupgateway_backuponpremises_v20210101 as backup_gateway;

use backup_gateway::state::{
    service_state, HYPERVISOR_NOT_FOUND, HYPERVISOR_ONLINE, HYPERVISOR_PENDING,
};

use crate::gateway_lifecycle::{
    build_engine, build_router, call, create, hypervisor_arns_for_host, hypervisor_count, Reply,
    REGION,
};

/// An ARN of this service naming `resource_type`/`id`, in the region the harness signs
/// for.
fn arn_of(resource_type: &str, id: &str) -> String {
    format!(
        "arn:aws:backup-gateway:{REGION}:{}:{resource_type}/{id}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// A hypervisor ARN in `partition`, so the rows span every partition the modelled
/// `ServerArn` pattern accepts (`aws`, `aws-cn`, `aws-us-gov`).
fn arn_in_partition(partition: &str, id: &str) -> String {
    format!(
        "arn:{partition}:backup-gateway:{REGION}:{}:hypervisor/{id}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// A `ServerArn`-shaped ARN of exactly `len` characters, for the published length bounds
/// of the modelled shape: `@length(min: 50, max: 500)`
/// (`smithy_query(query="shape",shape="ServerArn")`). The id segment absorbs the length,
/// so every character stays inside the pattern's `[a-zA-Z-0-9]+`.
fn arn_of_length(len: usize) -> String {
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

/// A `LogGroupArn` of the form the member's pattern accepts:
/// `^$|^arn:(aws|aws-cn|aws-us-gov):logs:([a-zA-Z0-9-]+):([0-9]+):log-group:[a-zA-Z0-9_\-\/\.]+:\*$`
/// (`smithy_query(query="shape",shape="LogGroupArn")`).
fn log_group_arn(name: &str) -> String {
    format!(
        "arn:aws:logs:{REGION}:{}:log-group:{name}:*",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// The refusal message, wherever the envelope carries it: a refusal the framework made
/// before the handler ran states it in `message`, a modelled error the handler returned
/// in the `Message` member.
fn message_of(reply: &Reply) -> String {
    reply
        .str_at("/message")
        .or_else(|| reply.str_at("/Message"))
        .unwrap_or_else(|| panic!("no message in {:?}", reply.body))
        .to_string()
}

/// Import one hypervisor over the wire and hand back the ARN the store holds it under.
async fn import(router: &MultiServiceRouter, name: &str, host: &str) -> String {
    let reply = call(
        router,
        "ImportHypervisorConfiguration",
        &json!({ "Name": name, "Host": host }),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "case {name:?}: ImportHypervisorConfiguration must answer 200, got {} {:?}",
        reply.status, reply.body
    );
    reply
        .str_at("/HypervisorArn")
        .unwrap_or_else(|| panic!("no HypervisorArn in {:?}", reply.body))
        .to_string()
}

async fn update(router: &MultiServiceRouter, body: &Value) -> Reply {
    call(router, "UpdateHypervisor", body).await
}

async fn get_hypervisor(router: &MultiServiceRouter, arn: &str) -> Reply {
    call(router, "GetHypervisor", &json!({ "HypervisorArn": arn })).await
}

/// `Name`, `Host` and `LogGroupArn` as `GetHypervisor` reports them for `arn` — the
/// round-trip every update below is checked through.
async fn reported(router: &MultiServiceRouter, arn: &str) -> (String, String, Option<String>) {
    let reply = get_hypervisor(router, arn).await;
    assert_eq!(
        reply.status, 200,
        "GetHypervisor on {arn} must answer 200, got {} {:?}",
        reply.status, reply.body
    );
    (
        reply.str_at("/Hypervisor/Name").unwrap_or_default().to_string(),
        reply.str_at("/Hypervisor/Host").unwrap_or_default().to_string(),
        reply.str_at("/Hypervisor/LogGroupArn").map(str::to_string),
    )
}

/// The `Host` the STORE holds for `arn`, read out of the record rather than off the wire.
fn persisted_host(engine: &Arc<engine::InMemoryEngine>, arn: &str) -> String {
    service_state(&**engine)
        .expect("the in-memory engine must expose this state")
        .lock()
        .hypervisor(arn)
        .map(|record| record.host.clone())
        .unwrap_or_else(|| panic!("no hypervisor stored under {arn}"))
}

/// The whole stored record, rendered — used to assert that an update's `@sensitive`
/// members never reach the store.
fn persisted_debug(engine: &Arc<engine::InMemoryEngine>, arn: &str) -> String {
    service_state(&**engine)
        .expect("the in-memory engine must expose this state")
        .lock()
        .hypervisor(arn)
        .map(|record| format!("{record:?}"))
        .unwrap_or_else(|| panic!("no hypervisor stored under {arn}"))
}

/// One update request, and the record it must leave behind.
#[derive(Debug)]
struct Row {
    case: &'static str,
    /// The members the request carries beside `HypervisorArn`.
    members: Vec<(&'static str, Value)>,
    /// `Name`, `Host` and `LogGroupArn` a read must report AFTER this update — the
    /// accumulated record, so a row that omits a member asserts the earlier value stood.
    expected: (String, String, Option<String>),
}

/// An update writes exactly the members it carries, and a later read reports the record
/// it left: the rows below span which members an update states (one, two, all, none), the
/// published length bounds of `Name` and `Host`, both host forms, setting and clearing the
/// log group, and a request carrying only the `@sensitive` credentials.
#[tokio::test]
async fn an_update_writes_only_the_members_it_carries_and_reads_back() {
    let engine = build_engine();
    let router = build_router(&engine);
    let arn = import(&router, "hv-start", "198.51.100.10").await;

    let password = "s3cret-pa55word";
    let username = "Administrator@example";
    // `Name` is len 1..100, pattern `^[a-zA-Z0-9-]*$`; `Host` is len 3..128, pattern
    // `^.+$` (`smithy_query(query="shape",shape="Name,Host")`).
    let longest_name = "n".repeat(100);
    let shortest_host = "1.1";
    let longest_host = format!("{}.example.com", "h".repeat(128 - ".example.com".len()));
    let first_log_group = log_group_arn("/aws/backup-gateway/one");
    let second_log_group = log_group_arn("/aws/backup-gateway/two");

    let rows: Vec<Row> = vec![
        Row {
            case: "the name alone",
            members: vec![("Name", json!("hv-renamed"))],
            expected: (
                "hv-renamed".to_string(),
                "198.51.100.10".to_string(),
                None,
            ),
        },
        Row {
            case: "the host alone, as an FQDN",
            members: vec![("Host", json!("hypervisor.example.com"))],
            expected: (
                "hv-renamed".to_string(),
                "hypervisor.example.com".to_string(),
                None,
            ),
        },
        Row {
            case: "a log group where the record carried none",
            members: vec![("LogGroupArn", json!(first_log_group))],
            expected: (
                "hv-renamed".to_string(),
                "hypervisor.example.com".to_string(),
                Some(first_log_group.clone()),
            ),
        },
        Row {
            case: "only the sensitive credentials",
            members: vec![
                ("Username", json!(username)),
                ("Password", json!(password)),
            ],
            expected: (
                "hv-renamed".to_string(),
                "hypervisor.example.com".to_string(),
                Some(first_log_group.clone()),
            ),
        },
        Row {
            case: "a log group replacing the one the record carried",
            members: vec![("LogGroupArn", json!(second_log_group))],
            expected: (
                "hv-renamed".to_string(),
                "hypervisor.example.com".to_string(),
                Some(second_log_group.clone()),
            ),
        },
        Row {
            // The member's pattern accepts the empty string (`^$|^arn:…`), which is the
            // only way a caller can state "no log group".
            case: "the empty log group, which detaches the one the record carried",
            members: vec![("LogGroupArn", json!(""))],
            expected: (
                "hv-renamed".to_string(),
                "hypervisor.example.com".to_string(),
                None,
            ),
        },
        Row {
            case: "every member at once, at the shortest host the model allows",
            members: vec![
                ("Name", json!("h")),
                ("Host", json!(shortest_host)),
                ("LogGroupArn", json!(first_log_group)),
                ("Username", json!(username)),
                ("Password", json!(password)),
            ],
            expected: (
                "h".to_string(),
                shortest_host.to_string(),
                Some(first_log_group.clone()),
            ),
        },
        Row {
            case: "the longest name and host the model allows",
            members: vec![
                ("Name", json!(longest_name)),
                ("Host", json!(longest_host)),
            ],
            expected: (
                longest_name.clone(),
                longest_host.clone(),
                Some(first_log_group.clone()),
            ),
        },
        Row {
            case: "nothing beyond the ARN",
            members: Vec::new(),
            expected: (
                longest_name.clone(),
                longest_host.clone(),
                Some(first_log_group.clone()),
            ),
        },
    ];

    for row in &rows {
        let mut body = json!({ "HypervisorArn": arn });
        for (member, value) in &row.members {
            body[*member] = value.clone();
        }
        let reply = update(&router, &body).await;
        assert_eq!(
            (reply.status, reply.str_at("/HypervisorArn")),
            (200, Some(arn.as_str())),
            "case {:?}: an update of a stored hypervisor must answer 200 with the ARN the \
             store holds it under, got {} {:?}",
            row.case,
            reply.status,
            reply.body
        );

        let actual = reported(&router, &arn).await;
        assert_eq!(
            actual, row.expected,
            "case {:?}: expected (Name, Host, LogGroupArn) {:?}, got {actual:?}",
            row.case, row.expected
        );
    }

    // The `@sensitive` `Username` / `Password` an update carries are credentials no
    // response member reports, so the store never holds them.
    let stored = persisted_debug(&engine, &arn);
    for secret in [password, username] {
        assert!(
            !stored.contains(secret),
            "an update must not persist a sensitive credential, but the stored record \
             carries {secret:?}: {stored}"
        );
    }

    // Nothing an update is not asked to change moves: the record is still the one the
    // import minted, with the activation that import started still derived from the clock.
    let state = service_state(&*engine).expect("the in-memory engine must expose this state");
    assert_eq!(
        state
            .lock()
            .hypervisor(&arn)
            .map(|record| record.state.clone()),
        Some(HYPERVISOR_PENDING.to_string()),
        "an update must not move the persisted HypervisorState"
    );
    state
        .lock()
        .hypervisor_mut(&arn)
        .expect("the updated record must be in the store")
        .online_due = Some(SystemTime::now() - Duration::from_millis(1));
    let reply = get_hypervisor(&router, &arn).await;
    assert_eq!(
        (reply.status, reply.str_at("/Hypervisor/State")),
        (200, Some(HYPERVISOR_ONLINE)),
        "an updated hypervisor whose activation is due must still report ONLINE, got {} {:?}",
        reply.status,
        reply.body
    );
}

/// An `UpdateHypervisor` whose `HypervisorArn` names no stored hypervisor is refused as a
/// missing hypervisor and writes nothing — whether the ARN was never imported, names a
/// record that has been deleted, or spells another resource type or partition altogether.
#[tokio::test]
async fn an_arn_with_no_stored_hypervisor_is_refused_as_a_missing_hypervisor() {
    let engine = build_engine();
    let router = build_router(&engine);
    let live = import(&router, "hv-live", "198.51.100.11").await;
    let gateway = create(&router, "gw-for-arn-rows").await;

    // A record imported and then deleted: its ARN is well formed and was real, and is now
    // behind no record at all.
    let deleted = import(&router, "hv-gone", "198.51.100.12").await;
    let removed = call(&router, "DeleteHypervisor", &json!({ "HypervisorArn": deleted })).await;
    assert_eq!(
        removed.status, 200,
        "DeleteHypervisor must answer 200, got {} {:?}",
        removed.status, removed.body
    );

    // (case, the ARN to update). The rows span resource type, partition, id and each
    // published length bound of the modelled `ServerArn` (50 and 500).
    let rows: Vec<(&str, String)> = vec![
        ("a hypervisor ARN nothing imported", arn_of("hypervisor", "hv-0000000000000999")),
        ("the ARN of a deleted hypervisor", deleted.clone()),
        ("a gateway ARN", gateway.clone()),
        ("a virtual-machine ARN", arn_of("vm", "vm-0000000000000001")),
        ("an ARN in the aws-cn partition", arn_in_partition("aws-cn", "hv-0000000000000001")),
        (
            "an ARN in the aws-us-gov partition",
            arn_in_partition("aws-us-gov", "hv-0000000000000001"),
        ),
        ("an ARN at the 50-character minimum", arn_of_length(50)),
        ("an ARN at the 500-character maximum", arn_of_length(500)),
    ];

    for (case, arn) in &rows {
        let reply = update(&router, &json!({ "HypervisorArn": arn, "Name": "hv-nope" })).await;
        assert_eq!(
            (reply.status, reply.code.as_deref()),
            (400, Some("ResourceNotFoundException")),
            "case {case:?}: an ARN with no stored hypervisor must be refused with 400 \
             ResourceNotFoundException, got {} {:?}",
            reply.status,
            reply.body
        );
        assert_eq!(
            reply.str_at("/ErrorCode"),
            Some(HYPERVISOR_NOT_FOUND),
            "case {case:?}: the refusal must state the missing-hypervisor error code, got \
             {:?}",
            reply.body
        );
        assert!(
            !message_of(&reply).is_empty(),
            "case {case:?}: the refusal must state a message, got {:?}",
            reply.body
        );
    }

    // One ARN past each published bound reaches the boundary validator instead, which is a
    // malformed request and not a missing resource.
    for (case, arn) in [
        ("one character under the minimum", arn_of_length(49)),
        ("one character over the maximum", arn_of_length(501)),
    ] {
        let reply = update(&router, &json!({ "HypervisorArn": arn })).await;
        assert_eq!(
            (reply.status, reply.code.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: an ARN outside the modelled length must be refused with 400 \
             ValidationException, got {} {:?}",
            reply.status,
            reply.body
        );
    }

    // None of those refusals touched the hypervisor the store does hold.
    let actual = reported(&router, &live).await;
    assert_eq!(
        actual,
        ("hv-live".to_string(), "198.51.100.11".to_string(), None),
        "a refused update must leave every stored record as it was, got {actual:?}"
    );
}

/// A hypervisor record is identified by the ARN minted for it and by nothing else, so an
/// update that gives two records one host is written like any other: it answers 200,
/// persists every member the same request carried, and leaves the other record reporting
/// its own host. The rows span every pairing of a stored record with ANOTHER record's
/// host, so this is not a property of one row; setting a record's own host again and
/// moving onto the host of a record that has been deleted are the same write.
#[tokio::test]
async fn an_update_onto_a_host_another_hypervisor_holds_is_written() {
    let hosts = ["198.51.100.20", "hypervisor-two.example.com", "10.0.0.3"];

    for taker in 0..hosts.len() {
        for holder in 0..hosts.len() {
            if taker == holder {
                continue;
            }
            // A store of its own per row, so each pairing starts from the same three
            // records and the rows cannot mask one another.
            let engine = build_engine();
            let router = build_router(&engine);
            let mut arns = Vec::new();
            for (index, host) in hosts.iter().enumerate() {
                arns.push(import(&router, &format!("hv-{index}"), host).await);
            }

            let case = format!("hypervisor {taker} onto the host of hypervisor {holder}");
            let reply = update(
                &router,
                &json!({
                    "HypervisorArn": arns[taker],
                    "Host": hosts[holder],
                    "Name": "hv-moved",
                }),
            )
            .await;
            assert_eq!(
                (reply.status, reply.str_at("/HypervisorArn")),
                (200, Some(arns[taker].as_str())),
                "case {case:?}: moving a hypervisor onto a host another one holds must \
                 answer 200 with the ARN the store holds it under, got {} {:?}",
                reply.status,
                reply.body
            );

            // Every member the request carried reached the record, the host included.
            let actual = reported(&router, &arns[taker]).await;
            assert_eq!(
                actual,
                ("hv-moved".to_string(), hosts[holder].to_string(), None),
                "case {case:?}: the updated record must report what the request carried, \
                 got {actual:?}"
            );
            // And the record that already held the host is untouched.
            let holder_record = reported(&router, &arns[holder]).await;
            assert_eq!(
                holder_record,
                (format!("hv-{holder}"), hosts[holder].to_string(), None),
                "case {case:?}: the record that already held the host must be unchanged, \
                 got {holder_record:?}"
            );
            // One host, two records, and no record minted or dropped by the update.
            assert_eq!(
                hypervisor_arns_for_host(&engine, hosts[holder]),
                BTreeSet::from([arns[taker].clone(), arns[holder].clone()]),
                "case {case:?}: both records must be stored for host {:?}",
                hosts[holder]
            );
            assert_eq!(
                hypervisor_count(&engine),
                hosts.len(),
                "case {case:?}: an update must neither mint nor drop a record"
            );
        }
    }

    let engine = build_engine();
    let router = build_router(&engine);
    let mut arns = Vec::new();
    for (index, host) in hosts.iter().enumerate() {
        arns.push(import(&router, &format!("hv-{index}"), host).await);
    }

    // Setting a record's own host again is the same record: it succeeds and changes
    // nothing.
    for (index, arn) in arns.iter().enumerate() {
        let case = format!("hypervisor {index} onto its own host");
        let reply = update(&router, &json!({ "HypervisorArn": arn, "Host": hosts[index] })).await;
        assert_eq!(
            (reply.status, reply.str_at("/HypervisorArn")),
            (200, Some(arn.as_str())),
            "case {case:?}: setting the host a record already has must answer 200, got {} {:?}",
            reply.status,
            reply.body
        );
        assert_eq!(
            persisted_host(&engine, arn),
            hosts[index],
            "case {case:?}: the stored host must be unchanged"
        );
    }

    // The host of a record that has been DELETED is written exactly like the host of one
    // still stored: nothing about a host makes an update conditional.
    let freed = call(
        &router,
        "DeleteHypervisor",
        &json!({ "HypervisorArn": arns[1] }),
    )
    .await;
    assert_eq!(
        freed.status, 200,
        "DeleteHypervisor must answer 200, got {} {:?}",
        freed.status, freed.body
    );
    let reply = update(&router, &json!({ "HypervisorArn": arns[0], "Host": hosts[1] })).await;
    assert_eq!(
        (reply.status, reply.str_at("/HypervisorArn")),
        (200, Some(arns[0].as_str())),
        "case a freed host: an update onto the host of a deleted record must answer 200, \
         got {} {:?}",
        reply.status,
        reply.body
    );
    assert_eq!(
        persisted_host(&engine, &arns[0]),
        hosts[1],
        "case a freed host: the store must hold the new host"
    );
}

/// Every stored hypervisor is updated through its own ARN: one update changes one record
/// and no other, whichever record it names.
#[tokio::test]
async fn an_update_changes_exactly_the_record_its_arn_names() {
    let engine = build_engine();
    let router = build_router(&engine);

    let mut expected: BTreeMap<String, (String, String)> = BTreeMap::new();
    let mut arns = Vec::new();
    for index in 0..4 {
        let name = format!("hv-{index}");
        let host = format!("198.51.100.{}", 30 + index);
        let arn = import(&router, &name, &host).await;
        expected.insert(arn.clone(), (name, host));
        arns.push(arn);
    }

    for (index, arn) in arns.iter().enumerate() {
        let name = format!("hv-updated-{index}");
        let host = format!("updated-{index}.example.com");
        let reply = update(
            &router,
            &json!({ "HypervisorArn": arn, "Name": name, "Host": host }),
        )
        .await;
        assert_eq!(
            reply.status, 200,
            "case {arn}: the update must answer 200, got {} {:?}",
            reply.status, reply.body
        );
        expected.insert(arn.clone(), (name, host));

        for (other, (expected_name, expected_host)) in &expected {
            let actual = reported(&router, other).await;
            assert_eq!(
                (actual.0.as_str(), actual.1.as_str()),
                (expected_name.as_str(), expected_host.as_str()),
                "case {arn} updated: {other} must report its own record, got {actual:?}"
            );
        }
    }
}
