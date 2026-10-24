// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the AWS Outposts site + Outpost lifecycle, driven over the
//! wire.
//!
//! Every case here goes through the real front door — `MultiServiceRouter::dispatch`
//! with a restJson1 request, exactly as a signed `aws outposts` call arrives — so what
//! it asserts is the emulator's HTTP behaviour, not a handler called directly.
//!
//! The rules under test, each stated once and then exercised over a SET of inputs:
//!
//! 1. A create mints an identifier of the form its model declares, persists the
//!    resource, and answers it from what was persisted.
//! 2. A read answers the stored record field for field, whether the caller named it
//!    by bare id or by full ARN — the two forms the modelled `@httpLabel` pattern
//!    `^(arn:…/)?<prefix>-[a-f0-9]{17}$` accepts.
//! 3. A list answers every persisted resource that matches its filters, in one
//!    stable order, cut into pages of at most `MaxResults` with a `NextToken` that
//!    resumes exactly where the previous page stopped — for any page size.
//! 4. A delete makes the resource observably gone to every later call, and is
//!    refused as a bad request (400 `ValidationException`) while another resource
//!    still references it.
//! 5. An identifier that names nothing is 404 `NotFoundException`; one that is not an
//!    identifier at all is 400 `ValidationException`.
//!
//! What a projection REPORTS — the tag map, the two Availability Zone identities and
//! the lifecycle status — is stated and exercised in
//! [`crate::site_and_outpost_projection`], which reuses this suite's harness.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_outposts_outpostsolafservice::EmulatedService;

/// The region every call that does not name its own is addressed to, and the region
/// the ARN prefixes below are written in.
pub(crate) const HOME_REGION: &str = "us-east-1";

/// A SigV4 `Authorization` header addressed to `region`: how a request names the
/// service and the region it is for. The signature itself is never checked here —
/// request authentication is off unless a router turns it on — but the credential
/// scope's region is what the service reads, and an Availability Zone is only usable
/// in its own region, so a case that names a zone has to name its region too.
pub(crate) fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTOUTPOSTS/20260101/{region}/outposts/aws4_request, \
         SignedHeaders=host;x-amz-date, \
         Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// The ARN prefix every site this emulator mints must carry, from the modelled
/// `SiteArn` pattern `^arn:aws([a-z-]+)?:outposts:[a-z\d-]+:\d{12}:site/(os-[a-f0-9]{17})$`
/// plus the canonical emulator account id.
pub(crate) const SITE_ARN_PREFIX: &str = "arn:aws:outposts:us-east-1:000000000000:site/";
/// The same, for the modelled `OutpostArn` pattern.
pub(crate) const OUTPOST_ARN_PREFIX: &str = "arn:aws:outposts:us-east-1:000000000000:outpost/";
/// The account id every generated emulator presents, which `Site$AccountId` and
/// `Outpost$OwnerId` (both `^\d{12}$`) carry.
const ACCOUNT_ID: &str = "000000000000";

/// An id this emulator never mints, used wherever a case needs an identifier that is
/// well formed (so the boundary accepts it) but names nothing.
pub(crate) const ABSENT_SITE_ID: &str = "os-00000000000000000";
/// See [`ABSENT_SITE_ID`].
pub(crate) const ABSENT_OUTPOST_ID: &str = "op-00000000000000000";

/// One Outposts emulator over one shared engine, reached the way the router reaches it.
pub(crate) struct Outposts {
    router: MultiServiceRouter,
    /// The ONE engine this emulator was built over — the same store every handler
    /// of it writes to.
    ///
    /// Kept so a case can reach this service's own records through
    /// `state::service_state`, which is the only way to age a stored resource whose
    /// clock-owed transition falls further out than a test may sleep for, or to put
    /// the store at an entry ceiling a test cannot fill one call at a time. Nothing
    /// else reads it: every assertion about behaviour goes over the wire.
    engine: Arc<engine::InMemoryEngine>,
}

/// One answer from the emulator: the status, the wire error code as BOTH restJson1
/// carries it (the body's `__type` discriminator and the `x-amzn-errortype` header,
/// which is how restJson1 resolves an error shape), and the parsed body.
pub(crate) struct Reply {
    pub(crate) status: u16,
    pub(crate) error_code: String,
    pub(crate) errortype_header: String,
    pub(crate) body: Value,
}

