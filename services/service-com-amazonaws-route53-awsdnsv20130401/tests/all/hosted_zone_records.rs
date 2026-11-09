// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the Route 53 hosted-zone and record-set slice.
//!
//! Every test drives the service through the real router — the same
//! `MultiServiceRouter::dispatch` a socket-served request goes through — so what
//! is asserted is the HTTP/restXml behaviour a Route 53 client sees, not the
//! handlers' internals. Each test builds its OWN engine, so the stores do not
//! bleed between them.
//!
//! The suites are table-driven on purpose: the rules under test ("a name is
//! normalized to a trailing dot", "records list in reversed-label DNS order",
//! "a page is `MaxItems` long and the remainder is reachable through the next
//! token") generalise over the name, the record type, the count and the page
//! size, so each table row differs along one of those axes rather than merely
//! renaming a zone.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

/// The endpoint host the tests address, which is also the routing signal the
/// router resolves this service from.
///
/// `pub(crate)` so a sibling suite asserting a `Location` URL names the same
/// host the request was sent to.
pub(crate) const ENDPOINT: &str = "route53.amazonaws.com";

/// A router with just this service mounted on a fresh shared engine.
///
/// `pub(crate)` so a sibling suite in this test target drives the service
/// through the SAME harness rather than growing a second copy of it.
pub(crate) fn router() -> MultiServiceRouter {
    router_holding_vpcs(&[])
}

/// The IPv4 block every VPC [`router_holding_vpcs`] registers carries. The block
/// itself is immaterial to Route 53 — nothing here reads it — but a VPC of the
/// account has one, so the record is stored as Amazon EC2 would report it.
const HELD_VPC_CIDR_BLOCK: &str = "10.0.0.0/16";

/// A router with just this service mounted on a fresh shared engine whose
/// account already HOLDS each `(VPCId, VPCRegion)` of `vpcs` in its Amazon EC2
/// VPC inventory.
///
/// Which VPCs an account has is a fact of that inventory — the one
/// `DescribeVpcs` answers from — and it is what Route 53 resolves the VPC a
/// request names against, so a suite whose request must be SERVED for a VPC
/// registers that VPC here first. Written into the Amazon EC2 store directly
/// (both the record and the Region it belongs to), because the id has to be the
/// one the request names and `CreateVpc` mints its own.
///
/// `pub(crate)` for the same reason [`router`] is.
pub(crate) fn router_holding_vpcs(vpcs: &[(&str, &str)]) -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    {
        let state = ec2::state::service_state(&*engine)
            .expect("the shared engine must carry the Amazon EC2 store");
        let mut store = state.lock();
        for (vpc_id, vpc_region) in vpcs {
            store.vpcs.insert(
                (*vpc_id).to_string(),
                ec2::ops::types::Vpc {
                    vpc_id: Some((*vpc_id).to_string()),
                    cidr_block: Some(HELD_VPC_CIDR_BLOCK.to_string()),
                    state: Some(ec2::state::VPC_STATE_AVAILABLE.to_string()),
                    ..Default::default()
                },
            );
            store
                .vpc_regions
                .insert((*vpc_id).to_string(), (*vpc_region).to_string());
        }
    }
    let mut router = MultiServiceRouter::new();
    router.register_service(
        service_com_amazonaws_route53_awsdnsv20130401::EmulatedService::new(engine),
    );
    router
}

/// One HTTP exchange against the router.
pub(crate) struct Reply {
    pub(crate) status: u16,
    pub(crate) location: Option<String>,
    /// Every element of the response body that carries text, as
    /// `(path from the root, text)` pairs in document order.
    doc: Vec<(String, String)>,
    pub(crate) body: String,
}

impl Reply {
    /// The text of the first element whose path ends with `suffix`
    /// (`"ChangeInfo/Status"`), or `None` when the response has no such element.
    pub(crate) fn first(&self, suffix: &str) -> Option<&str> {
        self.all(suffix).into_iter().next()
    }

    /// The text of every element whose path ends with `suffix`, in document order.
    pub(crate) fn all(&self, suffix: &str) -> Vec<&str> {
        let tail = format!("/{suffix}");
        self.doc
            .iter()
            .filter(|(path, _)| path == suffix || path.ends_with(&tail))
            .map(|(_, text)| text.as_str())
            .collect()
    }

    /// The wire error code (`<Error><Code>`) of a refusal.
    pub(crate) fn error_code(&self) -> &str {
        self.first("Error/Code").unwrap_or("")
    }
}

