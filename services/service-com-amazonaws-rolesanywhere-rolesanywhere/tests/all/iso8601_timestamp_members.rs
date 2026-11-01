// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the wire FORM of this service's timestamp members.
//!
//! The rule under test: a member the model marks `@timestampFormat("date-time")` goes onto
//! this service's restJson1 wire as an RFC 3339 / ISO-8601 STRING, never as the protocol
//! default epoch number. Every timestamp member of this model carries that trait
//! (`smithy_query(query="shape", shape="CrlDetail,TrustAnchorDetail,ProfileDetail,SubjectDetail,SubjectSummary,CredentialSummary,InstanceProperty")`
//! reports `"timestampFormat": "date-time"` on each one), so the rule spans every response
//! shape of this service rather than the one operation a divergence was first seen on.
//!
//! The tables below therefore differ along what the rule generalises over: the resource
//! type whose detail shape carries the member, the operation that serves it (a write, a
//! read, a list projection, a delete echo), and the member itself. The instants are
//! per-run values and are never asserted literally — what is asserted is the FORM, that
//! every path reports the same instant for one resource, and that the read of a resource
//! agrees with the write that created it.
//!
//! The harness below is local to this suite: each sibling suite's client is private to its
//! own module, and the targets are compiled into one test binary only as separate modules.

use std::sync::Arc;

use aws_smithy_schema::traits::TimestampFormat;
use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use service_com_amazonaws_rolesanywhere_rolesanywhere::ops::types::{
    CredentialSummary, CrlDetail, InstanceProperty, ProfileDetail, SubjectDetail, SubjectSummary,
    TrustAnchorDetail,
};
use smithy_server_runtime::MultiServiceRouter;

/// A placeholder PEM bundle for a trust anchor's certificate source.
/// `SourceData$x509CertificateData` is `@length(1, 204800)` with no `@pattern`.
const CERTIFICATE_BUNDLE_PEM: &str =
    "-----BEGIN CERTIFICATE-----\nZW11bGF0ZWQtdGVzdC1jZXJ0\n-----END CERTIFICATE-----\n";

/// A placeholder CRL in PEM form. `ImportCrlRequest$crlData` is a `Blob`
/// `@length(1, 300000)` with no `@pattern`, so the bytes are opaque to the service.
const CRL_PEM: &str = "-----BEGIN X509 CRL-----\nZW11bGF0ZWQtdGVzdC1jcmw=\n-----END X509 CRL-----\n";

/// The Region the suite signs for.
const REGION: &str = "us-west-2";

/// The account every emulated ARN carries (`engine::emulator_account_id`).
const ACCOUNT: &str = "000000000000";

/// The two members every detail shape of a trust anchor, profile or CRL carries.
const LIFECYCLE_TIMESTAMPS: [&str; 2] = ["createdAt", "updatedAt"];

/// A client over one emulator: one engine, one mounted service, so every request in a
/// test observes the state the previous one wrote.
struct Client {
    router: MultiServiceRouter,
}

/// One HTTP answer: the status and the parsed JSON body (`Value::Null` for an empty body).
#[derive(Debug)]
struct Answer {
    status: u16,
    body: Value,
}

impl Client {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_rolesanywhere_rolesanywhere::EmulatedService::new(engine),
        );
        Self { router }
    }

    async fn send(&self, method: &str, uri: &str, body: Option<Value>) -> Answer {
        let payload = body
            .map(|value| bytes::Bytes::from(serde_json::to_vec(&value).expect("serialize body")))
            .unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            // The SigV4 credential scope is how the router resolves the service and how
            // the handler learns the request's Region.
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/rolesanywhere/aws4_request, \
                     SignedHeaders=host, Signature=deadbeef"
                ),
            )
            .body(payload)
            .expect("build request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Answer { status, body }
    }
}

/// The body of a call that must have succeeded.
fn ok(case: &str, expected_status: u16, answer: Answer) -> Value {
    assert_eq!(
        answer.status, expected_status,
        "case {case:?}: expected HTTP {expected_status}, got {answer:?}"
    );
    answer.body
}

