// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListHypervisors`.
//!
//! Every case drives a real `POST /` awsJson1_0 request through the
//! `MultiServiceRouter` — the harness of `gateway_lifecycle` — so what is asserted is
//! the wire behaviour of the operation, and the only way the listing can see a
//! hypervisor is that an earlier `ImportHypervisorConfiguration` request wrote it to the
//! shared store.
//!
//! The rule under test: `ListHypervisors` reports the hypervisors the store holds,
//! ordered by their ARNs, one `MaxResults`-sized page at a time, each record's members
//! exactly as the store holds them and its `State` as of the clock — so a page is a
//! window on persisted state, a token resumes after the boundary it encodes, and a token
//! this listing never issued is refused. The tables below span that class: rows
//! differing in name, host form, KMS key and tag count; every page size over a fixed set
//! of records; every form a bad token can take; and every relation the activation
//! instant can have to the clock.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_backupgateway_backuponpremises_v20210101 as backup_gateway;

use backup_gateway::state::{
    service_state, HYPERVISOR_ONLINE, HYPERVISOR_PENDING, HYPERVISOR_PENDING_FOR,
    PAGE_TOKEN_INVALID_MESSAGE,
};

use crate::gateway_lifecycle::{build_engine, build_router, call, create, Reply, REGION};

/// The four values the modelled `HypervisorState` enum declares
/// (`smithy_query(query="shape",shape="HypervisorState")`).
const HYPERVISOR_STATE_VALUES: [&str; 4] = ["PENDING", "ONLINE", "OFFLINE", "ERROR"];

/// Longer than the span an import leaves a hypervisor `PENDING` for, so the clock has
/// made its activation due whether or not anybody called.
const PAST_THE_PENDING_SPAN: Duration =
    HYPERVISOR_PENDING_FOR.saturating_add(Duration::from_millis(250));

/// One hypervisor to import.
#[derive(Debug, Clone)]
struct Row {
    case: &'static str,
    /// `Name`: len 1..100, pattern `^[a-zA-Z0-9-]*$`
    /// (`smithy_query(query="shape",shape="Name")`).
    name: String,
    /// `Host`: len 3..128, pattern `^.+$`, "either an IP address or a fully-qualified
    /// domain name (FQDN)" (`smithy_query(query="shape",shape="Hypervisor")`). A
    /// hypervisor record stands for the machine at its host, so every row names a
    /// different one.
    host: String,
    /// `KmsKeyArn`: len 50..500, absent when the import names no key.
    kms_key_arn: Option<String>,
    tags: BTreeMap<String, String>,
}

/// A KMS key ARN in the form the modelled `KmsKeyArn` pattern accepts, long enough for
/// its 50-character minimum.
fn kms_key_arn(key: &str) -> String {
    format!(
        "arn:aws:kms:{REGION}:{}:key/{key}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

fn tags<'a>(pairs: impl IntoIterator<Item = (&'a str, &'a str)>) -> BTreeMap<String, String> {
    pairs
        .into_iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

/// The rows the listing tests import: they differ along everything the listing
/// generalises over — name length (the 1-character minimum and the 100-character
/// maximum), spelling and case, host form (IPv4, IPv6, FQDN, the 3-character minimum),
/// whether a KMS key was named, and how many tags the import carried. None of that may
/// change what the listing reports about a record.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a single-character name and an IPv4 host",
            name: "h".to_string(),
            host: "10.0.0.1".to_string(),
            kms_key_arn: None,
            tags: BTreeMap::new(),
        },
        Row {
            case: "a hyphenated name, an FQDN host and a KMS key",
            name: "hv-primary-01".to_string(),
            host: "hypervisor.example.internal".to_string(),
            kms_key_arn: Some(kms_key_arn("12345678-1234-1234-1234-123456789012")),
            tags: tags([("env", "test")]),
        },
        Row {
            case: "mixed case and digits, two tags",
            name: "HvMixedCase99".to_string(),
            host: "198.51.100.23".to_string(),
            kms_key_arn: None,
            tags: tags([("owner", "backup"), ("tier", "gold")]),
        },
        Row {
            case: "a name at the modelled 100-character maximum",
            name: "h".repeat(100),
            host: "vcenter-01.corp.example.com".to_string(),
            kms_key_arn: Some(kms_key_arn("abcdefab-1111-2222-3333-abcdefabcdef")),
            tags: BTreeMap::new(),
        },
        Row {
            case: "a host at the modelled 3-character minimum",
            name: "hv-short-host".to_string(),
            host: "a.b".to_string(),
            kms_key_arn: None,
            tags: tags([("env", "prod")]),
        },
        Row {
            case: "an IPv6 host",
            name: "hv-v6".to_string(),
            host: "2001:db8::1".to_string(),
            kms_key_arn: None,
            tags: tags([("env", "dr"), ("owner", "ops"), ("tier", "silver")]),
        },
        Row {
            case: "a host that differs from another only in its last label",
            name: "hv-v6-standby".to_string(),
            host: "hypervisor.example.internal.standby".to_string(),
            kms_key_arn: None,
            tags: BTreeMap::new(),
        },
    ]
}

