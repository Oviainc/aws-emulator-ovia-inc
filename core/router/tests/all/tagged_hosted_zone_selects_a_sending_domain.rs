// SPDX-License-Identifier: CC0-1.0

//! Finding one of the account's own DNS zones by tag, and sending from its apex.
//!
//! The rule under test: **a hosted zone the account created is enumerated by
//! `ListHostedZones` — across every `Marker`/`NextMarker` page, whatever page size the
//! caller asks for — reporting the `Config.PrivateZone` flag it was created with, and
//! `ListTagsForResource('hostedzone', <its id>)` reports exactly the tag map the last
//! `ChangeTagsForResource` batch left on it; so a client that pages the zones, keeps the
//! public ones and reads each one's tags resolves the zone carrying a selector tag, and
//! the SESv2 identity for that zone's apex reports itself verified for sending once the
//! verification the clock owes is due.**
//!
//! That is what every tag-driven discovery does — `route53 list-hosted-zones` plus
//! `list-tags-for-resource`, a Terraform/CDK data source, an inventory scan — and what a
//! mail sender does on top of it: resolve the account's verified domain from its own DNS
//! zones, then send from that apex.
//!
//! Two crates have to agree for the rule to hold (Route 53 enumerates and tags, SESv2
//! answers for the identity of the zone's apex), so the suite lives here: a crate-local
//! test only ever sees one side of it.
//!
//! The rows differ along what the rule generalises over: the number of labels in the zone
//! name and the case it was written in, how many tags the zone carries and the form of
//! their keys and values, whether a tag was added, edited or removed, whether the zone is
//! public or private, and the page size the enumeration is paged at (one zone per page up
//! to one page for all of them). The zone id and apex an issue happens to mention are one
//! row's values.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request here is signed for.
const REGION: &str = "us-east-1";

/// The endpoint host a Route 53 request addresses — Route 53 is global — which is also
/// the routing signal the router resolves that service from.
const ROUTE53_HOST: &str = "route53.amazonaws.com";

/// The `ResourceType` a hosted zone is tagged under (`TagResourceType$hostedzone`).
const HOSTED_ZONE: &str = "hostedzone";

/// How long a verification poll waits, and how often it looks.
///
/// The identity's `PENDING` -> `SUCCESS` move is owed on the emulator's own accelerated
/// schedule, so what is waited for takes well under a second; the deadline is only here so
/// a failure is a failure rather than a hang.
const DEADLINE: Duration = Duration::from_secs(30);
const POLL: Duration = Duration::from_millis(50);

/// The two refusals the ROUTER itself answers, which no row here may provoke.
const ROUTER_REFUSALS: &[&str] = &["AmbiguousService", "UnknownService"];

