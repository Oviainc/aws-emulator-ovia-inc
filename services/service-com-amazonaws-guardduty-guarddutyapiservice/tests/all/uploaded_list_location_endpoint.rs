// SPDX-License-Identifier: CC0-1.0

//! Where an uploaded list's `Location` may point, and what a change to a list whose
//! delete is already running answers, end to end over the wire.
//!
//! Every request goes through [`smithy_server_runtime::MultiServiceRouter::dispatch`]
//! (the harness lives in the `detector_lifecycle` suite), so routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers are exercised
//! together.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than over one
//! recorded call:
//!
//! 1. An uploaded list's `Location` is accepted when it addresses an OBJECT — a
//!    bucket and a non-empty key — at the Amazon S3 endpoint serving the caller: the
//!    three public S3 URL spellings, and the path-style object URL on the endpoint
//!    authority the request itself arrived at, which is the S3 endpoint a caller of
//!    this emulator reaches. A URL naming any other authority, or naming no object,
//!    is refused with HTTP 400 under the `InvalidInputException` class and stores
//!    nothing. The rule is the uploaded-list FAMILY's, so the rows span all four
//!    families — the IPSet, the threat list and the two entity sets — on the create
//!    that stores a location, and the stored value reads back verbatim.
//! 2. A change to a list whose own delete has already been accepted collides with
//!    work in flight rather than naming an absent resource, so `UpdateIPSet` and
//!    `UpdateThreatIntelSet` refuse it under the `InternalServerErrorException` class
//!    with the 400 of a request the service will not perform, carrying
//!    `state::OPERATION_IN_PROGRESS_MESSAGE` — distinct, in both the class and the
//!    sentence, from the absence an id the detector never held gets — and the refused
//!    change moves neither the list's values nor its status.

use serde_json::{json, Value};

use crate::detector_lifecycle::{
    call, call_at, enabled_detector, error_code, error_shape, new_router, Router,
};
use service_com_amazonaws_guardduty_guarddutyapiservice::state::{
    IP_SET_TRANSITION_MILLIS, OPERATION_IN_PROGRESS_MESSAGE, SET_ID_NOT_FOUND_MESSAGE,
};

/// The error class every client rejection on this service carries in the body's
/// `__type` discriminator.
const INVALID_INPUT: &str = "InvalidInputException";

/// The error class a change that collides with work already running on the list
/// carries, in the body's `__type` and in the `x-amzn-errortype` header that resolves
/// the error shape.
const IN_PROGRESS_CLASS: &str = "InternalServerErrorException";

/// The endpoint authority an emulated AWS endpoint is reached at when it is not a
/// public one: a loopback address and a port, which is the form a client configures as
/// its endpoint URL — and so the authority a request arrives at.
const LOOPBACK_ENDPOINT: &str = "127.0.0.1:9297";

/// A second spelling of a non-public endpoint, so the rule is not read off one host.
const NAMED_ENDPOINT: &str = "localhost:4566";

/// An endpoint reached on the port the `https` scheme implies, named explicitly in
/// the authority — and the same endpoint with that port left out. One endpoint, two
/// legal spellings.
const DEFAULT_PORT_ENDPOINT: &str = "gd-endpoint.internal:443";
/// See [`DEFAULT_PORT_ENDPOINT`].
const PORTLESS_ENDPOINT: &str = "gd-endpoint.internal";

/// One uploaded-list family: the route segment its sub-resource hangs off, the
/// `Format` value a create needs and the `jsonName` of the id the create reports.
struct Family {
    case: &'static str,
    segment: &'static str,
    id_member: &'static str,
    format: &'static str,
}

/// The four uploaded-list families. The `Format` values are members of `IpSetFormat`,
/// `ThreatIntelSetFormat`, `ThreatEntitySetFormat` and `TrustedEntitySetFormat`
/// (`smithy_query(shape="IpSetFormat,ThreatIntelSetFormat,ThreatEntitySetFormat,TrustedEntitySetFormat")`).
fn families() -> Vec<Family> {
    vec![
        Family {
            case: "ipset",
            segment: "ipset",
            id_member: "ipSetId",
            format: "TXT",
        },
        Family {
            case: "threatintelset",
            segment: "threatintelset",
            id_member: "threatIntelSetId",
            format: "STIX",
        },
        Family {
            case: "threatentityset",
            segment: "threatentityset",
            id_member: "threatEntitySetId",
            format: "TXT",
        },
        Family {
            case: "trustedentityset",
            segment: "trustedentityset",
            id_member: "trustedEntitySetId",
            format: "TXT",
        },
    ]
}

/// One `Location` case: the endpoint the request is addressed to, the location it
/// carries, and whether the service may store it.
struct LocationCase {
    why: &'static str,
    endpoint: &'static str,
    location: String,
    accepted: bool,
}

