// SPDX-License-Identifier: CC0-1.0

//! Deleting a Directory Service conditional forwarder, driven end to end over HTTP.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: DirectoryService_20150416.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router. The harness (`service`, `call`, `ok`,
//! `refused`, `create_directory`) is the one `directory_lifecycle.rs` defines, so nothing
//! here reaches into the state struct and a response that is not computed from persisted
//! state cannot pass.
//!
//! The cases assert the RULE over sets of inputs rather than one recorded call: a delete
//! drops exactly the one forwarder whose remote domain the request resolves to — over a
//! table of rows differing in the address families the forwarder carries, in the form and
//! length of the remote domain name (including the published `RemoteDomainName` length
//! bound) and in the spelling the delete names it with — and leaves every other forwarder
//! of that directory, and every other directory's, in place; and a delete that resolves
//! to no stored forwarder, or to a directory that is absent or not yet in service, is
//! refused and removes nothing.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::conditional_forwarders::{domain_of_length, REMOTE_DOMAIN_NAME_MAX_LEN};
use crate::directory_lifecycle::{
    call, create_directory, minimal_create, ok, refused, service, PAST_ONE_SPAN,
};

/// One row of the deletion table: a forwarder to create, and the spelling of its remote
/// domain the delete names it with. `None` means the member is absent from the create
/// request, which is not the same as an empty list.
struct DeletionCase {
    case: &'static str,
    remote_domain_name: String,
    dns_ip_addrs: Option<Vec<&'static str>>,
    dns_ipv6_addrs: Option<Vec<&'static str>>,
    /// How the delete spells the domain: the same name, or another spelling of it that
    /// names the same domain in DNS.
    deleted_as: String,
}

