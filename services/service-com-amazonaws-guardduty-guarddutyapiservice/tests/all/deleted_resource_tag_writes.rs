// SPDX-License-Identifier: CC0-1.0

//! `TagResource` and `UntagResource` against a resource whose delete has already
//! been ACCEPTED, end to end over the wire.
//!
//! Every request goes through [`smithy_server_runtime::MultiServiceRouter::dispatch`]
//! (the harness lives in the `detector_lifecycle` suite), so routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers are exercised
//! together. The resources come from the `resource_tag_removal` fixture, so this
//! suite and the two that drive the same operations against LIVE resources cannot
//! disagree about which ARN names which resource.
//!
//! The rules under test, each asserted over a SET of inputs rather than one call:
//!
//! 1. A resource whose delete this service has served can no longer be ACTED on:
//!    every write against it is refused exactly like an id the detector never held
//!    — HTTP 400 under the `InvalidInputException` class, with the message a
//!    well-formed ARN this endpoint resolves nothing for gets, which names the
//!    member and echoes nothing of the caller's. That holds whether the record was
//!    dropped by the delete or RETAINED in its terminal `DELETED` status, and
//!    whether or not any span this service opens has elapsed.
//! 2. Reporting is unaffected: a retained record's own `Get` still reports it in
//!    that terminal status, the tag report still answers wherever the store retains
//!    the record, and a refused write leaves that map exactly as it was.
//! 3. The refusal is scoped to the deleted record: every resource still live accepts
//!    both writes and reports the result.

use serde_json::Value;

use crate::detector_lifecycle::{call_in, error_code, new_router, Router};
use crate::resource_tag_addition::tag_in;
use crate::resource_tag_removal::{tag_map, tagged_resources, tags_of, untag_in, uri_label, Tagged};
use service_com_amazonaws_guardduty_guarddutyapiservice::service::refusals::INVALID_RESOURCE_ARN_MESSAGE;
use service_com_amazonaws_guardduty_guarddutyapiservice::state::{
    ENTITY_SET_TRANSITION_MILLIS, IP_SET_DELETED, IP_SET_TRANSITION_MILLIS,
    TAGGED_SUB_RESOURCE_SEGMENTS, THREAT_INTEL_SET_DELETED, THREAT_INTEL_SET_DELETION_SPAN_MILLIS,
};

/// The Region every request in this suite is signed for.
const REGION: &str = "us-east-1";

/// The error class every client rejection on this service carries in the body's
/// `__type` discriminator.
const INVALID_INPUT: &str = "InvalidInputException";

/// The tags every resource in this suite is created with, so a refused write is
/// observable as "this map did not move".
const BASE_TAGS: [(&str, &str); 2] = [("Env", "prod"), ("Reviewed", "")];

/// `BASE_TAGS` as a request/response `TagMap`.
fn base_tags() -> Value {
    tag_map(&BASE_TAGS)
}

/// Long enough for the clock to owe every retained record its settled `DELETED`,
/// plus slack for the wall clock the handlers read. The three families each publish
/// their own span, so the longest of them is what a single wait has to cover.
fn past_every_transition() -> std::time::Duration {
    let longest = IP_SET_TRANSITION_MILLIS
        .max(ENTITY_SET_TRANSITION_MILLIS)
        .max(THREAT_INTEL_SET_DELETION_SPAN_MILLIS);
    std::time::Duration::from_millis(u64::try_from(longest).unwrap_or_default() + 250)
}

