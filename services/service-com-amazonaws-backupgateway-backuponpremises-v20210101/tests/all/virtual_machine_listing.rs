// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListVirtualMachines`.
//!
//! Every case drives a real `POST /` awsJson1_0 request through the
//! `MultiServiceRouter` — the harness of `gateway_lifecycle` — so what is asserted is the
//! wire behaviour of the operation.
//!
//! The rule under test: `ListVirtualMachines` reports the virtual machines the store
//! holds on the hypervisor the request names — every machine the store holds when it
//! names none — ordered by their ARNs, one `MaxResults`-sized page at a time, each
//! record's members exactly as the store holds them. So a page is a window on persisted
//! state, a token resumes after the boundary it encodes, a token this listing never
//! issued is refused, and a hypervisor the service holds no inventory for lists nothing
//! rather than being refused (the operation declares no `ResourceNotFoundException`:
//! `smithy_query(query="operation",operation="ListVirtualMachines")`).
//!
//! A virtual machine is not created by any operation of this model: it is part of the
//! inventory of the hypervisor it runs on, which reaches the service out of band
//! (`state::record_virtual_machine`) — the same kind of out-of-band channel an
//! activation key reaches the service through. The tables
//! below span the class the rule generalises over: machines differing in name length
//! (each published bound of the modelled `Name`), spelling and case, host name, path form
//! and length (each published bound of the modelled `Path`), and whether they have ever
//! been backed up; several hypervisors including one with no inventory at all; every page
//! size over a fixed set of records, filtered and unfiltered; and every form a bad token
//! can take.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_backupgateway_backuponpremises_v20210101 as backup_gateway;

use backup_gateway::state::{service_state, VirtualMachineInventory, PAGE_TOKEN_INVALID_MESSAGE};

use crate::gateway_lifecycle::{build_engine, build_router, call, create, Reply, REGION};

/// A `LastBackupDate` to report: a fixed instant, so the assertion is about the value the
/// store holds rather than about the clock.
const LAST_BACKUP_EPOCH_SECS: u64 = 1_700_000_000;

/// One virtual machine of a hypervisor's inventory.
#[derive(Debug, Clone)]
struct Row {
    case: &'static str,
    /// `VirtualMachine$Name`: len 1..100, pattern `^[a-zA-Z0-9-]*$`
    /// (`smithy_query(query="shape",shape="VirtualMachine")`).
    name: String,
    /// `VirtualMachine$HostName`: the same `Name` shape.
    host_name: String,
    /// `VirtualMachine$Path`: len 1..4096, pattern `^[^\x00]+$` — the machine's place in
    /// its hypervisor's inventory, and its identity there.
    path: String,
    /// `VirtualMachine$LastBackupDate`, absent for a machine never backed up.
    last_backup_date: Option<SystemTime>,
}

fn backed_up_at(secs: u64) -> Option<SystemTime> {
    Some(SystemTime::UNIX_EPOCH + Duration::from_secs(secs))
}

/// The machines the tests report for the FIRST hypervisor: rows differing along
/// everything the listing generalises over — name at the modelled 1-character minimum and
/// 100-character maximum, mixed case and digits, host name equal to and different from
/// the name, path depth, spaces and non-ASCII characters, the 1-character minimum and the
/// 4096-character maximum of `Path`, and whether the machine has ever been backed up.
/// None of that may change what the listing reports about a record.
fn first_inventory() -> Vec<Row> {
    vec![
        Row {
            case: "a single-character name, a one-character path, never backed up",
            name: "v".to_string(),
            host_name: "v".to_string(),
            path: "/".to_string(),
            last_backup_date: None,
        },
        Row {
            case: "a hyphenated name, a nested path and a backup date",
            name: "vm-app-01".to_string(),
            host_name: "vm-app-01-host".to_string(),
            path: "/Datacenter/vm/production/vm-app-01".to_string(),
            last_backup_date: backed_up_at(LAST_BACKUP_EPOCH_SECS),
        },
        Row {
            case: "mixed case and digits, a path with spaces",
            name: "VmMixedCase99".to_string(),
            host_name: "VmMixedCase99".to_string(),
            path: "/Datacenter 2/vm folder/Vm Mixed Case 99".to_string(),
            last_backup_date: backed_up_at(LAST_BACKUP_EPOCH_SECS + 86_400),
        },
        Row {
            case: "a name at the modelled 100-character maximum",
            name: "v".repeat(100),
            host_name: "v".repeat(100),
            path: "/Datacenter/vm/long-name".to_string(),
            last_backup_date: None,
        },
        Row {
            case: "a path with non-ASCII characters",
            name: "vm-unicode".to_string(),
            host_name: "vm-unicode-host".to_string(),
            path: "/Rechenzentrum/вм/ünïcödé-vm".to_string(),
            last_backup_date: backed_up_at(1),
        },
        Row {
            case: "a path at the modelled 4096-character maximum",
            name: "vm-long-path".to_string(),
            host_name: "vm-long-path".to_string(),
            path: format!("/{}", "p".repeat(4095)),
            last_backup_date: backed_up_at(LAST_BACKUP_EPOCH_SECS - 1),
        },
    ]
}

