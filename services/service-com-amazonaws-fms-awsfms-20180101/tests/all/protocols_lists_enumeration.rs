// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListProtocolsLists`: the enumeration of the Firewall
//! Manager protocols lists this service stores.
//!
//! Every test drives the service the way a client does: an awsJson1_1 `POST /`
//! through [`MultiServiceRouter::dispatch`], with the `X-Amz-Target` header and
//! a SigV4 credential scope, so the whole deserialize → handle → serialize walk
//! runs and the assertions are about the WIRE response.
//!
//! The rules under test:
//!
//! 1. Every stored list of the namespace the request selects is reported once,
//!    as the `ProtocolsListDataSummary` projection of that record (its ARN, id,
//!    name and protocols — and nothing else), in the stable `ListId` order the
//!    store keeps them in.
//! 2. `MaxResults` bounds the page, and a `NextToken` appears ONLY while a list
//!    of the selected namespace remains after the page; the token round-trips
//!    into the next page, so following it walks every list exactly once whatever
//!    the page size.
//! 3. `DefaultLists` selects the namespace: the Firewall Manager MANAGED lists
//!    when it is true, the customer's own lists when it is false or absent —
//!    exactly the split `GetProtocolsList` resolves an id in.
//! 4. The enumeration is a READ: it writes nothing, so two identical calls
//!    answer identically and it agrees with `GetProtocolsList` on every list.
//! 5. It is organisation-scoped, so with no administrator association it answers
//!    HTTP 400 `AccessDeniedException`; a `NextToken` this service never minted
//!    is refused with a 400 `ValidationException` rather than served as a page.
//!
//! The table in [`cases`] is the class rules 1-4 range over: the rows differ in
//! protocol count (none, one, several), in protocol form (keywords and protocol
//! numbers), in name length (one character up to the modelled 128) and in which
//! namespace the list belongs to. The page sizes span the modelled `MaxResults`
//! range (1..100), including both published bounds and one past each.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_fms_awsfms_20180101::state;

use crate::administrator_gated_operations::{associate_admin, seed_protocols_list, with_store};
use crate::policy_lifecycle::{call, call_in, emulator_with_engine, error_code, ACCOUNT, REGION};

/// A second Region, to assert the administrator gate's answer does not depend on
/// which Region the request addresses.
const OTHER_REGION: &str = "eu-central-1";

/// One row of the protocols-list table: a stored list, and what its summary must
/// report.
struct Case {
    /// Names the row in every assertion message.
    name: &'static str,
    list_name: String,
    protocols: Vec<String>,
}

fn protocols(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_string()).collect()
}

/// The customer protocols lists the rules are asserted over.
///
/// Protocol tokens are `com.amazonaws.fms#Protocol` values (a string of 1..20
/// characters): IP protocol keywords and the protocol NUMBERS that name the same
/// protocols, both of which a client may send. `ListName` is
/// `com.amazonaws.fms#ResourceName`, 1..128 characters, and the rows include
/// both published bounds.
fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "one protocol",
            list_name: "tcp-only".to_string(),
            protocols: protocols(&["TCP"]),
        },
        Case {
            name: "several protocols",
            list_name: "list.with-many_protocols".to_string(),
            protocols: protocols(&["TCP", "UDP", "ICMP", "ESP", "AH", "GRE"]),
        },
        Case {
            name: "protocol numbers rather than keywords",
            list_name: "numbers".to_string(),
            // 6 is TCP, 17 is UDP, 1 is ICMP.
            protocols: protocols(&["6", "17", "1"]),
        },
        Case {
            name: "no protocols at all, shortest modelled name",
            list_name: "e".to_string(),
            protocols: Vec::new(),
        },
        Case {
            // The modelled bounds: a 128-character ResourceName and a
            // 20-character Protocol token.
            name: "longest modelled name and protocol token",
            list_name: "n".repeat(128),
            protocols: protocols(&["P".repeat(20).as_str(), "TCP"]),
        },
    ]
}