/// Send one request and read the reply.
pub(crate) async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: &str,
) -> Reply {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header(http::header::HOST, ENDPOINT)
        .header(http::header::CONTENT_TYPE, "application/xml")
        .body(Bytes::from(body.to_owned()))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let location = response
        .headers()
        .get(http::header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let body = String::from_utf8_lossy(response.body()).to_string();
    Reply {
        status,
        location,
        doc: flatten_xml(&body),
        body,
    }
}

/// The slash-joined path of `name` inside the currently open elements.
fn path_of(open: &[(String, String)], name: &str) -> String {
    let mut parts: Vec<&str> = open.iter().map(|(element, _)| element.as_str()).collect();
    parts.push(name);
    parts.join("/")
}

/// The element name of a start tag's contents, dropping any attributes.
fn element_name(tag: &str) -> &str {
    tag.split_whitespace().next().unwrap_or(tag)
}

/// Resolve the five XML predefined entities. `&amp;` is resolved LAST so an
/// escaped `&amp;lt;` does not turn into `<`.
pub(crate) fn unescape_xml(raw: &str) -> String {
    raw.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// Escape the five XML predefined entities, so a request body stays well-formed
/// whatever text a table row puts in it. The inverse of [`unescape_xml`], and
/// kept beside it so the pair cannot drift apart.
pub(crate) fn escape_xml(raw: &str) -> String {
    raw.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Flatten an XML document into `(path, text)` pairs, so a test can assert on a
/// value by its position in the response rather than by scanning the raw text.
///
/// Deliberately hand-written rather than pulling in an XML crate: the only
/// documents this harness ever reads are the emulator's OWN restXml responses —
/// a declaration, nested elements, one `xmlns` attribute on the root, text and
/// the five predefined entities, with no CDATA, comments, DTDs or processing
/// instructions — and keeping the harness dependency-free keeps the workspace
/// lockfile untouched by a change that bootstraps one service. Anything it does
/// not understand panics with the body, so a surprise is never read as a pass.
fn flatten_xml(xml: &str) -> Vec<(String, String)> {
    // One entry per open element: its name and the raw text seen directly inside it.
    let mut open: Vec<(String, String)> = Vec::new();
    let mut flat: Vec<(String, String)> = Vec::new();
    let bytes = xml.as_bytes();
    let mut at = 0usize;
    while at < bytes.len() {
        if bytes[at] == b'<' {
            let Some(close) = xml[at..].find('>').map(|offset| at + offset) else {
                panic!("the response body has an unterminated tag\n{xml}");
            };
            let tag = &xml[at + 1..close];
            at = close + 1;
            if tag.starts_with('?') || tag.starts_with('!') {
                // The XML declaration; nothing else of this form is expected.
                continue;
            }
            if let Some(closing) = tag.strip_prefix('/') {
                let Some((name, raw)) = open.pop() else {
                    panic!("the response body has an unbalanced close tag\n{xml}");
                };
                assert_eq!(
                    name,
                    closing.trim(),
                    "mismatched close tag in the response body\n{xml}"
                );
                let text = unescape_xml(&raw);
                if !text.trim().is_empty() {
                    flat.push((path_of(&open, &name), text.trim().to_string()));
                }
            } else if let Some(empty) = tag.strip_suffix('/') {
                flat.push((path_of(&open, element_name(empty)), String::new()));
            } else {
                open.push((element_name(tag).to_string(), String::new()));
            }
        } else {
            let text_end = xml[at..]
                .find('<')
                .map_or(bytes.len(), |offset| at + offset);
            if let Some((_, raw)) = open.last_mut() {
                raw.push_str(&xml[at..text_end]);
            }
            at = text_end;
        }
    }
    assert!(
        open.is_empty(),
        "the response body has {} unclosed element(s)\n{xml}",
        open.len()
    );
    flat
}

// ---------------------------------------------------------------------------
// Request builders
// ---------------------------------------------------------------------------

/// A `CreateHostedZone` body.
///
/// `pub(crate)` so a sibling suite builds its creates through the SAME body
/// rather than growing a second copy of the request shape.
pub(crate) fn create_zone_body(name: &str, caller_reference: &str, config: &str) -> String {
    format!(
        "<CreateHostedZoneRequest xmlns=\"https://route53.amazonaws.com/doc/2013-04-01/\">\
         <Name>{name}</Name><CallerReference>{caller_reference}</CallerReference>{config}\
         </CreateHostedZoneRequest>"
    )
}

/// Create a zone and return the reply.
async fn create_zone(
    router: &MultiServiceRouter,
    name: &str,
    caller_reference: &str,
    config: &str,
) -> Reply {
    call(
        router,
        "POST",
        "/2013-04-01/hostedzone",
        &create_zone_body(name, caller_reference, config),
    )
    .await
}

/// Create a zone, assert it was created, and return its BARE id.
pub(crate) async fn create_zone_ok(
    router: &MultiServiceRouter,
    name: &str,
    caller_reference: &str,
) -> String {
    let reply = create_zone(router, name, caller_reference, "").await;
    assert_eq!(
        reply.status, 201,
        "creating {name}: expected 201, got {} with body {}",
        reply.status, reply.body
    );
    bare_zone_id(&reply)
}

/// The bare (`Z…`) id from a reply carrying a `HostedZone`.
fn bare_zone_id(reply: &Reply) -> String {
    let id = reply
        .first("HostedZone/Id")
        .unwrap_or_else(|| panic!("the reply must carry HostedZone/Id; body {}", reply.body));
    id.trim_start_matches("/hostedzone/").to_string()
}

/// One row of a record-set table.
///
/// `pub(crate)` so a sibling suite drives the same change bodies rather than
/// growing a second builder for them.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RecordRow {
    pub(crate) name: &'static str,
    pub(crate) rr_type: &'static str,
    pub(crate) ttl: i64,
    pub(crate) values: &'static [&'static str],
}

/// A `<Change>` element for one action over one row.
pub(crate) fn change_element(action: &str, row: &RecordRow) -> String {
    let records: String = row
        .values
        .iter()
        .map(|value| format!("<ResourceRecord><Value>{value}</Value></ResourceRecord>"))
        .collect();
    format!(
        "<Change><Action>{action}</Action><ResourceRecordSet><Name>{}</Name><Type>{}</Type>\
         <TTL>{}</TTL><ResourceRecords>{records}</ResourceRecords></ResourceRecordSet></Change>",
        row.name, row.rr_type, row.ttl
    )
}

/// A `ChangeResourceRecordSets` body wrapping already-rendered `<Change>` elements.
pub(crate) fn change_batch_body(changes: &str, comment: Option<&str>) -> String {
    let comment = comment.map_or_else(String::new, |text| format!("<Comment>{text}</Comment>"));
    format!(
        "<ChangeResourceRecordSetsRequest xmlns=\"https://route53.amazonaws.com/doc/2013-04-01/\">\
         <ChangeBatch>{comment}<Changes>{changes}</Changes></ChangeBatch>\
         </ChangeResourceRecordSetsRequest>"
    )
}

/// Submit a change batch of one action applied to each row.
async fn submit_changes(
    router: &MultiServiceRouter,
    zone_id: &str,
    action: &str,
    rows: &[RecordRow],
) -> Reply {
    let changes: String = rows.iter().map(|row| change_element(action, row)).collect();
    call(
        router,
        "POST",
        &format!("/2013-04-01/hostedzone/{zone_id}/rrset"),
        &change_batch_body(&changes, None),
    )
    .await
}

/// List a zone's record sets, optionally paging.
async fn list_records(router: &MultiServiceRouter, zone_id: &str, query: &str) -> Reply {
    let uri = format!("/2013-04-01/hostedzone/{zone_id}/rrset{query}");
    call(router, "GET", &uri, "").await
}

// ---------------------------------------------------------------------------
// CreateHostedZone / GetHostedZone
// ---------------------------------------------------------------------------

/// A minted zone id is `Z` plus 14 uppercase alphanumerics.
fn is_zone_id(id: &str) -> bool {
    let mut chars = id.chars();
    chars.next() == Some('Z')
        && id.len() == 15
        && chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
}

/// A minted change id is `C` plus 14 uppercase alphanumerics, behind `/change/`.
fn is_change_id(id: &str) -> bool {
    let Some(bare) = id.strip_prefix("/change/") else {
        return false;
    };
    let mut chars = bare.chars();
    chars.next() == Some('C')
        && bare.len() == 15
        && chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
}

#[tokio::test]
async fn created_zones_are_normalized_minted_and_readable_back() {
    // The rule: a hosted zone is stored under a minted `Z…` id with its name
    // normalized to a trailing dot, and a public zone starts with exactly the
    // default SOA and NS records Route 53 creates. The rows differ in the number
    // of labels, in whether the caller already supplied the trailing dot, and in
    // whether a comment / private flag was given at all.
    struct Case {
        name: &'static str,
        config: &'static str,
        expect_name: &'static str,
        expect_comment: Option<&'static str>,
        expect_private: bool,
        expect_record_count: &'static str,
    }
    let cases = [
        Case {
            name: "example.com",
            config: "",
            expect_name: "example.com.",
            expect_comment: None,
            expect_private: false,
            expect_record_count: "2",
        },
        Case {
            name: "example.net.",
            config: "<HostedZoneConfig><Comment>already fully qualified</Comment></HostedZoneConfig>",
            expect_name: "example.net.",
            expect_comment: Some("already fully qualified"),
            expect_private: false,
            expect_record_count: "2",
        },
        Case {
            name: "deep.sub.example.org",
            config: "<HostedZoneConfig><Comment>three labels under the TLD</Comment></HostedZoneConfig>",
            expect_name: "deep.sub.example.org.",
            expect_comment: Some("three labels under the TLD"),
            expect_private: false,
            expect_record_count: "2",
        },
        Case {
            // A private zone is created with its first VPC association, and
            // gets the same default SOA and NS records.
            name: "internal.example.co.uk",
            config: "<HostedZoneConfig><PrivateZone>true</PrivateZone></HostedZoneConfig>\
                     <VPC><VPCRegion>us-east-1</VPCRegion><VPCId>vpc-1a2b3c4d</VPCId></VPC>",
            expect_name: "internal.example.co.uk.",
            expect_comment: None,
            expect_private: true,
            expect_record_count: "2",
        },
    ];

    let router = router();
    for (index, case) in cases.iter().enumerate() {
        let caller_reference = format!("ref-{index}");
        let reply = create_zone(&router, case.name, &caller_reference, case.config).await;
        assert_eq!(
            reply.status, 201,
            "case {}: expected 201, got {} with body {}",
            case.name, reply.status, reply.body
        );

        let zone_id = bare_zone_id(&reply);
        assert!(
            is_zone_id(&zone_id),
            "case {}: expected a minted Z-prefixed zone id, got {zone_id:?}",
            case.name
        );
        assert_eq!(
            reply.first("HostedZone/Name"),
            Some(case.expect_name),
            "case {}: expected the name normalized to {:?}, got {:?}",
            case.name,
            case.expect_name,
            reply.first("HostedZone/Name")
        );
        assert_eq!(
            reply.first("HostedZone/CallerReference"),
            Some(caller_reference.as_str()),
            "case {}: expected the CallerReference echoed from state, got {:?}",
            case.name,
            reply.first("HostedZone/CallerReference")
        );
        assert_eq!(
            reply.first("HostedZone/ResourceRecordSetCount"),
            Some(case.expect_record_count),
            "case {}: expected ResourceRecordSetCount {:?}, got {:?}",
            case.name,
            case.expect_record_count,
            reply.first("HostedZone/ResourceRecordSetCount")
        );
        assert_eq!(
            reply.first("Config/Comment"),
            case.expect_comment,
            "case {}: expected Config/Comment {:?}, got {:?}",
            case.name,
            case.expect_comment,
            reply.first("Config/Comment")
        );
        if case.expect_private {
            assert_eq!(
                reply.first("Config/PrivateZone"),
                Some("true"),
                "case {}: expected a private zone to report Config/PrivateZone, got {:?}",
                case.name,
                reply.first("Config/PrivateZone")
            );
        }

        // Four name servers, one per TLD Route 53 delegates across.
        let name_servers = reply.all("NameServers/NameServer");
        assert_eq!(
            name_servers.len(),
            4,
            "case {}: expected four name servers, got {name_servers:?}",
            case.name
        );
        for tld in ["com", "net", "org", "co.uk"] {
            assert!(
                name_servers.iter().any(|ns| ns.ends_with(&format!(".{tld}"))),
                "case {}: expected a name server in .{tld}, got {name_servers:?}",
                case.name
            );
        }

        // The change the create submitted is PENDING and addressable.
        let change_id = reply.first("ChangeInfo/Id").unwrap_or("");
        assert!(
            is_change_id(change_id),
            "case {}: expected a /change/C… id, got {change_id:?}",
            case.name
        );
        assert_eq!(
            reply.first("ChangeInfo/Status"),
            Some("PENDING"),
            "case {}: a just-submitted change must still be PENDING, got {:?}",
            case.name,
            reply.first("ChangeInfo/Status")
        );
        assert_eq!(
            reply.location.as_deref(),
            Some(format!("https://{ENDPOINT}/2013-04-01/hostedzone/{zone_id}").as_str()),
            "case {}: expected a Location header naming the new zone, got {:?}",
            case.name,
            reply.location
        );

        // Round trip: both the bare and the `/hostedzone/`-prefixed form of the
        // id read the same stored zone back.
        for form in [zone_id.clone(), format!("%2Fhostedzone%2F{zone_id}")] {
            let got = call(
                &router,
                "GET",
                &format!("/2013-04-01/hostedzone/{form}"),
                "",
            )
            .await;
            assert_eq!(
                got.status, 200,
                "case {} via {form}: expected 200, got {} with body {}",
                case.name, got.status, got.body
            );
            assert_eq!(
                got.first("HostedZone/Name"),
                Some(case.expect_name),
                "case {} via {form}: GetHostedZone must return the stored name, got {:?}",
                case.name,
                got.first("HostedZone/Name")
            );
            assert_eq!(
                got.first("HostedZone/ResourceRecordSetCount"),
                Some(case.expect_record_count),
                "case {} via {form}: expected a live ResourceRecordSetCount of {:?}, got {:?}",
                case.name,
                case.expect_record_count,
                got.first("HostedZone/ResourceRecordSetCount")
            );
            assert_eq!(
                got.all("NameServers/NameServer"),
                name_servers,
                "case {} via {form}: GetHostedZone must return the zone's own delegation set",
                case.name
            );
        }
    }
}

#[tokio::test]
async fn a_public_zone_starts_with_the_default_soa_and_ns_records() {
    // The rule: for a public hosted zone Route 53 creates one SOA and one NS
    // record at the apex; the NS record carries the delegation set's four name
    // servers and the TTLs are the documented defaults.
    let router = router();
    let reply = create_zone(&router, "example.com", "ref-defaults", "").await;
    let zone_id = bare_zone_id(&reply);
    let name_servers: Vec<String> = reply
        .all("NameServers/NameServer")
        .iter()
        .map(|ns| format!("{ns}."))
        .collect();

    let listed = list_records(&router, &zone_id, "").await;
    assert_eq!(listed.status, 200, "body {}", listed.body);
    assert_eq!(
        listed.all("ResourceRecordSet/Type"),
        vec!["NS", "SOA"],
        "expected exactly the apex NS and SOA records, got {}",
        listed.body
    );
    assert_eq!(
        listed.all("ResourceRecordSet/Name"),
        vec!["example.com.", "example.com."],
        "the default records must sit at the zone apex, got {}",
        listed.body
    );
    assert_eq!(
        listed.all("ResourceRecordSet/TTL"),
        vec!["172800", "900"],
        "expected the documented NS (172800s) and SOA (900s) TTLs, got {}",
        listed.body
    );
    let values = listed.all("ResourceRecord/Value");
    assert_eq!(
        values[..4].to_vec(),
        name_servers,
        "the NS record must carry the zone's own name servers, got {values:?}"
    );
    assert!(
        values[4].starts_with(&name_servers[0])
            && values[4].contains("awsdns-hostmaster.amazon.com."),
        "the SOA record must name the primary name server and the hostmaster mailbox, \
         got {:?}",
        values[4]
    );
}

#[tokio::test]
async fn a_repeated_caller_reference_is_refused_as_already_existing() {
    // The rule: CallerReference is the create's idempotency token, and a repeat
    // of one already used is refused with HostedZoneAlreadyExists (409). The
    // rows vary the reference's shape and whether the second create uses the
    // same zone name, which the rule does not key on.
    struct Case {
        first_name: &'static str,
        second_name: &'static str,
        caller_reference: &'static str,
    }
    let cases = [
        Case {
            first_name: "example.com",
            second_name: "example.com",
            caller_reference: "ref-1",
        },
        Case {
            first_name: "a.example.net",
            second_name: "b.example.net",
            caller_reference: "2026-01-01T00:00:00Z",
        },
        Case {
            first_name: "example.org",
            second_name: "example.org",
            caller_reference: "x",
        },
    ];

    let router = router();
    for case in &cases {
        let first = create_zone(&router, case.first_name, case.caller_reference, "").await;
        assert_eq!(
            first.status, 201,
            "case {}: expected the first create to succeed, got {} with body {}",
            case.caller_reference, first.status, first.body
        );
        let again = create_zone(&router, case.second_name, case.caller_reference, "").await;
        assert_eq!(
            again.status, 409,
            "case {}: expected 409 for a repeated CallerReference, got {} with body {}",
            case.caller_reference, again.status, again.body
        );
        assert_eq!(
            again.error_code(),
            "HostedZoneAlreadyExists",
            "case {}: expected HostedZoneAlreadyExists, got {}",
            case.caller_reference,
            again.body
        );

        // And the refusal changed nothing: a different reference is still free.
        let distinct = create_zone(
            &router,
            case.second_name,
            &format!("{}-distinct", case.caller_reference),
            "",
        )
        .await;
        assert_eq!(
            distinct.status, 201,
            "case {}: a different CallerReference must still be accepted, got {} with body {}",
            case.caller_reference, distinct.status, distinct.body
        );
    }
}

#[tokio::test]
async fn a_top_level_domain_cannot_be_a_hosted_zone() {
    // The rule, from the operation's documentation: "You can't create a hosted
    // zone for a top-level domain (TLD) such as .com." A zone name therefore
    // needs at least two labels. The rows span single-label names (refused) and
    // multi-label ones (accepted), including a name already carrying its dot.
    let refused = ["com", "net", "org.", "io", "localhost"];
    let accepted = ["example.com", "example.co.uk.", "a.b.c.example.net"];

    let router = router();
    for (index, name) in refused.iter().enumerate() {
        let reply = create_zone(&router, name, &format!("tld-{index}"), "").await;
        assert_eq!(
            reply.status, 400,
            "case {name}: expected 400, got {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "InvalidDomainName",
            "case {name}: expected InvalidDomainName, got {}",
            reply.body
        );
    }
    for (index, name) in accepted.iter().enumerate() {
        let reply = create_zone(&router, name, &format!("ok-{index}"), "").await;
        assert_eq!(
            reply.status, 201,
            "case {name}: expected 201, got {} with body {}",
            reply.status, reply.body
        );
    }
}

#[tokio::test]
async fn a_delegation_set_id_no_set_exists_for_is_refused() {
    // The rule: a reusable delegation set has to exist to be used, and this
    // emulator mints none, so every id a caller can pass names no set.
    let router = router();
    let body = create_zone_body("example.com", "ref-ds", "")
        .replace("</CreateHostedZoneRequest>", "<DelegationSetId>N1PA6795SAMPLE</DelegationSetId></CreateHostedZoneRequest>");
    let reply = call(&router, "POST", "/2013-04-01/hostedzone", &body).await;
    assert_eq!(
        reply.status, 400,
        "expected 400, got {} with body {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_code(),
        "NoSuchDelegationSet",
        "expected NoSuchDelegationSet, got {}",
        reply.body
    );
}

// ---------------------------------------------------------------------------
// ChangeResourceRecordSets / ListResourceRecordSets
// ---------------------------------------------------------------------------

#[tokio::test]
async fn each_change_action_is_applied_to_the_stored_zone() {
    // The rule: a change batch is applied against the stored zone keyed by
    // (Name, Type, SetIdentifier) — CREATE inserts and fails on an existing key,
    // UPSERT inserts or replaces, DELETE removes and fails on a missing key —
    // and ListResourceRecordSets returns exactly what is stored. The rows span
    // record types, TTLs and single- vs multi-valued record data.
    const ROWS: [RecordRow; 5] = [
        RecordRow {
            name: "www.example.com",
            rr_type: "A",
            ttl: 300,
            values: &["192.0.2.1"],
        },
        RecordRow {
            name: "ipv6.example.com",
            rr_type: "AAAA",
            ttl: 60,
            values: &["2001:db8::1"],
        },
        RecordRow {
            name: "example.com",
            rr_type: "MX",
            ttl: 3600,
            values: &["10 mail.example.com.", "20 backup.example.com."],
        },
        RecordRow {
            name: "notes.example.com",
            rr_type: "TXT",
            ttl: 120,
            values: &["\"a note\""],
        },
        RecordRow {
            name: "alias.example.com",
            rr_type: "CNAME",
            ttl: 900,
            values: &["www.example.com."],
        },
    ];

    let router = router();
    let zone_id = create_zone_ok(&router, "example.com", "ref-actions").await;

    // CREATE every row in one atomic batch.
    let created = submit_changes(&router, &zone_id, "CREATE", &ROWS).await;
    assert_eq!(
        created.status, 200,
        "expected 200 for the CREATE batch, got {} with body {}",
        created.status, created.body
    );
    assert!(
        is_change_id(created.first("ChangeInfo/Id").unwrap_or("")),
        "the CREATE batch must return a persisted change id, got {}",
        created.body
    );

    let listed = list_records(&router, &zone_id, "").await;
    let names = listed.all("ResourceRecordSet/Name");
    let types = listed.all("ResourceRecordSet/Type");
    for row in &ROWS {
        let want_name = format!("{}.", row.name.trim_end_matches('.'));
        let found = names
            .iter()
            .zip(types.iter())
            .any(|(name, rr_type)| *name == want_name && *rr_type == row.rr_type);
        assert!(
            found,
            "case {row:?}: expected the created record to be listed, got {}",
            listed.body
        );
    }
    for row in &ROWS {
        for value in row.values {
            assert!(
                listed.all("ResourceRecord/Value").contains(value),
                "case {row:?}: expected the stored value {value:?} to be listed, got {}",
                listed.body
            );
        }
    }
    assert!(
        listed.all("ResourceRecordSet/TTL").contains(&"300"),
        "case www/A: expected the stored TTL to be listed, got {}",
        listed.body
    );

    // A second identical CREATE is refused, and the batch is atomic: nothing moves.
    let repeat = submit_changes(&router, &zone_id, "CREATE", &ROWS).await;
    assert_eq!(
        repeat.status, 400,
        "expected 400 for a repeated CREATE, got {} with body {}",
        repeat.status, repeat.body
    );
    assert_eq!(
        repeat.error_code(),
        "InvalidChangeBatch",
        "expected InvalidChangeBatch, got {}",
        repeat.body
    );

    // UPSERT replaces the value of an existing key rather than adding a record.
    let upserted = RecordRow {
        name: "www.example.com",
        rr_type: "A",
        ttl: 60,
        values: &["192.0.2.2"],
    };
    let upsert = submit_changes(&router, &zone_id, "UPSERT", &[upserted]).await;
    assert_eq!(
        upsert.status, 200,
        "expected 200 for the UPSERT, got {} with body {}",
        upsert.status, upsert.body
    );
    let after = list_records(&router, &zone_id, "").await;
    let a_records = after
        .all("ResourceRecordSet/Name")
        .iter()
        .zip(after.all("ResourceRecordSet/Type"))
        .filter(|(name, rr_type)| **name == "www.example.com." && *rr_type == "A")
        .count();
    assert_eq!(
        a_records, 1,
        "case UPSERT www/A: expected exactly one A record after the upsert, got {}",
        after.body
    );
    assert!(
        after.all("ResourceRecord/Value").contains(&"192.0.2.2"),
        "case UPSERT www/A: expected the replaced value, got {}",
        after.body
    );
    assert!(
        !after.all("ResourceRecord/Value").contains(&"192.0.2.1"),
        "case UPSERT www/A: the replaced value must be gone, got {}",
        after.body
    );

    // UPSERT also INSERTS a key that does not exist yet.
    let inserted = RecordRow {
        name: "fresh.example.com",
        rr_type: "A",
        ttl: 30,
        values: &["192.0.2.9"],
    };
    assert_eq!(
        submit_changes(&router, &zone_id, "UPSERT", &[inserted])
            .await
            .status,
        200,
        "case UPSERT fresh/A: an upsert of an absent key must insert it"
    );
    assert!(
        list_records(&router, &zone_id, "")
            .await
            .all("ResourceRecordSet/Name")
            .contains(&"fresh.example.com."),
        "case UPSERT fresh/A: the inserted record must be listed"
    );

    // DELETE removes exactly the named keys; a second DELETE of the same key fails.
    let deletable = [ROWS[1], ROWS[3]];
    assert_eq!(
        submit_changes(&router, &zone_id, "DELETE", &deletable)
            .await
            .status,
        200,
        "expected the DELETE batch to succeed"
    );
    let remaining = list_records(&router, &zone_id, "").await;
    for row in &deletable {
        let want_name = format!("{}.", row.name.trim_end_matches('.'));
        assert!(
            !remaining
                .all("ResourceRecordSet/Name")
                .contains(&want_name.as_str()),
            "case DELETE {row:?}: the record must be gone, got {}",
            remaining.body
        );
    }
    let again = submit_changes(&router, &zone_id, "DELETE", &deletable).await;
    assert_eq!(
        again.status, 400,
        "expected 400 for deleting an absent record, got {} with body {}",
        again.status, again.body
    );
    assert_eq!(
        again.error_code(),
        "InvalidChangeBatch",
        "expected InvalidChangeBatch, got {}",
        again.body
    );
}

#[tokio::test]
async fn a_change_batch_is_bounded_at_the_documented_maximum() {
    // The rule: the model puts no length constraint on `Changes`, so the handler
    // itself bounds the batch at Route 53's documented maximum of 1,000 changes
    // in a single ChangeResourceRecordSets request; a batch at the bound is
    // applied and one past it is refused with InvalidChangeBatch. The rows are
    // the bound and one past it.
    const MAX_CHANGES_PER_BATCH: usize = 1_000;

    let router = router();
    for (label, count, expect_status) in [
        ("one under the bound", MAX_CHANGES_PER_BATCH - 1, 200),
        ("at the bound", MAX_CHANGES_PER_BATCH, 200),
        ("one past the bound", MAX_CHANGES_PER_BATCH + 1, 400),
    ] {
        let zone_id = create_zone_ok(&router, "example.com", &format!("ref-bound-{count}")).await;
        let changes: String = (0..count)
            .map(|index| {
                format!(
                    "<Change><Action>CREATE</Action><ResourceRecordSet>\
                     <Name>r{index}.example.com</Name><Type>A</Type><TTL>300</TTL>\
                     <ResourceRecords><ResourceRecord><Value>192.0.2.1</Value>\
                     </ResourceRecord></ResourceRecords></ResourceRecordSet></Change>"
                )
            })
            .collect();
        let reply = call(
            &router,
            "POST",
            &format!("/2013-04-01/hostedzone/{zone_id}/rrset"),
            &change_batch_body(&changes, None),
        )
        .await;
        assert_eq!(
            reply.status, expect_status,
            "case {label} ({count} changes): expected {expect_status}, got {} with body {}",
            reply.status,
            reply.body.chars().take(400).collect::<String>()
        );
        if expect_status == 400 {
            assert_eq!(
                reply.error_code(),
                "InvalidChangeBatch",
                "case {label}: expected InvalidChangeBatch, got {}",
                reply.body.chars().take(400).collect::<String>()
            );
            // The refused batch left the zone with only its default records.
            let listed = list_records(&router, &zone_id, "").await;
            assert_eq!(
                listed.all("ResourceRecordSet/Type"),
                vec!["NS", "SOA"],
                "case {label}: a refused batch must leave the zone unchanged"
            );
        } else {
            let listed = list_records(&router, &zone_id, "?maxitems=1").await;
            assert_eq!(
                listed.first("IsTruncated"),
                Some("true"),
                "case {label}: the accepted batch must have landed"
            );
        }
    }
}

#[tokio::test]
async fn an_invalid_change_batch_reports_a_bounded_number_of_problems() {
    // The rule: a refusal names the changes it could not apply, but the message
    // list is capped so the error body cannot be amplified by the batch size.
    // The rows span batch sizes either side of the cap.
    const MAX_REPORTED: usize = 32;

    let router = router();
    for bad_changes in [4, MAX_REPORTED, MAX_REPORTED + 50] {
        let zone_id =
            create_zone_ok(&router, "example.com", &format!("ref-cap-{bad_changes}")).await;
        // Every change deletes a record that does not exist, so every one is a problem.
        let changes: String = (0..bad_changes)
            .map(|index| {
                change_element(
                    "DELETE",
                    &RecordRow {
                        name: "absent.example.com",
                        rr_type: "A",
                        ttl: 300,
                        values: &["192.0.2.1"],
                    },
                )
                .replace("absent.", &format!("absent{index}."))
            })
            .collect();
        let reply = call(
            &router,
            "POST",
            &format!("/2013-04-01/hostedzone/{zone_id}/rrset"),
            &change_batch_body(&changes, None),
        )
        .await;
        assert_eq!(
            reply.status, 400,
            "case {bad_changes} bad changes: expected 400, got {} with body {}",
            reply.status,
            reply.body.chars().take(400).collect::<String>()
        );
        // The framework's error envelope renders a list error member as
        // `<messages><member>…`, so that is the path to count.
        let messages = reply.all("messages/member");
        let expected = bad_changes.min(MAX_REPORTED) + usize::from(bad_changes > MAX_REPORTED);
        assert_eq!(
            messages.len(),
            expected,
            "case {bad_changes} bad changes: expected {expected} reported messages, got \
             {messages:?} in {}",
            reply.body.chars().take(600).collect::<String>()
        );
    }
}

#[tokio::test]
async fn a_change_batch_is_refused_whole_when_any_change_is_invalid() {
    // The rule: the changes in one batch are a single transaction, so one
    // invalid change refuses the whole batch and leaves the zone untouched. The
    // rows span the reasons a change is invalid: a name outside the zone, a
    // record with neither record data nor an alias, a CREATE over an existing
    // key, and a DELETE of an absent key.
    let router = router();
    let zone_id = create_zone_ok(&router, "example.com", "ref-atomic").await;
    let good = RecordRow {
        name: "ok.example.com",
        rr_type: "A",
        ttl: 300,
        values: &["192.0.2.1"],
    };
    assert_eq!(
        submit_changes(&router, &zone_id, "CREATE", &[good])
            .await
            .status,
        200
    );
    let before = list_records(&router, &zone_id, "").await;
    let before_names = before.all("ResourceRecordSet/Name").join(",");

    // (label, the offending change, the error class it is refused with) — a change
    // that is not a well-formed change at all is a REQUEST fault (`InvalidInput`);
    // the others are refused by the batch evaluation.
    let cases: [(&str, String, &str); 4] = [
        (
            "a name outside the zone",
            change_element(
                "CREATE",
                &RecordRow {
                    name: "www.other-example.net",
                    rr_type: "A",
                    ttl: 300,
                    values: &["192.0.2.5"],
                },
            ),
            "InvalidChangeBatch",
        ),
        (
            "no record data and no alias",
            "<Change><Action>CREATE</Action><ResourceRecordSet><Name>bare.example.com</Name>\
             <Type>A</Type></ResourceRecordSet></Change>"
                .to_string(),
            "InvalidInput",
        ),
        (
            "a CREATE over an existing key",
            change_element("CREATE", &good),
            "InvalidChangeBatch",
        ),
        (
            "a DELETE of an absent key",
            change_element(
                "DELETE",
                &RecordRow {
                    name: "absent.example.com",
                    rr_type: "A",
                    ttl: 300,
                    values: &["192.0.2.6"],
                },
            ),
            "InvalidChangeBatch",
        ),
    ];

    for (label, offending, expect_code) in &cases {
        // Each batch pairs a VALID change with the offending one, so a batch
        // that was not atomic would leave the valid half behind.
        let valid = change_element(
            "CREATE",
            &RecordRow {
                name: "partial.example.com",
                rr_type: "TXT",
                ttl: 60,
                values: &["\"should not land\""],
            },
        );
        let reply = call(
            &router,
            "POST",
            &format!("/2013-04-01/hostedzone/{zone_id}/rrset"),
            &change_batch_body(&format!("{valid}{offending}"), Some("atomicity")),
        )
        .await;
        assert_eq!(
            reply.status, 400,
            "case {label}: expected 400, got {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            *expect_code,
            "case {label}: expected {expect_code}, got {}",
            reply.body
        );
        let after = list_records(&router, &zone_id, "").await;
        assert_eq!(
            after.all("ResourceRecordSet/Name").join(","),
            before_names,
            "case {label}: a refused batch must leave the zone unchanged, got {}",
            after.body
        );
    }
}

#[tokio::test]
async fn records_list_in_reversed_label_dns_order() {
    // The rule, from the operation's documentation: records sort "first by DNS
    // name with the labels reversed, for example: com.example.www." and, within
    // one name, by record type. The rows are inserted in an order unrelated to
    // the expected one, and include a wildcard label (whose `*` sorts before
    // `.`) and a name one level deeper than its parent.
    const ROWS: [RecordRow; 4] = [
        RecordRow {
            name: "zz.example.com",
            rr_type: "A",
            ttl: 300,
            values: &["192.0.2.4"],
        },
        RecordRow {
            name: "b.a.example.com",
            rr_type: "A",
            ttl: 300,
            values: &["192.0.2.3"],
        },
        RecordRow {
            name: "*.example.com",
            rr_type: "A",
            ttl: 300,
            values: &["192.0.2.1"],
        },
        RecordRow {
            name: "a.example.com",
            rr_type: "A",
            ttl: 300,
            values: &["192.0.2.2"],
        },
    ];

    let router = router();
    let zone_id = create_zone_ok(&router, "example.com", "ref-order").await;
    assert_eq!(
        submit_changes(&router, &zone_id, "CREATE", &ROWS).await.status,
        200
    );

    let listed = list_records(&router, &zone_id, "").await;
    let pairs: Vec<(String, String)> = listed
        .all("ResourceRecordSet/Name")
        .iter()
        .zip(listed.all("ResourceRecordSet/Type"))
        .map(|(name, rr_type)| ((*name).to_string(), rr_type.to_string()))
        .collect();
    // com.example. < com.example.* < com.example.a. < com.example.a.b. <
    // com.example.zz., and NS sorts before SOA at the shared apex name. The
    // wildcard label is PRESENTED as its octal escape code (see
    // `record_name_canonical_form`) while still sorting by the `*` it stands for.
    let expected = vec![
        ("example.com.".to_string(), "NS".to_string()),
        ("example.com.".to_string(), "SOA".to_string()),
        ("\\052.example.com.".to_string(), "A".to_string()),
        ("a.example.com.".to_string(), "A".to_string()),
        ("b.a.example.com.".to_string(), "A".to_string()),
        ("zz.example.com.".to_string(), "A".to_string()),
    ];
    assert_eq!(
        pairs, expected,
        "expected reversed-label DNS order, got {}",
        listed.body
    );
}

#[tokio::test]
async fn record_paging_covers_every_record_exactly_once() {
    // The rule (an invariant): a page holds at most MaxItems records, and
    // following NextRecordName/NextRecordType/NextRecordIdentifier walks the
    // remainder, so the concatenation of the pages equals the unpaged listing
    // for EVERY page size. The page size is the axis the rows vary along.
    const ROWS: [RecordRow; 6] = [
        RecordRow { name: "a.example.com", rr_type: "A", ttl: 300, values: &["192.0.2.1"] },
        RecordRow { name: "b.example.com", rr_type: "A", ttl: 300, values: &["192.0.2.2"] },
        RecordRow { name: "c.example.com", rr_type: "A", ttl: 300, values: &["192.0.2.3"] },
        RecordRow { name: "c.example.com", rr_type: "TXT", ttl: 300, values: &["\"c\""] },
        RecordRow { name: "d.example.com", rr_type: "A", ttl: 300, values: &["192.0.2.4"] },
        RecordRow { name: "e.example.com", rr_type: "A", ttl: 300, values: &["192.0.2.5"] },
    ];

    let router = router();
    let zone_id = create_zone_ok(&router, "example.com", "ref-paging").await;
    assert_eq!(
        submit_changes(&router, &zone_id, "CREATE", &ROWS).await.status,
        200
    );

    let unpaged = list_records(&router, &zone_id, "").await;
    let all: Vec<String> = unpaged
        .all("ResourceRecordSet/Name")
        .iter()
        .zip(unpaged.all("ResourceRecordSet/Type"))
        .map(|(name, rr_type)| format!("{name} {rr_type}"))
        .collect();
    assert_eq!(
        all.len(),
        ROWS.len() + 2,
        "the zone must hold the created records plus the default SOA and NS, got {}",
        unpaged.body
    );

    for page_size in 1..=all.len() + 1 {
        let mut walked: Vec<String> = Vec::new();
        let mut query = format!("?maxitems={page_size}");
        let mut pages = 0;
        loop {
            let page = list_records(&router, &zone_id, &query).await;
            assert_eq!(
                page.status, 200,
                "case maxitems={page_size}: expected 200, got {} with body {}",
                page.status, page.body
            );
            assert_eq!(
                page.first("MaxItems"),
                Some(page_size.to_string().as_str()),
                "case maxitems={page_size}: the response must report the page size it used, got {}",
                page.body
            );
            let names = page.all("ResourceRecordSet/Name");
            assert!(
                names.len() <= page_size,
                "case maxitems={page_size}: a page held {} records, got {}",
                names.len(),
                page.body
            );
            walked.extend(
                names
                    .iter()
                    .zip(page.all("ResourceRecordSet/Type"))
                    .map(|(name, rr_type)| format!("{name} {rr_type}")),
            );
            pages += 1;
            assert!(
                pages <= all.len() + 2,
                "case maxitems={page_size}: paging did not terminate"
            );
            if page.first("IsTruncated") != Some("true") {
                assert!(
                    page.first("NextRecordName").is_none(),
                    "case maxitems={page_size}: a final page must carry no next token, got {}",
                    page.body
                );
                break;
            }
            let next_name = page
                .first("NextRecordName")
                .expect("a truncated page must name where to resume");
            let next_type = page
                .first("NextRecordType")
                .expect("a truncated page must name the type to resume at");
            query = format!("?maxitems={page_size}&name={next_name}&type={next_type}");
        }
        assert_eq!(
            walked, all,
            "case maxitems={page_size}: paging must cover the listing exactly once"
        );
    }
}

#[tokio::test]
async fn a_start_record_type_without_a_name_is_refused() {
    // The rule, from the operation's documentation: "If you specify Type but not
    // Name … Amazon Route 53 returns the InvalidInput error." Type only orders
    // records that share a name. The rows span several types.
    let router = router();
    let zone_id = create_zone_ok(&router, "example.com", "ref-start-type").await;
    for rr_type in ["A", "TXT", "SOA"] {
        let reply = list_records(&router, &zone_id, &format!("?type={rr_type}")).await;
        assert_eq!(
            reply.status, 400,
            "case type={rr_type}: expected 400, got {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "InvalidInput",
            "case type={rr_type}: expected InvalidInput, got {}",
            reply.body
        );
        // The same type WITH a name is accepted.
        let ok = list_records(
            &router,
            &zone_id,
            &format!("?name=example.com.&type={rr_type}"),
        )
        .await;
        assert_eq!(
            ok.status, 200,
            "case type={rr_type} with a name: expected 200, got {} with body {}",
            ok.status, ok.body
        );
    }
}

// ---------------------------------------------------------------------------
// GetChange
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_minted_change_is_readable_and_reaches_insync() {
    // The rule: each of CreateHostedZone, ChangeResourceRecordSets and
    // DeleteHostedZone persists a new change, GetChange reports the persisted
    // one, and the change moves from PENDING to INSYNC on the clock alone. The
    // rows are the three operations that mint a change.
    let router = router();

    let created = create_zone(&router, "example.com", "ref-change", "").await;
    let zone_id = bare_zone_id(&created);
    let changed = submit_changes(
        &router,
        &zone_id,
        "CREATE",
        &[RecordRow {
            name: "www.example.com",
            rr_type: "A",
            ttl: 300,
            values: &["192.0.2.1"],
        }],
    )
    .await;
    let deleted = submit_changes(
        &router,
        &zone_id,
        "DELETE",
        &[RecordRow {
            name: "www.example.com",
            rr_type: "A",
            ttl: 300,
            values: &["192.0.2.1"],
        }],
    )
    .await;
    let removed = call(
        &router,
        "DELETE",
        &format!("/2013-04-01/hostedzone/{zone_id}"),
        "",
    )
    .await;

    let cases = [
        ("CreateHostedZone", &created),
        ("ChangeResourceRecordSets", &changed),
        ("DELETE of a record", &deleted),
        ("DeleteHostedZone", &removed),
    ];
    let mut ids = Vec::new();
    for (label, reply) in &cases {
        let id = reply
            .first("ChangeInfo/Id")
            .unwrap_or_else(|| panic!("case {label}: no ChangeInfo/Id in {}", reply.body));
        assert!(
            is_change_id(id),
            "case {label}: expected a /change/C… id, got {id:?}"
        );
        ids.push(id.to_string());
    }
    assert_eq!(
        ids.iter().collect::<std::collections::BTreeSet<_>>().len(),
        ids.len(),
        "each change must get its own id, got {ids:?}"
    );

    // Both the bare and the `/change/`-prefixed form read the same change back.
    for (label, id) in cases.iter().map(|(label, _)| label).zip(ids.iter()) {
        for form in [id.clone(), id.trim_start_matches("/change/").to_string()] {
            let got = call(
                &router,
                "GET",
                &format!("/2013-04-01/change/{}", form.replace('/', "%2F")),
                "",
            )
            .await;
            assert_eq!(
                got.status, 200,
                "case {label} via {form}: expected 200, got {} with body {}",
                got.status, got.body
            );
            assert_eq!(
                got.first("ChangeInfo/Id"),
                Some(id.as_str()),
                "case {label} via {form}: GetChange must return the persisted id, got {}",
                got.body
            );
            assert!(
                got.first("ChangeInfo/SubmittedAt").is_some(),
                "case {label} via {form}: a change must report when it was submitted, got {}",
                got.body
            );
        }
    }

    // The clock owes the PENDING -> INSYNC transition; nothing has to poll for
    // it. A blocking sleep, because this crate's tokio does not enable the
    // `time` feature and the harness deliberately takes no dependency of its
    // own; each test owns its runtime, so nothing else is waiting on it.
    std::thread::sleep(Duration::from_millis(700));
    for (label, id) in cases.iter().map(|(label, _)| label).zip(ids.iter()) {
        let bare = id.trim_start_matches("/change/");
        let got = call(&router, "GET", &format!("/2013-04-01/change/{bare}"), "").await;
        assert_eq!(
            got.first("ChangeInfo/Status"),
            Some("INSYNC"),
            "case {label}: the change must have reached INSYNC, got {}",
            got.body
        );
    }
}

#[tokio::test]
async fn an_unknown_change_id_is_a_not_found() {
    // The rule: GetChange reports a persisted change or nothing — never a
    // fabricated INSYNC. The rows span id shapes a caller might pass.
    let router = router();
    for id in ["C00000000000000", "BOGUS", "%2Fchange%2FC00000000000001"] {
        let reply = call(&router, "GET", &format!("/2013-04-01/change/{id}"), "").await;
        assert_eq!(
            reply.status, 404,
            "case {id}: expected 404, got {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "NoSuchChange",
            "case {id}: expected NoSuchChange, got {}",
            reply.body
        );
    }
}

// ---------------------------------------------------------------------------
// ListHostedZones / DeleteHostedZone
// ---------------------------------------------------------------------------

#[tokio::test]
async fn hosted_zone_paging_covers_every_zone_exactly_once() {
    // The rule (an invariant): a page holds at most MaxItems zones and
    // NextMarker names where the next page starts, so the pages concatenate to
    // the full list for EVERY page size. The page size is the varying axis.
    let router = router();
    let mut created = Vec::new();
    for index in 0..5 {
        created.push(create_zone_ok(&router, &format!("z{index}.example.com"), &format!("ref-{index}")).await);
    }

    let unpaged = call(&router, "GET", "/2013-04-01/hostedzone", "").await;
    assert_eq!(unpaged.status, 200, "body {}", unpaged.body);
    let all: Vec<String> = unpaged
        .all("HostedZone/Id")
        .iter()
        .map(|id| (*id).to_string())
        .collect();
    assert_eq!(
        all.len(),
        created.len(),
        "every created zone must be listed, got {}",
        unpaged.body
    );
    for id in &created {
        assert!(
            all.contains(&format!("/hostedzone/{id}")),
            "zone {id} must appear in the listing, got {}",
            unpaged.body
        );
    }

    for page_size in 1..=created.len() + 1 {
        let mut walked: Vec<String> = Vec::new();
        let mut marker: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match &marker {
                Some(m) => format!("?maxitems={page_size}&marker={m}"),
                None => format!("?maxitems={page_size}"),
            };
            let page = call(
                &router,
                "GET",
                &format!("/2013-04-01/hostedzone{query}"),
                "",
            )
            .await;
            assert_eq!(
                page.status, 200,
                "case maxitems={page_size}: expected 200, got {} with body {}",
                page.status, page.body
            );
            assert_eq!(
                page.first("Marker"),
                marker.as_deref(),
                "case maxitems={page_size}: the response must echo the request's marker, got {}",
                page.body
            );
            let ids = page.all("HostedZone/Id");
            assert!(
                ids.len() <= page_size,
                "case maxitems={page_size}: a page held {} zones, got {}",
                ids.len(),
                page.body
            );
            walked.extend(ids.iter().map(|id| (*id).to_string()));
            pages += 1;
            assert!(
                pages <= created.len() + 2,
                "case maxitems={page_size}: paging did not terminate"
            );
            if page.first("IsTruncated") != Some("true") {
                assert!(
                    page.first("NextMarker").is_none(),
                    "case maxitems={page_size}: a final page must carry no NextMarker, got {}",
                    page.body
                );
                break;
            }
            marker = Some(
                page.first("NextMarker")
                    .expect("a truncated page must carry a NextMarker")
                    .to_string(),
            );
        }
        assert_eq!(
            walked, all,
            "case maxitems={page_size}: paging must cover the listing exactly once"
        );
    }
}

