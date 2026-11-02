// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListInstances`: the paginated report of the instance records
//! `RegisterInstance` persisted under one service.
//!
//! The rule under test: `ListInstances` answers with the instances STORED for the
//! service the request names (by id or by ARN), in one stable order, capped at
//! `MaxResults` (100 when the request names none), handing back a `NextToken` exactly
//! when a further stored instance follows the page — and that token, which is bound to
//! this listing of this service, resumes the walk after the last instance the page
//! reported. The recorded case (one instance, registered then deregistered, listed
//! with no paging parameters) is one row of the tables below.
//!
//! Requests go through `MultiServiceRouter::dispatch`, reusing the harness in
//! `cloud_map_core.rs`.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use crate::cloud_map_core::{emulator, failure, namespace, ok, service};
use smithy_server_runtime::MultiServiceRouter;

/// `ListInstancesRequest$MaxResults`: "If you don't specify a value for MaxResults,
/// Cloud Map returns up to 100 instances." Its declared range is 1..100.
const DEFAULT_MAX_RESULTS: usize = 100;

/// One `ListInstances` page: the `(InstanceId, Attributes)` pairs it reported in the
/// order it reported them, plus the `NextToken` it handed back.
async fn page(
    router: &MultiServiceRouter,
    body: Value,
) -> (Vec<(String, Value)>, Option<String>) {
    let listed = ok(router, "ListInstances", body).await;
    let instances = listed["Instances"]
        .as_array()
        .unwrap_or_else(|| panic!("ListInstances answers with a list, got {listed}"))
        .iter()
        .map(|summary| {
            let id = summary["Id"]
                .as_str()
                .unwrap_or_else(|| panic!("every summary names its instance, got {summary}"))
                .to_string();
            (id, summary["Attributes"].clone())
        })
        .collect();
    let next_token = listed["NextToken"].as_str().map(str::to_string);
    (instances, next_token)
}

/// Register `ids` under `service_id`, each with an attribute that names it.
async fn register(router: &MultiServiceRouter, service_id: &str, ids: &[String]) {
    for (index, id) in ids.iter().enumerate() {
        ok(
            router,
            "RegisterInstance",
            json!({
                "ServiceId": service_id,
                "InstanceId": id,
                "Attributes": { "AWS_INSTANCE_IPV4": format!("10.0.0.{}", index + 1) },
            }),
        )
        .await;
    }
}

/// Walk every page for `base_body`, following the `NextToken`, and return the ids in
/// the order they were reported together with the number of pages it took.
async fn walk(
    router: &MultiServiceRouter,
    base_body: &Value,
    case: &str,
    guard: usize,
) -> (Vec<String>, usize) {
    let mut ids: Vec<String> = Vec::new();
    let mut pages = 0_usize;
    let mut token: Option<String> = None;
    loop {
        let mut body = base_body.clone();
        if let Some(token) = token.as_deref() {
            body["NextToken"] = json!(token);
        }
        let (instances, next) = page(router, body).await;
        pages += 1;
        ids.extend(instances.into_iter().map(|(id, _)| id));
        match next {
            Some(next) => token = Some(next),
            None => break,
        }
        assert!(
            pages <= guard,
            "case {case:?}: pagination must terminate, it took more than {guard} pages \
             and has reported {ids:?}"
        );
    }
    (ids, pages)
}

/// Every page size, including none at all, reports each stored instance exactly once,
/// in one stable order, and hands back a resumable token only while more is stored.
#[tokio::test]
async fn every_page_size_walks_the_stored_instances_exactly_once() {
    let router = emulator();
    let (_, namespace_id) = namespace(&router, "list-paging-ns").await;
    let service_id = service(&router, &namespace_id, "list-paging-svc").await;
    let stored: Vec<String> = (0..7).map(|index| format!("i-{index:02}")).collect();
    register(&router, &service_id, &stored).await;

    // (case, the MaxResults to send, the page size it caps a page at)
    let cases: Vec<(&str, Option<i64>, usize)> = vec![
        ("the declared low bound", Some(1), 1),
        ("two at a time", Some(2), 2),
        ("a size that does not divide the count", Some(3), 3),
        ("one below the stored count", Some(6), 6),
        ("exactly the stored count", Some(7), 7),
        ("above the stored count", Some(50), 50),
        ("the declared high bound", Some(100), 100),
        ("absent, so the documented default", None, DEFAULT_MAX_RESULTS),
    ];
    for (case, max_results, size) in cases {
        let mut body = json!({ "ServiceId": service_id });
        if let Some(max_results) = max_results {
            body["MaxResults"] = json!(max_results);
        }

        // The first page never reports more than the cap, and carries a token exactly
        // when the service holds more than one page.
        let (first, token) = page(&router, body.clone()).await;
        assert_eq!(
            first.len(),
            size.min(stored.len()),
            "case {case:?}: a page holds min(MaxResults, stored), got {first:?}"
        );
        assert_eq!(
            token.is_some(),
            size < stored.len(),
            "case {case:?}: a NextToken comes back only when a further instance is \
             stored, got {token:?}"
        );

        let expected_pages = stored.len().div_ceil(size);
        let (walked, pages) = walk(&router, &body, case, stored.len() + 1).await;
        assert_eq!(
            walked, stored,
            "case {case:?}: paginating must report every stored instance once, in \
             InstanceId order, got {walked:?}"
        );
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: {} instances at {size} per page is {expected_pages} pages, \
             got {pages}",
            stored.len()
        );
        assert_eq!(
            walked.iter().collect::<BTreeSet<_>>().len(),
            stored.len(),
            "case {case:?}: no instance may be reported twice, got {walked:?}"
        );
    }
}