/// The JSON value at `pointer`, or a failure naming the case.
fn at(case: &str, body: &Value, pointer: &str) -> Value {
    body.pointer(pointer)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: nothing at {pointer} in {body:?}"))
}

/// The instant a `date-time` member carries, proving its wire form on the way: the member
/// must be a JSON STRING, must parse as RFC 3339 / ISO-8601 (the form
/// `@timestampFormat("date-time")` names), and must re-render to the text it arrived as.
fn iso8601_instant(case: &str, holder: &Value, member: &str) -> DateTime {
    let value = holder
        .get(member)
        .unwrap_or_else(|| panic!("case {case:?}: {member} must be present, got {holder:?}"));
    let text = value.as_str().unwrap_or_else(|| {
        panic!(
            "case {case:?}: {member} must be an ISO-8601 date-time string, got the \
             {} {value}",
            match value {
                Value::Number(_) => "JSON number",
                Value::Bool(_) => "JSON boolean",
                Value::Null => "JSON null",
                _ => "JSON value",
            }
        )
    });
    let parsed = DateTime::from_str(text, Format::DateTime).unwrap_or_else(|error| {
        panic!("case {case:?}: {member} {text:?} must parse as a date-time, got {error}")
    });
    let rendered = parsed
        .fmt(Format::DateTime)
        .unwrap_or_else(|error| panic!("case {case:?}: {member} must re-render, got {error}"));
    assert_eq!(
        rendered, text,
        "case {case:?}: {member} must be in the canonical date-time form"
    );
    parsed
}

/// Assert every `date-time` member of one returned resource is an ISO-8601 string, and
/// answer the instants in the order the members were given.
fn iso8601_instants(case: &str, holder: &Value, members: &[&str]) -> Vec<DateTime> {
    members
        .iter()
        .map(|member| iso8601_instant(case, holder, member))
        .collect()
}

impl Client {
    async fn create_trust_anchor(&self, name: &str) -> Value {
        ok(
            name,
            201,
            self.send(
                "POST",
                "/trustanchors",
                Some(json!({
                    "name": name,
                    "source": {
                        "sourceType": "CERTIFICATE_BUNDLE",
                        "sourceData": {"x509CertificateData": CERTIFICATE_BUNDLE_PEM},
                    },
                })),
            )
            .await,
        )
    }

    async fn create_profile(&self, name: &str) -> Value {
        ok(
            name,
            201,
            self.send(
                "POST",
                "/profiles",
                Some(json!({
                    "name": name,
                    "roleArns": [format!("arn:aws:iam::{ACCOUNT}:role/TimestampDemoRole")],
                })),
            )
            .await,
        )
    }

    async fn import_crl(&self, name: &str, trust_anchor_arn: &str) -> Value {
        ok(
            name,
            201,
            self.send(
                "POST",
                "/crls",
                Some(json!({
                    "name": name,
                    "crlData": aws_smithy_types::base64::encode(CRL_PEM.as_bytes()),
                    "trustAnchorArn": trust_anchor_arn,
                })),
            )
            .await,
        )
    }
}

impl Client {
    /// Walk EVERY page of one paginated list projection and answer the elements it served,
    /// in the order they were served.
    async fn drain_list(
        &self,
        case: &str,
        path: &str,
        member: &str,
        page_size: Option<i32>,
    ) -> Vec<Value> {
        let mut elements: Vec<Value> = Vec::new();
        let mut next_token: Option<String> = None;
        loop {
            let mut query: Vec<String> = Vec::new();
            if let Some(size) = page_size {
                query.push(format!("pageSize={size}"));
            }
            if let Some(token) = &next_token {
                query.push(format!("nextToken={token}"));
            }
            let uri = if query.is_empty() {
                path.to_string()
            } else {
                format!("{path}?{}", query.join("&"))
            };
            let body = ok(case, 200, self.send("GET", &uri, None).await);
            let page = body
                .get(member)
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_else(|| panic!("case {case:?}: expected a {member} list, got {body:?}"));
            elements.extend(page);
            next_token = body
                .get("nextToken")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            if next_token.is_none() {
                break;
            }
            assert!(
                elements.len() <= PAGING_ELEMENT_CEILING,
                "case {case:?}: paging must terminate, but it has already served {} elements",
                elements.len()
            );
        }
        elements
    }
}