#[tokio::test]
async fn hosted_zones_can_be_listed_by_type() {
    // The rule: `hostedzonetype=PrivateHostedZone` narrows the listing to
    // private zones; without it every zone is listed. The rows are the two
    // kinds of zone.
    let router = router();
    let public = create_zone_ok(&router, "public.example.com", "ref-public").await;
    let private = create_zone(
        &router,
        "private.example.com",
        "ref-private",
        "<HostedZoneConfig><PrivateZone>true</PrivateZone></HostedZoneConfig>\
         <VPC><VPCRegion>us-west-2</VPCRegion><VPCId>vpc-abcdef01</VPCId></VPC>",
    )
    .await;
    let private = bare_zone_id(&private);

    let all = call(&router, "GET", "/2013-04-01/hostedzone", "").await;
    for id in [&public, &private] {
        assert!(
            all.all("HostedZone/Id").contains(&format!("/hostedzone/{id}").as_str()),
            "case unfiltered: zone {id} must be listed, got {}",
            all.body
        );
    }

    let only_private = call(
        &router,
        "GET",
        "/2013-04-01/hostedzone?hostedzonetype=PrivateHostedZone",
        "",
    )
    .await;
    assert_eq!(
        only_private.all("HostedZone/Id"),
        vec![format!("/hostedzone/{private}")],
        "case hostedzonetype=PrivateHostedZone: expected only the private zone, got {}",
        only_private.body
    );
}