/// The locations, spanning what the rule generalises over: the SPELLING of the S3
/// address (the `s3://` scheme, the path-style public endpoint, the virtual-hosted
/// one, and the path-style object URL on the endpoint the request arrived at), the
/// ENDPOINT the request is addressed to and how its authority spells the port, the
/// DEPTH of the object key, and the ways a value fails to address an object at all
/// (another authority, another port, no key, another scheme).
///
/// The catalogue of PUBLIC S3 spellings, and of the values that are no S3 URL at all,
/// is the `ip_set_location_syntax` suite's; the rows here carry one of each as a
/// control and own the endpoint-relative half of the rule.
fn location_cases() -> Vec<LocationCase> {
    vec![
        LocationCase {
            why: "an s3:// URL",
            endpoint: LOOPBACK_ENDPOINT,
            location: "s3://my-bucket/lists/trusted.txt".to_string(),
            accepted: true,
        },
        LocationCase {
            why: "a path-style public S3 endpoint",
            endpoint: LOOPBACK_ENDPOINT,
            location: "https://s3.eu-west-1.amazonaws.com/my-bucket/lists/trusted.txt".to_string(),
            accepted: true,
        },
        LocationCase {
            why: "a virtual-hosted-style public S3 endpoint",
            endpoint: LOOPBACK_ENDPOINT,
            location: "https://my-bucket.s3.us-west-2.amazonaws.com/lists/trusted.txt".to_string(),
            accepted: true,
        },
        LocationCase {
            why: "an object URL on the loopback endpoint the request arrived at",
            endpoint: LOOPBACK_ENDPOINT,
            location: format!("https://{LOOPBACK_ENDPOINT}/my-bucket/lists/trusted.txt"),
            accepted: true,
        },
        LocationCase {
            why: "an object URL on the named endpoint the request arrived at",
            endpoint: NAMED_ENDPOINT,
            location: format!("https://{NAMED_ENDPOINT}/my-bucket/trusted.txt"),
            accepted: true,
        },
        LocationCase {
            why: "an object URL on that endpoint with a deep key",
            endpoint: NAMED_ENDPOINT,
            location: format!("https://{NAMED_ENDPOINT}/my-bucket/lists/2026/03/trusted.txt"),
            accepted: true,
        },
        LocationCase {
            why: "an object URL omitting the default port the request named",
            endpoint: DEFAULT_PORT_ENDPOINT,
            location: format!("https://{PORTLESS_ENDPOINT}/my-bucket/trusted.txt"),
            accepted: true,
        },
        LocationCase {
            why: "an object URL naming the default port the request omitted",
            endpoint: PORTLESS_ENDPOINT,
            location: format!("https://{DEFAULT_PORT_ENDPOINT}/my-bucket/trusted.txt"),
            accepted: true,
        },
        LocationCase {
            why: "an object URL on an endpoint the request did not arrive at",
            endpoint: LOOPBACK_ENDPOINT,
            location: format!("https://{NAMED_ENDPOINT}/my-bucket/trusted.txt"),
            accepted: false,
        },
        LocationCase {
            why: "the endpoint's host on another port than the one the request named",
            endpoint: LOOPBACK_ENDPOINT,
            location: "https://127.0.0.1/my-bucket/trusted.txt".to_string(),
            accepted: false,
        },
        LocationCase {
            why: "the endpoint the request arrived at, naming only a bucket",
            endpoint: LOOPBACK_ENDPOINT,
            location: format!("https://{LOOPBACK_ENDPOINT}/my-bucket"),
            accepted: false,
        },
        LocationCase {
            why: "the endpoint the request arrived at, with an empty key",
            endpoint: LOOPBACK_ENDPOINT,
            location: format!("https://{LOOPBACK_ENDPOINT}/my-bucket/"),
            accepted: false,
        },
        LocationCase {
            why: "that endpoint over http rather than https",
            endpoint: LOOPBACK_ENDPOINT,
            location: format!("http://{LOOPBACK_ENDPOINT}/my-bucket/trusted.txt"),
            accepted: false,
        },
        LocationCase {
            why: "an unrelated https host",
            endpoint: LOOPBACK_ENDPOINT,
            location: "https://files.example-corp.com/my-bucket/trusted.txt".to_string(),
            accepted: false,
        },
    ]
}

/// A router holding one enabled detector.
async fn detector() -> (Router, String) {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;
    (router, detector_id)
}

async fn create_at(
    router: &Router,
    endpoint: &str,
    detector_id: &str,
    family: &Family,
    body: Value,
) -> (u16, Value) {
    call_at(
        router,
        endpoint,
        "POST",
        &format!("/detector/{detector_id}/{}", family.segment),
        Some(body),
    )
    .await
}

