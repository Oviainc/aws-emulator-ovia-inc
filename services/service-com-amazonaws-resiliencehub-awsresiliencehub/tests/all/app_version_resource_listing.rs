// SPDX-License-Identifier: CC0-1.0

//! Listing the resources of an app version, and reading each listed version's
//! `identifier`, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with a
//! restJson1 request — and asserts the response BODY, not just the status.
//!
//! The rules under test:
//!
//! * every `AppVersionSummary` `ListAppVersions` reports answers `identifier` —
//!   the draft's is `DRAFT_APP_VERSION_IDENTIFIER` and a published version's is
//!   the publish sequence number `PublishAppVersion` minted it with — so a client
//!   can key the listing by that member unconditionally;
//! * `ListAppVersionResources` reports exactly the physical resources the
//!   addressed app version holds — the draft's as the draft writers left them, a
//!   published version's as the snapshot it was frozen as, so a later draft change
//!   cannot alter a published listing — in pages of at most `maxResults` that a
//!   `nextToken` resumes exactly where the previous page stopped, under the
//!   identity of the resolution the listing is against: the one the request names,
//!   else the version's current resolution, else the identity of the mapping
//!   itself;
//! * an unknown application, an app version the application does not have and a
//!   `resolutionId` that version has no resolution for are each the modelled 404.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_resiliencehub_awsresiliencehub::state::{
    DRAFT_APP_VERSION, DRAFT_APP_VERSION_IDENTIFIER, RELEASE_APP_VERSION,
};
use service_com_amazonaws_resiliencehub_awsresiliencehub::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below signs for.
const REGION: &str = "us-east-1";

/// The account id `engine::mock_account_id` mints every ARN under.
const ACCOUNT: &str = "000000000000";

/// The modelled `@length` of `String255`, the shape `resolutionId` targets (from
/// `smithy_query query="operation" operation="ListAppVersionResources"`:
/// `resolutionId` targets `String255`, length 1..255).
const RESOLUTION_ID_LENGTH: std::ops::RangeInclusive<usize> = 1..=255;

/// An emulator holding only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // These requests carry an unverifiable placeholder signature: this suite tests
    // the operations, not SigV4.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What a dispatched request answered.
#[derive(Debug)]
struct Reply {
    status: u16,
    /// The restJson1 error shape name, from `x-amzn-errortype`; empty on success.
    error_code: String,
    body: Value,
}

impl Reply {
    /// The response body, asserting the call succeeded.
    fn ok(&self, case: &str) -> &Value {
        assert_eq!(
            self.status, 200,
            "case {case:?}: expected HTTP 200, got {} {:?} {:?}",
            self.status, self.error_code, self.body
        );
        &self.body
    }

    /// Assert the call was refused with `status` and `code`.
    fn refused(&self, case: &str, status: u16, code: &str) {
        assert_eq!(
            (self.status, self.error_code.as_str()),
            (status, code),
            "case {case:?}: expected HTTP {status} {code}, got {} {:?} {:?}",
            self.status,
            self.error_code,
            self.body
        );
    }
}

/// POST one request through the router, exactly as the server front door does.
async fn post(router: &MultiServiceRouter, target: &str, body: &Value) -> Reply {
    let payload = serde_json::to_vec(body).expect("serializable body");
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://resiliencehub.{REGION}.amazonaws.com{target}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/resiliencehub/\
                 aws4_request, SignedHeaders=host, Signature=00"
            ),
        )
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_code = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let raw = response.into_body();
    let body = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| json!({"raw": String::from_utf8_lossy(&raw)}))
    };
    Reply {
        status,
        error_code,
        body,
    }
}

/// An application to list the resources of, and its ARN.
async fn create_an_app(router: &MultiServiceRouter, name: &str) -> String {
    let reply = post(router, "/create-app", &json!({"name": name})).await;
    reply.ok(name)["app"]["appArn"]
        .as_str()
        .expect("a minted appArn")
        .to_string()
}

/// Add one physical resource, named `identifier`, to `app_arn`'s draft version.
async fn add_a_resource(router: &MultiServiceRouter, app_arn: &str, identifier: &str) {
    let reply = post(
        router,
        "/create-app-version-resource",
        &json!({
            "appArn": app_arn,
            "resourceName": identifier,
            "logicalResourceId": {"identifier": identifier},
            "physicalResourceId": identifier,
            "resourceType": "AWS::S3::Bucket",
            "appComponents": ["storage-tier"],
        }),
    )
    .await;
    reply.ok(identifier);
}

