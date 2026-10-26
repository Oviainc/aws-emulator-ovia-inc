// SPDX-License-Identifier: CC0-1.0

//! `PutAnomalyDetector` bringing a detector into existence, end to end over HTTP. (The
//! same operation REPLACING the specification of a detector the workspace already holds
//! is `tests/all/anomaly_detector_update.rs`.)
//!
//! The rule under test: this operation stores the specification the request carries at the
//! `{anomalyDetectorId}` its path addresses, so an id the workspace holds NOTHING for is
//! created rather than refused — "the operation creates a new anomaly detector if one
//! doesn't exist, or updates an existing one" — and every reader of a detector then serves
//! it: the created record reports the addressed id as its `@required` user-friendly name
//! (the only name its caller gave it, this request carrying no `alias` member), carries no
//! tags, and is `CREATING` until the clock owes its first training. The parent rules are
//! the ones the operation that creates detectors applies: a `{workspaceId}` that resolves
//! to nothing is a 404, a workspace whose own deletion has started is a 409, and a
//! specification the service refuses stores nothing at all.
//!
//! Each suite spans what the rule generalises over — the FORM of the addressed id across
//! `AnomalyDetectorId`'s published length bound and the characters its pattern allows, the
//! configuration's members and their documented defaults, the evaluation interval, the
//! `missingDataAction` union, the number of metric labels, the workspace the id is
//! addressed in, and the signing region — rather than one request's values.
//!
//! The harness (`Amp`, `text`, `wait_for_due_transition`), the `Spec` builder and the
//! route and persistence helpers are the ones the other suites in this target already
//! build.

use crate::anomaly_detector_lifecycle::assert_well_formed_detector_arn;
use crate::anomaly_detector_listing::list_detectors;
use crate::anomaly_detector_update::{
    assert_persisted, describe_detector, instant, put_detector, Spec,
};
use crate::workspace_lifecycle::{tags_of, text, wait_for_due_transition, Amp, Reply};
use service_com_amazonaws_amp_amazonprometheusservice::state::{
    workspace_refuses_new_detector_message, STATUS_ACTIVE, STATUS_CREATING, STATUS_DELETING,
    STATUS_UPDATING, WORKSPACE_RESOURCE_TYPE,
};
use serde_json::{json, Value};

/// `AnomalyDetectorId` is modelled `len 1..64` with pattern
/// `^ad-[0-9A-Za-z][-.0-9A-Z_a-z]*$` (`PutAnomalyDetectorRequest$anomalyDetectorId`), so
/// the longest id a caller may address is the prefix plus 61 more characters.
const DETECTOR_ID_MAX_LENGTH: usize = 64;

/// The longest id that bound allows, and one character past it.
fn detector_id_of_length(length: usize) -> String {
    let mut id = "ad-".to_string();
    while id.len() < length {
        id.push('a');
    }
    id
}

/// The summary the listing carries for one detector, if it carries one at all.
fn summary_of(reply: &Reply, detector_id: &str) -> Option<Value> {
    reply.body["anomalyDetectors"]
        .as_array()?
        .iter()
        .find(|entry| text(&entry["anomalyDetectorId"]) == detector_id)
        .cloned()
}