/// The `CreateConditionalForwarder` body one row sends.
fn create_request(directory_id: &str, row: &DeletionCase) -> Value {
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

/// The deletion table: rows differ along the address families the forwarder carries, the
/// form and length of its remote domain name, and the spelling the delete names it with
/// (the same name, another case, or the fully qualified form with the trailing root dot —
/// all forms the modelled `RemoteDomainName` pattern `^([a-zA-Z0-9]+[\.-])+([a-zA-Z0-9])+[.]?$`
/// admits for one domain).
fn deletion_cases() -> Vec<DeletionCase> {
    vec![
        DeletionCase {
            case: "one IPv4 address, deleted by the name it was created with",
            remote_domain_name: "one.example.com".to_string(),
            dns_ip_addrs: Some(vec!["10.0.0.1"]),
            dns_ipv6_addrs: None,
            deleted_as: "one.example.com".to_string(),
        },
        DeletionCase {
            case: "several IPv4 addresses, deleted in another case",
            remote_domain_name: "many.example.com".to_string(),
            dns_ip_addrs: Some(vec!["203.0.113.9", "10.0.0.2", "192.0.2.44"]),
            dns_ipv6_addrs: None,
            deleted_as: "MANY.Example.COM".to_string(),
        },
        DeletionCase {
            case: "IPv6 only, deleted in its fully qualified form",
            remote_domain_name: "v6.example.org".to_string(),
            dns_ip_addrs: None,
            dns_ipv6_addrs: Some(vec!["2001:db8::1"]),
            deleted_as: "v6.example.org.".to_string(),
        },
        DeletionCase {
            case: "both address families, several of each",
            remote_domain_name: "dual-stack.example.net".to_string(),
            dns_ip_addrs: Some(vec!["198.51.100.7", "198.51.100.8"]),
            dns_ipv6_addrs: Some(vec!["2001:db8::dead:beef", "fe80::1"]),
            deleted_as: "dual-stack.example.net".to_string(),
        },
        DeletionCase {
            case: "a domain created fully qualified, deleted without the root dot",
            remote_domain_name: "rooted.example.com.".to_string(),
            dns_ip_addrs: Some(vec!["172.16.0.2"]),
            dns_ipv6_addrs: None,
            deleted_as: "rooted.example.com".to_string(),
        },
        DeletionCase {
            case: "a two-label domain with digits",
            remote_domain_name: "ad2.example9.com".to_string(),
            dns_ip_addrs: Some(vec!["172.16.0.1"]),
            dns_ipv6_addrs: None,
            deleted_as: "ad2.example9.com".to_string(),
        },
        DeletionCase {
            case: "the longest domain the model admits",
            remote_domain_name: domain_of_length(REMOTE_DOMAIN_NAME_MAX_LEN),
            dns_ip_addrs: Some(vec!["172.16.0.3"]),
            dns_ipv6_addrs: Some(vec!["2001:db8:0:0:0:0:0:2"]),
            deleted_as: domain_of_length(REMOTE_DOMAIN_NAME_MAX_LEN),
        },
    ]
}

#[tokio::test]
async fn a_delete_drops_exactly_the_forwarder_its_remote_domain_resolves_to() {
    let router = service();
    let directory_id = active_directory(&router, "corp.example.com", "deletion").await;
    // A second directory forwarding the SAME remote domains: the index is per directory,
    // so deleting one directory's forwarder must leave the other's alone.
    let other = active_directory(&router, "other.example.com", "deletion").await;

    let cases = deletion_cases();
    for row in &cases {
        for id in [&directory_id, &other] {
            ok(
                &router,
                "CreateConditionalForwarder",
                create_request(id, row),
                row.case,
            )
            .await;
        }
    }

    let mut expected: BTreeSet<String> = cases
        .iter()
        .map(|row| row.remote_domain_name.clone())
        .collect();
    let seeded = stored_domains(&router, &directory_id, "after seeding").await;
    assert_eq!(
        seeded, expected,
        "case \"after seeding\": expected one forwarder per row, got {seeded:?}"
    );

    for row in &cases {
        let case = row.case;
        ok(
            &router,
            "DeleteConditionalForwarder",
            json!({ "DirectoryId": &directory_id, "RemoteDomainName": &row.deleted_as }),
            case,
        )
        .await;

        // Exactly the deleted domain is gone from the directory's forwarders; every other
        // row is still reported from persisted state.
        expected.remove(&row.remote_domain_name);
        let reported = stored_domains(&router, &directory_id, case).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected only {:?} to be dropped, got {reported:?}",
            row.remote_domain_name
        );

        // The filter resolves nothing for it either: the record is gone, not hidden.
        let narrowed = ok(
            &router,
            "DescribeConditionalForwarders",
            json!({
                "DirectoryId": &directory_id,
                "RemoteDomainNames": [&row.remote_domain_name],
            }),
            case,
        )
        .await;
        let narrowed = narrowed["ConditionalForwarders"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(
            narrowed.is_empty(),
            "case {case:?}: expected the filter to report no forwarder for the deleted domain, \
             got {narrowed:?}"
        );

        // The other directory's forwarder for the same remote domain is untouched.
        let untouched = stored_domains(&router, &other, case).await;
        assert!(
            untouched.contains(&row.remote_domain_name),
            "case {case:?}: expected the other directory to keep its own forwarder for {:?}, got \
             {untouched:?}",
            row.remote_domain_name
        );
    }

    let drained = stored_domains(&router, &directory_id, "every row deleted").await;
    assert!(
        drained.is_empty(),
        "case \"every row deleted\": expected no forwarder left, got {drained:?}"
    );
    let others = stored_domains(&router, &other, "every row deleted").await;
    assert_eq!(
        others.len(),
        cases.len(),
        "case \"every row deleted\": expected the other directory to keep every forwarder, got \
         {others:?}"
    );

    // The delete frees the remote domain, so a create of it is accepted again rather than
    // colliding with a record the delete left behind.
    for row in &cases {
        let case = row.case;
        ok(
            &router,
            "CreateConditionalForwarder",
            create_request(&directory_id, row),
            case,
        )
        .await;
    }
    let recreated = stored_domains(&router, &directory_id, "recreated after deletion").await;
    assert_eq!(
        recreated,
        cases
            .iter()
            .map(|row| row.remote_domain_name.clone())
            .collect::<BTreeSet<String>>(),
        "case \"recreated after deletion\": expected every forwarder to be creatable again, got \
         {recreated:?}"
    );
}

#[tokio::test]
async fn a_delete_that_resolves_to_no_stored_forwarder_is_refused_and_removes_nothing() {
    let router = service();
    let directory_id = active_directory(&router, "corp.example.com", "unknown domain").await;
    let elsewhere = active_directory(&router, "elsewhere.example.com", "unknown domain").await;

    let kept = "kept.example.com";
    ok(
        &router,
        "CreateConditionalForwarder",
        json!({ "DirectoryId": &directory_id, "RemoteDomainName": kept, "DnsIpAddrs": ["10.0.0.1"] }),
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
    // A forwarder that was created and then deleted, so a repeat of the delete is refused
    // rather than answering as though it had dropped something.
    let gone = "gone.example.com";
    ok(
        &router,
        "CreateConditionalForwarder",
        json!({ "DirectoryId": &directory_id, "RemoteDomainName": gone, "DnsIpAddrs": ["10.0.0.2"] }),
        "unknown domain: seeding the doomed forwarder",
    )
    .await;
    ok(
        &router,
        "DeleteConditionalForwarder",
        json!({ "DirectoryId": &directory_id, "RemoteDomainName": gone }),
        "unknown domain: the first delete",
    )
    .await;

    let expected: BTreeSet<String> = [kept.to_string()].into_iter().collect();
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
        let case = format!("a delete naming {domain:?}");
        let body = json!({ "DirectoryId": &directory_id, "RemoteDomainName": domain });
        refused(
            &router,
            "DeleteConditionalForwarder",
            body.clone(),
            "EntityDoesNotExistException",
            &case,
        )
        .await;

        // The refusal is built from the request: it quotes the domain the caller asked
        // about, so the caller can fix the request from the message alone.
        let (_, parsed) = call(&router, "DeleteConditionalForwarder", body).await;
        let reported = message(&parsed);
        assert!(
            reported.contains(&format!("'{domain}'")),
            "case {case:?}: expected the message to name the requested remote domain, got {parsed}"
        );

        let reported = stored_domains(&router, &directory_id, &case).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the refused delete to remove nothing, got {reported:?}"
        );
    }

    // ...and the other directory still holds its own forwarder.
    let untouched = stored_domains(&router, &elsewhere, "unknown domain: the other directory").await;
    assert_eq!(
        untouched,
        [only_elsewhere.to_string()].into_iter().collect::<BTreeSet<String>>(),
        "case \"unknown domain: the other directory\": expected its forwarder to be untouched, got \
         {untouched:?}"
    );
}