/// Publish `app_arn`'s draft and answer the `appVersion` the publish minted.
async fn publish(router: &MultiServiceRouter, app_arn: &str) -> String {
    let reply = post(router, "/publish-app-version", &json!({"appArn": app_arn})).await;
    reply.ok("publish")["appVersion"]
        .as_str()
        .expect("a published appVersion")
        .to_string()
}

/// `ListAppVersionResources` for one version of one application, with whatever
/// extra members `request` carries.
async fn list_resources(
    router: &MultiServiceRouter,
    app_arn: &str,
    app_version: &str,
    request: &Value,
) -> Reply {
    let mut body = request.clone();
    body["appArn"] = json!(app_arn);
    body["appVersion"] = json!(app_version);
    post(router, "/list-app-version-resources", &body).await
}

/// Start a resolution of `app_version` of `app_arn` and answer its id.
async fn resolve(router: &MultiServiceRouter, app_arn: &str, app_version: &str) -> String {
    let reply = post(
        router,
        "/resolve-app-version-resources",
        &json!({"appArn": app_arn, "appVersion": app_version}),
    )
    .await;
    reply.ok("resolve")["resolutionId"]
        .as_str()
        .expect("a minted resolutionId")
        .to_string()
}

/// The `physicalResources` of a successful listing.
fn physical_resources(reply: &Reply, case: &str) -> Vec<Value> {
    reply.ok(case)["physicalResources"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: physicalResources is @required, got {:?}",
                reply.body
            )
        })
        .clone()
}

/// The `logicalResourceId.identifier` of each listed resource, in the order they
/// were returned.
fn listed_order(resources: &[Value]) -> Vec<String> {
    resources
        .iter()
        .map(|resource| {
            resource["logicalResourceId"]["identifier"]
                .as_str()
                .expect("logicalResourceId.identifier is @required")
                .to_string()
        })
        .collect()
}

/// The identifiers of the resources one version reports, sorted, so a case can
/// compare WHAT a version holds without pinning the order the service pages in.
async fn held_by(router: &MultiServiceRouter, app_arn: &str, app_version: &str, case: &str) -> Vec<String> {
    let listed = list_resources(router, app_arn, app_version, &json!({})).await;
    let mut identifiers = listed_order(&physical_resources(&listed, case));
    identifiers.sort();
    identifiers
}

/// The `resolutionId` one listing reports.
async fn resolution_of(
    router: &MultiServiceRouter,
    app_arn: &str,
    app_version: &str,
    case: &str,
) -> String {
    let listed = list_resources(router, app_arn, app_version, &json!({})).await;
    listed.ok(case)["resolutionId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: resolutionId is @required, got {:?}", listed.body))
        .to_string()
}

/// A row of the publish table: the resources added to the draft before this
/// publish.
///
/// The rows differ along what the rule generalises over — how many resources the
/// published mapping holds (one, several, and a publish that adds nothing new) and
/// the form of the identifiers in it: a plain name, one carrying a hyphen, one
/// carrying an underscore (the punctuation `EntityName`'s modelled pattern
/// `^[A-Za-z0-9][A-Za-z0-9_\-]{1,59}$` admits), and one that is a prefix of
/// another.
struct Publish {
    label: &'static str,
    adds: &'static [&'static str],
}

/// The table: one publish per row, in order, against ONE application.
fn publish_table() -> Vec<Publish> {
    vec![
        Publish {
            label: "first publish, one resource",
            adds: &["orders"],
        },
        Publish {
            label: "second publish, two more resources",
            adds: &["orders-archive", "orders_audit_log"],
        },
        Publish {
            label: "third publish, nothing new in the draft",
            adds: &[],
        },
        Publish {
            label: "fourth publish, one more resource",
            adds: &["payments"],
        },
    ]
}