/// One listed `Hypervisor`, as the wire carried it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Listed {
    arn: Option<String>,
    name: Option<String>,
    host: Option<String>,
    kms_key_arn: Option<String>,
    state: Option<String>,
}

fn listed(reply: &Reply) -> Vec<Listed> {
    reply
        .body
        .get("Hypervisors")
        .and_then(Value::as_array)
        .map(|hypervisors| {
            hypervisors
                .iter()
                .map(|hypervisor| {
                    let member = |name: &str| {
                        hypervisor
                            .get(name)
                            .and_then(Value::as_str)
                            .map(str::to_string)
                    };
                    Listed {
                        arn: member("HypervisorArn"),
                        name: member("Name"),
                        host: member("Host"),
                        kms_key_arn: member("KmsKeyArn"),
                        state: member("State"),
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

fn listed_arns(reply: &Reply) -> Vec<String> {
    listed(reply)
        .into_iter()
        .map(|hypervisor| hypervisor.arn.unwrap_or_default())
        .collect()
}

/// Import `rows` through `ImportHypervisorConfiguration` and hand back the ARNs the
/// service minted, in the order the rows were imported.
async fn import(router: &MultiServiceRouter, rows: &[Row]) -> Vec<String> {
    let mut arns = Vec::new();
    for row in rows {
        let mut body = json!({ "Name": row.name, "Host": row.host });
        if let Some(kms_key_arn) = &row.kms_key_arn {
            body["KmsKeyArn"] = json!(kms_key_arn);
        }
        if !row.tags.is_empty() {
            body["Tags"] = Value::Array(
                row.tags
                    .iter()
                    .map(|(key, value)| json!({ "Key": key, "Value": value }))
                    .collect(),
            );
        }
        let reply = call(router, "ImportHypervisorConfiguration", &body).await;
        assert_eq!(
            reply.status,
            200,
            "case {:?}: ImportHypervisorConfiguration must answer 200, got {} {:?}",
            row.case,
            reply.status,
            reply.body
        );
        arns.push(
            reply
                .str_at("/HypervisorArn")
                .unwrap_or_else(|| panic!("no HypervisorArn in {:?}", reply.body))
                .to_string(),
        );
    }
    arns
}

/// Move the instant every stored hypervisor's activation falls due to `online_due`.
///
/// The activation span an import starts is short, so a test that wants records whose
/// activation the clock has ALREADY made due says so on the record rather than sleeping
/// for it — the same seam `hypervisor_reads::import_seed` uses.
fn set_online_due(engine: &Arc<engine::InMemoryEngine>, arns: &[String], online_due: SystemTime) {
    let state = service_state(&**engine).expect("the in-memory engine must expose this state");
    let mut store = state.lock();
    for arn in arns {
        store
            .hypervisor_mut(arn)
            .expect("an imported hypervisor must be in the store")
            .online_due = Some(online_due);
    }
}

/// The modelled `ServerArn`: len 50..500, pattern
/// `^arn:(aws|aws-cn|aws-us-gov):backup-gateway(:[a-zA-Z-0-9]+){3}\/[a-zA-Z-0-9]+$`
/// (`smithy_query(query="shape",shape="ServerArn")`), spelled out here rather than
/// reused from the crate so the assertion is independent of the code that mints it.
fn matches_modelled_server_arn(arn: &str) -> bool {
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

async fn list(router: &MultiServiceRouter, body: &Value) -> Reply {
    call(router, "ListHypervisors", body).await
}

/// Walk every page of `ListHypervisors` at one `MaxResults`, asserting the page-level
/// invariants as it goes, and return the ARNs in the order they were handed out.
async fn walk_pages(router: &MultiServiceRouter, max_results: Option<i64>) -> Vec<String> {
    let mut seen = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let mut body = json!({});
        if let Some(max) = max_results {
            body["MaxResults"] = json!(max);
        }
        if let Some(token) = &token {
            body["NextToken"] = json!(token);
        }
        let reply = list(router, &body).await;
        assert_eq!(
            reply.status, 200,
            "case MaxResults={max_results:?} page {pages}: ListHypervisors must answer \
             200, got {} {:?}",
            reply.status, reply.body
        );
        let page = listed_arns(&reply);
        if let Some(max) = max_results {
            assert!(
                page.len() as i64 <= max,
                "case MaxResults={max_results:?} page {pages}: a page must hold at most \
                 {max} hypervisors, got {}",
                page.len()
            );
        }
        token = reply.next_token();
        if token.is_some() {
            assert!(
                !page.is_empty(),
                "case MaxResults={max_results:?} page {pages}: a page that promises more \
                 records must not be empty, got {:?}",
                reply.body
            );
        }
        seen.extend(page);
        pages += 1;
        assert!(
            pages < 64,
            "case MaxResults={max_results:?}: pagination did not terminate after {pages} \
             pages, seen {seen:?}"
        );
        if token.is_none() {
            return seen;
        }
    }
}

/// Each listed hypervisor is the record the store holds, member for member, in ARN
/// order — and reading it twice reports the same thing, because listing writes nothing.
#[tokio::test]
async fn a_listing_reports_the_records_the_store_holds_in_arn_order() {
    let engine = build_engine();
    let router = build_router(&engine);
    let rows = rows();
    let arns = import(&router, &rows).await;
    // The activation of every record is already due, so the assertions below are about
    // the members and the order rather than about the clock (which has its own case).
    set_online_due(
        &engine,
        &arns,
        SystemTime::now() - Duration::from_secs(3_600),
    );

    let mut expected_order = arns.clone();
    expected_order.sort();

    let reply = list(&router, &json!({})).await;
    assert_eq!(
        reply.status, 200,
        "ListHypervisors must answer 200, got {} {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.next_token(),
        None,
        "a listing that fits in one page must carry no NextToken, got {:?}",
        reply.body
    );
    assert_eq!(
        listed_arns(&reply),
        expected_order,
        "ListHypervisors must report every stored hypervisor, ordered by ARN, got {:?}",
        reply.body
    );

    let by_arn: BTreeMap<String, Listed> = listed(&reply)
        .into_iter()
        .map(|hypervisor| (hypervisor.arn.clone().unwrap_or_default(), hypervisor))
        .collect();
    for (row, arn) in rows.iter().zip(&arns) {
        let case = row.case;
        let reported = by_arn
            .get(arn)
            .unwrap_or_else(|| panic!("case {case:?}: {arn} is missing from {:?}", reply.body));
        assert_eq!(
            (
                reported.name.as_deref(),
                reported.host.as_deref(),
                reported.kms_key_arn.as_deref(),
                reported.state.as_deref(),
            ),
            (
                Some(row.name.as_str()),
                Some(row.host.as_str()),
                row.kms_key_arn.as_deref(),
                Some(HYPERVISOR_ONLINE),
            ),
            "case {case:?}: the listed hypervisor must carry the members the store \
             holds, got {reported:?}"
        );
        assert!(
            matches_modelled_server_arn(arn),
            "case {case:?}: the minted ARN must satisfy the modelled ServerArn, got \
             {arn:?}"
        );
        // And the same record, read out of the store, says the same thing — so the
        // response describes persisted state and not the request that created it.
        let state = service_state(&*engine).expect("this service's state");
        let store = state.lock();
        let stored = store
            .hypervisor(arn)
            .unwrap_or_else(|| panic!("case {case:?}: {arn} must be in the store"));
        assert_eq!(
            (
                stored.name.as_str(),
                stored.host.as_str(),
                stored.tags.clone()
            ),
            (row.name.as_str(), row.host.as_str(), row.tags.clone()),
            "case {case:?}: the store must hold what was imported, got {stored:?}"
        );
    }

    assert_eq!(
        service_state(&*engine)
            .expect("this service's state")
            .lock()
            .hypervisor_count(),
        rows.len(),
        "the store must hold exactly the imported hypervisors"
    );

    // A read changes nothing a later caller sees.
    let again = list(&router, &json!({})).await;
    assert_eq!(
        (again.status, listed(&again)),
        (200, listed(&reply)),
        "listing twice must report the same records, got {:?}",
        again.body
    );
}

/// Every page size walks the whole listing exactly once, in ARN order, and only the
/// last page carries no token.
#[tokio::test]
async fn every_page_size_walks_the_whole_listing_exactly_once() {
    let engine = build_engine();
    let router = build_router(&engine);
    let rows = rows();
    let arns = import(&router, &rows).await;
    set_online_due(
        &engine,
        &arns,
        SystemTime::now() - Duration::from_secs(3_600),
    );
    let mut expected_order = arns.clone();
    expected_order.sort();

    // Every page size from one record per page up past the record count, plus the
    // request that names none at all: the page BOUNDARY may move, the sequence may not.
    let sizes: Vec<Option<i64>> = std::iter::once(None)
        .chain((1..=(rows.len() as i64 + 2)).map(Some))
        .collect();
    for max_results in sizes {
        let walked = walk_pages(&router, max_results).await;
        assert_eq!(
            walked, expected_order,
            "case MaxResults={max_results:?}: paging must hand out every hypervisor \
             exactly once in ARN order, got {walked:?}"
        );
        assert_eq!(
            walked.iter().collect::<BTreeSet<_>>().len(),
            walked.len(),
            "case MaxResults={max_results:?}: no hypervisor may be repeated across \
             pages, got {walked:?}"
        );
    }
}

/// A `NextToken` this listing did not issue is refused with HTTP 400
/// `ValidationException`, whatever form it takes — including a token another listing of
/// this service handed out, whose boundary is a key of a different map.
#[tokio::test]
async fn a_next_token_this_listing_did_not_issue_is_refused() {
    let engine = build_engine();
    let router = build_router(&engine);
    let arns = import(&router, &rows()).await;
    set_online_due(
        &engine,
        &arns,
        SystemTime::now() - Duration::from_secs(3_600),
    );

    // Two gateways, so a `ListGateways` page really hands out a token.
    create(&router, "gw-one").await;
    create(&router, "gw-two").await;
    let gateway_token = call(&router, "ListGateways", &json!({ "MaxResults": 1 }))
        .await
        .next_token()
        .expect("a ListGateways page with records left must carry a NextToken");

    // (case, the NextToken as the client sends it). Every row satisfies the modelled
    // `NextToken` constraints — len 1..1000, pattern `^.+$` — so each reaches the
    // handler rather than the boundary validator.
    let bad_tokens: [(&str, String); 6] = [
        ("not hex at all", "not-a-token".to_string()),
        ("hex of text that is no ARN", hex::encode("page two please")),
        (
            "hex of an ARN of another service",
            hex::encode("arn:aws:s3:::example-bucket"),
        ),
        (
            "hex of a gateway ARN of this service",
            hex::encode(format!(
                "arn:aws:backup-gateway:{REGION}:{}:gateway/bgw-0000000000000000",
                engine::EMULATOR_ACCOUNT_ID
            )),
        ),
        ("a token a ListGateways page issued", gateway_token),
        (
            "hex of a hypervisor ARN with an empty id",
            hex::encode(format!(
                "arn:aws:backup-gateway:{REGION}:{}:hypervisor/",
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
            "case {case:?}: a refused listing must hand back no hypervisors, got {:?}",
            refused.body
        );
    }

    // A token this listing DID issue still resumes after its boundary, so the refusals
    // above are about the token and not about the operation.
    let first = list(&router, &json!({ "MaxResults": 1 })).await;
    let token = first
        .next_token()
        .expect("a page with records left must carry a NextToken");
    let second = list(&router, &json!({ "MaxResults": 1, "NextToken": token })).await;
    let mut expected_order = arns.clone();
    expected_order.sort();
    assert_eq!(
        (second.status, listed_arns(&second)),
        (200, vec![expected_order[1].clone()]),
        "a token this listing issued must resume after the boundary it encodes, got {:?}",
        second.body
    );
}

/// An empty store lists no hypervisors — and says so with a 200 and no token, not with
/// a refusal.
#[tokio::test]
async fn an_empty_store_lists_no_hypervisors() {
    let engine = build_engine();
    let router = build_router(&engine);

    // A gateway exists, so an empty hypervisor listing means "no hypervisors" and not
    // "nothing was ever created".
    create(&router, "gw-only").await;

    for (case, body) in [
        ("no MaxResults", json!({})),
        ("MaxResults 1", json!({ "MaxResults": 1 })),
        ("MaxResults 100", json!({ "MaxResults": 100 })),
    ] {
        let reply = list(&router, &body).await;
        assert_eq!(
            (reply.status, listed(&reply), reply.next_token()),
            (200, Vec::new(), None),
            "case {case:?}: an empty store must list no hypervisors, got {} {:?}",
            reply.status,
            reply.body
        );
    }
}

/// `State` is the transition the import already owes: a hypervisor is `PENDING` until
/// its activation falls due and `ONLINE` from then on, whoever calls and however often.
#[tokio::test]
async fn the_state_a_listing_reports_is_the_one_the_clock_owes() {
    let engine = build_engine();
    let router = build_router(&engine);
    let now = SystemTime::now();

    // (case, when the activation falls due, the state the clock owes the record now)
    let cases: [(&str, SystemTime, &str); 4] = [
        (
            "an activation due an hour ago",
            now - Duration::from_secs(3_600),
            HYPERVISOR_ONLINE,
        ),
        (
            "an activation due just before now",
            now - PAST_THE_PENDING_SPAN,
            HYPERVISOR_ONLINE,
        ),
        ("the span an import leaves", now + HYPERVISOR_PENDING_FOR, HYPERVISOR_PENDING),
        (
            "an activation that falls due an hour out",
            now + Duration::from_secs(3_600),
            HYPERVISOR_PENDING,
        ),
    ];

    let mut expected: BTreeMap<String, (&str, &str)> = BTreeMap::new();
    for (index, (case, online_due, state)) in cases.iter().enumerate() {
        let row = Row {
            case,
            name: format!("hv-clock-{index}"),
            // A host of its own: a record stands for the machine at its host, so two
            // imports cannot share one.
            host: format!("10.0.0.{}", index + 1),
            kms_key_arn: None,
            tags: BTreeMap::new(),
        };
        let arn = import(&router, &[row])
            .await
            .pop()
            .expect("one imported hypervisor");
        set_online_due(&engine, std::slice::from_ref(&arn), *online_due);
        expected.insert(arn, (case, state));
    }

    let reply = list(&router, &json!({})).await;
    assert_eq!(
        reply.status, 200,
        "ListHypervisors must answer 200, got {} {:?}",
        reply.status, reply.body
    );
    for hypervisor in listed(&reply) {
        let arn = hypervisor.arn.clone().unwrap_or_default();
        let (case, state) = expected
            .get(&arn)
            .unwrap_or_else(|| panic!("the listing reported an ARN nothing imported: {arn:?}"));
        assert_eq!(
            hypervisor.state.as_deref(),
            Some(*state),
            "case {case:?}: the reported state must be the one the clock owes, got \
             {hypervisor:?}"
        );
        assert!(
            hypervisor
                .state
                .as_deref()
                .is_some_and(|reported| HYPERVISOR_STATE_VALUES.contains(&reported)),
            "case {case:?}: the reported state must be a modelled HypervisorState, got \
             {hypervisor:?}"
        );
    }
    assert_eq!(
        listed(&reply).len(),
        cases.len(),
        "every imported hypervisor must be listed, got {:?}",
        reply.body
    );

    // A pending hypervisor stays pending while nothing but the clock moves: listing it
    // again does not advance it.
    let again = list(&router, &json!({})).await;
    assert_eq!(
        (again.status, listed(&again)),
        (200, listed(&reply)),
        "a listing must not move a hypervisor's state, got {:?}",
        again.body
    );

    // And the clock alone does: past the span an import leaves, the record whose
    // activation was due within it is ONLINE, with no call in between having touched it.
    tokio::time::sleep(PAST_THE_PENDING_SPAN).await;
    let settled = list(&router, &json!({})).await;
    for hypervisor in listed(&settled) {
        let arn = hypervisor.arn.clone().unwrap_or_default();
        let (case, _) = expected
            .get(&arn)
            .unwrap_or_else(|| panic!("the listing reported an ARN nothing imported: {arn:?}"));
        let expected_state = if *case == "an activation that falls due an hour out" {
            HYPERVISOR_PENDING
        } else {
            HYPERVISOR_ONLINE
        };
        assert_eq!(
            hypervisor.state.as_deref(),
            Some(expected_state),
            "case {case:?}: past the pending span the clock owes {expected_state}, got \
             {hypervisor:?}"
        );
    }
}