/// What one class's own read answers once its delete has been served.
enum OwnRead {
    /// The record is RETAINED and still reported, with this terminal `Status`.
    Reported(&'static str),
    /// The record is no longer one the detector holds, so its own read refuses with
    /// this HTTP status: the entity-set families' 404 `ResourceNotFoundException`, or
    /// the 400 of this service's classified rejection for a record its delete drops.
    Refused(u16),
}

/// One class of resource a tagging ARN can name, and what its delete leaves behind.
struct Class {
    case: &'static str,
    /// The URI segment naming the collection it lives in, empty for the detector
    /// itself. One per entry of `state::TAGGED_SUB_RESOURCE_SEGMENTS`, which
    /// `every_tagged_class_is_covered` pins.
    segment: &'static str,
    /// What its OWN read answers after the delete.
    own_read: OwnRead,
    /// Whether the tag REPORT still resolves it. The tagging seam reports the tags of
    /// a record the store retains, which is every class whose delete does not DROP
    /// the record — so it is reported for the uploaded lists, whose own read reports
    /// them too, and for the entity sets, whose own read no longer resolves the id at
    /// all.
    tags_still_reported: bool,
}

/// Every class a tagging ARN names. A filter, a publishing destination and the
/// detector itself leave nothing behind: their delete drops the record.
const CLASSES: [Class; 7] = [
    Class {
        case: "the detector itself",
        segment: "",
        own_read: OwnRead::Refused(400),
        tags_still_reported: false,
    },
    Class {
        case: "a finding filter",
        segment: "filter",
        own_read: OwnRead::Refused(400),
        tags_still_reported: false,
    },
    Class {
        case: "an IPSet",
        segment: "ipset",
        own_read: OwnRead::Reported(IP_SET_DELETED),
        tags_still_reported: true,
    },
    Class {
        case: "a ThreatIntelSet",
        segment: "threatintelset",
        own_read: OwnRead::Reported(THREAT_INTEL_SET_DELETED),
        tags_still_reported: true,
    },
    Class {
        case: "a trusted entity set",
        segment: "trustedentityset",
        own_read: OwnRead::Refused(404),
        tags_still_reported: true,
    },
    Class {
        case: "a threat entity set",
        segment: "threatentityset",
        own_read: OwnRead::Refused(404),
        tags_still_reported: true,
    },
    Class {
        case: "a publishing destination",
        segment: "publishingDestination",
        own_read: OwnRead::Refused(400),
        tags_still_reported: false,
    },
];

/// The fixture resource `class` names.
fn resource_of<'a>(resources: &'a [Tagged], class: &Class) -> &'a Tagged {
    let found = resources.iter().find(|resource| {
        if class.segment.is_empty() {
            // The detector's own ARN path is `detector/{DetectorId}` and nothing else.
            resource.path.split('/').count() == 2
        } else {
            resource.path.contains(&format!("/{}/", class.segment))
        }
    });
    found.unwrap_or_else(|| {
        panic!(
            "case {:?}: the fixture must create one resource of every class",
            class.case
        )
    })
}

/// The resource's own read: `DELETE`/`GET` of a tagged resource is bound to the very
/// path its ARN carries, so the ARN resource path doubles as both URIs.
async fn read(router: &Router, resource: &Tagged) -> (u16, Value) {
    call_in(router, REGION, "GET", &resource.read_path(), None).await
}

/// Accept the deletion of the resource `resource` names, asserting the service took it.
async fn delete(router: &Router, resource: &Tagged, case: &str) {
    let (status, body) = call_in(router, REGION, "DELETE", &resource.read_path(), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the delete to be accepted with 200, got {status} {body:?}"
    );
}

/// `ListTagsForResource`, bound to `GET /tags/{ResourceArn}` with a NON-GREEDY URI
/// label.
async fn list_tags(router: &Router, arn: &str) -> (u16, Value) {
    call_in(router, REGION, "GET", &format!("/tags/{}", uri_label(arn)), None).await
}

/// Assert one write answered the refusal a well-formed ARN this endpoint resolves
/// nothing for gets.
fn assert_refused(case: &str, arn: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400, got {status} {body:?}"
    );
    let class = error_code(body);
    assert_eq!(
        class, INVALID_INPUT,
        "case {case:?}: expected the {INVALID_INPUT} class, got {class:?} {body:?}"
    );
    let expected = INVALID_RESOURCE_ARN_MESSAGE;
    let message = body["message"].as_str().unwrap_or_default();
    assert_eq!(
        message, expected,
        "case {case:?}: expected the refusal {expected:?}, got {message:?}"
    );
    assert!(
        !message.contains(arn),
        "case {case:?}: the refusal must not echo {arn:?}, got {message:?}"
    );
}

