// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `ListDiscoverers`.
//!
//! The rule these cases pin: `ListDiscoverers` REPORTS the discoverers this store
//! holds — every one of them, each as the last write to it left it — in the stable
//! order of the ids the service minted; `DiscovererIdPrefix` and `SourceArnPrefix`
//! each LIMIT that listing to the entries whose id, respectively source ARN, starts
//! with the prefix sent, and `Limit`/`NextToken` page over the selected order without
//! repeating or skipping an entry.
//!
//! The rule is asserted over a TABLE whose rows differ along what it generalises
//! over: which members the listed discoverers carry, which state they are in, how
//! many there are, which prefix selects them (length, both members at once, the empty
//! prefix and one that matches nothing), and what page size the walk uses. The
//! recorded `GET /v1/discoverers` with no query string is one row of it.
//!
//! Every case drives the real wire path: a signed `restJson1` request through
//! `MultiServiceRouter::dispatch`, exactly as the production router serves it. Nothing
//! here touches the state struct directly, so a handler that answered from its input
//! rather than from the store would still have to make `DescribeDiscoverer` agree.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use crate::discoverer_state::{
    create_discoverer, discoverer_uri, source_arn, stop_uri, DISCOVERERS_URI,
};
use crate::registry_schema_lifecycle::{walk, Answer, Emulator, ACCOUNT, REGION};

/// `ListDiscoverers`' route (`GET /v1/discoverers -> 200`).
const LIST_URI: &str = "/v1/discoverers";