/// An id the addressed workspace holds nothing for is CREATED: the call is accepted, the
/// record holds the specification it carried, the describe and the listing both serve it
/// under the id as its user-friendly name, and the clock owes its first training.
///
/// One row per thing the rule generalises over: the form of the addressed id (the UUID
/// shape the service itself mints, the shortest the pattern allows, the longest the length
/// bound allows, one carrying the `.` and `_` the pattern allows), the configuration
/// (minimal so the documented defaults show, full with both `ignoreNearExpected` unions),
/// the evaluation interval (omitted so the modelled default applies, and at each published
/// bound), the `missingDataAction` union (absent and each member), the label count and the
/// signing region.
#[tokio::test]
async fn put_creates_a_detector_whose_id_names_nothing() {
    struct Case {
        name: &'static str,
        region: &'static str,
        detector_id: String,
        spec: Spec,
    }

    let cases = vec![
        Case {
            name: "the UUID shape the service itself mints, minimal specification",
            region: "us-east-1",
            detector_id: "ad-11111111-1111-4111-8111-111111111111".to_string(),
            spec: Spec::new("sum(rate(requests_total[5m]))"),
        },
        Case {
            name: "the shortest id the pattern allows, every optional member set",
            region: "eu-west-1",
            detector_id: "ad-7".to_string(),
            spec: Spec::new("quantile(0.99, rate(http_request_duration_seconds[1m]))")
                .sizes(16, 512)
                .ignore_above(json!({ "amount": 2.5 }))
                .ignore_below(json!({ "ratio": 0.125 }))
                .interval(30)
                .missing_data_action(json!({ "skip": true }))
                .labels(&[("service", "checkout"), ("tier", "edge")]),
        },
        Case {
            name: "the longest id the length bound allows",
            region: "ap-southeast-2",
            detector_id: detector_id_of_length(DETECTOR_ID_MAX_LENGTH),
            spec: Spec::new("max without (instance) (up)")
                .interval(86_400)
                .missing_data_action(json!({ "markAsAnomaly": true }))
                .labels(&[("shard", "7")]),
        },
        Case {
            name: "an id carrying the dot and the underscore the pattern allows",
            region: "us-west-2",
            detector_id: "ad-Detector.one_2".to_string(),
            spec: Spec::new("stddev(rate(errors_total[5m]))").sizes(2, 256),
        },
    ];

    for case in cases {
        let name = case.name;
        let amp = Amp::new();
        let workspace_id = amp.active_workspace(case.region, "detectors").await;

        // Nothing holds this id before the call, which is what makes the call a create.
        let absent = describe_detector(&amp, case.region, &workspace_id, &case.detector_id).await;
        assert_eq!(
            absent.status, 404,
            "case {name:?}: the id must name nothing before the put, got {absent:?}"
        );

        let put = put_detector(
            &amp,
            case.region,
            &workspace_id,
            &case.detector_id,
            &case.spec.put_body(),
        )
        .await;
        assert_eq!(
            put.status, 202,
            "case {name:?}: PutAnomalyDetector must answer 202, got {put:?}"
        );
        assert_eq!(
            text(&put.body["anomalyDetectorId"]),
            case.detector_id,
            "case {name:?}: the created detector carries the addressed id, got {put:?}"
        );
        assert_well_formed_detector_arn(
            name,
            &text(&put.body["arn"]),
            case.region,
            &workspace_id,
            &case.detector_id,
        );
        assert_eq!(
            text(&put.body["status"]["statusCode"]),
            STATUS_CREATING,
            "case {name:?}: a detector that has just been created is not evaluating yet, \
             got {put:?}"
        );
        assert!(
            tags_of(&put.body["tags"]).is_empty(),
            "case {name:?}: this operation carries no tags, so the created detector holds \
             none, got {put:?}"
        );

        // The record itself: the specification the call carried, the addressed id as the
        // user-friendly name every reader of a detector must report, and no tags.
        let described = describe_detector(&amp, case.region, &workspace_id, &case.detector_id).await;
        assert_persisted(name, &described, &case.spec, STATUS_CREATING);
        let detector = &described.body["anomalyDetector"];
        assert_eq!(
            text(&detector["alias"]),
            case.detector_id,
            "case {name:?}: a request that carries no alias leaves the addressed id as the \
             detector's name, got {detector:?}"
        );
        assert_eq!(
            text(&detector["arn"]),
            text(&put.body["arn"]),
            "case {name:?}: the describe must report the ARN the create answered with, \
             got {detector:?}"
        );
        assert!(
            tags_of(&detector["tags"]).is_empty(),
            "case {name:?}: the created detector holds no tags, got {detector:?}"
        );
        let created_at = instant(name, &detector["createdAt"]);
        let modified_at = instant(name, &detector["modifiedAt"]);
        assert_eq!(
            created_at, modified_at,
            "case {name:?}: a detector nothing has changed yet was last modified when it \
             was created, got createdAt {created_at} and modifiedAt {modified_at}"
        );

        // The listing serves it too, and the alias filter finds it under that same name.
        let listed = list_detectors(&amp, case.region, &workspace_id, &[]).await;
        let summary = summary_of(&listed, &case.detector_id).unwrap_or_else(|| {
            panic!("case {name:?}: the listing must carry the created detector, got {listed:?}")
        });
        assert_eq!(
            text(&summary["alias"]),
            case.detector_id,
            "case {name:?}: the summary must report the same name as the describe, \
             got {summary:?}"
        );
        assert_eq!(
            text(&summary["status"]["statusCode"]),
            STATUS_CREATING,
            "case {name:?}: the summary must report the stored status, got {summary:?}"
        );
        let filtered = list_detectors(
            &amp,
            case.region,
            &workspace_id,
            &[("alias", case.detector_id.clone())],
        )
        .await;
        assert!(
            summary_of(&filtered, &case.detector_id).is_some(),
            "case {name:?}: the alias filter must find the detector under its name, \
             got {filtered:?}"
        );

        // The clock owes that first training, and settling it moves the status alone.
        wait_for_due_transition();
        let settled = describe_detector(&amp, case.region, &workspace_id, &case.detector_id).await;
        assert_persisted(name, &settled, &case.spec, STATUS_ACTIVE);
        assert_eq!(
            instant(name, &settled.body["anomalyDetector"]["modifiedAt"]),
            modified_at,
            "case {name:?}: settling the status must not move modifiedAt, got {settled:?}"
        );
    }
}