// ---------------------------------------------------------------------------
// Rules 1 + 2: both writes are refused on a sub-resource whose delete was
// accepted, in either deletion status, and the report of its tags does not move.
// ---------------------------------------------------------------------------

/// The sub-resource classes, deleted together in one store: they are independent of
/// each other, so one fixture covers every class and one wait covers every span.
///
/// `settled` chooses WHEN the writes arrive: `false` straight after the delete,
/// `true` once every span this service opens would have come due — the answer is the
/// same either way, which is the point.
async fn refuses_every_write_on_a_deleted_sub_resource(settled: bool) {
    let router = new_router();
    let (_, resources) = tagged_resources(&router, REGION, &base_tags()).await;
    let phase = if settled {
        "once every span would have come due"
    } else {
        "straight after the delete"
    };

    let sub_resources: Vec<&Class> = CLASSES
        .iter()
        .filter(|class| !class.segment.is_empty())
        .collect();
    for class in &sub_resources {
        let resource = resource_of(&resources, class);
        delete(&router, resource, class.case).await;
    }
    if settled {
        tokio::time::sleep(past_every_transition()).await;
    }

    let untouched = base_tags();
    for class in &sub_resources {
        let resource = resource_of(&resources, class);
        let arn = resource.arn(REGION);
        let case = format!("{} {phase}", class.case);

        // TagResource.
        let (status, body) = tag_in(&router, REGION, &arn, &tag_map(&[("Owner", "me")])).await;
        assert_refused(&format!("tagging {case}"), &arn, status, &body);

        // UntagResource, against a key the resource really carries — so an accepted
        // call would be observable rather than a no-op.
        let (status, body) = untag_in(&router, REGION, &arn, &["Env"]).await;
        assert_refused(&format!("untagging {case}"), &arn, status, &body);

        // Its own read: the families whose delete RETAINS a readable record report
        // the terminal status, and the families whose delete takes the record off
        // the detector refuse the id with their own absence refusal.
        let (status, body) = read(&router, resource).await;
        match class.own_read {
            OwnRead::Reported(expected_status) => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: a retained record is still reported, got {status} {body:?}"
                );
                let actual_status = body["status"].as_str().unwrap_or_default();
                assert_eq!(
                    actual_status, expected_status,
                    "case {case:?}: expected the status {expected_status:?}, got {actual_status:?}"
                );
            }
            OwnRead::Refused(expected_status) => assert_eq!(
                status, expected_status,
                "case {case:?}: a record its delete took off the detector must refuse its own \
                 read with {expected_status}, got {status} {body:?}"
            ),
        }

        // The refused writes left its tag map exactly as the create stored it,
        // wherever that map is still reported.
        let (status, body) = list_tags(&router, &arn).await;
        if class.tags_still_reported {
            assert_eq!(
                status, 200,
                "case {case:?}: a retained record's tags are still reported, got {status} {body:?}"
            );
            let listed = body.get("tags").cloned().unwrap_or(Value::Null);
            assert_eq!(
                listed, untouched,
                "case {case:?}: expected the reported tags {untouched:?}, got {listed:?}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: a dropped record has no tags to report, got {status} {body:?}"
            );
        }
        if matches!(class.own_read, OwnRead::Reported(_)) {
            let actual = tags_of(&router, REGION, resource, &case).await;
            assert_eq!(
                actual, untouched,
                "case {case:?}: a refused write must leave the tags as they were, expected \
                 {untouched:?}, got {actual:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_write_to_a_sub_resource_with_a_pending_deletion_is_refused() {
    refuses_every_write_on_a_deleted_sub_resource(false).await;
}

#[tokio::test]
async fn a_write_to_a_sub_resource_whose_deletion_settled_is_refused() {
    refuses_every_write_on_a_deleted_sub_resource(true).await;
}

// ---------------------------------------------------------------------------
// Rule 1, for the detector: its delete takes everything hanging off it, so every
// ARN under it resolves nothing.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_write_to_any_resource_of_a_deleted_detector_is_refused() {
    let router = new_router();
    let (_, resources) = tagged_resources(&router, REGION, &base_tags()).await;

    let detector_class = CLASSES
        .iter()
        .find(|class| class.segment.is_empty())
        .expect("the detector is one of the classes");
    delete(
        &router,
        resource_of(&resources, detector_class),
        detector_class.case,
    )
    .await;

    for class in &CLASSES {
        let resource = resource_of(&resources, class);
        let arn = resource.arn(REGION);
        let case = format!("{} of a deleted detector", class.case);

        let (status, body) = tag_in(&router, REGION, &arn, &tag_map(&[("Owner", "me")])).await;
        assert_refused(&format!("tagging {case}"), &arn, status, &body);

        let (status, body) = untag_in(&router, REGION, &arn, &["Env"]).await;
        assert_refused(&format!("untagging {case}"), &arn, status, &body);
    }
}

