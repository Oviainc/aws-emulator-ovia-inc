// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of ONE rule: a DNS name a request submits is held to the
//! domain-name format, and one that breaks it is refused with the modelled
//! `InvalidDomainName` (HTTP 400) naming the rule it broke and quoting the name.
//!
//! The rule under test: every `DNSName` member is modelled `@length 0..1024`
//! with no pattern (smithy_query(query="shape",shape="DNSName")), which bounds
//! the request FIELD and not the name, so the format itself is the service's to
//! enforce: the DNS domain name format each of those members documents fixes a
//! label at 63 octets, a whole name at 255 including the dots, and gives every
//! label at least one octet. A name that breaks one of those rules is no domain
//! name, whichever member carried it, and the refusal states the broken rule in
//! one vocabulary — `FATAL problem: <rule> encountered with '<the name>'`.
//!
//! One malformed name submitted to one operation is ONE row of that class, so
//! the tables below span it: each of the three rules, several spellings of the
//! empty label (leading, doubled and trailing), both length rules AT their
//! published bound (accepted) and one octet past it (refused), and both
//! operations that take a name from the caller — the listing positioned at a
//! `dnsname`, and the create that names a new zone.
//!
//! Every test drives the service through the real router, using the same harness
//! the sibling zone suites use, so what is asserted is the HTTP/restXml
//! behaviour a Route 53 client sees.

use smithy_server_runtime::MultiServiceRouter;

use crate::hosted_zone_records::{call, create_zone_body, router, Reply};
use crate::hosted_zones_by_name::list_by_name;

/// The published bound on one label: "the maximum length of each label is 63
/// characters" (the DNS domain name format every `DNSName` member's
/// documentation points at).
const MAX_LABEL_OCTETS: usize = 63;

/// The published bound on a whole name: "the total DNS name can't exceed 255
/// characters", dots included.
const MAX_NAME_OCTETS: usize = 255;

/// A label of `octets` characters.
fn label(octets: usize) -> String {
    "a".repeat(octets)
}

/// A name of exactly `octets` octets in canonical form — every label at most
/// [`MAX_LABEL_OCTETS`] long, counting the dot that ends each label.
fn name_of_octets(octets: usize) -> String {
    let mut name = String::new();
    while name.len() < octets {
        let remaining = octets - name.len();
        let chunk = remaining.min(MAX_LABEL_OCTETS + 1);
        name.push_str(&label(chunk - 1));
        name.push('.');
    }
    // The trailing dot is the root label, which the canonical form carries and
    // the octet count includes.
    name
}

/// One malformed name, and the rule its refusal must state.
struct FaultRow {
    case: &'static str,
    name: String,
    /// The `<rule>` clause the refusal names.
    rule: &'static str,
}

/// Every malformed name the rows below submit to both operations.
fn fault_rows() -> Vec<FaultRow> {
    vec![
        FaultRow {
            case: "an empty label in the middle of the name",
            name: "bad..example.com".to_string(),
            rule: "DomainLabelEmpty (Domain label is empty)",
        },
        FaultRow {
            case: "an empty label at the front of the name",
            name: ".example.com".to_string(),
            rule: "DomainLabelEmpty (Domain label is empty)",
        },
        FaultRow {
            case: "an empty label past the root dot",
            name: "example.com..".to_string(),
            rule: "DomainLabelEmpty (Domain label is empty)",
        },
        FaultRow {
            case: "three dots in a row, which spell two empty labels",
            name: "a...b.example.com".to_string(),
            rule: "DomainLabelEmpty (Domain label is empty)",
        },
        FaultRow {
            case: "a label one octet past the published maximum",
            name: format!("{}.example.com", label(MAX_LABEL_OCTETS + 1)),
            rule: "DomainLabelTooLong (Domain label is too long)",
        },
        FaultRow {
            case: "a label far past the published maximum",
            name: format!("{}.example.com", label(200)),
            rule: "DomainLabelTooLong (Domain label is too long)",
        },
        FaultRow {
            case: "a name one octet past the published maximum",
            name: name_of_octets(MAX_NAME_OCTETS + 1),
            rule: "DomainTooLong (Domain name is too long)",
        },
    ]
}

/// The `Error/Message` of a refusal.
fn message_of(reply: &Reply, case: &str) -> String {
    reply
        .first("Error/Message")
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: the refusal must carry a message; body {}",
                reply.body
            )
        })
        .to_string()
}

/// Assert `reply` is the domain-name refusal for `name`, stating `rule`.
fn assert_refused(reply: &Reply, case: &str, name: &str, rule: &str) {
    assert_eq!(
        reply.status, 400,
        "case {case:?}: expected HTTP 400, got {} with body {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_code(),
        "InvalidDomainName",
        "case {case:?}: expected the InvalidDomainName code, got body {}",
        reply.body
    );
    let got = message_of(reply, case);
    let expected = format!("FATAL problem: {rule} encountered with '{name}'");
    assert_eq!(
        got, expected,
        "case {case:?}: expected message {expected:?}, got {got:?}"
    );
}