/// A bound on a paging walk in this suite, so a cursor that never ends fails the test
/// instead of hanging: every row below creates far fewer resources than this.
const PAGING_ELEMENT_CEILING: usize = 64;

/// The id at `pointer` in a create response.
fn id_at(case: &str, body: &Value, pointer: &str) -> String {
    at(case, body, pointer)
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: {pointer} must be a string in {body:?}"))
        .to_string()
}

/// Every `date-time` member of every response this emulator serves is an ISO-8601 string,
/// on the write path, the read path, the list projection and the delete echo alike.
///
/// The rows span the three resource types whose detail shapes carry such a member
/// (`CrlDetail`, `TrustAnchorDetail`, `ProfileDetail`) and the four shapes of operation
/// that serve them.
#[tokio::test]
async fn every_served_date_time_member_is_an_iso8601_string() {
    let client = Client::new();

    let anchor = client.create_trust_anchor("timestamp-anchor").await;
    let anchor_id = id_at("trust-anchor", &anchor, "/trustAnchor/trustAnchorId");
    let anchor_arn = id_at("trust-anchor", &anchor, "/trustAnchor/trustAnchorArn");
    let profile = client.create_profile("timestamp-profile").await;
    let profile_id = id_at("profile", &profile, "/profile/profileId");
    let crl = client.import_crl("timestamp-crl", &anchor_arn).await;
    let crl_id = id_at("crl", &crl, "/crl/crlId");

    // (case, body, pointer to the detail object) — a write, a read, a list element and a
    // delete echo for each resource type.
    let mut rows: Vec<(String, Value, &str)> = vec![
        ("ImportCrl".to_string(), crl.clone(), "/crl"),
        ("CreateTrustAnchor".to_string(), anchor.clone(), "/trustAnchor"),
        ("CreateProfile".to_string(), profile.clone(), "/profile"),
        (
            "GetCrl".to_string(),
            ok(
                "GetCrl",
                200,
                client.send("GET", &format!("/crl/{crl_id}"), None).await,
            ),
            "/crl",
        ),
        (
            "GetTrustAnchor".to_string(),
            ok(
                "GetTrustAnchor",
                200,
                client
                    .send("GET", &format!("/trustanchor/{anchor_id}"), None)
                    .await,
            ),
            "/trustAnchor",
        ),
        (
            "GetProfile".to_string(),
            ok(
                "GetProfile",
                200,
                client
                    .send("GET", &format!("/profile/{profile_id}"), None)
                    .await,
            ),
            "/profile",
        ),
        (
            "ListCrls".to_string(),
            ok("ListCrls", 200, client.send("GET", "/crls", None).await),
            "/crls/0",
        ),
        (
            "ListTrustAnchors".to_string(),
            ok(
                "ListTrustAnchors",
                200,
                client.send("GET", "/trustanchors", None).await,
            ),
            "/trustAnchors/0",
        ),
        (
            "ListProfiles".to_string(),
            ok(
                "ListProfiles",
                200,
                client.send("GET", "/profiles", None).await,
            ),
            "/profiles/0",
        ),
    ];
    // The delete echoes come last: each one removes the resource the rows above read.
    rows.push((
        "DeleteCrl".to_string(),
        ok(
            "DeleteCrl",
            200,
            client.send("DELETE", &format!("/crl/{crl_id}"), None).await,
        ),
        "/crl",
    ));
    rows.push((
        "DeleteProfile".to_string(),
        ok(
            "DeleteProfile",
            200,
            client
                .send("DELETE", &format!("/profile/{profile_id}"), None)
                .await,
        ),
        "/profile",
    ));
    rows.push((
        "DeleteTrustAnchor".to_string(),
        ok(
            "DeleteTrustAnchor",
            200,
            client
                .send("DELETE", &format!("/trustanchor/{anchor_id}"), None)
                .await,
        ),
        "/trustAnchor",
    ));

    for (case, body, pointer) in &rows {
        let holder = at(case, body, pointer);
        let instants = iso8601_instants(case, &holder, &LIFECYCLE_TIMESTAMPS);
        let created = instants[0];
        let updated = instants[1];
        assert!(
            updated.as_nanos() >= created.as_nanos(),
            "case {case:?}: updatedAt must not precede createdAt, got {created:?} and \
             {updated:?}"
        );
    }
}