// ---------------------------------------------------------------------------
// Rule 3: the refusal reaches the deleted record only.
// ---------------------------------------------------------------------------

/// One sub-resource is deleted; every OTHER resource of the same detector still
/// takes both writes and reports the result. The rows differ along WHICH class was
/// deleted, so no class's refusal can leak onto its siblings.
#[tokio::test]
async fn every_live_resource_still_takes_both_writes() {
    for deleted in CLASSES.iter().filter(|class| !class.segment.is_empty()) {
        // One store per row, so what a read reports is this row's work.
        let router = new_router();
        let (_, resources) = tagged_resources(&router, REGION, &base_tags()).await;
        let target = resource_of(&resources, deleted);
        delete(&router, target, deleted.case).await;

        for resource in &resources {
            if resource.path == target.path {
                continue;
            }
            let case = format!("{} while {} is deleted", resource.case, deleted.case);
            let arn = resource.arn(REGION);

            let (status, body) = tag_in(&router, REGION, &arn, &tag_map(&[("Owner", "me")])).await;
            assert_eq!(
                status, 204,
                "case {case:?}: a live resource still takes a tag, got {status} {body:?}"
            );
            let (status, body) = untag_in(&router, REGION, &arn, &["Env"]).await;
            assert_eq!(
                status, 204,
                "case {case:?}: a live resource still takes an untag, got {status} {body:?}"
            );

            let expected = tag_map(&[("Owner", "me"), ("Reviewed", "")]);
            let actual = tags_of(&router, REGION, resource, &case).await;
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }
}

/// Every class a tagging ARN names is covered above: one `CLASSES` row per
/// `state::TAGGED_SUB_RESOURCE_SEGMENTS` entry plus the detector itself, so a
/// segment this suite does not classify fails here rather than passing silently.
#[test]
fn every_tagged_class_is_covered() {
    assert_eq!(
        CLASSES.len(),
        TAGGED_SUB_RESOURCE_SEGMENTS.len() + 1,
        "every tagged class must be classified here, got {} for {} segments",
        CLASSES.len(),
        TAGGED_SUB_RESOURCE_SEGMENTS.len()
    );
    for (segment, _) in TAGGED_SUB_RESOURCE_SEGMENTS {
        let matching = CLASSES
            .iter()
            .filter(|class| class.segment == segment)
            .count();
        assert_eq!(
            matching, 1,
            "case {segment:?}: exactly one class must name this segment, got {matching}"
        );
    }
    let detectors = CLASSES
        .iter()
        .filter(|class| class.segment.is_empty())
        .count();
    assert_eq!(
        detectors, 1,
        "exactly one class is the detector itself, got {detectors}"
    );
}