/// A request that names no `MaxResults` is capped at the documented default of 100,
/// which only a service holding more than that can show.
#[tokio::test]
async fn an_unspecified_max_results_caps_the_page_at_the_documented_default() {
    let router = emulator();
    let (_, namespace_id) = namespace(&router, "list-default-ns").await;
    let service_id = service(&router, &namespace_id, "list-default-svc").await;
    // Five past the default, so the second page is short and unmistakable.
    let stored: Vec<String> = (0..DEFAULT_MAX_RESULTS + 5)
        .map(|index| format!("i-{index:03}"))
        .collect();
    register(&router, &service_id, &stored).await;

    // (case, the MaxResults to send, the page size it must cap the first page at)
    for (case, max_results, size) in [
        ("absent, so the documented default", None, DEFAULT_MAX_RESULTS),
        (
            "the declared high bound, which is that same default",
            Some(100_i64),
            DEFAULT_MAX_RESULTS,
        ),
        ("well below the default", Some(40), 40),
    ] {
        let mut body = json!({ "ServiceId": service_id });
        if let Some(max_results) = max_results {
            body["MaxResults"] = json!(max_results);
        }
        let (first, token) = page(&router, body.clone()).await;
        assert_eq!(
            first.len(),
            size,
            "case {case:?}: the first page must hold {size} of the {} stored, got {}",
            stored.len(),
            first.len()
        );
        assert!(
            token.is_some(),
            "case {case:?}: {} instances are stored, so a NextToken must come back",
            stored.len()
        );

        let (walked, pages) = walk(&router, &body, case, stored.len() + 1).await;
        assert_eq!(
            walked, stored,
            "case {case:?}: paginating must report every stored instance once, in \
             InstanceId order"
        );
        assert_eq!(
            pages,
            stored.len().div_ceil(size),
            "case {case:?}: {} instances at {size} per page, got {pages} pages",
            stored.len()
        );
    }
}

/// The summary reports exactly the attribute map that was registered, whatever shape
/// it has — the keys `InstanceSummary$Attributes` documents, and keys of a caller's
/// own.
#[tokio::test]
async fn each_summary_reports_the_attribute_map_that_was_registered() {
    let router = emulator();
    let (_, namespace_id) = namespace(&router, "list-attrs-ns").await;
    let service_id = service(&router, &namespace_id, "list-attrs-svc").await;

    // (case, InstanceId, the attributes registered). The documented keys come from
    // `smithy_query(query="doc", shape="InstanceSummary", member="Attributes")`.
    let cases: Vec<(&str, &str, Value)> = vec![
        ("no attributes at all", "i-a-empty", json!({})),
        (
            "an A record's IPv4 address",
            "i-b-ipv4",
            json!({ "AWS_INSTANCE_IPV4": "192.0.2.44" }),
        ),
        (
            "an AAAA record's IPv6 address",
            "i-c-ipv6",
            json!({ "AWS_INSTANCE_IPV6": "2001:0db8:85a3:0000:0000:abcd:0001:2345" }),
        ),
        (
            "a CNAME record's domain name",
            "i-d-cname",
            json!({ "AWS_INSTANCE_CNAME": "example.com" }),
        ),
        (
            "an SRV record's port beside its address",
            "i-e-srv",
            json!({ "AWS_INSTANCE_IPV4": "192.0.2.45", "AWS_INSTANCE_PORT": "8080" }),
        ),
        (
            "an alias to a load balancer",
            "i-f-alias",
            json!({ "AWS_ALIAS_DNS_NAME": "lb.us-west-2.elb.amazonaws.com" }),
        ),
        (
            "an EC2 instance id, HTTP namespaces only",
            "i-g-ec2",
            json!({ "AWS_EC2_INSTANCE_ID": "i-0abcdef1234567890",
                    "AWS_INSTANCE_IPV4": "192.0.2.46" }),
        ),
        (
            "keys of the caller's own, one with a multi-byte value",
            "i-h-custom",
            json!({ "stage": "prod", "région": "ouest", "weight": "10" }),
        ),
    ];

    for (_case, instance_id, attributes) in &cases {
        ok(
            &router,
            "RegisterInstance",
            json!({ "ServiceId": service_id, "InstanceId": instance_id,
                    "Attributes": attributes }),
        )
        .await;
    }

    let (listed, token) = page(&router, json!({ "ServiceId": service_id })).await;
    assert_eq!(
        token, None,
        "all {} instances fit one default page, so no token is handed back",
        cases.len()
    );
    let reported: BTreeMap<String, Value> = listed.into_iter().collect();
    for (case, instance_id, attributes) in &cases {
        let actual = reported
            .get(*instance_id)
            .unwrap_or_else(|| panic!("case {case:?}: {instance_id} must be listed"));
        assert_eq!(
            actual, attributes,
            "case {case:?}: the summary must report the exact attribute map stored for \
             {instance_id}, got {actual}"
        );
    }
    assert_eq!(
        reported.len(),
        cases.len(),
        "case \"every registration\": the list holds exactly the registered instances, \
         got {reported:?}"
    );
}