/// One resource reports ONE instant, whatever path reports it: the stored record is the
/// source, so the write, the read, the list projection and the delete echo all carry the
/// same date-time text.
#[tokio::test]
async fn one_resource_reports_the_same_instant_on_every_path() {
    let client = Client::new();
    let anchor = client.create_trust_anchor("one-instant-anchor").await;
    let anchor_arn = id_at("anchor", &anchor, "/trustAnchor/trustAnchorArn");

    // Three CRLs, so the list projection has to report each element's own instants rather
    // than one shared value.
    let mut imported: Vec<(String, String, Value)> = Vec::new();
    for index in 0..3 {
        let name = format!("one-instant-crl-{index}");
        let body = client.import_crl(&name, &anchor_arn).await;
        let id = id_at(&name, &body, "/crl/crlId");
        imported.push((name, id, at("import", &body, "/crl")));
    }

    let listed = ok("ListCrls", 200, client.send("GET", "/crls", None).await);
    let listed = at("ListCrls", &listed, "/crls");
    let listed = listed
        .as_array()
        .unwrap_or_else(|| panic!("ListCrls must answer a crls list, got {listed:?}"));

    for (case, id, written) in &imported {
        let read = ok(
            case,
            200,
            client.send("GET", &format!("/crl/{id}"), None).await,
        );
        let read = at(case, &read, "/crl");
        let element = listed
            .iter()
            .find(|entry| entry.get("crlId").and_then(Value::as_str) == Some(id.as_str()))
            .unwrap_or_else(|| panic!("case {case:?}: {id} must appear in the list"));
        let deleted = ok(
            case,
            200,
            client.send("DELETE", &format!("/crl/{id}"), None).await,
        );
        let deleted = at(case, &deleted, "/crl");

        for member in LIFECYCLE_TIMESTAMPS {
            let written_text = written.get(member).cloned().unwrap_or(Value::Null);
            for (path, holder) in [("GetCrl", &read), ("ListCrls", element), ("DeleteCrl", &deleted)]
            {
                let actual = holder.get(member).cloned().unwrap_or(Value::Null);
                assert_eq!(
                    actual, written_text,
                    "case {case:?}: {path} must report the {member} the import wrote, got \
                     {actual:?}"
                );
            }
            // And it is a date-time string on each of those paths, not just equal.
            iso8601_instant(case, &read, member);
        }
    }
}

/// A fresh resource's `createdAt` is the clock at the call, not a value derived from the
/// request: two imports of the SAME body carry instants that do not go backwards, and both
/// are within a generous window around the test's own clock.
#[tokio::test]
async fn timestamps_come_from_the_clock_not_from_the_request() {
    let client = Client::new();
    let anchor = client.create_trust_anchor("clock-anchor").await;
    let anchor_arn = id_at("anchor", &anchor, "/trustAnchor/trustAnchorArn");

    let before = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the test clock is after the epoch")
        .as_secs_f64();

    let mut instants: Vec<(String, f64)> = Vec::new();
    for index in 0..3 {
        let case = format!("identical-import-{index}");
        let body = client.import_crl("same name every time", &anchor_arn).await;
        let holder = at(&case, &body, "/crl");
        let created = iso8601_instant(&case, &holder, "createdAt");
        instants.push((case, created.as_secs_f64()));
    }

    let after = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the test clock is after the epoch")
        .as_secs_f64();

    let mut previous = before;
    for (case, created) in &instants {
        assert!(
            *created >= before && *created <= after,
            "case {case:?}: createdAt must be the instant of the call, got {created} outside \
             [{before}, {after}]"
        );
        assert!(
            *created >= previous,
            "case {case:?}: a later import must not report an earlier createdAt, got \
             {created} after {previous}"
        );
        previous = *created;
    }
}