/// The two paths of the one operation meet: the id a put created is an id the workspace
/// holds, so the NEXT put on it replaces that detector's specification instead of creating
/// a second one — keeping the id, the ARN, the name and the creation instant, and
/// reporting `UPDATING`.
#[tokio::test]
async fn put_on_an_id_it_created_updates_that_same_detector() {
    let amp = Amp::new();
    let region = "us-east-1";
    let workspace_id = amp.active_workspace(region, "detectors").await;
    let detector_id = "ad-22222222-2222-4222-8222-222222222222";

    let created_spec = Spec::new("sum(rate(requests_total[5m]))");
    let created = put_detector(&amp, region, &workspace_id, detector_id, &created_spec.put_body())
        .await;
    assert_eq!(
        created.status, 202,
        "case \"create\": PutAnomalyDetector must answer 202, got {created:?}"
    );
    let arn = text(&created.body["arn"]);
    wait_for_due_transition();
    let before = describe_detector(&amp, region, &workspace_id, detector_id).await;
    assert_persisted("create", &before, &created_spec, STATUS_ACTIVE);
    let created_at = instant("create", &before.body["anomalyDetector"]["createdAt"]);

    let updated_spec = Spec::new("avg(rate(requests_total[1m]))")
        .interval(120)
        .labels(&[("zone", "b")]);
    let updated = put_detector(&amp, region, &workspace_id, detector_id, &updated_spec.put_body())
        .await;
    assert_eq!(
        updated.status, 202,
        "case \"update\": PutAnomalyDetector must answer 202, got {updated:?}"
    );
    assert_eq!(
        text(&updated.body["status"]["statusCode"]),
        STATUS_UPDATING,
        "case \"update\": a detector that already exists is retrained, got {updated:?}"
    );

    let after = describe_detector(&amp, region, &workspace_id, detector_id).await;
    assert_persisted("update", &after, &updated_spec, STATUS_UPDATING);
    let detector = &after.body["anomalyDetector"];
    assert_eq!(
        text(&detector["arn"]),
        arn,
        "case \"update\": the second put must not mint a second resource, got {detector:?}"
    );
    assert_eq!(
        text(&detector["alias"]),
        detector_id,
        "case \"update\": the stored name must not change, got {detector:?}"
    );
    assert_eq!(
        instant("update", &detector["createdAt"]),
        created_at,
        "case \"update\": the creation instant must not change, got {detector:?}"
    );

    // One resource, not two: the listing carries exactly one detector for that id.
    let listed = list_detectors(&amp, region, &workspace_id, &[]).await;
    let entries = listed.body["anomalyDetectors"]
        .as_array()
        .unwrap_or_else(|| panic!("case \"update\": anomalyDetectors must be a list, got {listed:?}"))
        .len();
    assert_eq!(
        entries, 1,
        "case \"update\": the workspace must hold one detector, got {listed:?}"
    );
}