/// One zone of the account: how it is created, and the tag map it is left carrying.
struct ZoneRow {
    /// What this row is, for the assertion messages.
    case: &'static str,
    /// The name the zone is created with, in the spelling the request uses.
    name: &'static str,
    /// The VPC a PRIVATE zone is created for; `None` for a public zone.
    vpc: Option<(&'static str, &'static str)>,
    /// The first tag batch: `(AddTags, RemoveTagKeys)`.
    first_batch: (&'static [(&'static str, &'static str)], &'static [&'static str]),
    /// The second batch, which edits and removes what the first one wrote. Empty when
    /// the row applies one batch only.
    second_batch: (&'static [(&'static str, &'static str)], &'static [&'static str]),
    /// The whole tag map the zone carries afterwards, in tag-key order.
    expected_tags: &'static [(&'static str, &'static str)],
}

/// The tag key a discovery selects a zone by. One row carries it; the rule is about any
/// tag, so the rows spell their other keys differently.
const SELECTOR_KEY: &str = "dns-baseline";
const SELECTOR_VALUE: &str = "true";

/// The zones the account holds, spanning the forms the rule generalises over.
fn zone_rows() -> Vec<ZoneRow> {
    vec![
        ZoneRow {
            case: "a two-label public zone carrying the selector tag alone",
            name: "selected-apex.example.com",
            vpc: None,
            first_batch: (&[(SELECTOR_KEY, SELECTOR_VALUE)], &[]),
            second_batch: (&[], &[]),
            expected_tags: &[(SELECTOR_KEY, SELECTOR_VALUE)],
        },
        ZoneRow {
            case: "a three-label public zone carrying several tags",
            name: "mail.second-zone.example.net",
            vpc: None,
            first_batch: (
                &[
                    ("Owner", "dns-team"),
                    ("cost:centre", "4815"),
                    ("Managed_By", "an inventory scan"),
                ],
                &[],
            ),
            second_batch: (&[], &[]),
            expected_tags: &[
                ("Managed_By", "an inventory scan"),
                ("Owner", "dns-team"),
                ("cost:centre", "4815"),
            ],
        },
        ZoneRow {
            case: "a mixed-case public zone whose tags were edited and removed",
            name: "Third-Zone.Example.ORG",
            vpc: None,
            first_batch: (&[("Stage", "staging"), ("Temporary", "yes")], &[]),
            second_batch: (&[("Stage", "production")], &["Temporary"]),
            expected_tags: &[("Stage", "production")],
        },
        ZoneRow {
            case: "a private zone tagged the same way",
            name: "internal.fourth-zone.example",
            vpc: Some(("vpc-0123456789abcdef0", REGION)),
            first_batch: (&[(SELECTOR_KEY, SELECTOR_VALUE), ("Scope", "vpc")], &[]),
            second_batch: (&[], &[]),
            expected_tags: &[("Scope", "vpc"), (SELECTOR_KEY, SELECTOR_VALUE)],
        },
        ZoneRow {
            case: "a public zone nobody tagged",
            name: "untagged.fifth-zone.example.com",
            vpc: None,
            first_batch: (&[], &[]),
            second_batch: (&[], &[]),
            expected_tags: &[],
        },
    ]
}

/// The page sizes an enumeration is paged at: one zone per page, two, exactly the number
/// of zones, and the operation's own documented maximum (one page for all of them).
const PAGE_SIZES: &[i32] = &[1, 2, 5, 100];

/// The two services this suite exercises, over ONE engine exactly as
/// `core/router/src/lib.rs` mounts them.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // These rows exercise routing and handler behaviour, not SigV4.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_route53_awsdnsv20130401::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_sesv2_simpleemailservice_v2::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `signing_name` — the
/// signal the router resolves a routing key from. The signature itself is never checked.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=deadbeef"
    )
}

/// One answered request: its status and its body as text.
struct Answer {
    status: u16,
    text: String,
}

/// Send one request, refusing at the door the two answers the ROUTER itself produces: a
/// request that reached nobody, or one several services claimed.
async fn send(router: &MultiServiceRouter, case: &str, leg: &str, request: http::Request<Bytes>) -> Answer {
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let text = String::from_utf8_lossy(response.body()).to_string();
    for refusal in ROUTER_REFUSALS {
        assert!(
            !text.contains(refusal) && !error_type.contains(refusal),
            "case {case:?}: {leg} must be served by exactly one service, got HTTP {status}: \
             {text}"
        );
    }
    Answer { status, text }
}

/// Call one Route 53 operation over its restXml wire, requiring HTTP `expected`.
async fn route53(
    router: &MultiServiceRouter,
    case: &str,
    method: &str,
    uri: &str,
    body: &str,
    expected: u16,
) -> String {
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{ROUTE53_HOST}{uri}"))
        .header(http::header::HOST, ROUTE53_HOST)
        .header(http::header::AUTHORIZATION, authorization("route53"))
        .header(http::header::CONTENT_TYPE, "application/xml")
        .body(Bytes::from(body.to_owned()))
        .expect("a well-formed request");
    let answered = send(router, case, &format!("Route 53 {method} {uri}"), request).await;
    assert_eq!(
        answered.status, expected,
        "case {case:?}: Route 53 {method} {uri} expected HTTP {expected}, got {} with body {}",
        answered.status, answered.text
    );
    answered.text
}

/// Call one SESv2 operation over its restJson1 wire, requiring HTTP `expected`.
async fn sesv2(
    router: &MultiServiceRouter,
    case: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
    expected: u16,
) -> Value {
    let payload = match &body {
        Some(value) => Bytes::from(value.to_string()),
        None => Bytes::new(),
    };
    let host = format!("email.{REGION}.amazonaws.com");
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header(http::header::HOST, host)
        .header(http::header::AUTHORIZATION, authorization("ses"))
        .header(http::header::CONTENT_TYPE, "application/json")
        .body(payload)
        .expect("a well-formed request");
    let answered = send(router, case, &format!("SESv2 {method} {path}"), request).await;
    assert_eq!(
        answered.status, expected,
        "case {case:?}: SESv2 {method} {path} expected HTTP {expected}, got {} with body {}",
        answered.status, answered.text
    );
    serde_json::from_str(&answered.text).unwrap_or(Value::Null)
}

// ---------------------------------------------------------------------------
// The narrow XML reading these two restXml operations need
// ---------------------------------------------------------------------------

/// The text of the first `<name>…</name>` in `xml`, or `None` when it carries none.
/// `<name/>` reads as the empty string.
fn element(xml: &str, name: &str) -> Option<String> {
    if xml.contains(&format!("<{name}/>")) {
        return Some(String::new());
    }
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&close)? + start;
    Some(xml[start..end].replace("&amp;", "&").replace("&quot;", "\""))
}

/// Every `<name>…</name>` block of `xml`, in document order, without its own tags.
fn blocks(xml: &str, name: &str) -> Vec<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let mut found = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find(&open) {
        let from = start + open.len();
        let Some(offset) = rest[from..].find(&close) else {
            break;
        };
        found.push(rest[from..from + offset].to_string());
        rest = &rest[from + offset + close.len()..];
    }
    found
}