/// The resource one paginated list projection enumerates: how to create it, where its
/// detail object and its id live in the create response, and the projection that lists it.
/// The `@pattern` on `ResourceName` (`^[ a-zA-Z0-9-_]*$`) is why every row's label below is
/// free of punctuation: the labels become resource names.
#[derive(Clone, Copy, Debug)]
enum Resource {
    Crl,
    TrustAnchor,
    Profile,
}

impl Resource {
    /// The URI of the paginated list projection that enumerates this resource, and the
    /// member of its response that carries the elements.
    fn list(self) -> (&'static str, &'static str) {
        match self {
            Self::Crl => ("/crls", "crls"),
            Self::TrustAnchor => ("/trustanchors", "trustAnchors"),
            Self::Profile => ("/profiles", "profiles"),
        }
    }

    /// The JSON pointer to the detail object in a create response, and the member of that
    /// object carrying the resource's id.
    fn detail(self) -> (&'static str, &'static str) {
        match self {
            Self::Crl => ("/crl", "crlId"),
            Self::TrustAnchor => ("/trustAnchor", "trustAnchorId"),
            Self::Profile => ("/profile", "profileId"),
        }
    }
}

/// Create `count` resources of one kind and answer them keyed by id, each with the detail
/// object its create response carried.
async fn seed(
    client: &Client,
    case: &str,
    resource: Resource,
    count: usize,
) -> std::collections::BTreeMap<String, Value> {
    // A CRL is imported against a trust anchor, so that one row needs an anchor first.
    let anchor_arn = match resource {
        Resource::Crl => {
            let anchor = client.create_trust_anchor(&format!("{case} anchor")).await;
            Some(id_at(case, &anchor, "/trustAnchor/trustAnchorArn"))
        }
        Resource::TrustAnchor | Resource::Profile => None,
    };

    let (pointer, id_member) = resource.detail();
    let mut created = std::collections::BTreeMap::new();
    for index in 0..count {
        let name = format!("{case} {index}");
        let body = match resource {
            Resource::Crl => {
                client
                    .import_crl(
                        &name,
                        anchor_arn.as_deref().expect("a CRL row creates an anchor"),
                    )
                    .await
            }
            Resource::TrustAnchor => client.create_trust_anchor(&name).await,
            Resource::Profile => client.create_profile(&name).await,
        };
        let id = id_at(case, &body, &format!("{pointer}/{id_member}"));
        let detail = at(case, &body, pointer);
        assert!(
            created.insert(id.clone(), detail).is_none(),
            "case {case:?}: every create must mint a distinct id, saw {id:?} twice"
        );
    }
    created
}

/// EVERY element of EVERY page of a list projection carries its `date-time` members as
/// ISO-8601 strings, and reports the instants the create stored — not only the first
/// element of the first page, and not only for the operation a divergence was first seen
/// on.
///
/// The rows vary what the rule generalises over: the list operation (so the element shape
/// and its items member), the store size, and the page size — absent, one entry per page,
/// several entries per page, and a page wider than the store — so an element served on a
/// second and a later page is checked as well as the first.
#[tokio::test]
async fn every_element_of_every_list_page_carries_iso8601_timestamps() {
    // (case, resource kind, store size, pageSize)
    let rows: [(&str, Resource, usize, Option<i32>); 7] = [
        ("ListCrls one per page", Resource::Crl, 3, Some(1)),
        ("ListCrls two per page", Resource::Crl, 5, Some(2)),
        ("ListCrls one page", Resource::Crl, 4, None),
        ("ListCrls page wider than the store", Resource::Crl, 2, Some(100)),
        ("ListTrustAnchors one per page", Resource::TrustAnchor, 3, Some(1)),
        ("ListTrustAnchors one page", Resource::TrustAnchor, 2, None),
        ("ListProfiles two per page", Resource::Profile, 5, Some(2)),
    ];

    for (case, resource, count, page_size) in rows {
        // One emulator per row, so the store holds exactly what this row created.
        let client = Client::new();
        let created = seed(&client, case, resource, count).await;
        let (path, member) = resource.list();
        let id_member = resource.detail().1;

        let elements = client.drain_list(case, path, member, page_size).await;
        assert_eq!(
            elements.len(),
            created.len(),
            "case {case:?}: the walk must serve every created resource once, expected {} \
             elements, got {}",
            created.len(),
            elements.len()
        );

        for element in &elements {
            let id = element
                .get(id_member)
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: a listed element carries no {id_member}: {element:?}")
                })
                .to_string();
            let written = created
                .get(&id)
                .unwrap_or_else(|| panic!("case {case:?}: {id:?} was never created"));
            let instants = iso8601_instants(case, element, &LIFECYCLE_TIMESTAMPS);
            assert!(
                instants[1].as_nanos() >= instants[0].as_nanos(),
                "case {case:?}: {id:?} must not report an updatedAt before its createdAt, got \
                 {instants:?}"
            );
            for stamp in LIFECYCLE_TIMESTAMPS {
                let actual = element.get(stamp).cloned().unwrap_or(Value::Null);
                let expected = written.get(stamp).cloned().unwrap_or(Value::Null);
                assert_eq!(
                    actual, expected,
                    "case {case:?}: the listed element for {id:?} must report the {stamp} the \
                     create wrote, got {actual:?}"
                );
            }
        }
    }
}