/// `clientToken` carries `@idempotencyToken`, and it holds for the create path too:
/// re-sending the very call that created the detector answers from the record that call
/// wrote, while any OTHER call arriving before the first training has settled is refused —
/// and once the clock has settled it, that same token is a new request.
#[tokio::test]
async fn put_create_is_idempotent_per_client_token() {
    let amp = Amp::new();
    let region = "eu-central-1";
    let workspace_id = amp.active_workspace(region, "idempotency").await;
    let detector_id = "ad-33333333-3333-4333-8333-333333333333";

    let spec = Spec::new("count(rate(requests_total[5m]))").interval(45);
    let mut body = spec.put_body();
    body.as_object_mut()
        .expect("an object body")
        .insert("clientToken".to_string(), json!("tok-create"));

    let created = put_detector(&amp, region, &workspace_id, detector_id, &body).await;
    assert_eq!(
        created.status, 202,
        "case \"first put\": expected 202, got {created:?}"
    );
    let described = describe_detector(&amp, region, &workspace_id, detector_id).await;
    assert_persisted("first put", &described, &spec, STATUS_CREATING);
    let created_at = instant("first put", &described.body["anomalyDetector"]["createdAt"]);

    // The same call again, while that first training is still owed: accepted, and the
    // record is the one the first call wrote.
    let replay = put_detector(&amp, region, &workspace_id, detector_id, &body).await;
    assert_eq!(
        replay.status, 202,
        "case \"replayed token\": expected 202, got {replay:?}"
    );
    assert_eq!(
        text(&replay.body["status"]["statusCode"]),
        STATUS_CREATING,
        "case \"replayed token\": expected the pending status, got {replay:?}"
    );
    let after_replay = describe_detector(&amp, region, &workspace_id, detector_id).await;
    assert_persisted("replayed token", &after_replay, &spec, STATUS_CREATING);
    assert_eq!(
        instant(
            "replayed token",
            &after_replay.body["anomalyDetector"]["createdAt"]
        ),
        created_at,
        "case \"replayed token\": a replay must not create the detector again, \
         got {after_replay:?}"
    );

    // Any other call on a detector that is not usable yet is refused, whether it carries a
    // different token or none at all.
    let other = Spec::new("min(node_memory_bytes)");
    let mut other_token_body = other.put_body();
    other_token_body
        .as_object_mut()
        .expect("an object body")
        .insert("clientToken".to_string(), json!("tok-other"));
    for (case, refused_body) in [
        ("another token", other_token_body),
        ("no token", other.put_body()),
    ] {
        let refused = put_detector(&amp, region, &workspace_id, detector_id, &refused_body).await;
        assert_eq!(
            refused.status, 409,
            "case {case:?}: a put on a CREATING detector must answer 409, got {refused:?}"
        );
        assert_eq!(
            refused.error_code(),
            "ConflictException",
            "case {case:?}: unexpected error code, got {refused:?}"
        );
        assert_persisted(
            case,
            &describe_detector(&amp, region, &workspace_id, detector_id).await,
            &spec,
            STATUS_CREATING,
        );
    }

    // Once the training has settled the token stands for no pending change, so the same
    // token carried again is a new request — an update of the detector it created.
    wait_for_due_transition();
    let reused = put_detector(&amp, region, &workspace_id, detector_id, &body).await;
    assert_eq!(
        reused.status, 202,
        "case \"token reused after settling\": expected 202, got {reused:?}"
    );
    assert_eq!(
        text(&reused.body["status"]["statusCode"]),
        STATUS_UPDATING,
        "case \"token reused after settling\": the detector exists now, so the call \
         retrains it, got {reused:?}"
    );
}