/// Each publish freezes the draft's mapping: the published version reports what
/// the draft held when it was published and keeps reporting it while the draft
/// goes on changing, and every superseded snapshot keeps its own mapping.
#[tokio::test]
async fn a_published_version_reports_the_mapping_it_froze() {
    let router = emulator();
    let app_arn = create_an_app(&router, "resource-listing-freeze").await;

    // A draft nobody has written to holds no resource — an empty listing of a
    // version that exists, not a refusal.
    let empty = list_resources(&router, &app_arn, DRAFT_APP_VERSION, &json!({})).await;
    assert_eq!(
        physical_resources(&empty, "an untouched draft"),
        Vec::<Value>::new(),
        "case \"an untouched draft\": a draft nobody wrote to holds nothing, got {:?}",
        empty.body
    );

    // The mapping each publish froze, in publish order: the newest is the one the
    // release alias reports, and every earlier one is reported by the version its
    // publish sequence number addresses.
    let mut frozen: Vec<Vec<String>> = Vec::new();
    let mut draft_holds: Vec<String> = Vec::new();

    for (index, row) in publish_table().into_iter().enumerate() {
        let case = row.label;
        for identifier in row.adds {
            add_a_resource(&router, &app_arn, identifier).await;
            draft_holds.push((*identifier).to_string());
        }
        draft_holds.sort();

        // Before the publish, the draft reports everything written into it.
        assert_eq!(
            held_by(&router, &app_arn, DRAFT_APP_VERSION, case).await,
            draft_holds,
            "case {case:?}: the draft reports every resource written into it"
        );

        let published = publish(&router, &app_arn).await;
        assert_eq!(
            published, RELEASE_APP_VERSION,
            "case {case:?}: the newest published version is addressed by the release \
             alias, got {published:?}"
        );
        frozen.push(draft_holds.clone());

        // Every version the application now has reports the mapping its own publish
        // froze — the newest under the alias, each superseded one under its number.
        for (position, expected) in frozen.iter().enumerate() {
            let app_version = if position + 1 == frozen.len() {
                RELEASE_APP_VERSION.to_string()
            } else {
                (position + 1).to_string()
            };
            let case = format!("{case}: version {app_version:?}");
            assert_eq!(
                &held_by(&router, &app_arn, &app_version, &case).await,
                expected,
                "case {case:?}: a frozen version reports the mapping it was published \
                 with"
            );
        }

        // A further draft change cannot reach anything already published.
        let probe = format!("drift-probe-{index}");
        add_a_resource(&router, &app_arn, &probe).await;
        draft_holds.push(probe.clone());
        draft_holds.sort();
        assert_eq!(
            &held_by(&router, &app_arn, RELEASE_APP_VERSION, case).await,
            frozen.last().expect("the newest frozen mapping"),
            "case {case:?}: adding {probe:?} to the draft must not change what the \
             release reports"
        );
        assert!(
            held_by(&router, &app_arn, DRAFT_APP_VERSION, case)
                .await
                .contains(&probe),
            "case {case:?}: the draft must report {probe:?} straight after it was added"
        );
    }

    // A read changes nothing a later caller sees: the same request answers the
    // same listing.
    let again = list_resources(&router, &app_arn, RELEASE_APP_VERSION, &json!({})).await;
    let repeated = list_resources(&router, &app_arn, RELEASE_APP_VERSION, &json!({})).await;
    assert_eq!(
        again.ok("repeated listing"),
        repeated.ok("repeated listing"),
        "case \"repeated listing\": listing twice must answer identically"
    );
}

/// `maxResults` is a real page bound and `nextToken` resumes exactly where the
/// previous page stopped, for every page size — including both published bounds.
#[tokio::test]
async fn pages_through_the_resources_with_every_page_size() {
    let router = emulator();
    let app_arn = create_an_app(&router, "resource-listing-pages").await;
    const RESOURCES: usize = 6;
    for index in 0..RESOURCES {
        add_a_resource(&router, &app_arn, &format!("resource-{index}")).await;
    }

    let unpaged = list_resources(&router, &app_arn, DRAFT_APP_VERSION, &json!({})).await;
    let forward = listed_order(&physical_resources(&unpaged, "unpaged"));
    assert_eq!(
        forward.len(),
        RESOURCES,
        "case \"unpaged\": expected all {RESOURCES} resources, got {forward:?}"
    );

    // Both published bounds of `maxResults` (@range 1..100), plus the sizes in
    // between that actually split this set.
    for page_size in [1usize, 2, 3, 5, 6, 7, 100] {
        let case = format!("maxResults={page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({"maxResults": page_size});
            if let Some(next) = token.as_deref() {
                request["nextToken"] = json!(next);
            }
            let reply = list_resources(&router, &app_arn, DRAFT_APP_VERSION, &request).await;
            let page = physical_resources(&reply, &case);
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page returned {} resources, more than maxResults",
                page.len()
            );
            walked.extend(listed_order(&page));
            pages += 1;
            assert!(
                pages <= RESOURCES + 1,
                "case {case:?}: pagination did not terminate after {pages} pages"
            );
            match reply.body["nextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            walked, forward,
            "case {case:?}: walking the pages must yield the unpaged order exactly"
        );
    }

    // One past each published bound of `maxResults` is refused by the modeled range.
    for out_of_range in [0, 101] {
        let case = format!("maxResults={out_of_range}");
        list_resources(
            &router,
            &app_arn,
            DRAFT_APP_VERSION,
            &json!({"maxResults": out_of_range}),
        )
        .await
        .refused(&case, 400, "ValidationException");
    }

    // A token this operation never issued is refused, not silently ignored.
    for token in ["not-a-token", "ff"] {
        let case = format!("nextToken={token}");
        list_resources(
            &router,
            &app_arn,
            DRAFT_APP_VERSION,
            &json!({"nextToken": token}),
        )
        .await
        .refused(&case, 400, "ValidationException");
    }
}