#[tokio::test]
async fn a_private_zone_must_be_created_with_a_complete_vpc() {
    // The rule: a private hosted zone is created with its first VPC
    // association, and that association needs both `VPCId` and `VPCRegion` —
    // "If you are associating a VPC with a hosted zone with this request, the
    // paramaters VPCId and VPCRegion are also required." The rows span the ways
    // the association can be incomplete, plus the complete one.
    let cases: [(&str, &str, u16); 5] = [
        ("no VPC at all", "", 400),
        (
            "a VPC element naming neither member",
            "<VPC></VPC>",
            400,
        ),
        (
            "a VPC naming only VPCId",
            "<VPC><VPCId>vpc-1a2b3c4d</VPCId></VPC>",
            400,
        ),
        (
            "a VPC naming only VPCRegion",
            "<VPC><VPCRegion>us-east-1</VPCRegion></VPC>",
            400,
        ),
        (
            "a complete VPC",
            "<VPC><VPCRegion>us-east-1</VPCRegion><VPCId>vpc-1a2b3c4d</VPCId></VPC>",
            201,
        ),
    ];

    let router = router();
    for (index, (label, vpc, expect_status)) in cases.iter().enumerate() {
        let config =
            format!("<HostedZoneConfig><PrivateZone>true</PrivateZone></HostedZoneConfig>{vpc}");
        let reply = create_zone(
            &router,
            "private.example.com",
            &format!("ref-vpc-required-{index}"),
            &config,
        )
        .await;
        assert_eq!(
            reply.status, *expect_status,
            "case {label}: expected {expect_status}, got {} with body {}",
            reply.status, reply.body
        );
        if *expect_status == 400 {
            assert_eq!(
                reply.error_code(),
                "InvalidInput",
                "case {label}: expected InvalidInput, got {}",
                reply.body
            );
        }
    }

    // A PUBLIC zone needs no VPC, so the same create without one is accepted —
    // and since the member is "(Private hosted zones only)", naming a VPC is
    // itself what asks for a private zone, with no `HostedZoneConfig` needed
    // (see `private_zone_creation::naming_a_vpc_is_what_makes_a_new_hosted_zone_private`
    // for the whole table).
    assert_eq!(
        create_zone(&router, "public.example.com", "ref-public-no-vpc", "")
            .await
            .status,
        201,
        "case a public zone with no VPC: expected 201"
    );
    let vpc_only = create_zone(
        &router,
        "vpc-only.example.com",
        "ref-vpc-only",
        "<VPC><VPCRegion>us-east-1</VPCRegion><VPCId>vpc-0a1b2c3d4e5f60718</VPCId></VPC>",
    )
    .await;
    assert_eq!(
        vpc_only.status, 201,
        "case a VPC with no HostedZoneConfig: expected 201, got {} with body {}",
        vpc_only.status, vpc_only.body
    );
    assert_eq!(
        vpc_only.first("Config/PrivateZone"),
        Some("true"),
        "case a VPC with no HostedZoneConfig: the zone must be private, got {}",
        vpc_only.body
    );

    // A `<VPC></VPC>` element naming neither member is not an association, so a
    // public zone created with one is accepted and reports NO VPC at all —
    // `VPCs` is a list of at least one member, so an empty one must not appear.
    let empty_element = create_zone(
        &router,
        "public2.example.com",
        "ref-public-empty-vpc",
        "<VPC></VPC>",
    )
    .await;
    assert_eq!(
        empty_element.status, 201,
        "case a public zone with an empty VPC element: expected 201, got {} with body {}",
        empty_element.status, empty_element.body
    );
    assert_eq!(
        empty_element.first("VPC/VPCId"),
        None,
        "case a public zone with an empty VPC element: no association may be echoed, got {}",
        empty_element.body
    );
    let read_back = call(
        &router,
        "GET",
        &format!("/2013-04-01/hostedzone/{}", bare_zone_id(&empty_element)),
        "",
    )
    .await;
    assert!(
        read_back.all("VPCs/VPC/VPCId").is_empty()
            && read_back.all("VPCs/VPC").is_empty(),
        "case a public zone with an empty VPC element: GetHostedZone must report no VPCs, got {}",
        read_back.body
    );
}