async fn read(router: &Router, detector_id: &str, family: &Family, id: &str) -> Value {
    let (status, body) = call(
        router,
        "GET",
        &format!("/detector/{detector_id}/{}/{id}", family.segment),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: the stored list must read back, got {status} {body:?}",
        family.case
    );
    body
}

// ---------------------------------------------------------------------------
// Rule 1: a Location addresses an object at the S3 endpoint serving the caller.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_location_is_accepted_exactly_when_it_addresses_an_object_at_the_served_endpoint() {
    for family in families() {
        for (index, row) in location_cases().into_iter().enumerate() {
            // A router per case: a detector holds at most one trusted entity set and
            // the account may have only one ACTIVATED IPSet, and neither allowance is
            // this rule's business.
            let (router, detector_id) = detector().await;
            let case = format!("{} create at {}: {}", family.case, row.endpoint, row.why);

            let (status, body) = create_at(
                &router,
                row.endpoint,
                &detector_id,
                &family,
                json!({
                    "name": format!("located-{index}"),
                    "format": family.format,
                    "location": row.location,
                    "activate": true
                }),
            )
            .await;

            if row.accepted {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the create to be served, got {status} {body:?}"
                );
                let id = body[family.id_member]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: expected an id, got {body:?}"))
                    .to_string();
                // Stored, and stored VERBATIM: the location a caller gave is the one
                // a reader reports.
                let stored = read(&router, &detector_id, &family, &id).await;
                let reported = stored["location"].as_str().unwrap_or_default();
                assert_eq!(
                    reported, row.location,
                    "case {case:?}: expected the location {:?} back, got {reported:?}",
                    row.location
                );
            } else {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected HTTP 400, got {status} {body:?}"
                );
                let class = error_code(&body);
                assert_eq!(
                    class, INVALID_INPUT,
                    "case {case:?}: expected the {INVALID_INPUT} class, got {class:?} {body:?}"
                );
                let message = body["message"].as_str().unwrap_or_default();
                assert!(
                    !message.contains(&row.location),
                    "case {case:?}: a refusal must not echo {:?}, got {message:?}",
                    row.location
                );
                // A refused create stores nothing: the family's collection is empty.
                let (status, listed) = call(
                    &router,
                    "GET",
                    &format!("/detector/{detector_id}/{}", family.segment),
                    None,
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: the listing must be served, got {status} {listed:?}"
                );
                let ids = listed[format!("{}s", family.id_member)]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                assert!(
                    ids.is_empty(),
                    "case {case:?}: a refused create must store no list, got {ids:?}"
                );
            }
        }
    }
}