#[tokio::test]
async fn a_delete_against_a_directory_that_is_absent_or_not_in_service_is_refused() {
    let router = service();

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
        "absent: delete the directory",
    )
    .await;
    absent.push(doomed);

    for directory_id in &absent {
        let case = format!("a delete against {directory_id}");
        let body = json!({
            "DirectoryId": directory_id,
            "RemoteDomainName": "doomed-remote.example.com",
        });
        refused(
            &router,
            "DeleteConditionalForwarder",
            body.clone(),
            "EntityDoesNotExistException",
            &case,
        )
        .await;
        let (_, parsed) = call(&router, "DeleteConditionalForwarder", body).await;
        let reported = message(&parsed);
        assert!(
            reported.contains(&format!("'{directory_id}'")),
            "case {case:?}: expected the message to name the requested directory, got {parsed}"
        );
    }

    // A directory still being built by the transition its create owes is not in a stage
    // that serves its DNS configuration, so a forwarder write against it is refused as a
    // parameter the call cannot be served with (see `tests/all/refusal_texts.rs`)
    // — and the forwarder it already holds stays stored.
    let fresh = active_directory(&router, "fresh.example.com", "not in service").await;
    let domain = "remote.example.com";
    ok(
        &router,
        "CreateConditionalForwarder",
        json!({ "DirectoryId": &fresh, "RemoteDomainName": domain, "DnsIpAddrs": ["10.0.0.1"] }),
        "not in service: seeding",
    )
    .await;

    let building = create_directory(&router, minimal_create("building.example.com"), "building").await;
    refused(
        &router,
        "DeleteConditionalForwarder",
        json!({ "DirectoryId": &building, "RemoteDomainName": domain }),
        "InvalidParameterException",
        "a directory still being built",
    )
    .await;

    // Once the clock has made that transition due, the directory is in service: the same
    // delete now resolves against its (empty) forwarders and is refused for the domain
    // rather than for the directory's stage.
    tokio::time::sleep(PAST_ONE_SPAN).await;
    refused(
        &router,
        "DeleteConditionalForwarder",
        json!({ "DirectoryId": &building, "RemoteDomainName": domain }),
        "EntityDoesNotExistException",
        "a directory in service, forwarding nothing",
    )
    .await;

    // The forwarder on the directory that IS in service is still deletable, and the
    // refusals above left it alone.
    let before = stored_domains(&router, &fresh, "not in service").await;
    assert_eq!(
        before,
        [domain.to_string()].into_iter().collect::<BTreeSet<String>>(),
        "case \"not in service\": expected the seeded forwarder to be untouched, got {before:?}"
    );
    ok(
        &router,
        "DeleteConditionalForwarder",
        json!({ "DirectoryId": &fresh, "RemoteDomainName": domain }),
        "not in service: the delete that is accepted",
    )
    .await;
    let after = stored_domains(&router, &fresh, "not in service: after the delete").await;
    assert!(
        after.is_empty(),
        "case \"not in service: after the delete\": expected no forwarder left, got {after:?}"
    );
}