/// The machines of the SECOND hypervisor, including a path another hypervisor also has:
/// a path identifies a machine ON its hypervisor, so two hypervisors may each hold one.
fn second_inventory() -> Vec<Row> {
    vec![
        Row {
            case: "a machine at a path the first hypervisor also has",
            name: "vm-app-01".to_string(),
            host_name: "vm-app-01-standby".to_string(),
            path: "/Datacenter/vm/production/vm-app-01".to_string(),
            last_backup_date: None,
        },
        Row {
            case: "a second machine on the second hypervisor",
            name: "vm-db-02".to_string(),
            host_name: "vm-db-02".to_string(),
            path: "/Datacenter/vm/databases/vm-db-02".to_string(),
            last_backup_date: backed_up_at(LAST_BACKUP_EPOCH_SECS + 3_600),
        },
    ]
}

/// One listed `VirtualMachine`, as the wire carried it.
#[derive(Debug, Clone, PartialEq)]
struct Listed {
    resource_arn: Option<String>,
    name: Option<String>,
    host_name: Option<String>,
    path: Option<String>,
    hypervisor_id: Option<String>,
    /// `LastBackupDate` as the awsJson wire carries a `Time`: epoch seconds.
    last_backup_date: Option<f64>,
}

fn listed(reply: &Reply) -> Vec<Listed> {
    reply
        .body
        .get("VirtualMachines")
        .and_then(Value::as_array)
        .map(|machines| {
            machines
                .iter()
                .map(|machine| {
                    let member = |name: &str| {
                        machine.get(name).and_then(Value::as_str).map(str::to_string)
                    };
                    Listed {
                        resource_arn: member("ResourceArn"),
                        name: member("Name"),
                        host_name: member("HostName"),
                        path: member("Path"),
                        hypervisor_id: member("HypervisorId"),
                        last_backup_date: machine
                            .get("LastBackupDate")
                            .and_then(Value::as_f64),
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

fn listed_arns(reply: &Reply) -> Vec<String> {
    listed(reply)
        .into_iter()
        .map(|machine| machine.resource_arn.unwrap_or_default())
        .collect()
}

/// Import a hypervisor through `ImportHypervisorConfiguration` and hand back the ARN the
/// service minted. A hypervisor record stands for the machine at its host, so every call
/// names a host of its own.
async fn import_hypervisor(router: &MultiServiceRouter, name: &str, host: &str) -> String {
    let reply = call(
        router,
        "ImportHypervisorConfiguration",
        &json!({ "Name": name, "Host": host }),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "ImportHypervisorConfiguration({name:?}) must answer 200, got {} {:?}",
        reply.status, reply.body
    );
    reply
        .str_at("/HypervisorArn")
        .unwrap_or_else(|| panic!("no HypervisorArn in {:?}", reply.body))
        .to_string()
}

/// Report `rows` as the inventory of `hypervisor_arn`, and hand back the ARNs the store
/// now holds them under, in row order.
///
/// On AWS a virtual machine reaches the service from the hypervisor it runs on, when that
/// hypervisor's metadata is synced — no operation of this model creates one — so the
/// store's inventory is the only place a machine can come from, and a test that wants one
/// listed puts it there, exactly as `ListVirtualMachines` reads it.
fn record_inventory(
    engine: &Arc<engine::InMemoryEngine>,
    hypervisor_arn: &str,
    rows: &[Row],
) -> Vec<String> {
    let state = service_state(&**engine).expect("the in-memory engine must expose this state");
    let mut store = state.lock();
    rows.iter()
        .map(|row| {
            store
                .record_virtual_machine(
                    REGION,
                    hypervisor_arn,
                    VirtualMachineInventory {
                        name: row.name.clone(),
                        host_name: row.host_name.clone(),
                        path: row.path.clone(),
                        last_backup_date: row.last_backup_date,
                        // The `VirtualMachine` summary a listing carries declares no
                        // `VmwareTags` member (`smithy_query(query="shape",
                        // shape="VirtualMachine")`) — only the `VirtualMachineDetails`
                        // a read of one machine returns does, which
                        // `virtual_machine_reads` covers — so the rows here report none.
                        vmware_tags: Vec::new(),
                    },
                    SystemTime::now(),
                )
                .unwrap_or_else(|| {
                    panic!(
                        "case {:?}: the store must record a machine on an imported \
                         hypervisor",
                        row.case
                    )
                })
                .arn
        })
        .collect()
}

/// The `HypervisorId` a machine on `hypervisor_arn` must report: the ARN's resource id,
/// read here out of the ARN the import handed back rather than out of the crate.
fn hypervisor_id(hypervisor_arn: &str) -> String {
    hypervisor_arn
        .rsplit('/')
        .next()
        .expect("a hypervisor ARN carries a resource id")
        .to_string()
}

/// Whether `arn` satisfies the modelled `VirtualMachine$ResourceArn`: `@length(min: 50,
/// max: 500)` and
/// `^arn:(aws|aws-cn|aws-us-gov):backup-gateway(:[a-zA-Z-0-9]+){3}\/[a-zA-Z-0-9]+$`
/// (`smithy_query(query="shape",shape="VirtualMachine")`), spelled out here rather than
/// reused from the crate so the assertion is independent of the code that mints it.
fn matches_modelled_resource_arn(arn: &str) -> bool {
    if arn.len() < 50 || arn.len() > 500 {
        return false;
    }
    let segment =
        |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
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

/// A well-formed hypervisor ARN the store holds no hypervisor under — the shape the
/// modelled `ServerArn` pattern accepts, so it reaches the handler rather than the
/// boundary validator.
fn never_imported_hypervisor_arn() -> String {
    format!(
        "arn:aws:backup-gateway:{REGION}:{}:hypervisor/hv-0000000000009999",
        engine::EMULATOR_ACCOUNT_ID
    )
}

async fn list(router: &MultiServiceRouter, body: &Value) -> Reply {
    call(router, "ListVirtualMachines", body).await
}

/// Walk every page of `ListVirtualMachines` at one `MaxResults`, asserting the page-level
/// invariants as it goes, and return the ARNs in the order they were handed out.
async fn walk_pages(
    router: &MultiServiceRouter,
    hypervisor_arn: Option<&str>,
    max_results: Option<i64>,
) -> Vec<String> {
    let mut seen = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let mut body = json!({});
        if let Some(arn) = hypervisor_arn {
            body["HypervisorArn"] = json!(arn);
        }
        if let Some(max) = max_results {
            body["MaxResults"] = json!(max);
        }
        if let Some(token) = &token {
            body["NextToken"] = json!(token);
        }
        let reply = list(router, &body).await;
        let case = format!("HypervisorArn={hypervisor_arn:?} MaxResults={max_results:?}");
        assert_eq!(
            reply.status, 200,
            "case {case:?} page {pages}: ListVirtualMachines must answer 200, got {} {:?}",
            reply.status, reply.body
        );
        let page = listed_arns(&reply);
        if let Some(max) = max_results {
            assert!(
                page.len() as i64 <= max,
                "case {case:?} page {pages}: a page must hold at most {max} machines, \
                 got {}",
                page.len()
            );
        }
        token = reply.next_token();
        if token.is_some() {
            assert!(
                !page.is_empty(),
                "case {case:?} page {pages}: a page that promises more records must not \
                 be empty, got {:?}",
                reply.body
            );
        }
        seen.extend(page);
        pages += 1;
        assert!(
            pages < 64,
            "case {case:?}: pagination did not terminate after {pages} pages, seen {seen:?}"
        );
        if token.is_none() {
            return seen;
        }
    }
}

/// One row of the filter table: the case name, the hypervisor the request names (none
/// for the whole-account listing), and the machines and ARNs the listing must report.
type FilterCase<'a> = (&'static str, Option<&'a str>, Vec<(Row, String)>);

/// Each listed machine is the record the store holds, member for member, in ARN order —
/// and `HypervisorArn` selects exactly the machines of the hypervisor it names.
#[tokio::test]
async fn a_listing_reports_the_machines_the_store_holds_on_the_named_hypervisor() {
    let engine = build_engine();
    let router = build_router(&engine);

    let first = import_hypervisor(&router, "hv-first", "10.0.0.1").await;
    let second = import_hypervisor(&router, "hv-second", "10.0.0.2").await;
    // A hypervisor whose inventory was never reported: it must list nothing while the
    // others list their own machines.
    let quiet = import_hypervisor(&router, "hv-quiet", "10.0.0.3").await;

    let first_rows = first_inventory();
    let second_rows = second_inventory();
    let first_arns = record_inventory(&engine, &first, &first_rows);
    let second_arns = record_inventory(&engine, &second, &second_rows);

    // (case, the hypervisor the request names, the rows and ARNs it must report)
    let cases: [FilterCase; 4] = [
        (
            "the first hypervisor",
            Some(first.as_str()),
            first_rows.iter().cloned().zip(first_arns.clone()).collect(),
        ),
        (
            "the second hypervisor",
            Some(second.as_str()),
            second_rows
                .iter()
                .cloned()
                .zip(second_arns.clone())
                .collect(),
        ),
        ("a hypervisor with no inventory", Some(quiet.as_str()), Vec::new()),
        (
            "no hypervisor at all",
            None,
            first_rows
                .iter()
                .cloned()
                .zip(first_arns.clone())
                .chain(second_rows.iter().cloned().zip(second_arns.clone()))
                .collect(),
        ),
    ];

    for (case, hypervisor_arn, expected) in &cases {
        let mut body = json!({});
        if let Some(arn) = hypervisor_arn {
            body["HypervisorArn"] = json!(arn);
        }
        let reply = list(&router, &body).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: ListVirtualMachines must answer 200, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.next_token(),
            None,
            "case {case:?}: a listing that fits in one page must carry no NextToken, got \
             {:?}",
            reply.body
        );

        let mut expected_arns: Vec<String> =
            expected.iter().map(|(_, arn)| arn.clone()).collect();
        expected_arns.sort();
        assert_eq!(
            listed_arns(&reply),
            expected_arns,
            "case {case:?}: the listing must report exactly the machines the store holds \
             on that hypervisor, ordered by ARN, got {:?}",
            reply.body
        );

        let by_arn: BTreeMap<String, Listed> = listed(&reply)
            .into_iter()
            .map(|machine| (machine.resource_arn.clone().unwrap_or_default(), machine))
            .collect();
        for (row, arn) in expected {
            let reported = by_arn.get(arn).unwrap_or_else(|| {
                panic!("case {case:?}/{:?}: {arn} is missing from {:?}", row.case, reply.body)
            });
            let owning = if first_arns.contains(arn) { &first } else { &second };
            assert_eq!(
                (
                    reported.name.as_deref(),
                    reported.host_name.as_deref(),
                    reported.path.as_deref(),
                    reported.hypervisor_id.as_deref(),
                ),
                (
                    Some(row.name.as_str()),
                    Some(row.host_name.as_str()),
                    Some(row.path.as_str()),
                    Some(hypervisor_id(owning).as_str()),
                ),
                "case {case:?}/{:?}: the listed machine must carry the members the store \
                 holds, got {reported:?}",
                row.case
            );
            let expected_backup_date = row.last_backup_date.map(|at| {
                at.duration_since(SystemTime::UNIX_EPOCH)
                    .expect("a backup date after the epoch")
                    .as_secs_f64()
            });
            assert_eq!(
                reported.last_backup_date, expected_backup_date,
                "case {case:?}/{:?}: LastBackupDate must be the instant the store holds \
                 (absent for a machine never backed up), got {reported:?}",
                row.case
            );
            assert!(
                matches_modelled_resource_arn(arn),
                "case {case:?}/{:?}: the machine's ARN must satisfy the modelled \
                 ResourceArn, got {arn:?}",
                row.case
            );
            // And the same record, read out of the store, says the same thing — so the
            // response describes persisted state.
            let state = service_state(&*engine).expect("this service's state");
            let store = state.lock();
            let stored = store.virtual_machine(arn).unwrap_or_else(|| {
                panic!("case {case:?}/{:?}: {arn} must be in the store", row.case)
            });
            assert_eq!(
                (
                    stored.name.as_str(),
                    stored.host_name.as_str(),
                    stored.path.as_str(),
                    stored.hypervisor_arn.as_str()
                ),
                (
                    row.name.as_str(),
                    row.host_name.as_str(),
                    row.path.as_str(),
                    owning.as_str()
                ),
                "case {case:?}/{:?}: the store must hold what was reported, got {stored:?}",
                row.case
            );
        }

        // A read changes nothing a later caller sees.
        let again = list(&router, &body).await;
        assert_eq!(
            (again.status, listed(&again)),
            (200, listed(&reply)),
            "case {case:?}: listing twice must report the same records, got {:?}",
            again.body
        );
    }

    assert_eq!(
        service_state(&*engine)
            .expect("this service's state")
            .lock()
            .virtual_machine_count(),
        first_rows.len() + second_rows.len(),
        "the store must hold exactly the machines that were reported"
    );
}

/// Every page size walks the whole listing — filtered and unfiltered — exactly once, in
/// ARN order, and only the last page carries no token.
#[tokio::test]
async fn every_page_size_walks_the_whole_listing_exactly_once() {
    let engine = build_engine();
    let router = build_router(&engine);

    let first = import_hypervisor(&router, "hv-first", "10.0.0.1").await;
    let second = import_hypervisor(&router, "hv-second", "10.0.0.2").await;
    let first_rows = first_inventory();
    let second_rows = second_inventory();
    let first_arns = record_inventory(&engine, &first, &first_rows);
    let second_arns = record_inventory(&engine, &second, &second_rows);

    let sorted = |arns: &[String]| {
        let mut sorted = arns.to_vec();
        sorted.sort();
        sorted
    };
    let all: Vec<String> = first_arns
        .iter()
        .chain(second_arns.iter())
        .cloned()
        .collect();

    // (case, the hypervisor the request names, the ARNs the walk must hand out)
    let filters: [(&str, Option<&str>, Vec<String>); 3] = [
        ("the first hypervisor", Some(first.as_str()), sorted(&first_arns)),
        ("the second hypervisor", Some(second.as_str()), sorted(&second_arns)),
        ("no hypervisor at all", None, sorted(&all)),
    ];

    for (case, hypervisor_arn, expected) in &filters {
        // Every page size from one record per page up past the record count, plus the
        // request that names none at all: the page BOUNDARY may move, the sequence may
        // not.
        let sizes: Vec<Option<i64>> = std::iter::once(None)
            .chain((1..=(all.len() as i64 + 2)).map(Some))
            .collect();
        for max_results in sizes {
            let walked = walk_pages(&router, *hypervisor_arn, max_results).await;
            assert_eq!(
                &walked, expected,
                "case {case:?} MaxResults={max_results:?}: paging must hand out every \
                 machine of the listing exactly once in ARN order, got {walked:?}"
            );
            assert_eq!(
                walked.iter().collect::<BTreeSet<_>>().len(),
                walked.len(),
                "case {case:?} MaxResults={max_results:?}: no machine may be repeated \
                 across pages, got {walked:?}"
            );
        }
    }
}

/// A `NextToken` this listing did not issue is refused with HTTP 400
/// `ValidationException`, whatever form it takes — including a token another listing of
/// this service handed out, whose boundary is a key of a different map.
#[tokio::test]
async fn a_next_token_this_listing_did_not_issue_is_refused() {
    let engine = build_engine();
    let router = build_router(&engine);

    let first = import_hypervisor(&router, "hv-first", "10.0.0.1").await;
    let arns = record_inventory(&engine, &first, &first_inventory());
    let mut expected_order = arns.clone();
    expected_order.sort();

    // Two gateways and a second hypervisor, so the sibling listings really hand out
    // tokens of their own.
    create(&router, "gw-one").await;
    create(&router, "gw-two").await;
    import_hypervisor(&router, "hv-second", "10.0.0.2").await;
    let gateway_token = call(&router, "ListGateways", &json!({ "MaxResults": 1 }))
        .await
        .next_token()
        .expect("a ListGateways page with records left must carry a NextToken");
    let hypervisor_token = call(&router, "ListHypervisors", &json!({ "MaxResults": 1 }))
        .await
        .next_token()
        .expect("a ListHypervisors page with records left must carry a NextToken");

    // (case, the NextToken as the client sends it). Every row satisfies the modelled
    // `NextToken` constraints — len 1..1000, pattern `^.+$` — so each reaches the handler
    // rather than the boundary validator.
    let bad_tokens: [(&str, String); 7] = [
        ("not hex at all", "not-a-token".to_string()),
        ("hex of text that is no ARN", hex::encode("page two please")),
        (
            "hex of an ARN of another service",
            hex::encode("arn:aws:s3:::example-bucket"),
        ),
        (
            "hex of a hypervisor ARN of this service",
            hex::encode(&first),
        ),
        ("a token a ListGateways page issued", gateway_token),
        ("a token a ListHypervisors page issued", hypervisor_token),
        (
            "hex of a machine ARN with an empty id",
            hex::encode(format!(
                "arn:aws:backup-gateway:{REGION}:{}:vm/",
                engine::EMULATOR_ACCOUNT_ID
            )),
        ),
    ];

    for (case, token) in &bad_tokens {
        let refused = list(&router, &json!({ "NextToken": token })).await;
        assert_eq!(
            (refused.status, refused.code.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: a NextToken this listing did not issue must be refused with \
             400 ValidationException, got {} {:?}",
            refused.status,
            refused.body
        );
        assert_eq!(
            refused.str_at("/Message"),
            Some(PAGE_TOKEN_INVALID_MESSAGE),
            "case {case:?}: the refusal must state the pagination-token sentence, got {:?}",
            refused.body
        );
        assert!(
            listed(&refused).is_empty(),
            "case {case:?}: a refused listing must hand back no machines, got {:?}",
            refused.body
        );
    }

    // A token this listing DID issue still resumes after its boundary, so the refusals
    // above are about the token and not about the operation.
    let page = list(&router, &json!({ "MaxResults": 1 })).await;
    let token = page
        .next_token()
        .expect("a page with records left must carry a NextToken");
    let second = list(&router, &json!({ "MaxResults": 1, "NextToken": token })).await;
    assert_eq!(
        (second.status, listed_arns(&second)),
        (200, vec![expected_order[1].clone()]),
        "a token this listing issued must resume after the boundary it encodes, got {:?}",
        second.body
    );
}

/// A hypervisor the service holds no inventory for lists no machines — with a 200 and no
/// token, not a refusal — whether the hypervisor was imported and never synced, was never
/// imported at all, or the ARN names another kind of resource.
#[tokio::test]
async fn a_hypervisor_with_no_inventory_lists_no_machines() {
    let engine = build_engine();
    let router = build_router(&engine);

    // A hypervisor WITH an inventory exists, so an empty answer means "no machines on
    // that hypervisor" and not "nothing was ever reported".
    let stocked = import_hypervisor(&router, "hv-stocked", "10.0.0.1").await;
    let stocked_arns = record_inventory(&engine, &stocked, &first_inventory());
    let never_synced = import_hypervisor(&router, "hv-quiet", "10.0.0.2").await;
    let gateway_arn = create(&router, "gw-one").await;

    // (case, the HypervisorArn the request names)
    let empty_cases: [(&str, String); 3] = [
        ("an imported hypervisor that was never synced", never_synced),
        (
            "a well-formed hypervisor ARN nothing imported",
            never_imported_hypervisor_arn(),
        ),
        ("a gateway ARN of this service", gateway_arn),
    ];

    for (case, hypervisor_arn) in &empty_cases {
        for (size, body) in [
            ("no MaxResults", json!({ "HypervisorArn": hypervisor_arn })),
            (
                "MaxResults 1",
                json!({ "HypervisorArn": hypervisor_arn, "MaxResults": 1 }),
            ),
            (
                "MaxResults 100",
                json!({ "HypervisorArn": hypervisor_arn, "MaxResults": 100 }),
            ),
        ] {
            let reply = list(&router, &body).await;
            assert_eq!(
                (reply.status, listed(&reply), reply.next_token()),
                (200, Vec::new(), None),
                "case {case:?}/{size:?}: a hypervisor with no inventory must list no \
                 machines, got {} {:?}",
                reply.status,
                reply.body
            );
        }
    }

    // And the hypervisor that does have an inventory still reports it, so the empty
    // answers above are about the filter and not about the operation.
    let mut expected = stocked_arns;
    expected.sort();
    let reply = list(&router, &json!({ "HypervisorArn": stocked })).await;
    assert_eq!(
        (reply.status, listed_arns(&reply)),
        (200, expected),
        "the stocked hypervisor must still list its machines, got {:?}",
        reply.body
    );
}

/// A machine is identified on its hypervisor by its path: re-reporting the same path
/// updates the one record under the ARN it already has, and deleting the hypervisor takes
/// its inventory with it.
#[tokio::test]
async fn an_inventory_holds_one_record_per_path_and_goes_with_its_hypervisor() {
    let engine = build_engine();
    let router = build_router(&engine);

    let first = import_hypervisor(&router, "hv-first", "10.0.0.1").await;
    let second = import_hypervisor(&router, "hv-second", "10.0.0.2").await;
    let first_rows = first_inventory();
    let first_arns = record_inventory(&engine, &first, &first_rows);
    let second_arns = record_inventory(&engine, &second, &second_inventory());

    // Reporting the WHOLE inventory again mints nothing: same ARNs, same count.
    let again = record_inventory(&engine, &first, &first_rows);
    assert_eq!(
        again, first_arns,
        "re-reporting an inventory must keep each machine's ARN, got {again:?}"
    );

    // A machine whose name and backup date changed, at a path the hypervisor already
    // had: one record, updated in place.
    let renamed = Row {
        case: "the first machine, renamed and backed up",
        name: "vm-renamed".to_string(),
        host_name: "vm-renamed-host".to_string(),
        path: first_rows[0].path.clone(),
        last_backup_date: backed_up_at(LAST_BACKUP_EPOCH_SECS + 7),
    };
    let renamed_arns = record_inventory(&engine, &first, std::slice::from_ref(&renamed));
    assert_eq!(
        renamed_arns,
        vec![first_arns[0].clone()],
        "a machine reported again at the same path must keep its ARN, got {renamed_arns:?}"
    );

    let reply = list(&router, &json!({ "HypervisorArn": first })).await;
    let mut expected = first_arns.clone();
    expected.sort();
    assert_eq!(
        (reply.status, listed_arns(&reply)),
        (200, expected),
        "an inventory must hold one machine per path, got {:?}",
        reply.body
    );
    let updated = listed(&reply)
        .into_iter()
        .find(|machine| machine.resource_arn.as_deref() == Some(first_arns[0].as_str()))
        .unwrap_or_else(|| panic!("the updated machine must be listed, got {:?}", reply.body));
    assert_eq!(
        (updated.name.as_deref(), updated.host_name.as_deref()),
        (Some(renamed.name.as_str()), Some(renamed.host_name.as_str())),
        "case {:?}: the listing must report the members the latest report stored, got \
         {updated:?}",
        renamed.case
    );

    // Deleting the hypervisor takes its machines with it — and leaves the other
    // hypervisor's inventory exactly as it was.
    let deleted = call(
        &router,
        "DeleteHypervisor",
        &json!({ "HypervisorArn": first }),
    )
    .await;
    assert_eq!(
        deleted.status, 200,
        "DeleteHypervisor must answer 200, got {} {:?}",
        deleted.status, deleted.body
    );

    let after_delete = list(&router, &json!({ "HypervisorArn": first })).await;
    assert_eq!(
        (after_delete.status, listed(&after_delete)),
        (200, Vec::new()),
        "a deleted hypervisor must list no machines, got {:?}",
        after_delete.body
    );
    let mut survivors = second_arns;
    survivors.sort();
    let whole = list(&router, &json!({})).await;
    assert_eq!(
        (whole.status, listed_arns(&whole)),
        (200, survivors),
        "deleting one hypervisor must leave every other hypervisor's inventory, got {:?}",
        whole.body
    );
}