/// The endpoint a list was uploaded through is the one the UPDATE is read against
/// too: the same location is accepted on an update sent to that endpoint and refused
/// on one sent elsewhere, and the refusal keeps the stored value.
#[tokio::test]
async fn an_update_reads_a_location_against_the_endpoint_it_arrives_at() {
    for family in families() {
        let (router, detector_id) = detector().await;
        let case = format!("{} update", family.case);
        let stored_location = "s3://my-bucket/lists/stored.txt";

        let (status, body) = create_at(
            &router,
            LOOPBACK_ENDPOINT,
            &detector_id,
            &family,
            json!({
                "name": "stored-list",
                "format": family.format,
                "location": stored_location,
                "activate": false
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the baseline create expected 200, got {status} {body:?}"
        );
        let id = body[family.id_member]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected an id, got {body:?}"))
            .to_string();

        let endpoint_location = format!("https://{LOOPBACK_ENDPOINT}/other-bucket/lists/new.txt");
        let path = format!("/detector/{detector_id}/{}/{id}", family.segment);

        // Sent to another endpoint, the same URL addresses a host this emulator does
        // not serve, so it is refused and the stored location stands.
        let (status, refused) = call_at(
            &router,
            NAMED_ENDPOINT,
            "POST",
            &path,
            Some(json!({"location": endpoint_location})),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: an object URL on another endpoint must be refused, got {status} \
             {refused:?}"
        );
        let kept = read(&router, &detector_id, &family, &id).await;
        assert_eq!(
            kept["location"].as_str(),
            Some(stored_location),
            "case {case:?}: a refused update must keep the stored location, got {kept:?}"
        );

        // Sent to the endpoint it names, it is the S3 endpoint serving the caller.
        let (status, updated) = call_at(
            &router,
            LOOPBACK_ENDPOINT,
            "POST",
            &path,
            Some(json!({"location": endpoint_location})),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: an object URL on the served endpoint must be accepted, got {status} \
             {updated:?}"
        );
        let after = read(&router, &detector_id, &family, &id).await;
        assert_eq!(
            after["location"].as_str(),
            Some(endpoint_location.as_str()),
            "case {case:?}: expected the updated location back, got {after:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: a change to a list whose delete is running collides with that delete.
// ---------------------------------------------------------------------------

/// Long enough for the clock to owe a deleted list its terminal status.
fn past_the_deletion() -> std::time::Duration {
    std::time::Duration::from_millis(IP_SET_TRANSITION_MILLIS as u64 + 250)
}

/// The two detector-rooted uploaded lists whose delete RETAINS the record, so the
/// collision is observable at all: the route segment and the `jsonName` of the id the
/// create reports.
const RETAINED: [(&str, &str); 2] = [("ipset", "ipSetId"), ("threatintelset", "threatIntelSetId")];

#[tokio::test]
async fn an_update_of_a_list_whose_delete_is_running_collides_with_it() {
    for (segment, id_member) in RETAINED {
        let case_family = segment;
        let format = if segment == "ipset" { "TXT" } else { "STIX" };
        // The rows: both ends of the deletion's own transition, and three shapes of
        // change — a rename, an activation and a relocation — so the refusal is not a
        // function of what the update asks for.
        for (when, wait) in [
            ("while the delete is still pending", false),
            ("once the deletion has settled", true),
        ] {
            for (shape, update) in [
                ("a rename", json!({"name": "resurrected"})),
                ("an activation", json!({"activate": true})),
                (
                    "a relocation",
                    json!({"location": "s3://my-bucket/lists/moved.txt"}),
                ),
            ] {
                let case = format!("{case_family} {when}: {shape}");
                let (router, detector_id) = detector().await;
                let (status, created) = call(
                    &router,
                    "POST",
                    &format!("/detector/{detector_id}/{segment}"),
                    Some(json!({
                        "name": "doomed-list",
                        "format": format,
                        "location": "s3://my-bucket/lists/doomed.txt",
                        "activate": true
                    })),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: the baseline create expected 200, got {status} {created:?}"
                );
                let id = created[id_member]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: expected an id, got {created:?}"))
                    .to_string();
                let path = format!("/detector/{detector_id}/{segment}/{id}");

                let (status, deleted) = call(&router, "DELETE", &path, None).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: the delete expected 200, got {status} {deleted:?}"
                );
                if wait {
                    tokio::time::sleep(past_the_deletion()).await;
                }
                let before = call(&router, "GET", &path, None).await.1;

                let (status, body) = call(&router, "POST", &path, Some(update.clone())).await;
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected HTTP 400, got {status} {body:?}"
                );
                let class = error_code(&body);
                assert_eq!(
                    class, IN_PROGRESS_CLASS,
                    "case {case:?}: expected the {IN_PROGRESS_CLASS} class, got {class:?} \
                     {body:?}"
                );
                let shape_header = error_shape(&body);
                assert_eq!(
                    shape_header, IN_PROGRESS_CLASS,
                    "case {case:?}: expected the {IN_PROGRESS_CLASS} error shape, got \
                     {shape_header:?} {body:?}"
                );
                let message = body["message"].as_str().unwrap_or_default();
                assert_eq!(
                    message, OPERATION_IN_PROGRESS_MESSAGE,
                    "case {case:?}: expected the in-progress sentence, got {message:?}"
                );
                assert!(
                    !message.contains(id.as_str()),
                    "case {case:?}: the refusal must not echo {id:?}, got {message:?}"
                );

                // The refusal wrote nothing: every member the read reports is the one
                // the delete left behind.
                let after = call(&router, "GET", &path, None).await.1;
                assert_eq!(
                    after, before,
                    "case {case:?}: a refused update must change nothing, expected {before:?}, \
                     got {after:?}"
                );

                // And the collision is NOT the absence refusal: an id the detector
                // never held is still the family's fixed not-found sentence under the
                // client-rejection class.
                let absent = format!(
                    "/detector/{detector_id}/{segment}/0123456789abcdef0123456789abcdef"
                );
                let (status, body) = call(&router, "POST", &absent, Some(update)).await;
                assert_eq!(
                    status, 400,
                    "case {case:?}: an absent id expected HTTP 400, got {status} {body:?}"
                );
                let class = error_code(&body);
                assert_eq!(
                    class, INVALID_INPUT,
                    "case {case:?}: an absent id expected the {INVALID_INPUT} class, got \
                     {class:?} {body:?}"
                );
                let message = body["message"].as_str().unwrap_or_default();
                assert_eq!(
                    message, SET_ID_NOT_FOUND_MESSAGE,
                    "case {case:?}: an absent id expected the not-found sentence, got {message:?}"
                );
            }
        }
    }
}