#[tokio::test]
async fn the_zones_own_delegation_records_cannot_be_deleted() {
    // The rule: a hosted zone holds exactly one SOA record and at least one NS
    // record for the zone itself, so a change batch cannot leave it without
    // them (which would also let DeleteHostedZone's "only the default SOA and
    // NS records" rule pass on a zone that no longer has them). The exact
    // refusals are asserted in `record_name_canonical_form`. The rows are the
    // two record types at the apex, against a record of the same type one label
    // down, which IS the caller's to delete.
    let router = router();
    let zone_id = create_zone_ok(&router, "example.com", "ref-apex-guard").await;

    // Delete each default record by naming it exactly as it is stored.
    let stored = list_records(&router, &zone_id, "").await;
    let values = stored.all("ResourceRecord/Value");
    let ns_values: Vec<&str> = values[..4].to_vec();
    let soa_value = values[4];

    for (label, rr_type, ttl, record_values) in [
        ("the apex NS record", "NS", 172_800, ns_values.clone()),
        ("the apex SOA record", "SOA", 900, vec![soa_value]),
    ] {
        let records: String = record_values
            .iter()
            .map(|value| format!("<ResourceRecord><Value>{value}</Value></ResourceRecord>"))
            .collect();
        let change = format!(
            "<Change><Action>DELETE</Action><ResourceRecordSet><Name>example.com</Name>\
             <Type>{rr_type}</Type><TTL>{ttl}</TTL><ResourceRecords>{records}</ResourceRecords>\
             </ResourceRecordSet></Change>"
        );
        let reply = call(
            &router,
            "POST",
            &format!("/2013-04-01/hostedzone/{zone_id}/rrset"),
            &change_batch_body(&change, None),
        )
        .await;
        assert_eq!(
            reply.status, 400,
            "case {label}: expected 400, got {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "InvalidChangeBatch",
            "case {label}: expected InvalidChangeBatch, got {}",
            reply.body
        );
        assert_eq!(
            list_records(&router, &zone_id, "")
                .await
                .all("ResourceRecordSet/Type"),
            vec!["NS", "SOA"],
            "case {label}: the zone's default records must still be there"
        );
    }

    // The same types one label down are ordinary records the caller owns.
    for (label, rr_type, ttl, value) in [
        ("a delegated subdomain NS record", "NS", 300, "ns1.example.net."),
        ("a subdomain TXT record", "TXT", 300, "\"owned\""),
    ] {
        let row = format!(
            "<Change><Action>{{}}</Action><ResourceRecordSet><Name>sub.example.com</Name>\
             <Type>{rr_type}</Type><TTL>{ttl}</TTL><ResourceRecords><ResourceRecord>\
             <Value>{value}</Value></ResourceRecord></ResourceRecords></ResourceRecordSet>\
             </Change>"
        );
        for action in ["CREATE", "DELETE"] {
            let reply = call(
                &router,
                "POST",
                &format!("/2013-04-01/hostedzone/{zone_id}/rrset"),
                &change_batch_body(&row.replace("{}", action), None),
            )
            .await;
            assert_eq!(
                reply.status, 200,
                "case {label} ({action}): expected 200, got {} with body {}",
                reply.status, reply.body
            );
        }
    }
}