/// The listing always answers `resolutionId`: the identity of the version's
/// mapping while nobody has resolved that version, the resolution the request
/// names once it has, and a 404 for a resolution that version does not have.
#[tokio::test]
async fn reports_the_resolution_the_listing_is_against() {
    let router = emulator();
    let app_arn = create_an_app(&router, "resource-listing-resolution").await;
    add_a_resource(&router, &app_arn, "orders").await;
    let published = publish(&router, &app_arn).await;

    // Unresolved, the identity comes from the mapping itself: it is a modelled
    // `String255`, two reads agree on it, and two versions do not share one.
    let draft_identity = resolution_of(&router, &app_arn, DRAFT_APP_VERSION, "the draft").await;
    let release_identity = resolution_of(&router, &app_arn, &published, "the release").await;
    for (case, app_version, identity) in [
        ("the draft", DRAFT_APP_VERSION, &draft_identity),
        ("the release", published.as_str(), &release_identity),
    ] {
        assert!(
            RESOLUTION_ID_LENGTH.contains(&identity.chars().count()),
            "case {case:?}: resolutionId must satisfy String255's modelled length \
             {RESOLUTION_ID_LENGTH:?}, got {} characters ({identity:?})",
            identity.chars().count()
        );
        let again = resolution_of(&router, &app_arn, app_version, case).await;
        assert_eq!(
            &again, identity,
            "case {case:?}: two reads of an unchanged mapping must report the same \
             resolutionId, got {again:?} after {identity:?}"
        );
    }
    assert_ne!(
        draft_identity, release_identity,
        "each app version's mapping is resolved on its own, so the draft and the \
         release must not report one resolutionId"
    );

    // The identity tracks the MAPPING: changing the draft changes the draft's,
    // and leaves the frozen release's alone.
    add_a_resource(&router, &app_arn, "payments").await;
    let draft_after = resolution_of(&router, &app_arn, DRAFT_APP_VERSION, "the changed draft").await;
    assert_ne!(
        draft_after, draft_identity,
        "case \"the changed draft\": a mapping that changed is a different resolution"
    );
    assert_eq!(
        resolution_of(&router, &app_arn, &published, "the untouched release").await,
        release_identity,
        "case \"the untouched release\": changing the draft must not change what the \
         release reports"
    );

    // Once a version has a resolution of its own, that is what the listing is
    // against — addressed by its id or as the version's current one.
    let resolution_id = resolve(&router, &app_arn, DRAFT_APP_VERSION).await;
    assert_eq!(
        resolution_of(&router, &app_arn, DRAFT_APP_VERSION, "a resolved draft").await,
        resolution_id,
        "case \"a resolved draft\": the listing reports the version's current resolution"
    );
    let named = list_resources(
        &router,
        &app_arn,
        DRAFT_APP_VERSION,
        &json!({"resolutionId": resolution_id}),
    )
    .await;
    assert_eq!(
        named.ok("a named resolution")["resolutionId"],
        json!(resolution_id),
        "case \"a named resolution\": the listing echoes the resolution it was asked \
         for, got {:?}",
        named.body
    );
    assert_eq!(
        listed_order(&physical_resources(&named, "a named resolution")),
        listed_order(&physical_resources(
            &list_resources(&router, &app_arn, DRAFT_APP_VERSION, &json!({})).await,
            "a resolved draft",
        )),
        "case \"a named resolution\": naming the version's current resolution reports \
         the same mapping as naming none"
    );

    // A resolution the addressed version does not have: ids at each end of the
    // modelled length, one shaped like a minted id, and the resolution of ANOTHER
    // version of the same application.
    for resolution_id in [
        "r".to_string(),
        "r".repeat(255),
        "00000000-0000-0000-0000-000000000000".to_string(),
        resolution_id.clone(),
    ] {
        let case = format!("the release has no resolution {resolution_id:?}");
        list_resources(
            &router,
            &app_arn,
            &published,
            &json!({"resolutionId": resolution_id}),
        )
        .await
        .refused(&case, 404, "ResourceNotFoundException");
    }
}