/// The `DiscovererId` of every summary a `ListDiscoverers` answer reports, in the
/// order the answer lists them.
fn discoverer_ids(answer: &Answer) -> Vec<String> {
    answer.body["Discoverers"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a Discoverers array in {:?}", answer.body))
        .iter()
        .map(|item| {
            item["DiscovererId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The one summary a listing reports for `discoverer_id`, or `None` when the listing
/// does not report it.
fn summary_of(answer: &Answer, discoverer_id: &str) -> Option<Value> {
    answer.body["Discoverers"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a Discoverers array in {:?}", answer.body))
        .iter()
        .find(|item| item["DiscovererId"].as_str() == Some(discoverer_id))
        .cloned()
}

/// Ask for a listing, asserting it was served, with `query` appended to the route.
async fn list(emulator: &Emulator, case: &str, query: &str) -> Answer {
    let uri = if query.is_empty() {
        LIST_URI.to_string()
    } else {
        format!("{LIST_URI}?{query}")
    };
    let answer = emulator.call("GET", &uri, None).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected 200 from GET {uri}, got {} {:?}",
        answer.status, answer.body
    );
    answer
}

/// One discoverer the tables below list.
#[derive(Debug)]
struct ListedCase {
    case: &'static str,
    /// The `SourceArn` this row's discoverer subscribes to; distinct per row, since
    /// one source carries at most one discoverer.
    source_arn: String,
    description: Option<String>,
    /// The `CrossAccount` the create sends, or `None` to omit the member.
    cross_account: Option<bool>,
    tags: BTreeMap<&'static str, &'static str>,
    /// Whether the discoverer is stopped after it is created, so the listing is
    /// asserted over both states a discoverer can be in.
    stopped: bool,
}

impl ListedCase {
    /// The `CreateDiscoverer` body this row sends. `Tags` travels as `tags`
    /// (`smithy_query(query="operation", operation="CreateDiscoverer")`: the member
    /// carries `jsonName: "tags"`).
    fn body(&self) -> Value {
        let mut body = json!({"SourceArn": self.source_arn});
        if let Some(description) = &self.description {
            body["Description"] = json!(description);
        }
        if let Some(cross_account) = self.cross_account {
            body["CrossAccount"] = json!(cross_account);
        }
        if !self.tags.is_empty() {
            body["tags"] = json!(self.tags);
        }
        body
    }

    /// The `CrossAccount` the stored discoverer carries: what the request sent, or
    /// `true` where it sent nothing (`smithy_query(query="doc",
    /// shape="CreateDiscovererRequest", member="CrossAccount")`: "(default: true)").
    fn expected_cross_account(&self) -> bool {
        self.cross_account.unwrap_or(true)
    }
}

/// The table of discoverers a listing must report. Rows differ along the presence of
/// `Description`, the presence and value of `CrossAccount`, how many tags the
/// discoverer holds, the form of the source ARN (partition, region, account and
/// resource path), and the state the discoverer ends up in.
fn listed_cases() -> Vec<ListedCase> {
    vec![
        ListedCase {
            case: "nothing but the required source",
            source_arn: source_arn("plain"),
            description: None,
            cross_account: None,
            tags: BTreeMap::new(),
            stopped: false,
        },
        ListedCase {
            case: "described, cross-account off, tagged",
            source_arn: source_arn("described"),
            description: Some("orders bus discoverer".to_string()),
            cross_account: Some(false),
            tags: BTreeMap::from([("env", "test"), ("team", "schemas")]),
            stopped: false,
        },
        ListedCase {
            case: "stopped after it was created",
            source_arn: source_arn("stopped"),
            description: Some(String::new()),
            cross_account: Some(true),
            tags: BTreeMap::from([("only", "one")]),
            stopped: true,
        },
        ListedCase {
            case: "a source in another partition, region and account",
            source_arn: "arn:aws-cn:events:cn-north-1:210987654321:event-bus/other".to_string(),
            description: Some("caf\u{e9} \u{2014} \u{30b9}\u{30ad}\u{30fc}\u{30de}".to_string()),
            cross_account: None,
            tags: BTreeMap::new(),
            stopped: true,
        },
        ListedCase {
            case: "a source ARN whose region and account fields are empty",
            source_arn: "arn:aws:events:::event-bus/global-events-bus".to_string(),
            description: None,
            cross_account: Some(false),
            tags: BTreeMap::from([("a", ""), ("b", "2"), ("c", "3")]),
            stopped: false,
        },
    ]
}

/// RULE: a listing reports every stored discoverer as the last write to it left it, so
/// each summary carries the id and the ARN the create minted, the source the create
/// named, the state the last start/stop left and the tags the resource holds — and the
/// same values a read of that one discoverer reports.
#[tokio::test]
async fn a_listing_reports_every_stored_discoverer_as_it_is_stored() {
    let emulator = Emulator::new();

    // An empty store lists nothing — and still answers the listing, with no token.
    let empty = list(&emulator, "an empty store", "").await;
    assert!(
        discoverer_ids(&empty).is_empty(),
        "case \"an empty store\": expected no discoverer, got {:?}",
        empty.body
    );
    assert!(
        empty.body.get("NextToken").is_none_or(Value::is_null),
        "case \"an empty store\": a complete listing carries no NextToken, got {:?}",
        empty.body
    );

    let cases = listed_cases();
    let mut created_ids: Vec<String> = Vec::new();
    for case in &cases {
        let created = emulator
            .call("POST", DISCOVERERS_URI, Some(case.body()))
            .await;
        assert_eq!(
            created.status, 201,
            "case {:?}: expected 201 from CreateDiscoverer, got {} {:?}",
            case.case, created.status, created.body
        );
        let discoverer_id = created.str_at("DiscovererId");
        if case.stopped {
            let stopped = emulator.call("POST", &stop_uri(&discoverer_id), None).await;
            assert_eq!(
                stopped.status, 200,
                "case {:?}: expected 200 from StopDiscoverer, got {} {:?}",
                case.case, stopped.status, stopped.body
            );
        }
        created_ids.push(discoverer_id);
    }

    let listed = list(&emulator, "every stored discoverer", "").await;

    // The listing is served in the stable order of the minted ids, so a NextToken can
    // name a position in it.
    let mut expected_order = created_ids.clone();
    expected_order.sort();
    assert_eq!(
        discoverer_ids(&listed),
        expected_order,
        "case \"every stored discoverer\": the listing must report every id once, in \
         key order, got {:?}",
        listed.body
    );

    for (case, discoverer_id) in cases.iter().zip(&created_ids) {
        let summary = summary_of(&listed, discoverer_id).unwrap_or_else(|| {
            panic!(
                "case {:?}: expected the listing to report {discoverer_id}, got {:?}",
                case.case, listed.body
            )
        });
        assert_eq!(
            summary["DiscovererArn"].as_str(),
            Some(format!("arn:aws:schemas:{REGION}:{ACCOUNT}:discoverer/{discoverer_id}").as_str()),
            "case {:?}: the summary reports the ARN the create minted, got {summary:?}",
            case.case
        );
        assert_eq!(
            summary["SourceArn"].as_str(),
            Some(case.source_arn.as_str()),
            "case {:?}: the summary reports the source the create named, got {summary:?}",
            case.case
        );
        let expected_state = if case.stopped { "STOPPED" } else { "STARTED" };
        assert_eq!(
            summary["State"].as_str(),
            Some(expected_state),
            "case {:?}: expected State {expected_state}, got {summary:?}",
            case.case
        );
        assert_eq!(
            summary["CrossAccount"].as_bool(),
            Some(case.expected_cross_account()),
            "case {:?}: expected CrossAccount {}, got {summary:?}",
            case.case,
            case.expected_cross_account()
        );
        assert_eq!(
            summary["tags"],
            json!(case.tags),
            "case {:?}: expected the stored tags {:?}, got {summary:?}",
            case.case,
            case.tags
        );

        // And the one-resource read agrees with the listing, member for member.
        let described = emulator.call("GET", &discoverer_uri(discoverer_id), None).await;
        assert_eq!(
            described.status, 200,
            "case {:?}: expected 200 from DescribeDiscoverer, got {} {:?}",
            case.case, described.status, described.body
        );
        for member in [
            "DiscovererId",
            "DiscovererArn",
            "SourceArn",
            "State",
            "CrossAccount",
            "tags",
        ] {
            assert_eq!(
                summary.get(member),
                described.body.get(member),
                "case {:?}: the listing's {member} must be what a read of that \
                 discoverer reports, got {:?} and {:?}",
                case.case,
                summary.get(member),
                described.body.get(member)
            );
        }
        // `DiscovererSummary` models no `Description`
        // (`smithy_query(query="shape", shape="DiscovererSummary")`), so a summary
        // carries none however the discoverer was described.
        assert!(
            summary.get("Description").is_none_or(Value::is_null),
            "case {:?}: a summary models no Description, got {summary:?}",
            case.case
        );
    }

    // A listing is a read: asking again reports exactly the same thing.
    let again = list(&emulator, "a listing read twice", "").await;
    assert_eq!(
        again.body, listed.body,
        "case \"a listing read twice\": a read must not change what a later caller sees"
    );
}

/// RULE: a deleted discoverer is no longer stored, so the listing stops reporting it
/// and reports every other discoverer unchanged.
#[tokio::test]
async fn a_deleted_discoverer_leaves_the_listing() {
    let emulator = Emulator::new();
    let case = "three discoverers, the middle one deleted";

    let mut ids: Vec<String> = Vec::new();
    for bus in ["gone-a", "gone-b", "gone-c"] {
        ids.push(create_discoverer(&emulator, case, bus).await);
    }
    ids.sort();

    let deleted = emulator.call("DELETE", &discoverer_uri(&ids[1]), None).await;
    assert_eq!(
        deleted.status, 204,
        "case {case:?}: expected 204 from DeleteDiscoverer, got {} {:?}",
        deleted.status, deleted.body
    );

    let listed = list(&emulator, case, "").await;
    assert_eq!(
        discoverer_ids(&listed),
        vec![ids[0].clone(), ids[2].clone()],
        "case {case:?}: the listing must report exactly the discoverers still stored, \
         got {:?}",
        listed.body
    );
}

/// One prefix selection and the discoverers it must select.
#[derive(Debug)]
struct PrefixCase {
    case: String,
    query: String,
    expected: Vec<String>,
}

/// RULE: each prefix member LIMITS the listing to the stored discoverers whose id,
/// respectively source ARN, starts with the value sent — the two together select the
/// entries matching both, the empty prefix selects everything, and a prefix nothing
/// starts with selects nothing.
#[tokio::test]
async fn each_prefix_member_selects_exactly_the_entries_that_start_with_it() {
    let emulator = Emulator::new();

    // Three sources that share structured ARN prefixes, so a prefix of the source ARN
    // can select one, two or all of them.
    let buses = ["alpha-one", "alpha-two", "beta-one"];
    let mut by_bus: BTreeMap<&str, String> = BTreeMap::new();
    for bus in buses {
        by_bus.insert(
            bus,
            create_discoverer(&emulator, "prefix selection", bus).await,
        );
    }
    let mut all_ids: Vec<String> = by_bus.values().cloned().collect();
    all_ids.sort();

    let sorted = all_ids.clone();
    let first = sorted[0].clone();
    let alpha_prefix = format!("arn:aws:events:{REGION}:{ACCOUNT}:event-bus/alpha");
    let bus_prefix = format!("arn:aws:events:{REGION}:{ACCOUNT}:event-bus/");
    let mut alpha_ids = vec![by_bus["alpha-one"].clone(), by_bus["alpha-two"].clone()];
    alpha_ids.sort();

    // Rows: the id prefix at several lengths (one character, half the id, the whole
    // id), the empty prefix, a prefix of a form no minted id has, the source-ARN
    // prefix selecting all, some and one, both members at once (agreeing and
    // disagreeing), and no member at all.
    let cases = vec![
        PrefixCase {
            case: "no prefix member at all".to_string(),
            query: String::new(),
            expected: all_ids.clone(),
        },
        PrefixCase {
            case: "the empty id prefix selects everything".to_string(),
            query: "discovererIdPrefix=".to_string(),
            expected: all_ids.clone(),
        },
        PrefixCase {
            case: "the whole id of one discoverer".to_string(),
            query: format!("discovererIdPrefix={first}"),
            expected: vec![first.clone()],
        },
        PrefixCase {
            case: "the first character of one id".to_string(),
            query: format!("discovererIdPrefix={}", &first[..1]),
            expected: all_ids
                .iter()
                .filter(|id| id.starts_with(&first[..1]))
                .cloned()
                .collect(),
        },
        PrefixCase {
            case: "half of one id".to_string(),
            query: format!("discovererIdPrefix={}", &first[..16]),
            expected: vec![first.clone()],
        },
        PrefixCase {
            // A minted id is 32 lower-case hex digits, so no stored id starts with
            // this: `tests/all/discoverer_state.rs` pins the form.
            case: "an id prefix no minted id can start with".to_string(),
            query: "discovererIdPrefix=zz".to_string(),
            expected: Vec::new(),
        },
        PrefixCase {
            case: "a source prefix every source shares".to_string(),
            query: format!("sourceArnPrefix={bus_prefix}"),
            expected: all_ids.clone(),
        },
        PrefixCase {
            case: "a source prefix two sources share".to_string(),
            query: format!("sourceArnPrefix={alpha_prefix}"),
            expected: alpha_ids.clone(),
        },
        PrefixCase {
            case: "a whole source ARN".to_string(),
            query: format!("sourceArnPrefix={}", source_arn("beta-one")),
            expected: vec![by_bus["beta-one"].clone()],
        },
        PrefixCase {
            case: "a source prefix no source starts with".to_string(),
            query: "sourceArnPrefix=arn:aws:sqs:".to_string(),
            expected: Vec::new(),
        },
        PrefixCase {
            case: "both members, agreeing on one discoverer".to_string(),
            query: format!(
                "discovererIdPrefix={}&sourceArnPrefix={}",
                by_bus["alpha-two"],
                source_arn("alpha-two")
            ),
            expected: vec![by_bus["alpha-two"].clone()],
        },
        PrefixCase {
            case: "both members, disagreeing".to_string(),
            query: format!(
                "discovererIdPrefix={}&sourceArnPrefix={}",
                by_bus["alpha-two"],
                source_arn("beta-one")
            ),
            expected: Vec::new(),
        },
        PrefixCase {
            case: "the empty prefix on both members".to_string(),
            query: "discovererIdPrefix=&sourceArnPrefix=".to_string(),
            expected: all_ids.clone(),
        },
    ];

    for case in cases {
        let answer = list(&emulator, &case.case, &case.query).await;
        assert_eq!(
            discoverer_ids(&answer),
            case.expected,
            "case {:?}: the prefix must select exactly the matching discoverers, got {:?}",
            case.case,
            answer.body
        );
    }
}

/// RULE (an invariant): following `NextToken` walks the listing exactly once — the
/// concatenated pages are the full selection in key order, with no repeat and no gap —
/// and the last page carries no token, for every page size.
#[tokio::test]
async fn following_next_token_walks_the_listing_exactly_once() {
    // Rows: the listing's size crossed with the page size, including an empty store, a
    // page size of 1, one that divides the total exactly, one larger than the total,
    // and the unpaged case.
    for total in [0_usize, 1, 2, 5, 7] {
        let emulator = Emulator::new();
        let mut expected: Vec<String> = Vec::new();
        for index in 0..total {
            expected.push(
                create_discoverer(&emulator, "paged listing", &format!("bus-{index:02}")).await,
            );
        }
        expected.sort();

        for limit in [None, Some(1), Some(2), Some(total.max(1)), Some(total + 3)] {
            let case = format!("total={total} limit={limit:?}");
            let walked = walk(&emulator, LIST_URI, limit, discoverer_ids).await;
            assert_eq!(
                walked, expected,
                "case {case}: ListDiscoverers must walk every discoverer once, in key \
                 order"
            );
            assert_eq!(
                walked.iter().collect::<BTreeSet<_>>().len(),
                walked.len(),
                "case {case}: a walk must not repeat an entry, got {walked:?}"
            );
        }

        // The same invariant over a SELECTED listing: a prefix that keeps a subset.
        let selected: Vec<String> = expected
            .iter()
            .filter(|id| id.starts_with('a'))
            .cloned()
            .collect();
        for limit in [None, Some(1), Some(3)] {
            let case = format!("total={total} limit={limit:?} prefix=a");
            let walked = walk(
                &emulator,
                &format!("{LIST_URI}?discovererIdPrefix=a"),
                limit,
                discoverer_ids,
            )
            .await;
            assert_eq!(
                walked, selected,
                "case {case}: a selected listing must walk its selection once, in key \
                 order"
            );
        }
    }
}

/// RULE: no single response materialises a whole listing — a page holds at most
/// `MAX_PAGE_SIZE` items whatever `Limit` asks for, and the rest is reachable only
/// through `NextToken`.
#[tokio::test]
async fn a_page_is_capped_server_side_however_large_the_limit() {
    use service_com_amazonaws_schemas_schemas::state::MAX_PAGE_SIZE;

    let emulator = Emulator::new();
    let over = MAX_PAGE_SIZE + 5;
    for index in 0..over {
        create_discoverer(&emulator, "capped page", &format!("bus-{index:04}")).await;
    }

    // Rows: `Limit` absent, exactly the ceiling, one past it, and absurdly large.
    let ceiling = i64::try_from(MAX_PAGE_SIZE).expect("the ceiling fits an i64");
    for query in [
        String::new(),
        format!("limit={ceiling}"),
        format!("limit={}", ceiling + 1),
        format!("limit={}", i32::MAX),
    ] {
        let answer = list(&emulator, &query, &query).await;
        let page = discoverer_ids(&answer);
        assert_eq!(
            page.len(),
            MAX_PAGE_SIZE,
            "case {query:?}: a page holds at most {MAX_PAGE_SIZE} items, got {}",
            page.len()
        );
        assert!(
            answer
                .body
                .get("NextToken")
                .and_then(Value::as_str)
                .is_some(),
            "case {query:?}: the rest of the listing is reachable through NextToken, \
             got {:?}",
            answer.body
        );
    }

    // And the whole selection is still reachable, one capped page at a time.
    let walked = walk(&emulator, LIST_URI, None, discoverer_ids).await;
    assert_eq!(
        walked.len(),
        over,
        "case \"walked to exhaustion\": expected {over} discoverers, got {}",
        walked.len()
    );
}

/// RULE: a pagination member outside what it can mean is refused 400 — a page of fewer
/// than one item, and a `NextToken` this service never issued — and a token names a
/// position within ONE selection, so it does not resume a differently-selected
/// listing.
#[tokio::test]
async fn an_unusable_pagination_member_is_refused() {
    let emulator = Emulator::new();
    for index in 0..4 {
        create_discoverer(&emulator, "refusals", &format!("bus-{index}")).await;
    }

    // Rows: a zero limit, a negative limit, and tokens of a form this service does not
    // mint.
    for query in ["limit=0", "limit=-1", "nextToken=not-hex", "nextToken=zz"] {
        let answer = emulator
            .call("GET", &format!("{LIST_URI}?{query}"), None)
            .await;
        assert_eq!(
            answer.status, 400,
            "case {query:?}: expected 400, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "BadRequestException",
            "case {query:?}: expected BadRequestException, got {:?}",
            answer.body
        );
    }

    // A token minted for the whole listing does not resume a prefixed one, and vice
    // versa.
    let whole = list(&emulator, "a token for the whole listing", "limit=1").await;
    let whole_token = whole
        .body
        .get("NextToken")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected a NextToken in {:?}", whole.body))
        .to_string();
    let listed = discoverer_ids(&whole);
    let common = listed[0][..1].to_string();
    let prefixed = list(
        &emulator,
        "a token for a prefixed listing",
        &format!("limit=1&discovererIdPrefix={common}"),
    )
    .await;

    for (case, query) in [
        (
            "the whole listing's token presented to a prefixed one",
            format!("limit=1&discovererIdPrefix={common}&nextToken={whole_token}"),
        ),
        (
            "a prefixed listing's token presented to the whole one",
            match prefixed.body.get("NextToken").and_then(Value::as_str) {
                Some(token) => format!("limit=1&nextToken={token}"),
                // With one matching entry the prefixed listing is complete and mints
                // no token; the row above already covers the cross-selection refusal.
                None => format!("limit=1&nextToken={whole_token}&sourceArnPrefix=arn:aws:"),
            },
        ),
    ] {
        let answer = emulator
            .call("GET", &format!("{LIST_URI}?{query}"), None)
            .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: a token does not resume another selection, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {:?}",
            answer.body
        );
    }
}
