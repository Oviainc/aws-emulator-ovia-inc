// SPDX-License-Identifier: CC0-1.0

//! Updating a Directory Service conditional forwarder, driven end to end over HTTP.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: DirectoryService_20150416.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router. The harness (`service`, `call`, `ok`,
//! `refused`, `create_directory`) is the one `directory_lifecycle.rs` defines, so nothing
//! here reaches into the state struct and a response that is not computed from persisted
//! state cannot pass.
//!
//! The cases assert the RULE over sets of inputs rather than one recorded call: an update
//! replaces the remote DNS server addresses of exactly the one forwarder its
//! (directory, remote domain) pair resolves to, with the addresses the request names in
//! the order it names them — over a table of rows differing in which address families the
//! stored forwarder and the update carry, in how many addresses each list holds and in its
//! order, and in the form, length and spelling of the remote domain name — and leaves
//! every other forwarder of that directory, and every other directory's, as it was; an
//! update that resolves to no stored forwarder, or to a directory that is absent or not
//! yet in service, is refused and changes nothing; and an update naming no remote DNS
//! server address at all is refused the way a create naming none is.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::conditional_forwarders::{domain_of_length, REMOTE_DOMAIN_NAME_MAX_LEN};
use crate::directory_lifecycle::{
    call, create_directory, minimal_create, ok, refused, service, PAST_ONE_SPAN,
};

/// The `ReplicationScope` every stored forwarder reports: the only value the modelled
/// `ReplicationScope` enum declares
/// (`smithy_query(query="shape",shape="ReplicationScope")` lists `Domain` alone). An
/// update moves addresses, so the scope it reports afterwards is still this one.
const REPLICATION_SCOPE_DOMAIN: &str = "Domain";

/// One row of the update table: the forwarder to create, the spelling the update names it
/// with, and the address lists the update carries. `None` means the member is absent from
/// that request, which is not the same as an empty list.
struct UpdateCase {
    case: &'static str,
    remote_domain_name: String,
    created_ip_addrs: Option<Vec<&'static str>>,
    created_ipv6_addrs: Option<Vec<&'static str>>,
    /// How the update spells the domain: the same name, or another spelling of it that
    /// names the same domain in DNS.
    updated_as: String,
    updated_ip_addrs: Option<Vec<&'static str>>,
    updated_ipv6_addrs: Option<Vec<&'static str>>,
}