/// Resources are per application and per app version: an unknown application, an
/// app version the application does not have, and a deleted application are each
/// the modelled 404 — never an empty listing, which would claim a version holding
/// nothing.
#[tokio::test]
async fn an_unknown_application_or_version_is_refused_with_the_modeled_404() {
    let router = emulator();
    let app_arn = create_an_app(&router, "resource-listing-not-found").await;
    add_a_resource(&router, &app_arn, "orders").await;
    let published = publish(&router, &app_arn).await;
    let unknown_arn =
        format!("arn:aws:resiliencehub:{REGION}:{ACCOUNT}:app/00000000-0000-0000-0000-000000000000");

    // The versions this application does NOT have: one past its only publish, a
    // label no publish minted, and a 50-character version at the top of
    // `EntityVersion`'s modelled pattern `^\S{1,50}$`.
    let longest_version = "v".repeat(50);
    for app_version in ["2", "no-such-version", longest_version.as_str()] {
        let case = format!("version {app_version:?}");
        list_resources(&router, &app_arn, app_version, &json!({}))
            .await
            .refused(&case, 404, "ResourceNotFoundException");
    }

    for app_version in [DRAFT_APP_VERSION, published.as_str()] {
        let case = format!("an unknown application, version {app_version:?}");
        list_resources(&router, &unknown_arn, app_version, &json!({}))
            .await
            .refused(&case, 404, "ResourceNotFoundException");
    }

    // Another application's resources are not this one's.
    let other_arn = create_an_app(&router, "resource-listing-other").await;
    add_a_resource(&router, &other_arn, "other-orders").await;
    assert_eq!(
        held_by(&router, &other_arn, DRAFT_APP_VERSION, "another application").await,
        vec!["other-orders".to_string()],
        "case \"another application\": each application reports only its own resources"
    );

    // And nothing of a deleted application's versions outlives it.
    post(&router, "/delete-app", &json!({"appArn": app_arn}))
        .await
        .ok("delete the application");
    for app_version in [DRAFT_APP_VERSION, published.as_str()] {
        let case = format!("a deleted application, version {app_version:?}");
        list_resources(&router, &app_arn, app_version, &json!({}))
            .await
            .refused(&case, 404, "ResourceNotFoundException");
    }
}

/// A row of the removal table: which of the three selectors
/// `DeleteAppVersionResource` addresses the resource by.
///
/// The rows differ along what the rule generalises over — the member the request
/// addresses the resource with, and whether a `physicalResourceId` is narrowed by
/// the account and region it lives in.
struct Removal {
    label: &'static str,
    /// The request members that address `identifier`, beyond `appArn`.
    selector: fn(&str) -> Value,
}

/// The table: one selector per row, each run against its own application.
fn removal_table() -> Vec<Removal> {
    vec![
        Removal {
            label: "by resourceName",
            selector: |identifier| json!({"resourceName": identifier}),
        },
        Removal {
            label: "by logicalResourceId",
            selector: |identifier| json!({"logicalResourceId": {"identifier": identifier}}),
        },
        Removal {
            label: "by physicalResourceId",
            selector: |identifier| json!({"physicalResourceId": identifier}),
        },
        Removal {
            label: "by physicalResourceId narrowed by its region and account",
            selector: |identifier| {
                json!({
                    "physicalResourceId": identifier,
                    "awsRegion": REGION,
                    "awsAccountId": ACCOUNT,
                })
            },
        },
    ]
}