/// Create one hosted zone named `name`.
async fn create_zone(router: &MultiServiceRouter, name: &str, reference: &str) -> Reply {
    call(
        router,
        "POST",
        "/2013-04-01/hostedzone",
        &create_zone_body(name, reference, ""),
    )
    .await
}

/// How many hosted zones the account holds, through `GetHostedZoneCount`.
async fn zone_count(router: &MultiServiceRouter, case: &str) -> String {
    let reply = call(router, "GET", "/2013-04-01/hostedzonecount", "").await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200 from GetHostedZoneCount, got {} with body {}",
        reply.status, reply.body
    );
    reply
        .first("HostedZoneCount")
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: the reply must carry HostedZoneCount; body {}",
                reply.body
            )
        })
        .to_string()
}

#[tokio::test]
async fn a_malformed_dnsname_is_refused_by_the_listing() {
    let router = router();
    let created = create_zone(&router, "example.com", "dns-name-listing").await;
    assert_eq!(
        created.status, 201,
        "seeding a zone: expected 201, got {} with body {}",
        created.status, created.body
    );

    for row in fault_rows() {
        let reply = list_by_name(&router, Some(&row.name), None, None).await;
        assert_refused(&reply, row.case, &row.name, row.rule);
        // A refusal positions no page at all.
        assert!(
            reply.all("HostedZone/Id").is_empty(),
            "case {:?}: a refused listing reports no zone, got {:?}; body {}",
            row.case,
            reply.all("HostedZone/Id"),
            reply.body
        );
    }

    // The positive control: the zone the account holds is still listed, so the
    // refusals above are the names' and not the listing's.
    let listed = list_by_name(&router, Some("example.com"), None, None).await;
    assert_eq!(
        listed.status, 200,
        "a well-formed dnsname must be served, got {} with body {}",
        listed.status, listed.body
    );
    assert_eq!(
        listed.first("HostedZone/Name"),
        Some("example.com."),
        "the fixture zone must be listed; body {}",
        listed.body
    );
}

#[tokio::test]
async fn a_malformed_name_is_refused_by_the_create() {
    let router = router();
    let before = zone_count(&router, "before the refusals").await;

    for (index, row) in fault_rows().iter().enumerate() {
        let reply = create_zone(&router, &row.name, &format!("dns-name-create-{index}")).await;
        assert_refused(&reply, row.case, &row.name, row.rule);
        assert!(
            reply.first("HostedZone/Id").is_none(),
            "case {:?}: a refused create answers no zone; body {}",
            row.case,
            reply.body
        );
    }

    // Nothing was created by any of them.
    let after = zone_count(&router, "after the refusals").await;
    assert_eq!(
        after, before,
        "a refused create must leave the account's zones as they were, GetHostedZoneCount \
         reports {after}"
    );
}

#[tokio::test]
async fn a_name_at_each_published_bound_is_accepted() {
    // Each length rule AT its bound, which is the row that separates "past the
    // bound" from "the bound itself": a label of exactly 63 octets, and a whole
    // name of exactly 255 including the dots.
    let rows = [
        (
            "a label at the published maximum",
            format!("{}.example.com", label(MAX_LABEL_OCTETS)),
        ),
        (
            "a whole name at the published maximum",
            name_of_octets(MAX_NAME_OCTETS),
        ),
    ];

    let router = router();
    for (index, (case, name)) in rows.iter().enumerate() {
        let created = create_zone(&router, name, &format!("dns-name-bound-{index}")).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: a name at the published bound must be created, got {} with body {}",
            created.status, created.body
        );
        let listed = list_by_name(&router, Some(name), None, None).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: a name at the published bound must position a listing, got {} with \
             body {}",
            listed.status, listed.body
        );
        // The round trip: the zone just created is the one the listing starts
        // at, under the canonical form of its name (terminated by the root dot).
        let canonical = if name.ends_with('.') {
            name.clone()
        } else {
            format!("{name}.")
        };
        assert_eq!(
            listed.first("HostedZone/Name").map(str::to_string),
            Some(canonical.clone()),
            "case {case:?}: the listing must start at the zone just created ({canonical}); \
             body {}",
            listed.body
        );
    }
}

#[tokio::test]
async fn a_name_that_is_a_domain_name_but_no_zone_name_keeps_its_own_refusal() {
    // The format rule is not the only rule a zone's name has to satisfy: "you
    // can't create a hosted zone for a top-level domain (TLD)", which is a
    // different refusal under the same code — so a single-label name is refused
    // for THAT reason rather than reported as a malformed domain name.
    let router = router();
    for (index, name) in ["com", "test", "example"].iter().enumerate() {
        let reply = create_zone(&router, name, &format!("dns-name-tld-{index}")).await;
        assert_eq!(
            reply.status, 400,
            "case {name:?}: expected HTTP 400, got {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "InvalidDomainName",
            "case {name:?}: expected the InvalidDomainName code, got body {}",
            reply.body
        );
        let got = message_of(&reply, name);
        assert!(
            !got.starts_with("FATAL problem:"),
            "case {name:?}: a well-formed domain name must not be reported as malformed, got \
             {got:?}"
        );
        assert!(
            got.contains("top-level domain"),
            "case {name:?}: the refusal must state the rule a zone's name broke, got {got:?}"
        );
    }
}