/// One enumerated hosted zone, as a client reads it off `ListHostedZones`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ListedZone {
    /// The zone id with the `/hostedzone/` prefix stripped, which is the form every
    /// `ResourceId` member takes — exactly what a client keeps "after the last '/'".
    id: String,
    /// The zone's name, as the service canonicalised it.
    name: String,
    /// `Config.PrivateZone`, which the enumeration always reports.
    private: bool,
}

/// Page `ListHostedZones` to the end at `page_size`, answering every zone it enumerated.
///
/// This IS the client-side procedure under test: follow `NextMarker` while `IsTruncated`
/// is true, exactly as the SDK paginator does.
async fn all_zones(router: &MultiServiceRouter, case: &str, page_size: i32) -> Vec<ListedZone> {
    let mut seen: Vec<ListedZone> = Vec::new();
    let mut marker: Option<String> = None;
    loop {
        let mut query = format!("?maxitems={page_size}");
        if let Some(marker) = &marker {
            query.push_str(&format!("&marker={marker}"));
        }
        let page = route53(
            router,
            case,
            "GET",
            &format!("/2013-04-01/hostedzone{query}"),
            "",
            200,
        )
        .await;
        for zone in blocks(&page, "HostedZone") {
            let id = element(&zone, "Id")
                .unwrap_or_else(|| panic!("case {case:?}: an enumerated zone must carry an Id: {zone}"));
            let private = element(&zone, "PrivateZone").unwrap_or_else(|| {
                panic!("case {case:?}: an enumerated zone must report Config.PrivateZone: {zone}")
            });
            seen.push(ListedZone {
                id: id.rsplit('/').next().unwrap_or_default().to_string(),
                name: element(&zone, "Name").unwrap_or_default(),
                private: private == "true",
            });
        }
        let truncated = element(&page, "IsTruncated").unwrap_or_default() == "true";
        let next = element(&page, "NextMarker");
        assert_eq!(
            truncated,
            next.is_some(),
            "case {case:?}: NextMarker is present exactly when IsTruncated is true, got \
             IsTruncated {truncated} and NextMarker {next:?}"
        );
        if !truncated {
            break;
        }
        marker = next;
        assert!(
            seen.len() < 1_000,
            "case {case:?}: the ListHostedZones pagination did not terminate, saw {seen:?}"
        );
    }
    seen
}

/// The tag map `ListTagsForResource` reports for one hosted zone.
async fn zone_tags(router: &MultiServiceRouter, case: &str, id: &str) -> BTreeMap<String, String> {
    let read = route53(
        router,
        case,
        "GET",
        &format!("/2013-04-01/tags/{HOSTED_ZONE}/{id}"),
        "",
        200,
    )
    .await;
    assert_eq!(
        element(&read, "ResourceId").as_deref(),
        Some(id),
        "case {case:?}: the tag set must name the zone it was read for, got {read}"
    );
    blocks(&read, "Tag")
        .iter()
        .map(|tag| {
            (
                element(tag, "Key").unwrap_or_default(),
                element(tag, "Value").unwrap_or_default(),
            )
        })
        .collect()
}

