// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the QUERY a `NextToken` of this service resumes.
//!
//! Every case drives a real `POST /` awsJson1_0 request through the `MultiServiceRouter`
//! — the harness of `gateway_lifecycle` — so what is asserted is the wire behaviour.
//!
//! The rule under test: a `NextToken` identifies the whole query it resumes, not merely
//! where the last page stopped, so a listing resumes a token only when the request
//! presenting it carries the same FILTERS the page was cut under, and refuses every other
//! token with HTTP 400 `ValidationException`. `ListVirtualMachines` is the one listing of
//! this service that takes a filter beside its page controls — `HypervisorArn`, optional
//! (`smithy_query(query="operation",operation="ListVirtualMachines")`; `ListGateways` and
//! `ListHypervisors` take `MaxResults` and `NextToken` only) — and the records of one
//! hypervisor may sort entirely below a boundary another hypervisor's page ended on, so a
//! cross-filter resume would drop every matching machine and answer 200 with no records
//! and no token: the shape of a correct final page, telling a client that a hypervisor
//! whose inventory the store holds has none.
//!
//! The page CONTROLS are the other side of that rule: `MaxResults` is the modelled
//! `@paginated` trait's `pageSize`, so a client may ask for a different page size on every
//! page of one listing and still walk it whole.
//!
//! The tables below span the class rather than one pairing: every filter a request can
//! carry (each of two stocked hypervisors, a hypervisor with no inventory, a well-formed
//! hypervisor ARN nothing imported, and no filter at all) crossed with every filter a
//! token can have been issued under, in both directions.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::SystemTime;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_backupgateway_backuponpremises_v20210101 as backup_gateway;

use backup_gateway::state::{service_state, VirtualMachineInventory, PAGE_TOKEN_INVALID_MESSAGE};

use crate::gateway_lifecycle::{build_engine, build_router, call, Reply, REGION};

/// How many machines each stocked hypervisor reports: at least two, so a page at
/// `MaxResults=1` really hands out a token, and more than two so a resumed page holds
/// several records.
const MACHINES_PER_HYPERVISOR: usize = 3;

/// One row of the token table: the filter the token was ISSUED under, and the filter the
/// request presenting it carries. The token resumes exactly when the two are the same
/// query.
#[derive(Debug, Clone)]
struct TokenCase {
    case: String,
    issued_under: Option<String>,
    presented_with: Option<String>,
}

impl TokenCase {
    /// Whether the token must resume: the request presenting it narrows the listing by
    /// exactly what the page it ended was cut under.
    fn resumes(&self) -> bool {
        self.issued_under == self.presented_with
    }
}

/// Import a hypervisor through `ImportHypervisorConfiguration` and hand back the ARN the
/// service minted. A record stands for the appliance at its host, so each names its own.
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

/// Report `count` machines as the inventory of `hypervisor_arn` and hand back the ARNs the
/// store now holds them under, in ARN order.
///
/// A virtual machine is minted by no operation of this model: it reaches the service as
/// part of the inventory of the hypervisor it runs on, which `record_virtual_machine`
/// stands for — the same channel a metadata sync records one through.
fn record_inventory(
    engine: &Arc<engine::InMemoryEngine>,
    hypervisor_arn: &str,
    name_prefix: &str,
    count: usize,
) -> Vec<String> {
    let state = service_state(&**engine).expect("the in-memory engine must expose this state");
    let mut store = state.lock();
    let mut arns: Vec<String> = (0..count)
        .map(|index| {
            store
                .record_virtual_machine(
                    REGION,
                    hypervisor_arn,
                    VirtualMachineInventory {
                        name: format!("{name_prefix}-{index}"),
                        host_name: format!("{name_prefix}-{index}"),
                        path: format!("/Datacenter/vm/{name_prefix}/{index}"),
                        last_backup_date: None,
                        vmware_tags: Vec::new(),
                    },
                    SystemTime::now(),
                )
                .unwrap_or_else(|| {
                    panic!("the store must record a machine on {hypervisor_arn}, {index}")
                })
                .arn
        })
        .collect();
    arns.sort();
    arns
}

