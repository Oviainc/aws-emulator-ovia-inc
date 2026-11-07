// SPDX-License-Identifier: CC0-1.0

//! The Directory Service conditional forwarders, driven end to end over HTTP.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: DirectoryService_20150416.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router. The harness (`service`, `call`, `ok`,
//! `refused`, `create_directory`) is the one `directory_lifecycle.rs` already defines, so
//! nothing here reaches into the state struct and a response that is not computed from
//! persisted state cannot pass.
//!
//! The cases assert RULES over sets of inputs rather than one recorded call:
//! `CreateConditionalForwarder` then `DescribeConditionalForwarders` over a table of
//! forwarders whose rows differ in which address families they name, in how many
//! addresses each list carries and in its order, and in the form and length of the remote
//! domain name (including the published `RemoteDomainName` length bound and one past it);
//! the duplicate rule over a table of spellings of one remote domain; the narrowing the
//! `RemoteDomainNames` filter performs over a table of filters; and the refusals over
//! tables of absent directory ids, of directories not yet in service and of requests
//! naming no DNS server.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::directory_lifecycle::{
    call, create_directory, minimal_create, ok, refused, service, PAST_ONE_SPAN,
};

/// The `ReplicationScope` every stored forwarder reports: the only value the modelled
/// `ReplicationScope` enum declares
/// (`smithy_query(query="shape",shape="ReplicationScope")` lists `Domain` alone).
const REPLICATION_SCOPE_DOMAIN: &str = "Domain";

/// The published upper length bound of the modelled `RemoteDomainName` shape,
/// `length 0..1024`.
pub(crate) const REMOTE_DOMAIN_NAME_MAX_LEN: usize = 1024;

/// This emulator's documented per-directory forwarder guard
/// (`state::MAX_CONDITIONAL_FORWARDERS_PER_DIRECTORY`).
const FORWARDER_GUARD: usize = 50;

/// This emulator's documented `RemoteDomainNames` filter guard
/// (`state::MAX_REMOTE_DOMAIN_NAMES_PER_REQUEST`).
const REMOTE_DOMAIN_NAMES_GUARD: usize = 100;

/// A domain name of exactly `len` characters that the modelled `RemoteDomainName` pattern
/// `^([a-zA-Z0-9]+[\.-])+([a-zA-Z0-9])+[.]?$` admits: dot-separated two-letter labels,
/// closed by a label of one to three letters so the total lands on `len`.
pub(crate) fn domain_of_length(len: usize) -> String {
    let mut name = String::new();
    while name.len() + 3 < len {
        name.push_str("ab.");
    }
    while name.len() < len {
        name.push('c');
    }
    name
}

/// One row of the forwarder table: the remote domain a forwarder points at and the
/// address lists the request carries for it. `None` means the member is absent from the
/// request, which is not the same as an empty list.
struct ForwarderCase {
    case: &'static str,
    remote_domain_name: String,
    dns_ip_addrs: Option<Vec<&'static str>>,
    dns_ipv6_addrs: Option<Vec<&'static str>>,
}

/// One row of the filter table: a `DescribeConditionalForwarders` request and the remote
/// domains it must narrow the directory's forwarders to.
struct FilterCase {
    case: &'static str,
    request: Value,
    expected: BTreeSet<String>,
}

/// The `CreateConditionalForwarder` body one row of the forwarder table sends.
fn create_request(directory_id: &str, row: &ForwarderCase) -> Value {
    let mut body = json!({
        "DirectoryId": directory_id,
        "RemoteDomainName": &row.remote_domain_name,
    });
    if let Some(addrs) = &row.dns_ip_addrs {
        body["DnsIpAddrs"] = json!(addrs);
    }
    if let Some(addrs) = &row.dns_ipv6_addrs {
        body["DnsIpv6Addrs"] = json!(addrs);
    }
    body
}

/// What a request member holding `addrs` must come back as: the list in the order it was
/// sent, or absent when the request named no address of that family.
fn expected_list(addrs: &Option<Vec<&'static str>>) -> Value {
    match addrs {
        Some(addrs) if !addrs.is_empty() => json!(addrs),
        _ => Value::Null,
    }
}