/// The list follows what register and deregister persisted — the recorded lifecycle —
/// and the service can be named by its ARN as well as by its id.
#[tokio::test]
async fn the_list_follows_what_registration_persisted() {
    let router = emulator();
    let (_, namespace_id) = namespace(&router, "list-lifecycle-ns").await;
    let created = ok(
        &router,
        "CreateService",
        json!({ "Name": "list-lifecycle-svc", "NamespaceId": namespace_id }),
    )
    .await;
    let service_id = created["Service"]["Id"]
        .as_str()
        .expect("a service id")
        .to_string();
    let service_arn = created["Service"]["Arn"]
        .as_str()
        .expect("a service ARN")
        .to_string();

    // (case, what to do before listing, the ids the list must then report)
    let stored: Vec<String> = vec!["i-1".to_string(), "i-2".to_string(), "i-3".to_string()];
    let steps: Vec<(&str, Option<&str>, Vec<&str>)> = vec![
        ("a service with nothing registered", None, vec![]),
        ("after the three registrations", Some(""), vec!["i-1", "i-2", "i-3"]),
        ("after one is deregistered", Some("i-2"), vec!["i-1", "i-3"]),
        ("after another is deregistered", Some("i-1"), vec!["i-3"]),
        ("after the last one is deregistered", Some("i-3"), vec![]),
    ];
    for (case, action, expected) in steps {
        match action {
            None => {}
            Some("") => register(&router, &service_id, &stored).await,
            Some(instance_id) => {
                ok(
                    &router,
                    "DeregisterInstance",
                    json!({ "ServiceId": service_id, "InstanceId": instance_id }),
                )
                .await;
            }
        }
        // By id and by ARN: both name the one stored service, so both report the same.
        for reference in [&service_id, &service_arn] {
            let (listed, token) = page(&router, json!({ "ServiceId": reference })).await;
            let ids: Vec<String> = listed.into_iter().map(|(id, _)| id).collect();
            assert_eq!(
                ids, expected,
                "case {case:?} by {reference}: expected {expected:?}, got {ids:?}"
            );
            assert_eq!(
                token, None,
                "case {case:?} by {reference}: one page holds them all, got {token:?}"
            );
        }
    }
}