/// Apply one tag batch to a hosted zone. A batch that asks for nothing is not sent:
/// `AddTags` and `RemoveTagKeys` are both `@length(min: 1, …)`.
async fn change_tags(
    router: &MultiServiceRouter,
    case: &str,
    id: &str,
    add: &[(&str, &str)],
    remove: &[&str],
) {
    if add.is_empty() && remove.is_empty() {
        return;
    }
    let mut members = String::new();
    if !add.is_empty() {
        let tags: String = add
            .iter()
            .map(|(key, value)| format!("<Tag><Key>{key}</Key><Value>{value}</Value></Tag>"))
            .collect();
        members.push_str(&format!("<AddTags>{tags}</AddTags>"));
    }
    if !remove.is_empty() {
        let keys: String = remove.iter().map(|key| format!("<Key>{key}</Key>")).collect();
        members.push_str(&format!("<RemoveTagKeys>{keys}</RemoveTagKeys>"));
    }
    route53(
        router,
        case,
        "POST",
        &format!("/2013-04-01/tags/{HOSTED_ZONE}/{id}"),
        &format!(
            "<ChangeTagsForResourceRequest xmlns=\"https://route53.amazonaws.com/doc/2013-04-01/\">\
             {members}</ChangeTagsForResourceRequest>"
        ),
        200,
    )
    .await;
}

/// Create the zone `row` describes and answer its bare id.
async fn create_zone(router: &MultiServiceRouter, row: &ZoneRow) -> String {
    let vpc = match row.vpc {
        Some((id, region)) => {
            format!("<VPC><VPCId>{id}</VPCId><VPCRegion>{region}</VPCRegion></VPC>")
        }
        None => String::new(),
    };
    let created = route53(
        router,
        row.case,
        "POST",
        "/2013-04-01/hostedzone",
        &format!(
            "<CreateHostedZoneRequest xmlns=\"https://route53.amazonaws.com/doc/2013-04-01/\">\
             <Name>{}</Name>{vpc}<CallerReference>{}</CallerReference>\
             <HostedZoneConfig><Comment>created by {}</Comment></HostedZoneConfig>\
             </CreateHostedZoneRequest>",
            row.name, row.case, row.case
        ),
        201,
    )
    .await;
    let id = element(&created, "Id")
        .unwrap_or_else(|| panic!("case {:?}: the create must answer HostedZone.Id: {created}", row.case));
    id.rsplit('/').next().unwrap_or_default().to_string()
}

/// Create every row's zone, apply its tag batches, and answer the bare id of each.
async fn create_the_account_zones(router: &MultiServiceRouter, rows: &[ZoneRow]) -> Vec<String> {
    let mut ids = Vec::new();
    for row in rows {
        let id = create_zone(router, row).await;
        let (add, remove) = row.first_batch;
        change_tags(router, row.case, &id, add, remove).await;
        let (add, remove) = row.second_batch;
        change_tags(router, row.case, &id, add, remove).await;
        ids.push(id);
    }
    ids
}