/// The Firewall Manager MANAGED (default) lists, which no API creates, so they
/// are seeded through the store the way a handler reaches it.
///
/// Their ids are chosen to INTERLEAVE with the ids `PutProtocolsList` mints
/// (36 lowercase hex characters): one sorts before every minted id, one inside
/// their range and one after all of them, so the namespace filter has to hold
/// per page rather than only at the ends. Each is a `com.amazonaws.fms#ListId`:
/// 36 characters matching `^[a-z0-9A-Z-]{36}$`.
fn managed_lists() -> Vec<(String, &'static str, Vec<String>)> {
    vec![
        (
            "0".repeat(36),
            "fms-managed-before",
            protocols(&["TCP", "UDP"]),
        ),
        ("8".repeat(36), "fms-managed-inside", protocols(&["ICMP"])),
        (
            "z".repeat(36),
            "fms-managed-after",
            protocols(&["TCP", "ICMP", "ESP"]),
        ),
    ]
}

/// The `ProtocolsList` array of a JSON value, as the strings it carries.
fn protocol_strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|values| {
            values
                .iter()
                .map(|value| value.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// What a stored list's summary must be: the four members
/// `ProtocolsListDataSummary` declares, and nothing else.
fn want_summary(list_id: &str, region: &str, list_name: &str, protocols: &[String]) -> Value {
    json!({
        "ListArn": state::protocols_list_arn(region, list_id),
        "ListId": list_id,
        "ListName": list_name,
        "ProtocolsList": protocols,
    })
}

/// Create the table's customer lists through `PutProtocolsList` and return what
/// each one's summary must report, in `ListId` order (the order pages follow).
async fn create_customer_lists(router: &MultiServiceRouter) -> Vec<(String, Value)> {
    let mut expected: Vec<(String, Value)> = Vec::new();
    for case in cases() {
        let (status, body) = call(
            router,
            "PutProtocolsList",
            &json!({
                "ProtocolsList": {
                    "ListName": case.list_name,
                    "ProtocolsList": case.protocols,
                },
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: PutProtocolsList must answer 200, got {status} with {body}",
            case.name
        );
        let list_id = body["ProtocolsList"]["ListId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: the create minted no ListId: {body}", case.name))
            .to_string();
        let want = want_summary(&list_id, REGION, &case.list_name, &case.protocols);
        expected.push((list_id, want));
    }
    expected.sort_by(|a, b| a.0.cmp(&b.0));
    expected
}

/// Walk every page of one `ListProtocolsLists` enumeration and return the
/// summaries in the order they were served.
async fn walk_pages(
    router: &MultiServiceRouter,
    default_lists: Option<bool>,
    max_results: u32,
    total: usize,
) -> Vec<Value> {
    let label = format!("DefaultLists={default_lists:?}/MaxResults={max_results}");
    let mut seen: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let mut request = json!({ "MaxResults": max_results });
        if let Some(default_lists) = default_lists {
            request["DefaultLists"] = json!(default_lists);
        }
        if let Some(token) = &token {
            request["NextToken"] = json!(token);
        }
        let (status, body) = call(router, "ListProtocolsLists", &request).await;
        assert_eq!(
            status, 200,
            "case {label}: expected 200, got {status} with {body}"
        );
        let page = body["ProtocolsLists"]
            .as_array()
            .unwrap_or_else(|| panic!("case {label}: no ProtocolsLists in {body}"))
            .clone();
        assert!(
            page.len() <= max_results as usize,
            "case {label}: a page carried {} summaries, more than MaxResults",
            page.len()
        );
        seen.extend(page);
        pages += 1;
        assert!(
            pages <= total + 1,
            "case {label}: paging did not terminate after {pages} pages"
        );
        match body["NextToken"].as_str() {
            Some(next) => {
                assert!(
                    seen.len() < total,
                    "case {label}: a NextToken was served after all {total} lists had been \
                     reported, got {body}"
                );
                token = Some(next.to_string());
            }
            None => break,
        }
    }
    seen
}

/// Rules 1 and 2: every stored customer list is reported once, as its own
/// projection, in `ListId` order, whatever the page size — and the managed lists
/// seeded around them are never reported in that namespace.
#[tokio::test]
async fn every_stored_list_is_reported_once_as_its_projection_whatever_the_page_size() {
    let (router, engine) = emulator_with_engine();
    associate_admin(&router).await;

    // An empty store enumerates to an empty page with no token.
    let (status, body) = call(
        &router,
        "ListProtocolsLists",
        &json!({ "MaxResults": 100 }),
    )
    .await;
    assert_eq!(
        status, 200,
        "an empty store must still answer 200, got {status} with {body}"
    );
    assert_eq!(
        body["ProtocolsLists"],
        json!([]),
        "an empty store must report no lists, got {body}"
    );
    assert!(
        body["NextToken"].is_null(),
        "a last page must carry no NextToken, got {body}"
    );

    let expected = create_customer_lists(&router).await;
    with_store(&engine, |store| {
        for (list_id, list_name, values) in managed_lists() {
            let refs: Vec<&str> = values.iter().map(String::as_str).collect();
            seed_protocols_list(store, &list_id, list_name, &refs, true);
        }
    });

    // Page sizes spanning the modelled MaxResults range (1..100): both published
    // bounds, and sizes above and below the number of stored lists.
    for max_results in [1, 2, 3, expected.len() as u32, 100] {
        let seen = walk_pages(&router, Some(false), max_results, expected.len()).await;
        assert_eq!(
            seen.len(),
            expected.len(),
            "case MaxResults={max_results}: expected {} summaries across all pages, got {}",
            expected.len(),
            seen.len()
        );
        for (summary, (list_id, want)) in seen.iter().zip(&expected) {
            assert_eq!(
                summary, want,
                "case MaxResults={max_results}: the summary of {list_id} is not the stored \
                 list's projection, got {summary}"
            );
        }
    }
}

/// Rule 3: `DefaultLists` selects the namespace, and absent means false.
#[tokio::test]
async fn default_lists_selects_the_managed_namespace() {
    let (router, engine) = emulator_with_engine();
    associate_admin(&router).await;

    let customer = create_customer_lists(&router).await;
    with_store(&engine, |store| {
        for (list_id, list_name, values) in managed_lists() {
            let refs: Vec<&str> = values.iter().map(String::as_str).collect();
            seed_protocols_list(store, &list_id, list_name, &refs, true);
        }
    });

    // The managed namespace reports exactly the seeded managed lists, in id
    // order, and never a customer list.
    let mut managed: Vec<(String, Value)> = managed_lists()
        .into_iter()
        .map(|(list_id, list_name, values)| {
            let want = want_summary(&list_id, REGION, list_name, &values);
            (list_id, want)
        })
        .collect();
    managed.sort_by(|a, b| a.0.cmp(&b.0));

    for max_results in [1, 2, 100] {
        let seen = walk_pages(&router, Some(true), max_results, managed.len()).await;
        assert_eq!(
            seen.len(),
            managed.len(),
            "case MaxResults={max_results}: expected the {} managed lists, got {seen:?}",
            managed.len()
        );
        for (summary, (list_id, want)) in seen.iter().zip(&managed) {
            assert_eq!(
                summary, want,
                "case MaxResults={max_results}: the summary of managed list {list_id} is not its \
                 projection, got {summary}"
            );
        }
    }

    // DefaultLists is modelled with a default of false, so sending nothing
    // enumerates the customer namespace.
    let absent = walk_pages(&router, None, 2, customer.len()).await;
    let explicit = walk_pages(&router, Some(false), 2, customer.len()).await;
    assert_eq!(
        absent, explicit,
        "an absent DefaultLists must enumerate the same namespace as DefaultLists=false, got \
         {absent:?} then {explicit:?}"
    );
    let customer_summaries: Vec<Value> =
        customer.iter().map(|(_, want)| want.clone()).collect();
    assert_eq!(
        absent, customer_summaries,
        "the customer namespace must report exactly the created lists, got {absent:?}"
    );
}

/// Rule 4: the enumeration is a read — it changes nothing a later caller sees,
/// and it agrees with `GetProtocolsList` on every list it reports.
#[tokio::test]
async fn the_enumeration_is_a_read_and_agrees_with_get_protocols_list() {
    let (router, engine) = emulator_with_engine();
    associate_admin(&router).await;

    let expected = create_customer_lists(&router).await;
    with_store(&engine, |store| {
        for (list_id, list_name, values) in managed_lists() {
            let refs: Vec<&str> = values.iter().map(String::as_str).collect();
            seed_protocols_list(store, &list_id, list_name, &refs, true);
        }
    });

    // Two identical calls answer identically, for each namespace and page size.
    for default_lists in [false, true] {
        for max_results in [1, 100] {
            let request = json!({ "DefaultLists": default_lists, "MaxResults": max_results });
            let first = call(&router, "ListProtocolsLists", &request).await;
            let second = call(&router, "ListProtocolsLists", &request).await;
            assert_eq!(
                first, second,
                "case DefaultLists={default_lists}/MaxResults={max_results}: a read must not \
                 change what the next caller sees, got {first:?} then {second:?}"
            );
        }
    }

    // And the summary is the same record `GetProtocolsList` reports.
    for (default_lists, listed) in [
        (false, walk_pages(&router, Some(false), 2, expected.len()).await),
        (
            true,
            walk_pages(&router, Some(true), 2, managed_lists().len()).await,
        ),
    ] {
        for summary in &listed {
            let list_id = summary["ListId"].as_str().expect("a listed ListId");
            let (status, got) = call(
                &router,
                "GetProtocolsList",
                &json!({ "ListId": list_id, "DefaultList": default_lists }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {list_id:?}: a listed list must be readable, got {status} with {got}"
            );
            let want = want_summary(
                list_id,
                REGION,
                got["ProtocolsList"]["ListName"].as_str().unwrap_or_default(),
                &protocol_strings(&got["ProtocolsList"]["ProtocolsList"]),
            );
            assert_eq!(
                *summary, want,
                "case {list_id:?}: the summary must project the record GetProtocolsList reports, \
                 got {summary} beside {got}"
            );
            assert_eq!(
                summary["ListArn"], got["ProtocolsListArn"],
                "case {list_id:?}: the summary's ARN must be the one GetProtocolsList reports, \
                 got {summary} beside {got}"
            );
        }
    }
}

/// Rule 5: the administrator gate, the refusal of a foreign token, and the
/// modelled input constraints.
#[tokio::test]
async fn the_enumeration_is_gated_and_refuses_input_the_model_does_not_allow() {
    // With no administrator association the enumeration is refused in every
    // Region and for either namespace, before any token is decoded.
    for region in [REGION, OTHER_REGION] {
        let (router, _engine) = emulator_with_engine();
        for request in [
            json!({ "MaxResults": 1 }),
            json!({ "DefaultLists": true, "MaxResults": 100 }),
            json!({ "MaxResults": 5, "NextToken": "not-a-token-this-service-minted" }),
        ] {
            let (status, body) =
                call_in(&router, "ListProtocolsLists", &request, region).await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "AccessDeniedException"),
                "case {request} in {region}: with no administrator the call must be refused, got \
                 {status} with {body}"
            );
            let message = body
                .get("message")
                .or_else(|| body.get("Message"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            assert!(
                message.contains(ACCOUNT) && message.contains(region),
                "case {request} in {region}: the refusal's message is built from the request, so \
                 it must name the account and Region, got {body}"
            );
        }
    }

    let (router, _engine) = emulator_with_engine();
    associate_admin(&router).await;
    create_customer_lists(&router).await;

    // A token this service never minted names no page boundary.
    let foreign_tokens = vec![
        "not-a-token-this-service-minted".to_string(),
        // Valid hex, but not a 36-character `ListId` once decoded.
        ::hex::encode("short"),
        ::hex::encode("x".repeat(37)),
    ];
    for foreign in &foreign_tokens {
        let (status, body) = call(
            &router,
            "ListProtocolsLists",
            &json!({ "MaxResults": 2, "NextToken": foreign }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {foreign:?}: a foreign NextToken must be refused, got {status} with {body}"
        );
    }

    // MaxResults is required, and one past each published bound of its modelled
    // range (1..100) is refused rather than served.
    let (status, body) = call(&router, "ListProtocolsLists", &json!({})).await;
    assert_eq!(
        status, 400,
        "MaxResults is a required member, so a request without it must be refused, got {status} \
         with {body}"
    );
    for out_of_range in [0, 101] {
        let (status, body) = call(
            &router,
            "ListProtocolsLists",
            &json!({ "MaxResults": out_of_range }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case MaxResults={out_of_range}: a value outside 1..100 must be refused, got {status} \
             with {body}"
        );
    }
}