/// A well-formed hypervisor ARN the store holds no hypervisor under — the shape the
/// modelled `ServerArn` pattern accepts
/// (`^arn:(aws|aws-cn|aws-us-gov):backup-gateway(:[a-zA-Z-0-9]+){3}\/[a-zA-Z-0-9]+$`), so
/// it reaches the handler rather than the boundary validator.
fn never_imported_hypervisor_arn() -> String {
    format!(
        "arn:aws:backup-gateway:{REGION}:{}:hypervisor/hv-0000000000009999",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// `ListVirtualMachines` with the members a case states and no others.
async fn list(
    router: &MultiServiceRouter,
    hypervisor_arn: Option<&str>,
    max_results: Option<i64>,
    next_token: Option<&str>,
) -> Reply {
    let mut body = json!({});
    if let Some(arn) = hypervisor_arn {
        body["HypervisorArn"] = json!(arn);
    }
    if let Some(max) = max_results {
        body["MaxResults"] = json!(max);
    }
    if let Some(token) = next_token {
        body["NextToken"] = json!(token);
    }
    call(router, "ListVirtualMachines", &body).await
}

/// The machine ARNs a listing reply carries, in the order it carried them.
fn listed_arns(reply: &Reply) -> Vec<String> {
    reply
        .body
        .get("VirtualMachines")
        .and_then(Value::as_array)
        .map(|machines| {
            machines
                .iter()
                .map(|machine| {
                    machine
                        .get("ResourceArn")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The store seeded for these tests: two stocked hypervisors, one imported and never
/// synced, and the machine ARNs of each.
struct Seeded {
    first: String,
    second: String,
    quiet: String,
    first_machines: Vec<String>,
    second_machines: Vec<String>,
}

impl Seeded {
    /// The machines `filter` selects, in ARN order — every machine of the region when it
    /// names no hypervisor, and none for a hypervisor the store holds no inventory for.
    fn machines_of(&self, filter: Option<&str>) -> Vec<String> {
        let mut selected = match filter {
            None => self
                .first_machines
                .iter()
                .chain(self.second_machines.iter())
                .cloned()
                .collect(),
            Some(arn) if arn == self.first => self.first_machines.clone(),
            Some(arn) if arn == self.second => self.second_machines.clone(),
            Some(_) => Vec::new(),
        };
        selected.sort();
        selected
    }

    /// Every filter a request of these tests can carry, with the one a token can be issued
    /// under first: a filter selecting no machine at all issues no token.
    fn filters(&self) -> Vec<(&'static str, Option<String>)> {
        vec![
            ("the first hypervisor", Some(self.first.clone())),
            ("the second hypervisor", Some(self.second.clone())),
            ("no hypervisor at all", None),
            ("a hypervisor with no inventory", Some(self.quiet.clone())),
            (
                "a hypervisor ARN nothing imported",
                Some(never_imported_hypervisor_arn()),
            ),
        ]
    }
}

async fn seed(engine: &Arc<engine::InMemoryEngine>, router: &MultiServiceRouter) -> Seeded {
    let first = import_hypervisor(router, "hv-first", "10.0.0.1").await;
    let second = import_hypervisor(router, "hv-second", "10.0.0.2").await;
    let quiet = import_hypervisor(router, "hv-quiet", "10.0.0.3").await;
    let first_machines = record_inventory(engine, &first, "vm-first", MACHINES_PER_HYPERVISOR);
    let second_machines = record_inventory(engine, &second, "vm-second", MACHINES_PER_HYPERVISOR);
    Seeded {
        first,
        second,
        quiet,
        first_machines,
        second_machines,
    }
}

/// A token is issued for ONE query: presenting it to a listing narrowed by a different
/// `HypervisorArn` — or by none, or presenting a token an unfiltered listing issued to a
/// filtered one — is refused with 400 `ValidationException`, while the token of the very
/// query that issued it resumes after its boundary.
#[tokio::test]
async fn a_token_resumes_only_the_query_it_was_issued_for() {
    let engine = build_engine();
    let router = build_router(&engine);
    let seeded = seed(&engine, &router).await;

    // Every (issuing filter, presenting filter) pairing: the issuing side is only the
    // filters that select machines, since a listing with no records hands out no token.
    let filters = seeded.filters();
    let mut cases: Vec<TokenCase> = Vec::new();
    for (issuing_case, issued_under) in &filters {
        if seeded.machines_of(issued_under.as_deref()).len() < 2 {
            continue;
        }
        for (presenting_case, presented_with) in &filters {
            cases.push(TokenCase {
                case: format!("a token of {issuing_case}, presented with {presenting_case}"),
                issued_under: issued_under.clone(),
                presented_with: presented_with.clone(),
            });
        }
    }
    assert!(
        cases.iter().any(TokenCase::resumes) && cases.iter().any(|case| !case.resumes()),
        "case table: the pairings must cover both a token resuming its own query and one \
         presented to another, got {cases:?}"
    );

    for case in &cases {
        let issued_machines = seeded.machines_of(case.issued_under.as_deref());
        // Page one of the issuing query, at one record per page, so the boundary the token
        // encodes is the first machine that query selects.
        let page_one = list(&router, case.issued_under.as_deref(), Some(1), None).await;
        assert_eq!(
            (page_one.status, listed_arns(&page_one)),
            (200, vec![issued_machines[0].clone()]),
            "case {:?}: page one of the issuing query must hold its first machine, got {} {:?}",
            case.case,
            page_one.status,
            page_one.body
        );
        let token = page_one.next_token().unwrap_or_else(|| {
            panic!(
                "case {:?}: a truncated page must carry a NextToken, got {:?}",
                case.case, page_one.body
            )
        });

        let reply = list(
            &router,
            case.presented_with.as_deref(),
            None,
            Some(&token),
        )
        .await;
        if case.resumes() {
            assert_eq!(
                (reply.status, listed_arns(&reply)),
                (200, issued_machines[1..].to_vec()),
                "case {:?}: a token of this very query must resume after its boundary, got \
                 {} {:?}",
                case.case,
                reply.status,
                reply.body
            );
            assert_eq!(
                reply.next_token(),
                None,
                "case {:?}: the final page must promise no successor, got {:?}",
                case.case,
                reply.body
            );
        } else {
            assert_eq!(
                (reply.status, reply.code.as_deref()),
                (400, Some("ValidationException")),
                "case {:?}: a token issued for another query must be refused with 400 \
                 ValidationException, got {} {:?}",
                case.case,
                reply.status,
                reply.body
            );
            assert_eq!(
                reply.str_at("/Message"),
                Some(PAGE_TOKEN_INVALID_MESSAGE),
                "case {:?}: the refusal must state the pagination-token sentence, got {:?}",
                case.case,
                reply.body
            );
            assert!(
                listed_arns(&reply).is_empty(),
                "case {:?}: a refused listing must hand back no machines, got {:?}",
                case.case,
                reply.body
            );
            assert_eq!(
                reply.next_token(),
                None,
                "case {:?}: a refused listing must promise no successor, got {:?}",
                case.case,
                reply.body
            );
        }
    }

    // Every refusal above changed nothing: each query still reports exactly the machines
    // the store holds for it.
    for (case, filter) in seeded.filters() {
        let reply = list(&router, filter.as_deref(), None, None).await;
        assert_eq!(
            (reply.status, listed_arns(&reply)),
            (200, seeded.machines_of(filter.as_deref())),
            "case {case:?}: after the refusals the listing must still report the machines \
             the store holds, got {} {:?}",
            reply.status,
            reply.body
        );
    }
}

/// The reachable skip the rule is about: the machines of one hypervisor sort entirely
/// BELOW a boundary another hypervisor's page ended on, so a cross-filter resume must be
/// refused and never answered with the empty final page that would report a stocked
/// hypervisor as having no machines.
#[tokio::test]
async fn a_cross_filter_token_is_refused_rather_than_answered_with_an_empty_page() {
    let engine = build_engine();
    let router = build_router(&engine);
    let seeded = seed(&engine, &router).await;

    let highest_of_first = seeded
        .first_machines
        .last()
        .expect("the first hypervisor holds machines");
    let lowest_of_second = seeded
        .second_machines
        .first()
        .expect("the second hypervisor holds machines");
    assert!(
        lowest_of_second > highest_of_first,
        "case ordering: every machine of the hypervisor synced second must sort above the \
         machines of the first, or the skip this rule prevents is unreachable, got \
         {lowest_of_second} and {highest_of_first}"
    );

    // A token the SECOND hypervisor's listing really issued: its boundary sorts above
    // every machine of the first.
    let page_one = list(&router, Some(&seeded.second), Some(1), None).await;
    let token = page_one.next_token().unwrap_or_else(|| {
        panic!(
            "a truncated page of the second hypervisor must carry a NextToken, got {:?}",
            page_one.body
        )
    });

    // The first hypervisor really has machines, so an empty answer could only mean "this
    // hypervisor has none".
    let stocked = list(&router, Some(&seeded.first), None, None).await;
    assert_eq!(
        (stocked.status, listed_arns(&stocked)),
        (200, seeded.first_machines.clone()),
        "case stocked: the first hypervisor must report its whole inventory, got {} {:?}",
        stocked.status,
        stocked.body
    );

    let refused = list(&router, Some(&seeded.first), None, Some(&token)).await;
    assert_eq!(
        (refused.status, refused.code.as_deref()),
        (400, Some("ValidationException")),
        "case cross-filter: a token the second hypervisor's listing issued must not resume \
         the first hypervisor's listing, got {} {:?}",
        refused.status,
        refused.body
    );
    assert!(
        listed_arns(&refused).is_empty() && refused.next_token().is_none(),
        "case cross-filter: a refused listing must hand back neither records nor a \
         successor, got {:?}",
        refused.body
    );

    // And the listing is unharmed: the same query without the token still reports every
    // machine of the first hypervisor.
    let again = list(&router, Some(&seeded.first), None, None).await;
    assert_eq!(
        (again.status, listed_arns(&again)),
        (200, seeded.first_machines.clone()),
        "case cross-filter: the refusal must leave the listing reporting the whole \
         inventory, got {} {:?}",
        again.status,
        again.body
    );
}

/// `MaxResults` is a page CONTROL, not part of the query a token resumes: a client may ask
/// for a different page size on every page and still walk one listing exactly once, in ARN
/// order.
#[tokio::test]
async fn the_page_size_is_not_part_of_the_query_a_token_resumes() {
    let engine = build_engine();
    let router = build_router(&engine);
    let seeded = seed(&engine, &router).await;

    // (case, the filter the walk carries, the machines it must hand out)
    let queries: [(&str, Option<String>, Vec<String>); 3] = [
        (
            "the first hypervisor",
            Some(seeded.first.clone()),
            seeded.machines_of(Some(&seeded.first)),
        ),
        (
            "the second hypervisor",
            Some(seeded.second.clone()),
            seeded.machines_of(Some(&seeded.second)),
        ),
        ("no hypervisor at all", None, seeded.machines_of(None)),
    ];

    // The page sizes a walk cycles through, page by page: a size below, at and above the
    // number of records left, plus the request that names none at all.
    let sizes: [Option<i64>; 4] = [Some(1), Some(2), None, Some(MACHINES_PER_HYPERVISOR as i64)];

    for (case, filter, expected) in &queries {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let max_results = sizes[pages % sizes.len()];
            let reply = list(&router, filter.as_deref(), max_results, token.as_deref()).await;
            assert_eq!(
                reply.status, 200,
                "case {case:?} page {pages} MaxResults={max_results:?}: a page size that \
                 differs from the previous page's must still be served, got {} {:?}",
                reply.status, reply.body
            );
            seen.extend(listed_arns(&reply));
            token = reply.next_token();
            pages += 1;
            assert!(
                pages < 32,
                "case {case:?}: the walk must terminate, got {pages} pages and {seen:?}"
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            &seen, expected,
            "case {case:?}: a walk whose page size changes on every page must hand out every \
             machine of the query exactly once in ARN order, got {seen:?}"
        );
        assert_eq!(
            seen.iter().collect::<BTreeSet<_>>().len(),
            seen.len(),
            "case {case:?}: no machine may be repeated across pages, got {seen:?}"
        );
    }
}