/// Listing is a report: it writes nothing, so repeating it — and paginating through it
/// — leaves every later reader seeing exactly what it saw before.
#[tokio::test]
async fn listing_changes_nothing_a_later_caller_sees() {
    let router = emulator();
    let (_, namespace_id) = namespace(&router, "list-readonly-ns").await;
    let service_id = service(&router, &namespace_id, "list-readonly-svc").await;
    let stored: Vec<String> = (0..4).map(|index| format!("i-{index:02}")).collect();
    register(&router, &service_id, &stored).await;

    let before = ok(&router, "GetService", json!({ "Id": service_id })).await;
    let revision_before = ok(
        &router,
        "DiscoverInstances",
        json!({ "NamespaceName": "list-readonly-ns", "ServiceName": "list-readonly-svc" }),
    )
    .await["InstancesRevision"]
        .clone();

    let first = ok(&router, "ListInstances", json!({ "ServiceId": service_id })).await;
    for round in 1..=3 {
        let again = ok(&router, "ListInstances", json!({ "ServiceId": service_id })).await;
        assert_eq!(
            again, first,
            "case \"round {round}\": repeating a read must answer identically, got {again}"
        );
    }
    // Paginating with a page size of one touches every record, still without writing.
    let (walked, _) = walk(
        &router,
        &json!({ "ServiceId": service_id, "MaxResults": 1 }),
        "one at a time",
        stored.len() + 1,
    )
    .await;
    assert_eq!(walked, stored, "the walk must report every instance");

    let after = ok(&router, "GetService", json!({ "Id": service_id })).await;
    assert_eq!(
        after["Service"], before["Service"],
        "case \"the service record\": listing must not change it, got {after}"
    );
    let revision_after = ok(
        &router,
        "DiscoverInstances",
        json!({ "NamespaceName": "list-readonly-ns", "ServiceName": "list-readonly-svc" }),
    )
    .await["InstancesRevision"]
        .clone();
    assert_eq!(
        revision_after, revision_before,
        "case \"InstancesRevision\": only a register or deregister moves it, got \
         {revision_after} after {revision_before}"
    );
}

/// What the operation refuses: a service reference that resolves to nothing, a
/// `NextToken` this service never minted, and a `MaxResults` outside its declared
/// range.
#[tokio::test]
async fn bad_requests_answer_their_modelled_errors() {
    let router = emulator();
    let (_, namespace_id) = namespace(&router, "list-errors-ns").await;
    let service_id = service(&router, &namespace_id, "list-errors-svc").await;
    register(&router, &service_id, &["i-1".to_string(), "i-2".to_string()]).await;

    // A service reference nothing resolves to: 400 ServiceNotFound.
    for (case, reference) in [
        ("an id no service carries", "srv-00000000000000000"),
        (
            "an ARN no service carries",
            "arn:aws:servicediscovery:us-west-2:000000000000:service/srv-missing",
        ),
        ("a namespace id in the service slot", namespace_id.as_str()),
    ] {
        let (status, code) = failure(&router, "ListInstances", json!({ "ServiceId": reference })).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ServiceNotFound"),
            "case {case:?} ({reference}): expected 400 ServiceNotFound"
        );
    }

    // A `NextToken` this service never minted FOR THIS listing is a bad request, NOT a
    // silent restart from the first page and not a resume of someone else's walk.
    let other_service = service(&router, &namespace_id, "list-errors-other-svc").await;
    register(&router, &other_service, &["i-1".to_string(), "i-2".to_string()]).await;
    let foreign = ok(
        &router,
        "ListInstances",
        json!({ "ServiceId": other_service, "MaxResults": 1 }),
    )
    .await["NextToken"]
        .as_str()
        .expect("a one-at-a-time page over two instances hands back a token")
        .to_string();
    for (case, token) in [
        ("a length no encoded token has", "a"),
        ("characters outside the alphabet", "!!!!"),
        ("a sentence", "page two please"),
        ("padding no encoded token carries", "aS0x===="),
        ("an empty token", ""),
        // 692d31 is `i-1` hex-encoded: a bare cursor carrying no scope at all.
        ("a bare cursor with no scope", "692d31"),
        ("a token minted for another service's listing", foreign.as_str()),
    ] {
        let (status, code) = failure(
            &router,
            "ListInstances",
            json!({ "ServiceId": service_id, "NextToken": token }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidInput"),
            "case {case:?} (NextToken={token:?}): expected 400 InvalidInput"
        );
    }
    // The same token IS accepted for the listing it was minted for.
    let (resumed, _) = page(
        &router,
        json!({ "ServiceId": other_service, "NextToken": foreign }),
    )
    .await;
    assert_eq!(
        resumed.into_iter().map(|(id, _)| id).collect::<Vec<_>>(),
        vec!["i-2".to_string()],
        "case \"the listing it was minted for\": the token must resume after i-1"
    );

    // `MaxResults` is declared `range 1..100`: the framework refuses a value outside it
    // before the handler runs, and the handler refuses one too, so the class of the
    // answer is pinned without pinning which layer produced it.
    for (case, max_results) in [
        ("one below the low bound", 0_i64),
        ("one above the high bound", 101),
        ("far above the high bound", 100_000),
        ("a negative count", -1),
    ] {
        let (status, code) = failure(
            &router,
            "ListInstances",
            json!({ "ServiceId": service_id, "MaxResults": max_results }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?} (MaxResults={max_results}): a value outside the declared \
             range is refused, got {code:?}"
        );
        assert!(
            matches!(code.as_str(), "InvalidInput" | "ValidationException"),
            "case {case:?} (MaxResults={max_results}): expected the modelled \
             bad-request refusal, got {code:?}"
        );
    }
}