/// `expected` as the owned map a tag read is compared against.
fn owned(expected: &[(&str, &str)]) -> BTreeMap<String, String> {
    expected
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

#[tokio::test]
async fn the_enumeration_reports_the_zones_the_account_created_and_no_others() {
    // The other half of the rule: `ListHostedZones` reports the account's OWN hosted
    // zones, so an account that has created none enumerates none — a zone no request
    // created is a zone no read may invent — and the first create is the first zone the
    // enumeration carries.
    let router = build_router();

    let case = "an account that has created no hosted zone";
    let empty = route53(&router, case, "GET", "/2013-04-01/hostedzone", "", 200).await;
    assert!(
        blocks(&empty, "HostedZone").is_empty(),
        "case {case:?}: the enumeration must carry no zone, got {empty}"
    );
    assert_eq!(
        element(&empty, "IsTruncated").as_deref(),
        Some("false"),
        "case {case:?}: an exhausted enumeration is not truncated, got {empty}"
    );
    assert_eq!(
        all_zones(&router, case, 1).await,
        Vec::new(),
        "case {case:?}: paging an empty enumeration carries no zone"
    );

    let case = "the one zone the account then creates";
    let rows = zone_rows();
    let id = create_zone(&router, &rows[0]).await;
    let listed = all_zones(&router, case, 1).await;
    assert_eq!(
        listed.len(),
        1,
        "case {case:?}: the enumeration carries exactly the one created zone, got {listed:?}"
    );
    assert_eq!(
        listed[0].id, id,
        "case {case:?}: the enumerated zone is the one the create minted, got {listed:?}"
    );
    let tags = zone_tags(&router, case, &id).await;
    assert!(
        tags.is_empty(),
        "case {case:?}: a zone nobody has tagged reports an empty tag set, got {tags:?}"
    );
}

#[tokio::test]
async fn every_created_zone_is_enumerated_at_every_page_size_with_the_tags_written_on_it() {
    let router = build_router();
    let rows = zone_rows();
    let ids = create_the_account_zones(&router, &rows).await;

    for &page_size in PAGE_SIZES {
        let case = format!("paged {page_size} zones at a time");
        let listed = all_zones(&router, &case, page_size).await;
        assert_eq!(
            listed.len(),
            rows.len(),
            "case {case:?}: the account holds {} zones, and the enumeration carried {}: \
             {listed:?}",
            rows.len(),
            listed.len()
        );
        // Every id appears exactly once, however the pages fell.
        let mut unique: Vec<String> = listed.iter().map(|zone| zone.id.clone()).collect();
        unique.sort();
        unique.dedup();
        assert_eq!(
            unique.len(),
            listed.len(),
            "case {case:?}: no zone may be enumerated twice, got {listed:?}"
        );

        for (row, id) in rows.iter().zip(&ids) {
            let found = listed
                .iter()
                .find(|zone| &zone.id == id)
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: the zone {id} created for {:?} must be enumerated, got \
                         {listed:?}",
                        row.case
                    )
                });
            assert_eq!(
                found.private,
                row.vpc.is_some(),
                "case {case:?}: zone {id} ({:?}) must report PrivateZone {}, got {found:?}",
                row.case,
                row.vpc.is_some()
            );
            // The enumerated name is the one the create canonicalised, which the
            // comparison a DNS client makes is case-insensitive about.
            assert_eq!(
                found.name.trim_end_matches('.').to_ascii_lowercase(),
                row.name.trim_end_matches('.').to_ascii_lowercase(),
                "case {case:?}: zone {id} ({:?}) must be enumerated under the name it was \
                 created with, got {found:?}",
                row.case
            );
            let reported = zone_tags(&router, &case, id).await;
            assert_eq!(
                reported,
                owned(row.expected_tags),
                "case {case:?}: zone {id} ({:?}) must report the tags written on it, got \
                 {reported:?}",
                row.case
            );
        }
    }
}