/// Wait until a clock-owed Outpost lifecycle transition has fallen due.
///
/// `CreateOutpost` answers with a provisioning Outpost and the service materialises
/// its `PENDING` -> `ACTIVE` transition at the instant it stored, so a case that
/// wants settled Outposts waits past that span (plus a margin, so the case is not
/// racing it). Blocking with `std::thread::sleep` rather than `tokio::time::sleep`:
/// the workspace's `tokio` carries only the `macros`/`rt` features, so the timer
/// driver `tokio::time` needs is not compiled in.
pub(crate) fn settle() {
    std::thread::sleep(
        service_com_amazonaws_outposts_outpostsolafservice::state::OUTPOST_PROVISIONING
            + std::time::Duration::from_millis(400),
    );
}

impl Outposts {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// This service's store in that shared engine. See [`Outposts::engine`].
    pub(crate) fn store(
        &self,
    ) -> ::engine::ServiceState<
        service_com_amazonaws_outposts_outpostsolafservice::ComAmazonawsOutpostsOutpostsolafserviceState,
    > {
        service_com_amazonaws_outposts_outpostsolafservice::state::service_state(&*self.engine)
            .expect("the Outposts store is reachable through the shared engine")
    }

    /// Dispatch one restJson1 request at `method path`, addressed to `region`, with
    /// `body` as the JSON payload (absent for the GET and DELETE routes, which bind
    /// every member to the URI).
    pub(crate) async fn call_in(
        &self,
        region: &str,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> Reply {
        let payload = body.map(Value::to_string).unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://outposts.{region}.amazonaws.com{path}"))
            .header("content-type", "application/json")
            .header("authorization", authorization(region))
            .body(bytes::Bytes::from(payload))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let errortype_header = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let parsed: Value = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        let error_code = parsed
            .get("__type")
            .and_then(Value::as_str)
            .map(|code| code.rsplit('#').next().unwrap_or(code).to_string())
            .unwrap_or_default();
        Reply {
            status,
            error_code,
            errortype_header,
            body: parsed,
        }
    }

    /// Dispatch in [`HOME_REGION`], which every resource a case does not place
    /// itself is created in.
    pub(crate) async fn call(&self, method: &str, path: &str, body: Option<&Value>) -> Reply {
        self.call_in(HOME_REGION, method, path, body).await
    }

    /// Dispatch `method path` in `region` and require HTTP 200, returning the body.
    pub(crate) async fn ok_in(
        &self,
        region: &str,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> Value {
        let reply = self.call_in(region, method, path, body).await;
        assert_eq!(
            reply.status, 200,
            "case {method} {path} in {region}: expected HTTP 200, got {} {}",
            reply.status, reply.body
        );
        reply.body
    }

    /// Dispatch `method path` and require HTTP 200, returning the body.
    pub(crate) async fn ok(&self, method: &str, path: &str, body: Option<&Value>) -> Value {
        self.ok_in(HOME_REGION, method, path, body).await
    }

    /// `CreateSite`, returning the `Site` it answered with.
    pub(crate) async fn create_site(&self, request: &Value) -> Value {
        self.ok("POST", "/sites", Some(request)).await["Site"].clone()
    }

    /// `CreateOutpost`, returning the `Outpost` it answered with.
    pub(crate) async fn create_outpost(&self, request: &Value) -> Value {
        self.create_outpost_in(HOME_REGION, request).await
    }

    /// `CreateOutpost` addressed to `region`, returning the `Outpost` it answered
    /// with. An Outpost's Availability Zone has to be one of its own region's, so a
    /// case naming a zone names the region that zone is in.
    pub(crate) async fn create_outpost_in(&self, region: &str, request: &Value) -> Value {
        self.ok_in(region, "POST", "/outposts", Some(request)).await["Outpost"].clone()
    }

    /// `GetSite`, returning the `Site` it answered with.
    pub(crate) async fn get_site(&self, identifier: &str) -> Value {
        self.ok("GET", &format!("/sites/{}", segment(identifier)), None)
            .await["Site"]
            .clone()
    }

    /// `GetOutpost`, returning the `Outpost` it answered with.
    pub(crate) async fn get_outpost(&self, identifier: &str) -> Value {
        self.ok("GET", &format!("/outposts/{}", segment(identifier)), None)
            .await["Outpost"]
            .clone()
    }

    /// `ListOutposts` with the given query string, returning the whole output.
    pub(crate) async fn list_outposts(&self, query: &str) -> Value {
        self.ok("GET", &format!("/outposts{query}"), None).await
    }

    /// `ListSites` with the given query string, returning the whole output.
    pub(crate) async fn list_sites(&self, query: &str) -> Value {
        self.ok("GET", &format!("/sites{query}"), None).await
    }

    /// Dispatch every [`Refusal`] in a table and require the status and wire error
    /// code each one names.
    pub(crate) async fn expect_refusals(&self, cases: &[Refusal]) {
        for expected in cases {
            let reply = self
                .call(expected.method, &expected.path, expected.body.as_ref())
                .await;
            assert_eq!(
                (reply.status, reply.error_code.as_str()),
                (expected.status, expected.code),
                "case {:?} ({} {}): expected {} {}, got {} {} {:?}",
                expected.case,
                expected.method,
                expected.path,
                expected.status,
                expected.code,
                reply.status,
                reply.error_code,
                reply.body
            );
        }
    }
}

/// Percent-encode a value for use as ONE `@httpLabel` path segment.
///
/// A non-greedy Smithy label is encoded including `/`, which is what lets the full
/// ARN form of `SiteId` / `OutpostId` travel in a single-segment route — exactly what
/// an AWS SDK puts on the wire.
pub(crate) fn segment(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// Percent-encode a value for use as ONE query-string parameter value.
pub(crate) fn query_value(value: &str) -> String {
    segment(value)
}

/// A string member, read as the empty string when absent — which is what a client
/// sees for a member the wire omits.
pub(crate) fn text(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Whether `id` is `<prefix>-` plus exactly 17 lowercase hex characters, the form
/// `SiteId` (`^os-[a-f0-9]{17}$`), `OutpostId` (`^op-[a-f0-9]{17}$`) and `QuoteId`
/// (`^oq-[a-f0-9]{17}$`) each declare.
pub(crate) fn is_minted_id(id: &str, prefix: &str) -> bool {
    match id.strip_prefix(prefix).and_then(|id| id.strip_prefix('-')) {
        Some(hex) => {
            hex.len() == 17 && hex.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        }
        None => false,
    }
}

/// A whole, valid `Address` whose operating-address projection is
/// `country`/`state`/`city`.
///
/// Every member the modelled `Address` marks `@required` is present, including the
/// two the API requires but `Site` does not project (`ContactName`,
/// `ContactPhoneNumber`), so the boundary accepts the address.
pub(crate) fn address(country: &str, state: &str, city: &str) -> Value {
    json!({
        "ContactName": "Sam Site",
        "ContactPhoneNumber": "+12065550100",
        "AddressLine1": "410 Terry Ave N",
        "City": city,
        "StateOrRegion": state,
        "PostalCode": "98109",
        "CountryCode": country,
    })
}

/// Rule 1 + rule 2 for sites: a `CreateSite` mints a `SiteId`/`SiteArn` of the
/// modelled form, stores what the caller sent, and `GetSite` answers that same
/// record — by bare id and by ARN alike.
///
/// The rows differ along everything `CreateSite` generalises over: how many tags
/// (none, one, several), whether the optional description, notes, rack properties and
/// each address are present, and the country/state/city the operating address
/// projects into `Site`.
#[tokio::test]
async fn create_site_persists_and_both_identifier_forms_read_it_back() {
    let outposts = Outposts::new();
    let cases: Vec<(&str, Value)> = vec![
        (
            "name only",
            json!({ "Name": "bootstrap-site" }),
        ),
        (
            "every optional member, one tag",
            json!({
                "Name": "seattle site",
                "Description": "the first site",
                "Notes": "badge required\nescort to the cage",
                "Tags": { "Env": "dev" },
                "OperatingAddress": address("US", "WA", "Seattle"),
                "ShippingAddress": address("US", "WA", "Kirkland"),
                "RackPhysicalProperties": {
                    "PowerDrawKva": "POWER_10_KVA",
                    "PowerPhase": "THREE_PHASE",
                    "PowerConnector": "AH530P7W",
                    "PowerFeedDrop": "ABOVE_RACK",
                    "UplinkGbps": "UPLINK_40G",
                    "UplinkCount": "UPLINK_COUNT_4",
                    "FiberOpticCableType": "SINGLE_MODE",
                    "OpticalStandard": "OPTIC_40GBASE_LR4",
                    "MaximumSupportedWeightLbs": "MAX_1800_LBS",
                },
            }),
        ),
        (
            "several tags, address in another country",
            json!({
                "Name": "dublin site",
                "Tags": { "Env": "prod", "team": "platform", "cost.center": "4711" },
                "OperatingAddress": address("IE", "Leinster", "Dublin"),
            }),
        ),
        (
            "shipping address only, so no operating projection",
            json!({
                "Name": "ship here, operate later",
                "ShippingAddress": address("US", "OR", "Portland"),
            }),
        ),
        (
            "name at the modelled 1000-character maximum",
            json!({ "Name": "n".repeat(1000) }),
        ),
    ];

    for (case, request) in &cases {
        let site = outposts.create_site(request).await;
        let site_id = text(&site, "SiteId");
        assert!(
            is_minted_id(&site_id, "os"),
            "case {case:?}: SiteId must be os- plus 17 hex chars, got {site_id:?}"
        );
        assert_eq!(
            text(&site, "SiteArn"),
            format!("{SITE_ARN_PREFIX}{site_id}"),
            "case {case:?}: SiteArn must name the minted SiteId, got {site:?}"
        );
        assert_eq!(
            text(&site, "AccountId"),
            ACCOUNT_ID,
            "case {case:?}: AccountId must be the emulator account, got {site:?}"
        );
        for member in ["Name", "Description", "Notes"] {
            assert_eq!(
                site.get(member),
                request.get(member),
                "case {case:?}: {member} must come back as it was stored, got {site:?}"
            );
        }
        // Every projection of a taggable resource reports its tag map, empty when
        // the caller supplied none (see `tags_projection` in `site_and_outpost_projection`).
        assert_eq!(
            site.get("Tags"),
            Some(request.get("Tags").cloned().unwrap_or_else(|| json!({}))).as_ref(),
            "case {case:?}: Tags must come back as they were stored, got {site:?}"
        );
        assert_eq!(
            site.get("RackPhysicalProperties"),
            request.get("RackPhysicalProperties"),
            "case {case:?}: RackPhysicalProperties must round-trip, got {site:?}"
        );
        // `Site` projects only the operating address, and only its country, state
        // and city — never the shipping address.
        let operating = request.get("OperatingAddress");
        for (member, source) in [
            ("OperatingAddressCountryCode", "CountryCode"),
            ("OperatingAddressStateOrRegion", "StateOrRegion"),
            ("OperatingAddressCity", "City"),
        ] {
            assert_eq!(
                site.get(member),
                operating.and_then(|address| address.get(source)),
                "case {case:?}: {member} must come from OperatingAddress.{source}, got {site:?}"
            );
        }

        // Rule 2: both identifier forms name the one stored record.
        assert_eq!(
            outposts.get_site(&site_id).await,
            site,
            "case {case:?}: GetSite by id must answer the stored site"
        );
        assert_eq!(
            outposts.get_site(&text(&site, "SiteArn")).await,
            site,
            "case {case:?}: GetSite by ARN must answer the same stored site"
        );
    }

    // Every case created exactly one distinct site, and the list reports all of them.
    let listed = outposts.list_sites("").await["Sites"]
        .as_array()
        .expect("ListSites answers a list")
        .len();
    assert_eq!(
        listed,
        cases.len(),
        "ListSites must report every created site exactly once, got {listed}"
    );
}

/// Rule 1 + rule 2 for Outposts, plus the `LifeCycleStatus` `CreateOutpost` derives.
///
/// The rows differ along the supported hardware type (each modelled enum value, and
/// omitted), whether the optional description, availability zone, zone id and tags
/// are present, and the site the Outpost is attached to — named by bare id in some
/// rows and by site ARN in others, since `SiteId` accepts both.
#[tokio::test]
async fn create_outpost_persists_against_its_site_and_reads_back() {
    let outposts = Outposts::new();
    let site = outposts
        .create_site(&json!({
            "Name": "host site",
            "OperatingAddress": address("US", "WA", "Seattle"),
        }))
        .await;
    let site_id = text(&site, "SiteId");
    let site_arn = text(&site, "SiteArn");

    // `SupportedHardwareType`'s value set, from the modelled enum
    // `com.amazonaws.outposts#SupportedHardwareType` (RACK, SERVER); `None` is the
    // member omitted, which takes the documented rack default.
    let hardware_types: [Option<&str>; 3] = [Some("RACK"), Some("SERVER"), None];
    let mut cases: Vec<(String, Value)> = Vec::new();
    for hardware in hardware_types {
        let mut request = json!({ "Name": format!("outpost-{hardware:?}"), "SiteId": site_id });
        if let Some(hardware) = hardware {
            request["SupportedHardwareType"] = json!(hardware);
        }
        cases.push((format!("hardware {hardware:?}"), request));
    }
    cases.push((
        "every optional member, site named by ARN".to_string(),
        json!({
            "Name": "fully specified",
            "SiteId": site_arn,
            "Description": "the busy one",
            "AvailabilityZone": "us-east-1a",
            "AvailabilityZoneId": "use1-az1",
            "Tags": { "Env": "dev", "team": "platform" },
            "SupportedHardwareType": "RACK",
        }),
    ));
    cases.push((
        "empty description, another zone of the request's region".to_string(),
        json!({
            "Name": "second zone outpost",
            "SiteId": site_id,
            "Description": "",
            "AvailabilityZone": "us-east-1b",
            "AvailabilityZoneId": "use1-az2",
        }),
    ));

    for (case, request) in &cases {
        let outpost = outposts.create_outpost(request).await;
        let outpost_id = text(&outpost, "OutpostId");
        assert!(
            is_minted_id(&outpost_id, "op"),
            "case {case:?}: OutpostId must be op- plus 17 hex chars, got {outpost_id:?}"
        );
        assert_eq!(
            text(&outpost, "OutpostArn"),
            format!("{OUTPOST_ARN_PREFIX}{outpost_id}"),
            "case {case:?}: OutpostArn must name the minted OutpostId, got {outpost:?}"
        );
        assert_eq!(
            text(&outpost, "OwnerId"),
            ACCOUNT_ID,
            "case {case:?}: OwnerId must be the emulator account, got {outpost:?}"
        );
        // Whichever form the request named the site by, the stored Outpost carries
        // the site's bare id and its ARN.
        assert_eq!(
            text(&outpost, "SiteId"),
            site_id,
            "case {case:?}: SiteId must resolve to the stored site, got {outpost:?}"
        );
        assert_eq!(
            text(&outpost, "SiteArn"),
            site_arn,
            "case {case:?}: SiteArn must be the stored site's ARN, got {outpost:?}"
        );
        assert_eq!(
            text(&outpost, "LifeCycleStatus"),
            "PENDING",
            "case {case:?}: a just-created Outpost is still provisioning, got {outpost:?}"
        );
        for member in [
            "Name",
            "Description",
            "AvailabilityZone",
            "AvailabilityZoneId",
            "SupportedHardwareType",
        ] {
            assert_eq!(
                outpost.get(member),
                request.get(member),
                "case {case:?}: {member} must come back as it was stored, got {outpost:?}"
            );
        }
        assert_eq!(
            outpost.get("Tags"),
            Some(request.get("Tags").cloned().unwrap_or_else(|| json!({}))).as_ref(),
            "case {case:?}: Tags must come back as they were stored, got {outpost:?}"
        );

        // Rule 2: both identifier forms name the one stored record. The read is
        // compared without `LifeCycleStatus`, which the clock moves on its own
        // schedule (see `site_and_outpost_projection`).
        for identifier in [outpost_id.clone(), text(&outpost, "OutpostArn")] {
            let read = outposts.get_outpost(&identifier).await;
            assert_eq!(
                without(&read, "LifeCycleStatus"),
                without(&outpost, "LifeCycleStatus"),
                "case {case:?}: GetOutpost on {identifier:?} must answer the stored Outpost"
            );
            let status = text(&read, "LifeCycleStatus");
            assert!(
                ["PENDING", "ACTIVE"].contains(&status.as_str()),
                "case {case:?}: GetOutpost on {identifier:?} reports a provisioning or a \
                 settled Outpost, got {status:?}"
            );
        }
    }
}

/// The same value with one member removed, for a comparison that must not depend on
/// a member the clock owns.
pub(crate) fn without(value: &Value, member: &str) -> Value {
    let mut copy = value.clone();
    if let Some(object) = copy.as_object_mut() {
        object.remove(member);
    }
    copy
}

/// Rule 3: for EVERY page size, paging a list from the first `NextToken` to the last
/// yields exactly the persisted resources, once each, in one stable order.
///
/// The page sizes span one item per page, an exact divisor of the count, a size that
/// leaves a short last page, the count itself, and more than the count — so a token
/// that duplicated or skipped an item at any boundary would show up.
#[tokio::test]
async fn listing_pages_cover_every_persisted_resource_exactly_once() {
    let outposts = Outposts::new();
    let site = outposts.create_site(&json!({ "Name": "paging site" })).await;
    let site_id = text(&site, "SiteId");

    let mut created: Vec<String> = Vec::new();
    for index in 0..5 {
        let outpost = outposts
            .create_outpost(&json!({
                "Name": format!("paged-{index}"),
                "SiteId": site_id,
                "AvailabilityZone": "us-east-1a",
            }))
            .await;
        created.push(text(&outpost, "OutpostId"));
    }

    // One page, no token: the whole store.
    let all = outposts.list_outposts("").await;
    assert!(
        all.get("NextToken").is_none(),
        "a page that exhausts the list issues no NextToken, got {all:?}"
    );
    assert_eq!(
        ids(&all, "Outposts"),
        created,
        "ListOutposts must report every persisted Outpost in creation order"
    );

    for size in [1usize, 2, 3, 5, 9] {
        let mut collected: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let query = match &token {
                Some(token) => format!("?MaxResults={size}&NextToken={}", query_value(token)),
                None => format!("?MaxResults={size}"),
            };
            let page = outposts.list_outposts(&query).await;
            let page_ids = ids(&page, "Outposts");
            assert!(
                page_ids.len() <= size,
                "case size {size}: a page holds at most MaxResults items, got {page:?}"
            );
            collected.extend(page_ids);
            pages += 1;
            assert!(
                pages <= created.len() + 1,
                "case size {size}: paging must terminate, got {collected:?}"
            );
            match page.get("NextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            collected, created,
            "case size {size}: paging must yield every Outpost once, in creation order"
        );
        assert_eq!(
            collected.iter().collect::<BTreeSet<_>>().len(),
            created.len(),
            "case size {size}: paging must not duplicate an Outpost, got {collected:?}"
        );
    }

    // A token stamped for one list is not a boundary of another.
    let sites_page = outposts.list_sites("?MaxResults=1").await;
    let site_token = sites_page
        .get("NextToken")
        .and_then(Value::as_str)
        .unwrap_or("1##sites");
    let replayed = outposts
        .call(
            "GET",
            &format!("/outposts?NextToken={}", query_value(site_token)),
            None,
        )
        .await;
    assert_eq!(
        (replayed.status, replayed.error_code.as_str()),
        (400, "ValidationException"),
        "a ListSites token replayed on ListOutposts is refused, got {:?}",
        replayed.body
    );
}

/// Rule 3's filters: each `*Filter` accepts a LIST of values and narrows the list to
/// the records whose member is one of them; an absent filter matches everything.
#[tokio::test]
async fn listing_filters_select_only_matching_stored_records() {
    let outposts = Outposts::new();
    // Two sites in different cities, so the ListSites filters have something to cut.
    let seattle = outposts
        .create_site(&json!({
            "Name": "seattle",
            "OperatingAddress": address("US", "WA", "Seattle"),
        }))
        .await;
    let dublin = outposts
        .create_site(&json!({
            "Name": "dublin",
            "OperatingAddress": address("IE", "Leinster", "Dublin"),
        }))
        .await;

    // Three zones of the region these requests are addressed to (us-east-1), since
    // an Outpost can only be placed in a zone of its own region.
    let zones = [
        ("us-east-1a", "use1-az1"),
        ("us-east-1b", "use1-az2"),
        ("us-east-1c", "use1-az3"),
    ];
    let mut by_zone: Vec<(String, String, String)> = Vec::new();
    for (zone, zone_id) in zones {
        let outpost = outposts
            .create_outpost(&json!({
                "Name": format!("in-{zone}"),
                "SiteId": text(&seattle, "SiteId"),
                "AvailabilityZone": zone,
                "AvailabilityZoneId": zone_id,
            }))
            .await;
        by_zone.push((
            zone.to_string(),
            zone_id.to_string(),
            text(&outpost, "OutpostId"),
        ));
    }
    let expect = |wanted: &[&str]| -> Vec<String> {
        by_zone
            .iter()
            .filter(|(zone, _, _)| wanted.contains(&zone.as_str()))
            .map(|(_, _, id)| id.clone())
            .collect()
    };
    // Every Outpost above was created provisioning, so the cases below first let the
    // clock-owed transition fall due: once it has, every Outpost has settled, which
    // is what makes the status filter's two cases exact rather than racy.
    settle();

    let outpost_cases: Vec<(&str, String, Vec<String>)> = vec![
        (
            "one availability zone",
            "?AvailabilityZoneFilter=us-east-1a".to_string(),
            expect(&["us-east-1a"]),
        ),
        (
            "two availability zones",
            "?AvailabilityZoneFilter=us-east-1a&AvailabilityZoneFilter=us-east-1c".to_string(),
            expect(&["us-east-1a", "us-east-1c"]),
        ),
        (
            "a zone nothing is in",
            "?AvailabilityZoneFilter=ap-south-1a".to_string(),
            Vec::new(),
        ),
        (
            "one availability zone id",
            "?AvailabilityZoneIdFilter=use1-az2".to_string(),
            expect(&["us-east-1b"]),
        ),
        (
            "the status every Outpost carries once it has settled",
            "?LifeCycleStatusFilter=ACTIVE".to_string(),
            expect(&["us-east-1a", "us-east-1b", "us-east-1c"]),
        ),
        (
            "a status no settled Outpost carries",
            "?LifeCycleStatusFilter=PENDING".to_string(),
            Vec::new(),
        ),
        (
            "a status no Outpost carries",
            "?LifeCycleStatusFilter=DEGRADED".to_string(),
            Vec::new(),
        ),
        (
            "two filters narrow rather than widen",
            "?AvailabilityZoneFilter=us-east-1a&AvailabilityZoneIdFilter=use1-az2".to_string(),
            Vec::new(),
        ),
    ];
    for (case, query, wanted) in &outpost_cases {
        let page = outposts.list_outposts(query).await;
        assert_eq!(
            &ids(&page, "Outposts"),
            wanted,
            "case {case:?} ({query}): ListOutposts must answer only matching records, \
             got {page:?}"
        );
    }

    let site_cases: Vec<(&str, String, Vec<String>)> = vec![
        (
            "country code",
            "?OperatingAddressCountryCodeFilter=IE".to_string(),
            vec![text(&dublin, "SiteId")],
        ),
        (
            "city",
            "?OperatingAddressCityFilter=Seattle".to_string(),
            vec![text(&seattle, "SiteId")],
        ),
        (
            "state or region",
            "?OperatingAddressStateOrRegionFilter=WA".to_string(),
            vec![text(&seattle, "SiteId")],
        ),
        (
            "two cities",
            "?OperatingAddressCityFilter=Seattle&OperatingAddressCityFilter=Dublin".to_string(),
            vec![text(&seattle, "SiteId"), text(&dublin, "SiteId")],
        ),
        (
            "a city nothing is in",
            "?OperatingAddressCityFilter=Paris".to_string(),
            Vec::new(),
        ),
    ];
    for (case, query, wanted) in &site_cases {
        let page = outposts.list_sites(query).await;
        assert_eq!(
            &ids(&page, "Sites"),
            wanted,
            "case {case:?} ({query}): ListSites must answer only matching records, got {page:?}"
        );
    }
}

/// Rule 4: a delete removes the record, so every later call sees it gone; a site an
/// Outpost still references is refused; and the delete is observable through both
/// the read and the list.
#[tokio::test]
async fn delete_removes_the_record_and_is_refused_while_referenced() {
    let outposts = Outposts::new();
    let site = outposts.create_site(&json!({ "Name": "doomed site" })).await;
    let site_id = text(&site, "SiteId");
    let keeper = outposts.create_site(&json!({ "Name": "keeper" })).await;

    let outpost = outposts
        .create_outpost(&json!({ "Name": "doomed outpost", "SiteId": site_id }))
        .await;
    let outpost_id = text(&outpost, "OutpostId");

    // A site an Outpost is still at cannot be deleted, by either identifier form.
    for identifier in [site_id.clone(), text(&site, "SiteArn")] {
        let refused = outposts
            .call("DELETE", &format!("/sites/{}", segment(&identifier)), None)
            .await;
        assert_eq!(
            (refused.status, refused.error_code.as_str()),
            (400, "ValidationException"),
            "case {identifier:?}: deleting a referenced site is refused as a bad \
             request, got {:?}",
            refused.body
        );
        assert!(
            text(&refused.body, "Message").contains(&site_id),
            "case {identifier:?}: the refusal must name the site it refused, got {:?}",
            refused.body
        );
    }
    // Refused, so the site is still there.
    assert_eq!(
        outposts.get_site(&site_id).await,
        site,
        "a refused delete leaves the site exactly as it was"
    );

    // Deleting the Outpost succeeds with an empty body, and is observable everywhere.
    let deleted = outposts
        .ok("DELETE", &format!("/outposts/{outpost_id}"), None)
        .await;
    assert_eq!(
        deleted,
        json!({}),
        "DeleteOutpost answers an empty body, got {deleted:?}"
    );
    let gone = outposts
        .call("GET", &format!("/outposts/{outpost_id}"), None)
        .await;
    assert_eq!(
        (gone.status, gone.error_code.as_str()),
        (404, "NotFoundException"),
        "a deleted Outpost is gone to GetOutpost, got {:?}",
        gone.body
    );
    assert_eq!(
        ids(&outposts.list_outposts("").await, "Outposts"),
        Vec::<String>::new(),
        "a deleted Outpost is gone from ListOutposts"
    );
    let again = outposts
        .call("DELETE", &format!("/outposts/{outpost_id}"), None)
        .await;
    assert_eq!(
        (again.status, again.error_code.as_str()),
        (404, "NotFoundException"),
        "deleting the same Outpost twice is not found the second time, got {:?}",
        again.body
    );

    // With nothing referencing it, the site deletes — and only that site.
    outposts
        .ok("DELETE", &format!("/sites/{site_id}"), None)
        .await;
    let gone = outposts
        .call("GET", &format!("/sites/{site_id}"), None)
        .await;
    assert_eq!(
        (gone.status, gone.error_code.as_str()),
        (404, "NotFoundException"),
        "a deleted site is gone to GetSite, got {:?}",
        gone.body
    );
    assert_eq!(
        ids(&outposts.list_sites("").await, "Sites"),
        vec![text(&keeper, "SiteId")],
        "deleting one site leaves every other site listed"
    );
    assert_eq!(
        outposts.get_site(&text(&keeper, "SiteId")).await,
        keeper,
        "deleting one site leaves every other site readable"
    );
}

/// One request the emulator must refuse, and the status and wire error code it must
/// refuse it with.
pub(crate) struct Refusal {
    pub(crate) case: &'static str,
    pub(crate) method: &'static str,
    pub(crate) path: String,
    pub(crate) body: Option<Value>,
    pub(crate) status: u16,
    pub(crate) code: &'static str,
}

impl Refusal {
    /// The same case, with a JSON request payload.
    pub(crate) fn with_body(mut self, body: Value) -> Self {
        self.body = Some(body);
        self
    }
}

/// A [`Refusal`] on a route that binds every member to the URI, so it carries no body.
pub(crate) fn refusal(
    case: &'static str,
    method: &'static str,
    path: String,
    status: u16,
    code: &'static str,
) -> Refusal {
    Refusal {
        case,
        method,
        path,
        body: None,
        status,
        code,
    }
}

/// Rule 5: an identifier that is well formed but names nothing is 404
/// `NotFoundException`; one that is not an identifier at all is 400
/// `ValidationException`. Stated over every route in the slice that takes one.
#[tokio::test]
async fn absent_identifiers_are_not_found_and_malformed_ones_are_refused() {
    let outposts = Outposts::new();
    let absent_outpost_arn = format!("{OUTPOST_ARN_PREFIX}{ABSENT_OUTPOST_ID}");
    let absent_site_arn = format!("{SITE_ARN_PREFIX}{ABSENT_SITE_ID}");

    let cases: Vec<Refusal> = vec![
        refusal(
            "GetOutpost on an id nothing holds",
            "GET",
            format!("/outposts/{ABSENT_OUTPOST_ID}"),
            404,
            "NotFoundException",
        ),
        refusal(
            "GetOutpost on an ARN nothing holds",
            "GET",
            format!("/outposts/{}", segment(&absent_outpost_arn)),
            404,
            "NotFoundException",
        ),
        refusal(
            "GetSite on an id nothing holds",
            "GET",
            format!("/sites/{ABSENT_SITE_ID}"),
            404,
            "NotFoundException",
        ),
        refusal(
            "GetSite on an ARN nothing holds",
            "GET",
            format!("/sites/{}", segment(&absent_site_arn)),
            404,
            "NotFoundException",
        ),
        refusal(
            "DeleteOutpost on an id nothing holds",
            "DELETE",
            format!("/outposts/{ABSENT_OUTPOST_ID}"),
            404,
            "NotFoundException",
        ),
        refusal(
            "DeleteSite on an id nothing holds",
            "DELETE",
            format!("/sites/{ABSENT_SITE_ID}"),
            404,
            "NotFoundException",
        ),
        refusal(
            "CreateOutpost against a site nothing holds",
            "POST",
            "/outposts".to_string(),
            404,
            "NotFoundException",
        )
        .with_body(json!({ "Name": "orphan", "SiteId": ABSENT_SITE_ID })),
        refusal(
            "GetOutpost on a value that is no identifier",
            "GET",
            "/outposts/not-an-outpost-id".to_string(),
            400,
            "ValidationException",
        ),
        refusal(
            "GetOutpost on a site id",
            "GET",
            format!("/outposts/{ABSENT_SITE_ID}"),
            400,
            "ValidationException",
        ),
        refusal(
            "GetSite on a value that is no identifier",
            "GET",
            "/sites/not-a-site-id".to_string(),
            400,
            "ValidationException",
        ),
        refusal(
            "GetOutpost on an id whose hex is too short",
            "GET",
            "/outposts/op-0000000000000000".to_string(),
            400,
            "ValidationException",
        ),
        refusal(
            "GetOutpost on an id whose suffix is not hex",
            "GET",
            "/outposts/op-zzzzzzzzzzzzzzzzz".to_string(),
            400,
            "ValidationException",
        ),
        refusal(
            "DeleteSite on a value that is no identifier",
            "DELETE",
            "/sites/nope".to_string(),
            400,
            "ValidationException",
        ),
        refusal(
            "ListOutposts with a token this service never issued",
            "GET",
            "/outposts?NextToken=1%23%23nonsense".to_string(),
            400,
            "ValidationException",
        ),
    ];

    outposts.expect_refusals(&cases).await;
}

/// The identifiers a list page reports, in the order it reported them.
pub(crate) fn ids(output: &Value, member: &str) -> Vec<String> {
    output
        .get(member)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    let id = item.get("OutpostId").or_else(|| item.get("SiteId"));
                    id.and_then(Value::as_str).unwrap_or_default().to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}