/// A detector is created INSIDE the workspace the request addresses: the same id put into
/// two workspaces makes two distinct resources, each reachable only through its own
/// workspace.
#[tokio::test]
async fn put_creates_the_detector_inside_the_addressed_workspace_only() {
    let amp = Amp::new();
    let region = "us-east-1";
    let left = amp.active_workspace(region, "left").await;
    let right = amp.active_workspace(region, "right").await;
    let detector_id = "ad-44444444-4444-4444-8444-444444444444";

    let left_spec = Spec::new("sum(rate(requests_total[5m]))");
    let right_spec = Spec::new("group(up)").interval(300);
    for (case, workspace_id, spec) in [
        ("left", left.clone(), left_spec.clone()),
        ("right", right.clone(), right_spec.clone()),
    ] {
        let put = put_detector(&amp, region, &workspace_id, detector_id, &spec.put_body()).await;
        assert_eq!(
            put.status, 202,
            "case {case:?}: PutAnomalyDetector must answer 202, got {put:?}"
        );
        assert_well_formed_detector_arn(
            case,
            &text(&put.body["arn"]),
            region,
            &workspace_id,
            detector_id,
        );
    }

    // Each workspace holds its OWN record under that id, with its own specification.
    assert_persisted(
        "left",
        &describe_detector(&amp, region, &left, detector_id).await,
        &left_spec,
        STATUS_CREATING,
    );
    assert_persisted(
        "right",
        &describe_detector(&amp, region, &right, detector_id).await,
        &right_spec,
        STATUS_CREATING,
    );
    for (case, workspace_id) in [("left", &left), ("right", &right)] {
        let listed = list_detectors(&amp, region, workspace_id, &[]).await;
        let entries = listed.body["anomalyDetectors"]
            .as_array()
            .unwrap_or_else(|| {
                panic!("case {case:?}: anomalyDetectors must be a list, got {listed:?}")
            })
            .len();
        assert_eq!(
            entries, 1,
            "case {case:?}: a workspace must hold only the detector put into it, \
             got {listed:?}"
        );
    }
}

/// The parent still has to be able to take a new child: a workspace whose own deletion has
/// started refuses the create with the modelled `ConflictException` (409) naming the
/// workspace, exactly as the operation that creates detectors refuses it, and stores
/// nothing.
#[tokio::test]
async fn put_refuses_to_create_inside_a_workspace_that_is_going_away() {
    let amp = Amp::new();
    let region = "us-west-2";
    let workspace_id = amp.active_workspace(region, "cascade").await;
    let detector_id = "ad-55555555-5555-4555-8555-555555555555";

    let deleted = amp.delete(region, &workspace_id, None).await;
    assert_eq!(
        deleted.status, 202,
        "case \"seed\": DeleteWorkspace must answer 202, got {deleted:?}"
    );

    let spec = Spec::new("sum(rate(requests_total[5m]))");
    let refused = put_detector(&amp, region, &workspace_id, detector_id, &spec.put_body()).await;
    assert_eq!(
        refused.status, 409,
        "case \"workspace DELETING\": PutAnomalyDetector must answer 409, got {refused:?}"
    );
    assert_eq!(
        refused.error_code(),
        "ConflictException",
        "case \"workspace DELETING\": unexpected error code, got {refused:?}"
    );
    assert_eq!(
        text(&refused.body["resourceId"]),
        workspace_id,
        "case \"workspace DELETING\": the refusal must name the workspace, got {refused:?}"
    );
    assert_eq!(
        text(&refused.body["resourceType"]),
        WORKSPACE_RESOURCE_TYPE,
        "case \"workspace DELETING\": unexpected resourceType, got {refused:?}"
    );
    assert_eq!(
        text(&refused.body["message"]),
        workspace_refuses_new_detector_message(STATUS_DELETING),
        "case \"workspace DELETING\": unexpected message, got {refused:?}"
    );

    // Nothing was stored: the id still names nothing inside that workspace.
    let described = describe_detector(&amp, region, &workspace_id, detector_id).await;
    assert_eq!(
        described.status, 404,
        "case \"workspace DELETING\": the refusal must store nothing, got {described:?}"
    );
}