/// Every member this model marks `@timestampFormat("date-time")` carries that format on
/// its member schema — including the shapes whose operations are still scaffolded, because
/// the format is a property of the model, not of which handler is written.
///
/// The catalog below is the model's own: every timestamp member of every structure
/// `smithy_query(query="shape", shape="CrlDetail,TrustAnchorDetail,ProfileDetail,SubjectDetail,SubjectSummary,CredentialSummary,InstanceProperty")`
/// reports with `"timestampFormat": "date-time"`.
#[test]
fn every_modelled_date_time_member_schema_carries_the_format() {
    let date_time_members: Vec<(&str, &aws_smithy_schema::Schema)> = vec![
        ("CrlDetail$createdAt", &CrlDetail::M_CREATED_AT),
        ("CrlDetail$updatedAt", &CrlDetail::M_UPDATED_AT),
        (
            "TrustAnchorDetail$createdAt",
            &TrustAnchorDetail::M_CREATED_AT,
        ),
        (
            "TrustAnchorDetail$updatedAt",
            &TrustAnchorDetail::M_UPDATED_AT,
        ),
        ("ProfileDetail$createdAt", &ProfileDetail::M_CREATED_AT),
        ("ProfileDetail$updatedAt", &ProfileDetail::M_UPDATED_AT),
        ("SubjectDetail$lastSeenAt", &SubjectDetail::M_LAST_SEEN_AT),
        ("SubjectDetail$createdAt", &SubjectDetail::M_CREATED_AT),
        ("SubjectDetail$updatedAt", &SubjectDetail::M_UPDATED_AT),
        ("SubjectSummary$lastSeenAt", &SubjectSummary::M_LAST_SEEN_AT),
        ("SubjectSummary$createdAt", &SubjectSummary::M_CREATED_AT),
        ("SubjectSummary$updatedAt", &SubjectSummary::M_UPDATED_AT),
        ("CredentialSummary$seenAt", &CredentialSummary::M_SEEN_AT),
        ("InstanceProperty$seenAt", &InstanceProperty::M_SEEN_AT),
    ];

    for (case, schema) in date_time_members {
        let format = schema.timestamp_format().map(|trait_| trait_.format());
        assert!(
            matches!(format, Some(TimestampFormat::DateTime)),
            "case {case:?}: the member schema must carry @timestampFormat(date-time), got \
             {format:?}"
        );
    }

    // A member the model annotates with no format keeps the protocol default, so the fix
    // did not put a format where the model names none.
    for (case, schema) in [
        ("CrlDetail$crlArn", &CrlDetail::M_CRL_ARN),
        ("CrlDetail$crlData", &CrlDetail::M_CRL_DATA),
        ("ProfileDetail$durationSeconds", &ProfileDetail::M_DURATION_SECONDS),
        ("TrustAnchorDetail$source", &TrustAnchorDetail::M_SOURCE),
    ] {
        let format = schema.timestamp_format().map(|trait_| trait_.format());
        assert!(
            format.is_none(),
            "case {case:?}: the model annotates no format for this member, got {format:?}"
        );
    }
}