#[tokio::test]
async fn a_delete_must_name_the_record_it_deletes() {
    // The rule, from the operation's documentation: "To delete a resource record
    // set, you must specify all the same values that you specified when you
    // created it." The rows span the ways a DELETE can fail to name the stored
    // record set — no record data at all, a different value, a subset of the
    // values, a different TTL — and the one that names it exactly.
    const STORED: RecordRow = RecordRow {
        name: "www.example.com",
        rr_type: "A",
        ttl: 300,
        values: &["192.0.2.1", "192.0.2.2"],
    };

    let router = router();
    let zone_id = create_zone_ok(&router, "example.com", "ref-delete-match").await;
    assert_eq!(
        submit_changes(&router, &zone_id, "CREATE", &[STORED])
            .await
            .status,
        200
    );

    // (label, the change, the expected status, the expected error code) — a change
    // that is not a well-formed change at all is refused as a REQUEST fault
    // (`InvalidInput`), while a change the zone cannot apply is refused by the batch
    // evaluation (`InvalidChangeBatch`).
    let cases: [(&str, String, u16, &str); 5] = [
        (
            "no record data at all",
            "<Change><Action>DELETE</Action><ResourceRecordSet>\
             <Name>www.example.com</Name><Type>A</Type></ResourceRecordSet></Change>"
                .to_string(),
            400,
            "InvalidInput",
        ),
        (
            "a different value",
            change_element(
                "DELETE",
                &RecordRow {
                    values: &["192.0.2.9", "192.0.2.2"],
                    ..STORED
                },
            ),
            400,
            "InvalidChangeBatch",
        ),
        (
            "only one of the two values",
            change_element(
                "DELETE",
                &RecordRow {
                    values: &["192.0.2.1"],
                    ..STORED
                },
            ),
            400,
            "InvalidChangeBatch",
        ),
        (
            "a different TTL",
            change_element("DELETE", &RecordRow { ttl: 60, ..STORED }),
            400,
            "InvalidChangeBatch",
        ),
        (
            "the stored values, in a different order",
            change_element(
                "DELETE",
                &RecordRow {
                    values: &["192.0.2.2", "192.0.2.1"],
                    ..STORED
                },
            ),
            200,
            "",
        ),
    ];

    for (label, change, expect_status, expect_code) in &cases {
        let reply = call(
            &router,
            "POST",
            &format!("/2013-04-01/hostedzone/{zone_id}/rrset"),
            &change_batch_body(change, None),
        )
        .await;
        assert_eq!(
            reply.status, *expect_status,
            "case {label}: expected {expect_status}, got {} with body {}",
            reply.status, reply.body
        );
        if *expect_status == 400 {
            assert_eq!(
                reply.error_code(),
                *expect_code,
                "case {label}: expected {expect_code}, got {}",
                reply.body
            );
            assert!(
                list_records(&router, &zone_id, "")
                    .await
                    .all("ResourceRecordSet/Name")
                    .contains(&"www.example.com."),
                "case {label}: the record must still be there after a refused delete"
            );
        } else {
            assert!(
                !list_records(&router, &zone_id, "")
                    .await
                    .all("ResourceRecordSet/Name")
                    .contains(&"www.example.com."),
                "case {label}: the record must be gone after an accepted delete"
            );
        }
    }
}