/// A create is held to the same rules on the request's own members as an update is, and a
/// refused request stores nothing — so a caller cannot bring a detector into existence
/// with a specification the service does not accept.
///
/// The rows span the member each refusal belongs to: the Random Cut Forest query (a bare
/// selector, an operator the published catalog does not list), a metric label key outside
/// `PrometheusMetricLabelKey`'s pattern, the evaluation interval one step past each
/// published bound of its modelled `@range(min: 30, max: 86400)`, and an addressed id one
/// character past `AnomalyDetectorId`'s modelled length bound.
#[tokio::test]
async fn put_refuses_a_create_it_cannot_store() {
    let amp = Amp::new();
    let region = "eu-west-1";
    let workspace_id = amp.active_workspace(region, "refusals").await;

    struct Case {
        name: &'static str,
        detector_id: String,
        body: Value,
    }

    let mut bad_label = Spec::new("sum(rate(requests_total[5m]))").put_body();
    bad_label
        .as_object_mut()
        .expect("an object body")
        .insert("labels".to_string(), json!({ "__reserved": "value" }));

    let cases = vec![
        Case {
            name: "a query no aggregation operator wraps",
            detector_id: "ad-aaaaaaaa-0000-4000-8000-000000000001".to_string(),
            body: Spec::new("rate(requests_total[5m])").put_body(),
        },
        Case {
            name: "a query wrapped by an operator the catalog does not list",
            detector_id: "ad-aaaaaaaa-0000-4000-8000-000000000002".to_string(),
            body: Spec::new("topk(5, rate(requests_total[5m]))").put_body(),
        },
        Case {
            name: "a metric label key outside the modelled pattern",
            detector_id: "ad-aaaaaaaa-0000-4000-8000-000000000003".to_string(),
            body: bad_label,
        },
        Case {
            name: "an evaluation interval one second below the published minimum",
            detector_id: "ad-aaaaaaaa-0000-4000-8000-000000000004".to_string(),
            body: Spec::new("sum(rate(requests_total[5m]))")
                .interval(29)
                .put_body(),
        },
        Case {
            name: "an evaluation interval one second above the published maximum",
            detector_id: "ad-aaaaaaaa-0000-4000-8000-000000000005".to_string(),
            body: Spec::new("sum(rate(requests_total[5m]))")
                .interval(86_401)
                .put_body(),
        },
        Case {
            name: "an addressed id one character past the published length bound",
            detector_id: detector_id_of_length(DETECTOR_ID_MAX_LENGTH + 1),
            body: Spec::new("sum(rate(requests_total[5m]))").put_body(),
        },
    ];

    for case in cases {
        let name = case.name;
        let refused = put_detector(&amp, region, &workspace_id, &case.detector_id, &case.body).await;
        assert_eq!(
            refused.status, 400,
            "case {name:?}: PutAnomalyDetector must answer 400, got {refused:?}"
        );
        assert_eq!(
            refused.error_code(),
            "ValidationException",
            "case {name:?}: unexpected error code, got {refused:?}"
        );
        // Nothing was stored: the workspace holds no detector at all. (Asserted through
        // the listing rather than a describe, because a row that refuses the ADDRESSED id
        // refuses it on every operation the id is a path label of.)
        let listed = list_detectors(&amp, region, &workspace_id, &[]).await;
        let entries = listed.body["anomalyDetectors"]
            .as_array()
            .unwrap_or_else(|| {
                panic!("case {name:?}: anomalyDetectors must be a list, got {listed:?}")
            })
            .len();
        assert_eq!(
            entries, 0,
            "case {name:?}: a refused create must store nothing, got {listed:?}"
        );
    }
}