#[tokio::test]
async fn a_delete_one_past_the_remote_domain_name_length_bound_is_refused_and_removes_nothing() {
    let router = service();
    let directory_id = active_directory(&router, "corp.example.com", "length bound").await;

    // At the published upper `RemoteDomainName` length bound (`length 0..1024`) the delete
    // reaches the handler and resolves the stored forwarder...
    let at_bound = domain_of_length(REMOTE_DOMAIN_NAME_MAX_LEN);
    ok(
        &router,
        "CreateConditionalForwarder",
        json!({ "DirectoryId": &directory_id, "RemoteDomainName": &at_bound, "DnsIpAddrs": ["10.0.0.1"] }),
        "at the RemoteDomainName length bound",
    )
    .await;
    let kept = "kept.example.com";
    ok(
        &router,
        "CreateConditionalForwarder",
        json!({ "DirectoryId": &directory_id, "RemoteDomainName": kept, "DnsIpAddrs": ["10.0.0.2"] }),
        "length bound: a second forwarder",
    )
    .await;

    // ...and one character past it is refused by the modelled constraint, before any
    // handler runs, so nothing is removed.
    let past_bound = domain_of_length(REMOTE_DOMAIN_NAME_MAX_LEN + 1);
    let (status, parsed) = call(
        &router,
        "DeleteConditionalForwarder",
        json!({ "DirectoryId": &directory_id, "RemoteDomainName": &past_bound }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"one past the RemoteDomainName length bound\": expected 400, got {status} with \
         {parsed}"
    );
    let still_there = stored_domains(&router, &directory_id, "one past the length bound").await;
    assert_eq!(
        still_there,
        [at_bound.clone(), kept.to_string()]
            .into_iter()
            .collect::<BTreeSet<String>>(),
        "case \"one past the RemoteDomainName length bound\": expected the refused delete to \
         remove nothing, got {still_there:?}"
    );

    ok(
        &router,
        "DeleteConditionalForwarder",
        json!({ "DirectoryId": &directory_id, "RemoteDomainName": &at_bound }),
        "at the RemoteDomainName length bound",
    )
    .await;
    let left = stored_domains(&router, &directory_id, "at the length bound").await;
    assert_eq!(
        left,
        [kept.to_string()].into_iter().collect::<BTreeSet<String>>(),
        "case \"at the RemoteDomainName length bound\": expected only that forwarder to be \
         dropped, got {left:?}"
    );
}