#[tokio::test]
async fn hosted_zone_creation_is_bounded_at_the_account_quota() {
    // The rule: `CallerReference` is caller-chosen, so the number of zones an
    // account may hold is bounded by Route 53's documented account quota of 500
    // hosted zones, and the create past it answers the modelled
    // `TooManyHostedZones`. The rows are the bound and one past it.
    const MAX_HOSTED_ZONES: usize = 500;

    let router = router();
    for index in 0..MAX_HOSTED_ZONES {
        let reply = create_zone(
            &router,
            &format!("z{index}.example.com"),
            &format!("ref-quota-{index}"),
            "",
        )
        .await;
        assert_eq!(
            reply.status, 201,
            "case zone {index} of {MAX_HOSTED_ZONES}: expected 201, got {} with body {}",
            reply.status, reply.body
        );
    }

    let past = create_zone(
        &router,
        "one-too-many.example.com",
        "ref-quota-past",
        "",
    )
    .await;
    assert_eq!(
        past.status, 400,
        "case one past the quota: expected 400, got {} with body {}",
        past.status, past.body
    );
    assert_eq!(
        past.error_code(),
        "TooManyHostedZones",
        "case one past the quota: expected TooManyHostedZones, got {}",
        past.body
    );

    // The refusal stored nothing: deleting one zone frees exactly one slot.
    let listed = call(&router, "GET", "/2013-04-01/hostedzone?maxitems=1", "").await;
    let first = listed
        .first("HostedZone/Id")
        .expect("the listing must name a zone")
        .trim_start_matches("/hostedzone/")
        .to_string();
    assert_eq!(
        call(
            &router,
            "DELETE",
            &format!("/2013-04-01/hostedzone/{first}"),
            ""
        )
        .await
        .status,
        200,
        "case freeing a slot: the zone must delete"
    );
    assert_eq!(
        create_zone(&router, "now-there-is-room.example.com", "ref-quota-again", "")
            .await
            .status,
        201,
        "case freeing a slot: a create must be accepted again"
    );
}