/// `DeleteAppVersionResource` removes exactly the resource the request addresses
/// from the DRAFT version, reports the record it removed, leaves every other
/// resource of the draft alone, and is not idempotent: a second delete of one
/// resource finds nothing to delete.
#[tokio::test]
async fn removes_only_the_addressed_resource_from_the_draft() {
    let router = emulator();

    for (index, row) in removal_table().into_iter().enumerate() {
        let case = row.label;
        // One application per row, named within `EntityName`'s modelled pattern.
        let app_arn = create_an_app(&router, &format!("remove-selector-{index}")).await;
        for identifier in ["orders", "payments", "shipping"] {
            add_a_resource(&router, &app_arn, identifier).await;
        }

        let removed = post(
            &router,
            "/delete-app-version-resource",
            &{
                let mut body = (row.selector)("payments");
                body["appArn"] = json!(app_arn);
                body
            },
        )
        .await;
        let reported = removed.ok(case).clone();
        assert_eq!(
            reported["appVersion"],
            json!(DRAFT_APP_VERSION),
            "case {case:?}: the delete updates the draft version, got {reported:?}"
        );
        assert_eq!(
            reported["appArn"], json!(app_arn),
            "case {case:?}: the delete echoes the application it removed from, got {reported:?}"
        );
        assert_eq!(
            reported["physicalResource"]["logicalResourceId"]["identifier"],
            json!("payments"),
            "case {case:?}: the delete reports the record it removed, got {reported:?}"
        );
        assert_eq!(
            reported["physicalResource"]["resourceType"],
            json!("AWS::S3::Bucket"),
            "case {case:?}: the removed record is the one that was stored, got {reported:?}"
        );

        // What the draft holds afterwards: everything but the removed resource.
        assert_eq!(
            held_by(&router, &app_arn, DRAFT_APP_VERSION, case).await,
            vec!["orders".to_string(), "shipping".to_string()],
            "case {case:?}: only the addressed resource is removed"
        );
        // And the reader agrees the resource is gone.
        post(
            &router,
            "/describe-app-version-resource",
            &json!({
                "appArn": app_arn,
                "appVersion": DRAFT_APP_VERSION,
                "resourceName": "payments",
            }),
        )
        .await
        .refused(case, 404, "ResourceNotFoundException");

        // Deleting it again finds nothing to delete.
        let mut again = (row.selector)("payments");
        again["appArn"] = json!(app_arn);
        post(&router, "/delete-app-version-resource", &again)
            .await
            .refused(case, 404, "ResourceNotFoundException");
    }
}

/// A delete reaches the draft only: a published version keeps the resource it
/// froze, and the next publish is what carries the removal into a release.
#[tokio::test]
async fn a_delete_cannot_reach_a_published_version() {
    let router = emulator();
    let app_arn = create_an_app(&router, "remove-after-publish").await;
    add_a_resource(&router, &app_arn, "orders").await;
    add_a_resource(&router, &app_arn, "payments").await;
    publish(&router, &app_arn).await;

    post(
        &router,
        "/delete-app-version-resource",
        &json!({"appArn": app_arn, "resourceName": "payments"}),
    )
    .await
    .ok("remove payments from the draft");

    assert_eq!(
        held_by(&router, &app_arn, DRAFT_APP_VERSION, "the reduced draft").await,
        vec!["orders".to_string()],
        "case \"the reduced draft\": the draft loses the removed resource"
    );
    assert_eq!(
        held_by(&router, &app_arn, RELEASE_APP_VERSION, "the frozen release").await,
        vec!["orders".to_string(), "payments".to_string()],
        "case \"the frozen release\": a published version keeps what it froze"
    );

    // Publishing again is what carries the removal forward: the new release holds
    // the reduced mapping, and the superseded snapshot still reports its own.
    publish(&router, &app_arn).await;
    assert_eq!(
        held_by(&router, &app_arn, RELEASE_APP_VERSION, "the second release").await,
        vec!["orders".to_string()],
        "case \"the second release\": the newest publish freezes the reduced draft"
    );
    assert_eq!(
        held_by(&router, &app_arn, "1", "the superseded version").await,
        vec!["orders".to_string(), "payments".to_string()],
        "case \"the superseded version\": a numbered version is immutable as published"
    );
}