#[tokio::test]
async fn a_tag_selected_public_zone_resolves_to_a_domain_verified_for_sending() {
    let router = build_router();
    let rows = zone_rows();
    create_the_account_zones(&router, &rows).await;

    // The discovery under test, at every page size: page the zones, keep the PUBLIC
    // ones, read each one's tags, and select the zone carrying the selector tag.
    let mut selected_id = String::new();
    for &page_size in PAGE_SIZES {
        let case = format!("selecting by tag over pages of {page_size}");
        let mut selected: Vec<ListedZone> = Vec::new();
        for zone in all_zones(&router, &case, page_size).await {
            if zone.private {
                continue;
            }
            let tags = zone_tags(&router, &case, &zone.id).await;
            if tags.get(SELECTOR_KEY).map(String::as_str) == Some(SELECTOR_VALUE) {
                selected.push(zone);
            }
        }
        assert_eq!(
            selected.len(),
            1,
            "case {case:?}: exactly one PUBLIC zone carries {SELECTOR_KEY}={SELECTOR_VALUE} \
             (the private zone carrying it is not one), got {selected:?}"
        );
        let apex = selected[0].name.trim_end_matches('.').to_string();
        assert_eq!(
            apex, "selected-apex.example.com",
            "case {case:?}: the selected zone's apex must be the one tagged with the \
             selector, got {apex:?}"
        );
        if selected_id.is_empty() {
            selected_id = selected[0].id.clone();
        }
        assert_eq!(
            selected[0].id, selected_id,
            "case {case:?}: the page size cannot change WHICH zone the selector resolves to"
        );
    }

    // And the apex of the selected zone is a domain the account can send from: the
    // identity is created for it, and once the verification the clock owes is due the
    // identity reports itself verified for sending with its DKIM set up.
    let case = "the selected zone's apex as a sending identity";
    let apex = "selected-apex.example.com";
    let created = sesv2(
        &router,
        case,
        "POST",
        "/v2/email/identities",
        Some(json!({ "EmailIdentity": apex })),
        200,
    )
    .await;
    assert_eq!(
        created["IdentityType"], json!("DOMAIN"),
        "case {case:?}: the apex of a hosted zone is a DOMAIN identity, got {created}"
    );

    // Easy DKIM tells the owner which CNAMEs to publish in the domain's own zone, and
    // that zone is the one just selected — so the records go in through
    // `ChangeResourceRecordSets`, each at a name under the apex, and the batch's change
    // reaches INSYNC. This is the leg where a record-set rule could refuse a name whose
    // first label begins with an underscore, so the assertion is that it does not.
    let tokens: Vec<String> = created["DkimAttributes"]["Tokens"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: Easy DKIM must answer its tokens: {created}"))
        .iter()
        .map(|token| token.as_str().unwrap_or_default().to_string())
        .collect();
    assert!(
        !tokens.is_empty(),
        "case {case:?}: Easy DKIM must answer its tokens: {created}"
    );
    let mut changes = String::new();
    for token in &tokens {
        changes.push_str(&format!(
            "<Change><Action>UPSERT</Action><ResourceRecordSet>\
             <Name>{token}._domainkey.{apex}</Name><Type>CNAME</Type><TTL>1800</TTL>\
             <ResourceRecords><ResourceRecord><Value>{token}.dkim.amazonses.com</Value>\
             </ResourceRecord></ResourceRecords></ResourceRecordSet></Change>"
        ));
    }
    let submitted = route53(
        &router,
        case,
        "POST",
        &format!("/2013-04-01/hostedzone/{selected_id}/rrset"),
        &format!(
            "<ChangeResourceRecordSetsRequest \
             xmlns=\"https://route53.amazonaws.com/doc/2013-04-01/\">\
             <ChangeBatch><Comment>the identity's Easy DKIM records</Comment>\
             <Changes>{changes}</Changes></ChangeBatch>\
             </ChangeResourceRecordSetsRequest>"
        ),
        200,
    )
    .await;
    let change_id = element(&submitted, "Id")
        .unwrap_or_else(|| panic!("case {case:?}: the batch must answer ChangeInfo.Id: {submitted}"));
    let change_id = change_id.rsplit('/').next().unwrap_or_default().to_string();

    // The change the batch submitted propagates, which is what a client waits for before
    // it treats the records as published.
    let deadline = Instant::now() + DEADLINE;
    loop {
        let read = route53(
            &router,
            case,
            "GET",
            &format!("/2013-04-01/change/{change_id}"),
            "",
            200,
        )
        .await;
        let status = element(&read, "Status").unwrap_or_default();
        if status == "INSYNC" {
            break;
        }
        assert_eq!(
            status, "PENDING",
            "case {case:?}: a submitted change is PENDING until it is INSYNC, got {read}"
        );
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the change {change_id} must reach INSYNC"
        );
        tokio::time::sleep(POLL).await;
    }

    // And the published records are in the zone a later reader walks.
    let listed = route53(
        &router,
        case,
        "GET",
        &format!("/2013-04-01/hostedzone/{selected_id}/rrset"),
        "",
        200,
    )
    .await;
    for token in &tokens {
        let published = format!("{token}._domainkey.{apex}");
        assert!(
            listed.to_ascii_lowercase().contains(&published.to_ascii_lowercase()),
            "case {case:?}: the zone must hold the published record {published}, got {listed}"
        );
    }

    let deadline = Instant::now() + DEADLINE;
    let read = loop {
        let read = sesv2(
            &router,
            case,
            "GET",
            &format!("/v2/email/identities/{apex}"),
            None,
            200,
        )
        .await;
        if read["VerifiedForSendingStatus"] == json!(true) {
            break read;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the identity for {apex} must reach VerifiedForSendingStatus true, \
             last read {read}"
        );
        tokio::time::sleep(POLL).await;
    };
    assert_eq!(
        read["DkimAttributes"]["Status"], json!("SUCCESS"),
        "case {case:?}: a verified domain identity reports its DKIM set up, got {read}"
    );
}