/// `DescribeConditionalForwarders` with `body`, returning its `ConditionalForwarders`.
async fn describe(router: &MultiServiceRouter, body: Value, case: &str) -> Vec<Value> {
    let answer = ok(router, "DescribeConditionalForwarders", body, case).await;
    answer["ConditionalForwarders"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected ConditionalForwarders, got {answer}"))
        .clone()
}

/// Every forwarder a directory holds.
async fn describe_all(router: &MultiServiceRouter, directory_id: &str, case: &str) -> Vec<Value> {
    describe(router, json!({ "DirectoryId": directory_id }), case).await
}

/// The remote domain names an answer reports, as a set.
fn reported_domains(forwarders: &[Value]) -> BTreeSet<String> {
    forwarders
        .iter()
        .map(|forwarder| {
            forwarder["RemoteDomainName"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The one forwarder an answer reports for `domain`.
fn forwarder_for(forwarders: &[Value], domain: &str, case: &str) -> Value {
    let mut matching = forwarders
        .iter()
        .filter(|forwarder| forwarder["RemoteDomainName"] == json!(domain));
    let found = matching
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: expected a forwarder for {domain:?}, got {forwarders:?}"))
        .clone();
    assert!(
        matching.next().is_none(),
        "case {case:?}: expected exactly one forwarder for {domain:?}, got {forwarders:?}"
    );
    found
}

/// A directory the clock has already taken past the transition its create owes, so it is
/// in service for a forwarder write.
async fn active_directory(router: &MultiServiceRouter, name: &str, case: &str) -> String {
    let directory_id = create_directory(router, minimal_create(name), case).await;
    tokio::time::sleep(PAST_ONE_SPAN).await;
    directory_id
}

/// The forwarder table: rows differ along which address families the request names, how
/// many addresses each list carries and in what order, and the form and length of the
/// remote domain name.
fn forwarder_cases() -> Vec<ForwarderCase> {
    vec![
        ForwarderCase {
            case: "one IPv4 address",
            remote_domain_name: "one.example.com".to_string(),
            dns_ip_addrs: Some(vec!["10.0.0.1"]),
            dns_ipv6_addrs: None,
        },
        ForwarderCase {
            case: "several IPv4 addresses, not in sorted order",
            remote_domain_name: "many.example.com".to_string(),
            dns_ip_addrs: Some(vec!["203.0.113.9", "10.0.0.2", "192.0.2.44"]),
            dns_ipv6_addrs: None,
        },
        ForwarderCase {
            case: "IPv6 only",
            remote_domain_name: "v6.example.org".to_string(),
            dns_ip_addrs: None,
            dns_ipv6_addrs: Some(vec!["2001:db8::1"]),
        },
        ForwarderCase {
            case: "both families, several of each",
            remote_domain_name: "dual-stack.example.net".to_string(),
            dns_ip_addrs: Some(vec!["198.51.100.7", "198.51.100.8"]),
            dns_ipv6_addrs: Some(vec!["2001:db8::dead:beef", "fe80::1"]),
        },
        ForwarderCase {
            case: "a two-label domain with digits",
            remote_domain_name: "ad2.example9.com".to_string(),
            dns_ip_addrs: Some(vec!["172.16.0.1"]),
            dns_ipv6_addrs: None,
        },
        ForwarderCase {
            case: "a fully qualified domain, with the trailing root dot",
            remote_domain_name: "rooted.example.com.".to_string(),
            dns_ip_addrs: Some(vec!["172.16.0.2"]),
            dns_ipv6_addrs: None,
        },
        ForwarderCase {
            case: "the longest domain the model admits",
            remote_domain_name: domain_of_length(REMOTE_DOMAIN_NAME_MAX_LEN),
            dns_ip_addrs: Some(vec!["172.16.0.3"]),
            dns_ipv6_addrs: Some(vec!["2001:db8:0:0:0:0:0:2"]),
        },
    ]
}

#[tokio::test]
async fn created_conditional_forwarders_are_reported_from_persisted_state() {
    let router = service();
    let directory_id = active_directory(&router, "corp.example.com", "round trip").await;

    // A directory with no forwarder reports an empty list, not a 501 and not a forwarder
    // nobody created.
    let before = describe_all(&router, &directory_id, "before creating").await;
    assert!(
        before.is_empty(),
        "case \"before creating\": expected no forwarder, got {before:?}"
    );

    let cases = forwarder_cases();
    for row in &cases {
        ok(
            &router,
            "CreateConditionalForwarder",
            create_request(&directory_id, row),
            row.case,
        )
        .await;
    }

    let reported = describe_all(&router, &directory_id, "after creating").await;
    assert_eq!(
        reported.len(),
        cases.len(),
        "case \"after creating\": expected one forwarder per row, got {reported:?}"
    );

    for row in &cases {
        let case = row.case;
        let forwarder = forwarder_for(&reported, &row.remote_domain_name, case);
        assert_eq!(
            forwarder["DnsIpAddrs"],
            expected_list(&row.dns_ip_addrs),
            "case {case:?}: expected the IPv4 addresses the request sent, in order, got {forwarder}"
        );
        assert_eq!(
            forwarder["DnsIpv6Addrs"],
            expected_list(&row.dns_ipv6_addrs),
            "case {case:?}: expected the IPv6 addresses the request sent, in order, got {forwarder}"
        );
        assert_eq!(
            forwarder["ReplicationScope"],
            json!(REPLICATION_SCOPE_DOMAIN),
            "case {case:?}: expected the forwarder to be replicated to the domain, got {forwarder}"
        );

        // The filter resolves the same stored forwarder the unfiltered read reported.
        let narrowed = describe(
            &router,
            json!({
                "DirectoryId": &directory_id,
                "RemoteDomainNames": [&row.remote_domain_name],
            }),
            case,
        )
        .await;
        assert_eq!(
            narrowed,
            vec![forwarder.clone()],
            "case {case:?}: expected the filter to report exactly the stored forwarder, got \
             {narrowed:?}"
        );
    }

    // A read does not change what the next caller sees.
    let again = describe_all(&router, &directory_id, "repeated read").await;
    assert_eq!(
        reported, again,
        "case \"repeated read\": expected a repeated describe to report the same forwarders, got \
         {again:?}"
    );

    // One past the upper `RemoteDomainName` length bound is refused by the modelled
    // constraint, before any handler runs, and stores nothing.
    let too_long = domain_of_length(REMOTE_DOMAIN_NAME_MAX_LEN + 1);
    let (status, parsed) = call(
        &router,
        "CreateConditionalForwarder",
        json!({
            "DirectoryId": &directory_id,
            "RemoteDomainName": &too_long,
            "DnsIpAddrs": ["10.0.0.9"],
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"one past the RemoteDomainName length bound\": expected 400, got {status} with \
         {parsed}"
    );
    let after_refusal = describe_all(&router, &directory_id, "after the refused create").await;
    assert_eq!(
        after_refusal, reported,
        "case \"one past the RemoteDomainName length bound\": expected the refused create to store \
         nothing, got {after_refusal:?}"
    );
}

#[tokio::test]
async fn a_second_forwarder_for_the_same_remote_domain_is_refused_and_changes_nothing() {
    let router = service();
    let first = active_directory(&router, "corp.example.com", "duplicates").await;
    let second = active_directory(&router, "other.example.com", "duplicates").await;

    let domain = "remote.example.com";
    let addresses = ["10.0.0.1", "10.0.0.2"];
    ok(
        &router,
        "CreateConditionalForwarder",
        json!({ "DirectoryId": &first, "RemoteDomainName": domain, "DnsIpAddrs": addresses }),
        "duplicates: the first create",
    )
    .await;

    // A domain name is compared case-insensitively, and its fully qualified form names
    // the same domain, so every row below names the domain the directory already
    // forwards — whatever addresses it asks for.
    for spelling in [
        "remote.example.com",
        "REMOTE.EXAMPLE.COM",
        "Remote.Example.Com",
        "remote.example.com.",
        "REMOTE.example.COM.",
    ] {
        let case = format!("a second create spelled {spelling:?}");
        refused(
            &router,
            "CreateConditionalForwarder",
            json!({
                "DirectoryId": &first,
                "RemoteDomainName": spelling,
                "DnsIpAddrs": ["203.0.113.1"],
            }),
            "EntityAlreadyExistsException",
            &case,
        )
        .await;

        let reported = describe_all(&router, &first, &case).await;
        assert_eq!(
            reported.len(),
            1,
            "case {case:?}: expected the refused create to leave one forwarder, got {reported:?}"
        );
        assert_eq!(
            reported[0]["DnsIpAddrs"],
            json!(addresses),
            "case {case:?}: expected the stored addresses to be untouched, got {}",
            reported[0]
        );
        assert_eq!(
            reported[0]["RemoteDomainName"],
            json!(domain),
            "case {case:?}: expected the stored domain to keep the spelling it was created with, \
             got {}",
            reported[0]
        );
    }

    // Another remote domain on the same directory is a second forwarder...
    ok(
        &router,
        "CreateConditionalForwarder",
        json!({
            "DirectoryId": &first,
            "RemoteDomainName": "elsewhere.example.com",
            "DnsIpAddrs": ["10.0.0.3"],
        }),
        "duplicates: another domain",
    )
    .await;
    let reported = describe_all(&router, &first, "duplicates: another domain").await;
    assert_eq!(
        reported_domains(&reported),
        ["remote.example.com".to_string(), "elsewhere.example.com".to_string()]
            .into_iter()
            .collect::<BTreeSet<String>>(),
        "case \"duplicates: another domain\": expected both forwarders, got {reported:?}"
    );

    // ...and the same remote domain on ANOTHER directory is that directory's own
    // forwarder: the index is per directory.
    ok(
        &router,
        "CreateConditionalForwarder",
        json!({ "DirectoryId": &second, "RemoteDomainName": domain, "DnsIpAddrs": ["10.1.0.1"] }),
        "duplicates: the other directory",
    )
    .await;
    let other = describe_all(&router, &second, "duplicates: the other directory").await;
    assert_eq!(
        other.len(),
        1,
        "case \"duplicates: the other directory\": expected one forwarder, got {other:?}"
    );
    assert_eq!(
        other[0]["DnsIpAddrs"],
        json!(["10.1.0.1"]),
        "case \"duplicates: the other directory\": expected that directory's own addresses, got {}",
        other[0]
    );
}

#[tokio::test]
async fn conditional_forwarders_are_narrowed_by_the_remote_domain_filter() {
    let router = service();
    let directory_id = active_directory(&router, "corp.example.com", "filters").await;

    let domains = ["alpha.example.com", "beta.example.com", "gamma.example.org"];
    for (index, domain) in domains.iter().enumerate() {
        ok(
            &router,
            "CreateConditionalForwarder",
            json!({
                "DirectoryId": &directory_id,
                "RemoteDomainName": domain,
                "DnsIpAddrs": [format!("10.0.0.{}", index + 1)],
            }),
            "filters: seeding",
        )
        .await;
    }
    let everything: BTreeSet<String> = domains.iter().map(|d| d.to_string()).collect();

    // The rows differ along what the filter names: nothing at all, one domain, several,
    // a spelling that differs in case or in the trailing root dot, a domain nothing
    // forwards, and a mix of stored and unknown domains.
    let cases: Vec<FilterCase> = vec![
        FilterCase {
            case: "no filter: every forwarder of the directory",
            request: json!({ "DirectoryId": &directory_id }),
            expected: everything.clone(),
        },
        FilterCase {
            case: "one domain",
            request: json!({ "DirectoryId": &directory_id, "RemoteDomainNames": ["beta.example.com"] }),
            expected: ["beta.example.com".to_string()].into_iter().collect(),
        },
        FilterCase {
            case: "two domains",
            request: json!({
                "DirectoryId": &directory_id,
                "RemoteDomainNames": ["alpha.example.com", "gamma.example.org"],
            }),
            expected: ["alpha.example.com".to_string(), "gamma.example.org".to_string()]
                .into_iter()
                .collect(),
        },
        FilterCase {
            case: "a domain named in another case",
            request: json!({ "DirectoryId": &directory_id, "RemoteDomainNames": ["ALPHA.Example.com"] }),
            expected: ["alpha.example.com".to_string()].into_iter().collect(),
        },
        FilterCase {
            case: "a domain named in its fully qualified form",
            request: json!({ "DirectoryId": &directory_id, "RemoteDomainNames": ["beta.example.com."] }),
            expected: ["beta.example.com".to_string()].into_iter().collect(),
        },
        FilterCase {
            case: "a domain nothing forwards",
            request: json!({ "DirectoryId": &directory_id, "RemoteDomainNames": ["nowhere.example.com"] }),
            expected: BTreeSet::new(),
        },
        FilterCase {
            case: "stored and unknown domains together",
            request: json!({
                "DirectoryId": &directory_id,
                "RemoteDomainNames": ["gamma.example.org", "nowhere.example.com"],
            }),
            expected: ["gamma.example.org".to_string()].into_iter().collect(),
        },
        FilterCase {
            case: "a filter naming no domain reports no forwarder",
            request: json!({ "DirectoryId": &directory_id, "RemoteDomainNames": [] }),
            expected: BTreeSet::new(),
        },
    ];

    for FilterCase {
        case,
        request,
        expected,
    } in cases
    {
        let reported = describe(&router, request, case).await;
        assert_eq!(
            reported_domains(&reported),
            expected,
            "case {case:?}: expected the filter to narrow the forwarders, got {reported:?}"
        );
    }

    // A filter exactly at this emulator's documented guard is resolved...
    let at_guard: Vec<String> = (0..REMOTE_DOMAIN_NAMES_GUARD)
        .map(|index| format!("d{index}.example.com"))
        .collect();
    let reported = describe(
        &router,
        json!({ "DirectoryId": &directory_id, "RemoteDomainNames": at_guard }),
        "a filter at the RemoteDomainNames guard",
    )
    .await;
    assert!(
        reported.is_empty(),
        "case \"a filter at the RemoteDomainNames guard\": expected none of the generated domains \
         to match, got {reported:?}"
    );
    // ...and one entry past it is refused before anything is allocated from it.
    let past_guard: Vec<String> = (0..=REMOTE_DOMAIN_NAMES_GUARD)
        .map(|index| format!("d{index}.example.com"))
        .collect();
    refused(
        &router,
        "DescribeConditionalForwarders",
        json!({ "DirectoryId": &directory_id, "RemoteDomainNames": past_guard }),
        "InvalidParameterException",
        "one past the RemoteDomainNames guard",
    )
    .await;
}

#[tokio::test]
async fn a_forwarder_naming_no_dns_server_is_refused_and_stores_nothing() {
    let router = service();
    let directory_id = active_directory(&router, "corp.example.com", "no address").await;

    // A forwarder forwards to a remote DNS server, so every row below names none: the
    // members absent, either list empty, or both empty.
    let cases: Vec<(&str, Value)> = vec![
        (
            "neither member",
            json!({ "DirectoryId": &directory_id, "RemoteDomainName": "a.example.com" }),
        ),
        (
            "an empty DnsIpAddrs",
            json!({
                "DirectoryId": &directory_id,
                "RemoteDomainName": "b.example.com",
                "DnsIpAddrs": [],
            }),
        ),
        (
            "an empty DnsIpv6Addrs",
            json!({
                "DirectoryId": &directory_id,
                "RemoteDomainName": "c.example.com",
                "DnsIpv6Addrs": [],
            }),
        ),
        (
            "both lists empty",
            json!({
                "DirectoryId": &directory_id,
                "RemoteDomainName": "d.example.com",
                "DnsIpAddrs": [],
                "DnsIpv6Addrs": [],
            }),
        ),
    ];

    for (case, request) in cases {
        refused(
            &router,
            "CreateConditionalForwarder",
            request,
            "InvalidParameterException",
            case,
        )
        .await;
    }

    let reported = describe_all(&router, &directory_id, "no address").await;
    assert!(
        reported.is_empty(),
        "case \"no address\": expected the refused creates to store nothing, got {reported:?}"
    );
}

#[tokio::test]
async fn a_directory_that_is_absent_or_not_yet_in_service_is_refused() {
    let router = service();

    // A directory this account never created, in several forms the modelled `DirectoryId`
    // pattern `^d-[0-9a-f]{10}$` admits...
    let mut absent: Vec<String> = vec![
        "d-0000000000".to_string(),
        "d-00000000ff".to_string(),
        "d-abcdef0123".to_string(),
        "d-ffffffffff".to_string(),
    ];
    // ...and one that existed, forwarded a domain, and was deleted, so the refusal
    // follows the store rather than the shape of the id — and the forwarders went with
    // the directory.
    let doomed = active_directory(&router, "doomed.example.com", "absent").await;
    ok(
        &router,
        "CreateConditionalForwarder",
        json!({
            "DirectoryId": &doomed,
            "RemoteDomainName": "doomed-remote.example.com",
            "DnsIpAddrs": ["10.0.0.1"],
        }),
        "absent: seeding the doomed directory",
    )
    .await;
    ok(
        &router,
        "DeleteDirectory",
        json!({ "DirectoryId": &doomed }),
        "absent: delete",
    )
    .await;
    absent.push(doomed);

    for directory_id in &absent {
        for (operation, body) in [
            (
                "CreateConditionalForwarder",
                json!({
                    "DirectoryId": directory_id,
                    "RemoteDomainName": "remote.example.com",
                    "DnsIpAddrs": ["10.0.0.1"],
                }),
            ),
            (
                "DescribeConditionalForwarders",
                json!({ "DirectoryId": directory_id }),
            ),
        ] {
            let case = format!("{operation} against {directory_id}");
            refused(
                &router,
                operation,
                body.clone(),
                "EntityDoesNotExistException",
                &case,
            )
            .await;
            // The refusal is built from the request: it quotes the id the caller asked
            // for, so the caller can fix the request from the message alone.
            let (_, parsed) = call(&router, operation, body).await;
            let message = parsed["message"]
                .as_str()
                .or_else(|| parsed["Message"].as_str())
                .unwrap_or_default();
            assert!(
                message.contains(&format!("'{directory_id}'")),
                "case {case:?}: expected the message to name the requested directory, got {parsed}"
            );
        }
    }

    // A directory still being built by the transition its create owes is not in a stage
    // that serves its DNS configuration, so the write is refused as a parameter the call
    // cannot be served with (see `tests/all/refusal_texts.rs`) while the read
    // reports its (empty) forwarders.
    let fresh = create_directory(&router, minimal_create("fresh.example.com"), "not in service").await;
    let request = json!({
        "DirectoryId": &fresh,
        "RemoteDomainName": "remote.example.com",
        "DnsIpAddrs": ["10.0.0.1"],
    });
    refused(
        &router,
        "CreateConditionalForwarder",
        request.clone(),
        "InvalidParameterException",
        "a directory still being built",
    )
    .await;
    let while_building = describe_all(&router, &fresh, "a directory still being built").await;
    assert!(
        while_building.is_empty(),
        "case \"a directory still being built\": expected the refused create to store nothing, got \
         {while_building:?}"
    );

    // Once the clock has made that transition due, the same request is accepted.
    tokio::time::sleep(PAST_ONE_SPAN).await;
    ok(
        &router,
        "CreateConditionalForwarder",
        request,
        "a directory in service",
    )
    .await;
    let reported = describe_all(&router, &fresh, "a directory in service").await;
    assert_eq!(
        reported_domains(&reported),
        ["remote.example.com".to_string()]
            .into_iter()
            .collect::<BTreeSet<String>>(),
        "case \"a directory in service\": expected the forwarder the accepted create stored, got \
         {reported:?}"
    );
}

#[tokio::test]
async fn forwarders_are_capped_by_the_documented_per_directory_guard() {
    let router = service();
    let directory_id = active_directory(&router, "busy.example.com", "guard").await;

    for index in 0..FORWARDER_GUARD {
        ok(
            &router,
            "CreateConditionalForwarder",
            json!({
                "DirectoryId": &directory_id,
                "RemoteDomainName": format!("remote{index}.example.com"),
                "DnsIpAddrs": ["10.0.0.1"],
            }),
            "guard: filling",
        )
        .await;
    }

    // A fresh remote domain past the guard is refused, and the stored forwarders are
    // untouched.
    refused(
        &router,
        "CreateConditionalForwarder",
        json!({
            "DirectoryId": &directory_id,
            "RemoteDomainName": "one-too-many.example.com",
            "DnsIpAddrs": ["10.0.0.1"],
        }),
        "ClientException",
        "one past the per-directory guard",
    )
    .await;

    let reported = describe_all(&router, &directory_id, "guard").await;
    assert_eq!(
        reported.len(),
        FORWARDER_GUARD,
        "case \"guard\": expected exactly {FORWARDER_GUARD} forwarders, got {}",
        reported.len()
    );
    assert!(
        !reported_domains(&reported).contains("one-too-many.example.com"),
        "case \"guard\": expected the refused create to store nothing, got {reported:?}"
    );
}