/// A delete has to address exactly one resource of one application: naming no
/// selector or two of them is the modelled 400, and an unknown resource or
/// application the modelled 404.
#[tokio::test]
async fn an_unaddressable_delete_is_refused() {
    let router = emulator();
    let app_arn = create_an_app(&router, "remove-refusals").await;
    add_a_resource(&router, &app_arn, "orders").await;
    let unknown_arn =
        format!("arn:aws:resiliencehub:{REGION}:{ACCOUNT}:app/00000000-0000-0000-0000-000000000000");

    for (case, request) in [
        ("no selector at all", json!({})),
        (
            "two selectors",
            json!({"resourceName": "orders", "physicalResourceId": "orders"}),
        ),
        (
            "all three selectors",
            json!({
                "resourceName": "orders",
                "logicalResourceId": {"identifier": "orders"},
                "physicalResourceId": "orders",
            }),
        ),
        (
            "a region with nothing to narrow",
            json!({"resourceName": "orders", "awsRegion": REGION}),
        ),
        (
            "an account with nothing to narrow",
            json!({"resourceName": "orders", "awsAccountId": ACCOUNT}),
        ),
    ] {
        let mut body = request;
        body["appArn"] = json!(app_arn);
        post(&router, "/delete-app-version-resource", &body)
            .await
            .refused(case, 400, "ValidationException");
    }

    for (case, body) in [
        (
            "a resource the draft does not hold",
            json!({"appArn": app_arn, "resourceName": "no-such-resource"}),
        ),
        (
            "a physical id narrowed to another region",
            json!({
                "appArn": app_arn,
                "physicalResourceId": "orders",
                "awsRegion": "eu-west-1",
            }),
        ),
        (
            "an application that was never created",
            json!({"appArn": unknown_arn, "resourceName": "orders"}),
        ),
    ] {
        post(&router, "/delete-app-version-resource", &body)
            .await
            .refused(case, 404, "ResourceNotFoundException");
    }

    // The refused calls removed nothing.
    assert_eq!(
        held_by(&router, &app_arn, DRAFT_APP_VERSION, "after the refusals").await,
        vec!["orders".to_string()],
        "case \"after the refusals\": a refused delete removes nothing"
    );
}

/// Every listed app version carries its `identifier`, so a client can key the
/// listing by that member without a null guard: the draft's is
/// `DRAFT_APP_VERSION_IDENTIFIER`, and a published version's is the publish
/// sequence number it was minted with.
#[tokio::test]
async fn every_listed_app_version_carries_its_identifier() {
    let router = emulator();
    let app_arn = create_an_app(&router, "version-identifiers").await;
    add_a_resource(&router, &app_arn, "orders").await;

    // Row after row of publishes against ONE application, so the table spans a
    // listing of one version, of two, and of several superseded ones.
    for publishes in 1..=4i64 {
        let case = format!("after publish number {publishes}");
        let minted = post(&router, "/publish-app-version", &json!({"appArn": app_arn})).await;
        assert_eq!(
            minted.ok(&case)["identifier"],
            json!(publishes),
            "case {case:?}: the publish reports its sequence number, got {:?}",
            minted.body
        );

        let listed = post(&router, "/list-app-versions", &json!({"appArn": app_arn})).await;
        let summaries = listed.ok(&case)["appVersions"]
            .as_array()
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: appVersions is @required, got {:?}",
                    listed.body
                )
            })
            .clone();

        // What the check a client writes does: key the listing by `identifier`.
        let mut by_version: BTreeMap<String, i64> = BTreeMap::new();
        for summary in &summaries {
            let app_version = summary["appVersion"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: appVersion is @required, got {summary:?}")
                })
                .to_string();
            let identifier = summary["identifier"].as_i64().unwrap_or_else(|| {
                panic!(
                    "case {case:?}: every AppVersionSummary answers identifier, but \
                     {app_version:?} reported {:?}",
                    summary["identifier"]
                )
            });
            by_version.insert(app_version, identifier);
        }

        // The draft sits below every publish sequence number; the newest publish is
        // the release alias; every superseded version is addressed by, and reports,
        // its own number.
        let mut expected: BTreeMap<String, i64> = BTreeMap::new();
        expected.insert(
            DRAFT_APP_VERSION.to_string(),
            DRAFT_APP_VERSION_IDENTIFIER,
        );
        expected.insert(RELEASE_APP_VERSION.to_string(), publishes);
        for superseded in 1..publishes {
            expected.insert(superseded.to_string(), superseded);
        }
        assert_eq!(
            by_version, expected,
            "case {case:?}: expected {expected:?}, got {by_version:?}"
        );
    }
}