/// A `CreateConditionalForwarder` / `UpdateConditionalForwarder` body: the pair of
/// operations takes the same input shape, so one builder serves both.
fn forwarder_request(
    directory_id: &str,
    remote_domain_name: &str,
    dns_ip_addrs: &Option<Vec<&'static str>>,
    dns_ipv6_addrs: &Option<Vec<&'static str>>,
) -> Value {
    let mut body = json!({
        "DirectoryId": directory_id,
        "RemoteDomainName": remote_domain_name,
    });
    if let Some(addrs) = dns_ip_addrs {
        body["DnsIpAddrs"] = json!(addrs);
    }
    if let Some(addrs) = dns_ipv6_addrs {
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

/// The forwarder a directory reports for `remote_domain_name`, read back through the
/// filtered `DescribeConditionalForwarders`, or `None` when it reports none.
async fn stored_forwarder(
    router: &MultiServiceRouter,
    directory_id: &str,
    remote_domain_name: &str,
    case: &str,
) -> Option<Value> {
    let answer = ok(
        router,
        "DescribeConditionalForwarders",
        json!({
            "DirectoryId": directory_id,
            "RemoteDomainNames": [remote_domain_name],
        }),
        case,
    )
    .await;
    let reported = answer["ConditionalForwarders"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected ConditionalForwarders, got {answer}"))
        .clone();
    assert!(
        reported.len() <= 1,
        "case {case:?}: expected at most one forwarder for one remote domain, got {reported:?}"
    );
    reported.into_iter().next()
}

/// Every forwarder a directory holds, as the remote domain names it reports.
async fn stored_domains(
    router: &MultiServiceRouter,
    directory_id: &str,
    case: &str,
) -> BTreeSet<String> {
    let answer = ok(
        router,
        "DescribeConditionalForwarders",
        json!({ "DirectoryId": directory_id }),
        case,
    )
    .await;
    answer["ConditionalForwarders"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected ConditionalForwarders, got {answer}"))
        .iter()
        .map(|forwarder| {
            forwarder["RemoteDomainName"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// A directory the clock has already taken past the transition its create owes, so it is
/// in service for a forwarder write.
async fn active_directory(router: &MultiServiceRouter, name: &str, case: &str) -> String {
    let directory_id = create_directory(router, minimal_create(name), case).await;
    tokio::time::sleep(PAST_ONE_SPAN).await;
    directory_id
}

/// The error message a refusal reports, whichever casing the envelope uses for it.
fn message(parsed: &Value) -> String {
    parsed["message"]
        .as_str()
        .or_else(|| parsed["Message"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// The update table: rows differ along which address families the stored forwarder and the
/// update name, how many addresses each list holds and in what order, the form and length
/// of the remote domain name, and the spelling the update names it with (the same name,
/// another case, or the fully qualified form with the trailing root dot — all forms the
/// modelled `RemoteDomainName` pattern `^([a-zA-Z0-9]+[\.-])+([a-zA-Z0-9])+[.]?$` admits
/// for one domain).
fn update_cases() -> Vec<UpdateCase> {
    vec![
        UpdateCase {
            case: "one IPv4 address replaced by another",
            remote_domain_name: "one.example.com".to_string(),
            created_ip_addrs: Some(vec!["10.0.0.1"]),
            created_ipv6_addrs: None,
            updated_as: "one.example.com".to_string(),
            updated_ip_addrs: Some(vec!["10.0.0.9"]),
            updated_ipv6_addrs: None,
        },
        UpdateCase {
            case: "several IPv4 addresses narrowed to one, updated in another case",
            remote_domain_name: "many.example.com".to_string(),
            created_ip_addrs: Some(vec!["10.30.40.11", "10.30.40.222", "10.30.40.3"]),
            created_ipv6_addrs: None,
            updated_as: "MANY.Example.COM".to_string(),
            updated_ip_addrs: Some(vec!["10.30.40.99"]),
            updated_ipv6_addrs: None,
        },
        UpdateCase {
            case: "one IPv4 address grown to a list whose order is not sorted",
            remote_domain_name: "grown.example.com".to_string(),
            created_ip_addrs: Some(vec!["192.0.2.1"]),
            created_ipv6_addrs: None,
            updated_as: "grown.example.com".to_string(),
            updated_ip_addrs: Some(vec!["203.0.113.9", "10.0.0.2", "192.0.2.44"]),
            updated_ipv6_addrs: None,
        },
        UpdateCase {
            case: "the same addresses in the reverse order",
            remote_domain_name: "reordered.example.com".to_string(),
            created_ip_addrs: Some(vec!["198.51.100.1", "198.51.100.2", "198.51.100.3"]),
            created_ipv6_addrs: None,
            updated_as: "reordered.example.com".to_string(),
            updated_ip_addrs: Some(vec!["198.51.100.3", "198.51.100.2", "198.51.100.1"]),
            updated_ipv6_addrs: None,
        },
        UpdateCase {
            case: "IPv6 only, replaced by IPv6 only, updated fully qualified",
            remote_domain_name: "v6.example.org".to_string(),
            created_ip_addrs: None,
            created_ipv6_addrs: Some(vec!["2001:db8::1"]),
            updated_as: "v6.example.org.".to_string(),
            updated_ip_addrs: None,
            updated_ipv6_addrs: Some(vec!["2001:db8::2", "fe80::1"]),
        },
        UpdateCase {
            case: "IPv4 only, given both families by the update",
            remote_domain_name: "dual-stack.example.net".to_string(),
            created_ip_addrs: Some(vec!["198.51.100.7"]),
            created_ipv6_addrs: None,
            updated_as: "dual-stack.example.net".to_string(),
            updated_ip_addrs: Some(vec!["198.51.100.8", "198.51.100.9"]),
            updated_ipv6_addrs: Some(vec!["2001:db8::dead:beef"]),
        },
        UpdateCase {
            case: "both families, the update naming IPv6 alone",
            remote_domain_name: "v6only.example.net".to_string(),
            created_ip_addrs: Some(vec!["172.16.0.2"]),
            created_ipv6_addrs: Some(vec!["2001:db8::5"]),
            updated_as: "v6only.example.net".to_string(),
            updated_ip_addrs: None,
            updated_ipv6_addrs: Some(vec!["2001:db8::6"]),
        },
        UpdateCase {
            case: "a domain created fully qualified, updated without the root dot",
            remote_domain_name: "rooted.example.com.".to_string(),
            created_ip_addrs: Some(vec!["172.16.0.3"]),
            created_ipv6_addrs: None,
            updated_as: "rooted.example.com".to_string(),
            updated_ip_addrs: Some(vec!["172.16.0.4", "172.16.0.5"]),
            updated_ipv6_addrs: None,
        },
        UpdateCase {
            case: "the longest domain the model admits",
            remote_domain_name: domain_of_length(REMOTE_DOMAIN_NAME_MAX_LEN),
            created_ip_addrs: Some(vec!["172.16.1.1"]),
            created_ipv6_addrs: Some(vec!["2001:db8:0:0:0:0:0:2"]),
            updated_as: domain_of_length(REMOTE_DOMAIN_NAME_MAX_LEN),
            updated_ip_addrs: Some(vec!["172.16.1.2"]),
            updated_ipv6_addrs: None,
        },
    ]
}

#[tokio::test]
async fn an_update_replaces_the_addresses_of_exactly_the_forwarder_it_resolves_to() {
    let router = service();
    let directory_id = active_directory(&router, "corp.example.com", "update").await;
    // A second directory forwarding the SAME remote domains: the index is per directory,
    // so updating one directory's forwarder must leave the other's addresses alone.
    let other = active_directory(&router, "other.example.com", "update").await;

    let cases = update_cases();
    for row in &cases {
        for id in [&directory_id, &other] {
            ok(
                &router,
                "CreateConditionalForwarder",
                forwarder_request(
                    id,
                    &row.remote_domain_name,
                    &row.created_ip_addrs,
                    &row.created_ipv6_addrs,
                ),
                row.case,
            )
            .await;
        }
    }

    for row in &cases {
        let case = row.case;
        let answer = ok(
            &router,
            "UpdateConditionalForwarder",
            forwarder_request(
                &directory_id,
                &row.updated_as,
                &row.updated_ip_addrs,
                &row.updated_ipv6_addrs,
            ),
            case,
        )
        .await;
        // The operation models no output members, so an accepted update answers an empty
        // result and the describe is what reports the change.
        assert_eq!(
            answer,
            json!({}),
            "case {case:?}: expected an empty UpdateConditionalForwarderResult, got {answer}"
        );

        let reported = stored_forwarder(&router, &directory_id, &row.remote_domain_name, case)
            .await
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected the updated forwarder to still be reported")
            });
        assert_eq!(
            reported["DnsIpAddrs"],
            expected_list(&row.updated_ip_addrs),
            "case {case:?}: expected the update's DnsIpAddrs in the order it sent them, got \
             {reported}"
        );
        assert_eq!(
            reported["DnsIpv6Addrs"],
            expected_list(&row.updated_ipv6_addrs),
            "case {case:?}: expected the update's DnsIpv6Addrs in the order it sent them, got \
             {reported}"
        );
        // The update moves addresses only: the forwarder is still keyed by, and still
        // reports, the remote domain the create spelled, with the same replication scope.
        assert_eq!(
            reported["RemoteDomainName"],
            json!(&row.remote_domain_name),
            "case {case:?}: expected the stored remote domain name to be unchanged, got {reported}"
        );
        assert_eq!(
            reported["ReplicationScope"],
            json!(REPLICATION_SCOPE_DOMAIN),
            "case {case:?}: expected the replication scope to be unchanged, got {reported}"
        );

        // The other directory's forwarder for the same remote domain still holds what its
        // own create stored.
        let untouched = stored_forwarder(&router, &other, &row.remote_domain_name, case)
            .await
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected the other directory to keep its own forwarder")
            });
        assert_eq!(
            untouched["DnsIpAddrs"],
            expected_list(&row.created_ip_addrs),
            "case {case:?}: expected the other directory's addresses to be untouched, got \
             {untouched}"
        );
        assert_eq!(
            untouched["DnsIpv6Addrs"],
            expected_list(&row.created_ipv6_addrs),
            "case {case:?}: expected the other directory's addresses to be untouched, got \
             {untouched}"
        );
    }

    // No row's update created, dropped or renamed a forwarder.
    let domains = stored_domains(&router, &directory_id, "every row updated").await;
    assert_eq!(
        domains,
        cases
            .iter()
            .map(|row| row.remote_domain_name.clone())
            .collect::<BTreeSet<String>>(),
        "case \"every row updated\": expected the same forwarders as before the updates, got \
         {domains:?}"
    );

    // An update is repeatable: applying the same row twice reports the same addresses, and
    // a second update puts the original addresses back, so the change is a replacement
    // rather than an accumulation.
    for row in &cases {
        let case = row.case;
        ok(
            &router,
            "UpdateConditionalForwarder",
            forwarder_request(
                &directory_id,
                &row.updated_as,
                &row.updated_ip_addrs,
                &row.updated_ipv6_addrs,
            ),
            case,
        )
        .await;
        ok(
            &router,
            "UpdateConditionalForwarder",
            forwarder_request(
                &directory_id,
                &row.remote_domain_name,
                &row.created_ip_addrs,
                &row.created_ipv6_addrs,
            ),
            case,
        )
        .await;
        let restored = stored_forwarder(&router, &directory_id, &row.remote_domain_name, case)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: expected the forwarder to still be reported"));
        assert_eq!(
            restored["DnsIpAddrs"],
            expected_list(&row.created_ip_addrs),
            "case {case:?}: expected the second update to restore the created DnsIpAddrs, got \
             {restored}"
        );
        assert_eq!(
            restored["DnsIpv6Addrs"],
            expected_list(&row.created_ipv6_addrs),
            "case {case:?}: expected the second update to restore the created DnsIpv6Addrs, got \
             {restored}"
        );
    }
}

#[tokio::test]
async fn an_update_that_resolves_to_no_stored_forwarder_is_refused_and_changes_nothing() {
    let router = service();
    let directory_id = active_directory(&router, "corp.example.com", "unknown domain").await;
    let elsewhere = active_directory(&router, "elsewhere.example.com", "unknown domain").await;

    let kept = "kept.example.com";
    let kept_addrs = ["10.0.0.1", "10.0.0.2"];
    ok(
        &router,
        "CreateConditionalForwarder",
        json!({ "DirectoryId": &directory_id, "RemoteDomainName": kept, "DnsIpAddrs": kept_addrs }),
        "unknown domain: seeding",
    )
    .await;
    // A forwarder only the OTHER directory holds, so naming it here resolves to nothing.
    let only_elsewhere = "only-there.example.com";
    ok(
        &router,
        "CreateConditionalForwarder",
        json!({
            "DirectoryId": &elsewhere,
            "RemoteDomainName": only_elsewhere,
            "DnsIpAddrs": ["10.1.0.1"],
        }),
        "unknown domain: seeding the other directory",
    )
    .await;
    // A forwarder that was created and then deleted: the pair no longer exists, so an
    // update of it is refused as the missing entity rather than recreating it.
    let gone = "gone.example.com";
    ok(
        &router,
        "CreateConditionalForwarder",
        json!({ "DirectoryId": &directory_id, "RemoteDomainName": gone, "DnsIpAddrs": ["10.0.0.3"] }),
        "unknown domain: seeding the doomed forwarder",
    )
    .await;
    ok(
        &router,
        "DeleteConditionalForwarder",
        json!({ "DirectoryId": &directory_id, "RemoteDomainName": gone }),
        "unknown domain: the delete",
    )
    .await;

    for domain in [
        // never forwarded by any directory
        "nowhere.example.com",
        // forwarded only by another directory
        only_elsewhere,
        // forwarded, then deleted
        gone,
        // another spelling of a domain nothing forwards
        "NOWHERE.example.com.",
        // a one-label-deeper name under a domain that IS forwarded: a different domain
        "sub.kept.example.com",
        // the longest name the model admits, which nothing forwards
        &domain_of_length(REMOTE_DOMAIN_NAME_MAX_LEN),
    ] {
        let case = format!("an update naming {domain:?}");
        let body = json!({
            "DirectoryId": &directory_id,
            "RemoteDomainName": domain,
            "DnsIpAddrs": ["10.9.9.9"],
        });
        refused(
            &router,
            "UpdateConditionalForwarder",
            body.clone(),
            "EntityDoesNotExistException",
            &case,
        )
        .await;

        // The refusal is built from the request: it quotes the domain the caller asked
        // about, so the caller can fix the request from the message alone.
        let (_, parsed) = call(&router, "UpdateConditionalForwarder", body).await;
        let reported = message(&parsed);
        assert!(
            reported.contains(&format!("'{domain}'")),
            "case {case:?}: expected the message to name the requested remote domain, got {parsed}"
        );

        // The refused update stored nothing: no new forwarder, and the one the directory
        // does hold still reports the addresses its create stored.
        let domains = stored_domains(&router, &directory_id, &case).await;
        assert_eq!(
            domains,
            [kept.to_string()].into_iter().collect::<BTreeSet<String>>(),
            "case {case:?}: expected the refused update to store nothing, got {domains:?}"
        );
        let untouched = stored_forwarder(&router, &directory_id, kept, &case)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: expected the kept forwarder to be reported"));
        assert_eq!(
            untouched["DnsIpAddrs"],
            json!(kept_addrs),
            "case {case:?}: expected the kept forwarder's addresses to be untouched, got \
             {untouched}"
        );
    }

    // ...and the other directory's forwarder still holds its own addresses.
    let untouched = stored_forwarder(
        &router,
        &elsewhere,
        only_elsewhere,
        "unknown domain: the other directory",
    )
    .await
    .unwrap_or_else(|| panic!("expected the other directory to keep its forwarder"));
    assert_eq!(
        untouched["DnsIpAddrs"],
        json!(["10.1.0.1"]),
        "case \"unknown domain: the other directory\": expected its addresses to be untouched, got \
         {untouched}"
    );
}

#[tokio::test]
async fn an_update_against_a_directory_that_is_absent_or_not_in_service_is_refused() {
    let router = service();
    let domain = "remote.example.com";

    // A directory this account never created, in several forms the modelled `DirectoryId`
    // pattern `^d-[0-9a-f]{10}$` admits...
    let mut absent: Vec<String> = vec![
        "d-0000000000".to_string(),
        "d-00000000ff".to_string(),
        "d-abcdef0123".to_string(),
        "d-ffffffffff".to_string(),
    ];
    // ...and one that existed, forwarded a domain and was deleted, so the refusal follows
    // the store rather than the shape of the id.
    let doomed = active_directory(&router, "doomed.example.com", "absent").await;
    ok(
        &router,
        "CreateConditionalForwarder",
        json!({ "DirectoryId": &doomed, "RemoteDomainName": domain, "DnsIpAddrs": ["10.0.0.1"] }),
        "absent: seeding the doomed directory",
    )
    .await;
    ok(
        &router,
        "DeleteDirectory",
        json!({ "DirectoryId": &doomed }),
        "absent: delete the directory",
    )
    .await;
    absent.push(doomed);

    for directory_id in &absent {
        let case = format!("an update against {directory_id}");
        let body = json!({
            "DirectoryId": directory_id,
            "RemoteDomainName": domain,
            "DnsIpAddrs": ["10.0.0.2"],
        });
        refused(
            &router,
            "UpdateConditionalForwarder",
            body.clone(),
            "EntityDoesNotExistException",
            &case,
        )
        .await;
        let (_, parsed) = call(&router, "UpdateConditionalForwarder", body).await;
        let reported = message(&parsed);
        assert!(
            reported.contains(&format!("'{directory_id}'")),
            "case {case:?}: expected the message to name the requested directory, got {parsed}"
        );
    }

    // A directory still being built by the transition its create owes is not in a stage
    // that serves its DNS configuration, so a forwarder write against it is refused as a
    // parameter the call cannot be served with (see `tests/all/refusal_texts.rs`,
    // which asserts the class and the sentence over the whole family)...
    let building =
        create_directory(&router, minimal_create("building.example.com"), "building").await;
    refused(
        &router,
        "UpdateConditionalForwarder",
        json!({
            "DirectoryId": &building,
            "RemoteDomainName": domain,
            "DnsIpAddrs": ["10.0.0.3"],
        }),
        "InvalidParameterException",
        "a directory still being built",
    )
    .await;

    // ...and once the clock has made that transition due, the directory is in service: the
    // same update now resolves against its (empty) forwarders and is refused for the
    // domain rather than for the directory's stage.
    tokio::time::sleep(PAST_ONE_SPAN).await;
    refused(
        &router,
        "UpdateConditionalForwarder",
        json!({
            "DirectoryId": &building,
            "RemoteDomainName": domain,
            "DnsIpAddrs": ["10.0.0.3"],
        }),
        "EntityDoesNotExistException",
        "a directory in service, forwarding nothing",
    )
    .await;
}

#[tokio::test]
async fn an_update_naming_no_remote_dns_server_address_is_refused_and_changes_nothing() {
    let router = service();
    let directory_id = active_directory(&router, "corp.example.com", "no address").await;

    let domain = "remote.example.com";
    let created = ["10.0.0.1", "10.0.0.2"];
    ok(
        &router,
        "CreateConditionalForwarder",
        json!({ "DirectoryId": &directory_id, "RemoteDomainName": domain, "DnsIpAddrs": created }),
        "no address: seeding",
    )
    .await;

    // A forwarder has to name at least one remote DNS server, so an update that names
    // none — neither member, either member empty, or both empty — is refused the way a
    // create naming none is, and the stored addresses are left alone.
    for (case, body) in [
        (
            "neither address member",
            json!({ "DirectoryId": &directory_id, "RemoteDomainName": domain }),
        ),
        (
            "an empty DnsIpAddrs",
            json!({
                "DirectoryId": &directory_id,
                "RemoteDomainName": domain,
                "DnsIpAddrs": [],
            }),
        ),
        (
            "an empty DnsIpv6Addrs",
            json!({
                "DirectoryId": &directory_id,
                "RemoteDomainName": domain,
                "DnsIpv6Addrs": [],
            }),
        ),
        (
            "both address lists empty",
            json!({
                "DirectoryId": &directory_id,
                "RemoteDomainName": domain,
                "DnsIpAddrs": [],
                "DnsIpv6Addrs": [],
            }),
        ),
    ] {
        refused(
            &router,
            "UpdateConditionalForwarder",
            body,
            "InvalidParameterException",
            case,
        )
        .await;

        let reported = stored_forwarder(&router, &directory_id, domain, case)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: expected the forwarder to still be reported"));
        assert_eq!(
            reported["DnsIpAddrs"],
            json!(created),
            "case {case:?}: expected the refused update to change nothing, got {reported}"
        );
    }

    // The same request is refused that way whether or not the pair it names is stored, so
    // the address rule is decided on the input rather than after the lookup.
    refused(
        &router,
        "UpdateConditionalForwarder",
        json!({ "DirectoryId": &directory_id, "RemoteDomainName": "nowhere.example.com" }),
        "InvalidParameterException",
        "no address, and no such forwarder",
    )
    .await;

    // One past the published `RemoteDomainName` length bound (`length 0..1024`) the
    // modelled constraint refuses the request before the handler runs, so nothing changes.
    let past_bound = domain_of_length(REMOTE_DOMAIN_NAME_MAX_LEN + 1);
    let (status, parsed) = call(
        &router,
        "UpdateConditionalForwarder",
        json!({
            "DirectoryId": &directory_id,
            "RemoteDomainName": &past_bound,
            "DnsIpAddrs": ["10.0.0.9"],
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"one past the RemoteDomainName length bound\": expected 400, got {status} with \
         {parsed}"
    );
    let reported = stored_forwarder(
        &router,
        &directory_id,
        domain,
        "one past the RemoteDomainName length bound",
    )
    .await
    .unwrap_or_else(|| panic!("expected the forwarder to still be reported"));
    assert_eq!(
        reported["DnsIpAddrs"],
        json!(created),
        "case \"one past the RemoteDomainName length bound\": expected the refused update to \
         change nothing, got {reported}"
    );
}