#[tokio::test]
async fn a_private_zone_keeps_the_vpc_it_was_created_with() {
    // The rule: the VPC a private hosted zone is created with is stored on the
    // zone, echoed by the create and reported by GetHostedZone's `VPCs`. The
    // rows vary the VPC id and the region, which the rule does not key on.
    let cases = [
        ("vpc-1a2b3c4d", "us-east-1"),
        ("vpc-0123456789abcdef0", "eu-west-2"),
        ("vpc-ffffffff", "ap-southeast-1"),
    ];

    let router = router();
    for (index, (vpc_id, vpc_region)) in cases.iter().enumerate() {
        let body = create_zone_body(
            &format!("private{index}.example.com"),
            &format!("ref-vpc-{index}"),
            "<HostedZoneConfig><PrivateZone>true</PrivateZone></HostedZoneConfig>",
        )
        .replace(
            "</CreateHostedZoneRequest>",
            &format!(
                "<VPC><VPCRegion>{vpc_region}</VPCRegion><VPCId>{vpc_id}</VPCId></VPC>\
                 </CreateHostedZoneRequest>"
            ),
        );
        let created = call(&router, "POST", "/2013-04-01/hostedzone", &body).await;
        assert_eq!(
            created.status, 201,
            "case {vpc_id}/{vpc_region}: expected 201, got {} with body {}",
            created.status, created.body
        );
        assert_eq!(
            created.first("VPC/VPCId"),
            Some(*vpc_id),
            "case {vpc_id}/{vpc_region}: the create must echo the stored VPC, got {}",
            created.body
        );

        let zone_id = bare_zone_id(&created);
        let got = call(
            &router,
            "GET",
            &format!("/2013-04-01/hostedzone/{zone_id}"),
            "",
        )
        .await;
        assert_eq!(
            got.first("VPCs/VPC/VPCId"),
            Some(*vpc_id),
            "case {vpc_id}/{vpc_region}: GetHostedZone must report the association, got {}",
            got.body
        );
        assert_eq!(
            got.first("VPCs/VPC/VPCRegion"),
            Some(*vpc_region),
            "case {vpc_id}/{vpc_region}: GetHostedZone must report the VPC's region, got {}",
            got.body
        );
    }
}

#[tokio::test]
async fn a_zone_is_deletable_only_once_it_holds_only_its_default_records() {
    // The rule, from the operation's documentation: "You can delete a hosted
    // zone only if it contains only the default SOA and NS records." The rows
    // span the kinds of record that keep a zone non-empty.
    const BLOCKERS: [RecordRow; 3] = [
        RecordRow { name: "www.example.com", rr_type: "A", ttl: 300, values: &["192.0.2.1"] },
        RecordRow { name: "example.com", rr_type: "TXT", ttl: 60, values: &["\"apex txt\""] },
        RecordRow { name: "deep.sub.example.com", rr_type: "CNAME", ttl: 60, values: &["www.example.com."] },
    ];

    let router = router();
    for (index, blocker) in BLOCKERS.iter().enumerate() {
        let zone_id = create_zone_ok(&router, "example.com", &format!("ref-del-{index}")).await;
        assert_eq!(
            submit_changes(&router, &zone_id, "CREATE", &[*blocker])
                .await
                .status,
            200
        );

        let refused = call(
            &router,
            "DELETE",
            &format!("/2013-04-01/hostedzone/{zone_id}"),
            "",
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {blocker:?}: expected 400 while the zone holds the record, got {} with body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "HostedZoneNotEmpty",
            "case {blocker:?}: expected HostedZoneNotEmpty, got {}",
            refused.body
        );
        assert_eq!(
            call(
                &router,
                "GET",
                &format!("/2013-04-01/hostedzone/{zone_id}"),
                ""
            )
            .await
            .status,
            200,
            "case {blocker:?}: a refused delete must leave the zone in place"
        );

        // Remove the blocking record and the zone deletes.
        assert_eq!(
            submit_changes(&router, &zone_id, "DELETE", &[*blocker])
                .await
                .status,
            200
        );
        let deleted = call(
            &router,
            "DELETE",
            &format!("/2013-04-01/hostedzone/{zone_id}"),
            "",
        )
        .await;
        assert_eq!(
            deleted.status, 200,
            "case {blocker:?}: expected 200 once only the default records remain, got {} with \
             body {}",
            deleted.status, deleted.body
        );
        assert!(
            is_change_id(deleted.first("ChangeInfo/Id").unwrap_or("")),
            "case {blocker:?}: the delete must return a persisted change, got {}",
            deleted.body
        );

        // And the delete is observable by later reads.
        let gone = call(
            &router,
            "GET",
            &format!("/2013-04-01/hostedzone/{zone_id}"),
            "",
        )
        .await;
        assert_eq!(
            gone.status, 404,
            "case {blocker:?}: expected 404 after the delete, got {} with body {}",
            gone.status, gone.body
        );
        assert_eq!(
            gone.error_code(),
            "NoSuchHostedZone",
            "case {blocker:?}: expected NoSuchHostedZone, got {}",
            gone.body
        );
        let listed = call(&router, "GET", "/2013-04-01/hostedzone", "").await;
        assert!(
            !listed
                .all("HostedZone/Id")
                .contains(&format!("/hostedzone/{zone_id}").as_str()),
            "case {blocker:?}: the deleted zone must be gone from the listing, got {}",
            listed.body
        );
    }
}

#[tokio::test]
async fn every_zone_scoped_operation_reports_an_unknown_zone() {
    // The rule: an operation addressed at a zone id no zone has answers
    // NoSuchHostedZone (404). The rows are the zone-scoped operations of this
    // slice, each exercised with an id that was never minted.
    let router = router();
    let unknown = "Z00000000000000";
    let cases: [(&str, &str, &str, String); 4] = [
        ("GetHostedZone", "GET", "", format!("/2013-04-01/hostedzone/{unknown}")),
        (
            "ListResourceRecordSets",
            "GET",
            "",
            format!("/2013-04-01/hostedzone/{unknown}/rrset"),
        ),
        (
            "ChangeResourceRecordSets",
            "POST",
            "body",
            format!("/2013-04-01/hostedzone/{unknown}/rrset"),
        ),
        (
            "DeleteHostedZone",
            "DELETE",
            "",
            format!("/2013-04-01/hostedzone/{unknown}"),
        ),
    ];

    for (label, method, needs_body, uri) in &cases {
        let body = if needs_body.is_empty() {
            String::new()
        } else {
            change_batch_body(
                &change_element(
                    "CREATE",
                    &RecordRow {
                        name: "www.example.com",
                        rr_type: "A",
                        ttl: 300,
                        values: &["192.0.2.1"],
                    },
                ),
                None,
            )
        };
        let reply = call(&router, method, uri, &body).await;
        assert_eq!(
            reply.status, 404,
            "case {label}: expected 404, got {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "NoSuchHostedZone",
            "case {label}: expected NoSuchHostedZone, got {}",
            reply.body
        );
    }
}
