// SPDX-License-Identifier: CC0-1.0

//! What a delete LEAVES BEHIND on the four detector-scoped set families — an
//! `IPSet`, a `ThreatIntelSet`, a `ThreatEntitySet` and a `TrustedEntitySet` — end to
//! end over the wire.
//!
//! Every request goes through [`smithy_server_runtime::MultiServiceRouter::dispatch`]
//! (the harness lives in the `detector_lifecycle` suite), so routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers are exercised
//! together.
//!
//! The rules under test, each asserted over a TABLE spanning the four families and
//! the status the set was in when the delete arrived, rather than over one call:
//!
//! 1. A delete COMPLETES: the detector stops holding the set the instant the call is
//!    served. There is no pending span for a caller to observe — an uploaded list's
//!    own read reports the terminal `DELETED` straight after the delete and for ever
//!    afterwards (the terminal member of the status enum,
//!    `smithy_query(shape="IpSetStatus")`), and an entity set the detector no longer
//!    has is not readable at all.
//! 2. An id the detector does not hold is refused with its family's own absence
//!    refusal, and the SAME one whatever put the id out of reach — a value no create
//!    ever minted, or one a delete took out of the detector's holdings: the uploaded
//!    lists answer HTTP 400 under the `InvalidInputException` class, the entity sets
//!    HTTP 404 under `ResourceNotFoundException`, each with its own fixed sentence
//!    (see [`assert_uploaded_list_absent`] and [`assert_entity_set_absent`]) — except
//!    that an id outside the alphabet an entity-set id is constrained to is refused
//!    as a CONSTRAINT violation there, before anything is resolved.
//! 3. A SECOND delete is that same refusal, and writes nothing: the detector's own
//!    `UpdatedAt` moved on the first delete and does not move again.
//! 4. The collection stops naming a deleted set, and the name it held is free: a
//!    create that reuses it is served and mints a different id, so a client that
//!    deletes and re-creates never collides with a ghost.

use serde_json::{json, Value};

use crate::detector_lifecycle::{call, enabled_detector, error_code, error_shape, new_router, Router};
use crate::entity_set_id_refusal_classes::assert_entity_set_id_constraint_violation;
use service_com_amazonaws_guardduty_guarddutyapiservice::scaffolded::{
    BAD_REQUEST_EXCEPTION, ENTITY_SET_NOT_FOUND_MESSAGE, INVALID_INPUT_EXCEPTION,
    RESOURCE_NOT_FOUND_EXCEPTION,
};
use service_com_amazonaws_guardduty_guarddutyapiservice::state::{
    ENTITY_SET_TRANSITION_MILLIS, IP_SET_DELETED, IP_SET_TRANSITION_MILLIS,
    NO_SUCH_RESOURCE_MESSAGE, THREAT_INTEL_SET_ACTIVATION_SPAN_MILLIS, THREAT_INTEL_SET_DELETED,
};

/// A well-formed set id (32 lowercase hex characters, the form this service mints)
/// that no create ever minted.
pub(crate) const ABSENT_SET_ID: &str = "0123456789abcdef0123456789abcdef";

/// Long enough that any transition this service opens would have come due, plus
/// slack: what rule 1 waits out to show a deleted set's status does not move again.
/// Each family publishes its own span, so the longest of them is what one wait has
/// to cover.
fn past_every_transition() -> std::time::Duration {
    let longest = IP_SET_TRANSITION_MILLIS
        .max(ENTITY_SET_TRANSITION_MILLIS)
        .max(THREAT_INTEL_SET_ACTIVATION_SPAN_MILLIS);
    std::time::Duration::from_millis(u64::try_from(longest).unwrap_or_default() + 250)
}

/// Assert `status`/`body` is the absence refusal the UPLOADED-LIST families — an
/// IPSet and a threat list — answer an id the detector does not hold with: HTTP 400,
/// the `InvalidInputException` class in the body's `__type` and its `type` member,
/// the modelled `BadRequestException` envelope in `x-amzn-errortype`, and one fixed
/// sentence that names neither the member nor the caller's value.
pub(crate) fn assert_uploaded_list_absent(case: &str, status: u16, body: &Value, id: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400, got {status} {body:?}"
    );
    let class = error_code(body);
    assert_eq!(
        class, INVALID_INPUT_EXCEPTION,
        "case {case:?}: expected the {INVALID_INPUT_EXCEPTION} class, got {class:?} {body:?}"
    );
    assert_eq!(
        body["type"].as_str().unwrap_or_default(),
        INVALID_INPUT_EXCEPTION,
        "case {case:?}: expected the class in the body's type member, got {body:?}"
    );
    let shape = error_shape(body);
    assert_eq!(
        shape, BAD_REQUEST_EXCEPTION,
        "case {case:?}: expected the {BAD_REQUEST_EXCEPTION} shape in x-amzn-errortype, got \
         {shape:?} {body:?}"
    );
    let message = body["message"].as_str().unwrap_or_default();
    assert_eq!(
        message, NO_SUCH_RESOURCE_MESSAGE,
        "case {case:?}: expected {NO_SUCH_RESOURCE_MESSAGE:?}, got {message:?}"
    );
    assert!(
        !message.contains(id),
        "case {case:?}: the refusal must not echo the id {id:?}, got {message:?}"
    );
}

/// Assert `status`/`body` is the absence refusal BOTH ENTITY-SET families answer an
/// id the detector does not hold with: HTTP 404 with the `ResourceNotFoundException`
/// class in the body's `__type`, in its `type` member AND in `x-amzn-errortype` (the
/// class is a modelled shape of this service, `httpError` 404 —
/// `smithy_query(shape="ResourceNotFoundException")`), carrying one fixed sentence
/// that names neither the member nor the caller's value.
pub(crate) fn assert_entity_set_absent(case: &str, status: u16, body: &Value, id: &str) {
    assert_eq!(
        status, 404,
        "case {case:?}: expected 404, got {status} {body:?}"
    );
    let class = error_code(body);
    assert_eq!(
        class, RESOURCE_NOT_FOUND_EXCEPTION,
        "case {case:?}: expected the {RESOURCE_NOT_FOUND_EXCEPTION} class, got {class:?} {body:?}"
    );
    assert_eq!(
        body["type"].as_str().unwrap_or_default(),
        RESOURCE_NOT_FOUND_EXCEPTION,
        "case {case:?}: expected the class in the body's type member, got {body:?}"
    );
    let shape = error_shape(body);
    assert_eq!(
        shape, RESOURCE_NOT_FOUND_EXCEPTION,
        "case {case:?}: expected {RESOURCE_NOT_FOUND_EXCEPTION} in x-amzn-errortype, got \
         {shape:?} {body:?}"
    );
    let message = body["message"].as_str().unwrap_or_default();
    assert_eq!(
        message, ENTITY_SET_NOT_FOUND_MESSAGE,
        "case {case:?}: expected {ENTITY_SET_NOT_FOUND_MESSAGE:?}, got {message:?}"
    );
    assert!(
        !message.contains(id),
        "case {case:?}: the refusal must not echo the id {id:?}, got {message:?}"
    );
}

/// One set family: the route it hangs off a detector, the members its create and its
/// listing report, and what its delete leaves readable.
struct Family {
    case: &'static str,
    /// The URI segment naming the family's collection under
    /// `/detector/{DetectorId}`.
    segment: &'static str,
    /// The response member the create reports the minted id in.
    id_member: &'static str,
    /// The response member the listing reports the family's ids in.
    ids_member: &'static str,
    /// A member of this family's `*Format` enum — each family models the same six
    /// (`smithy_query(shape="IpSetFormat")`), so the row varies it to show the rule
    /// does not depend on the format a set was created with.
    format: &'static str,
    /// The terminal `Status` the family's own read reports after a delete, or `None`
    /// when the delete leaves nothing readable at all.
    deleted_status: Option<&'static str>,
}

/// Every detector-scoped set family, with the two whose delete RETAINS a readable
/// record and the two whose delete does not.
fn families() -> Vec<Family> {
    vec![
        Family {
            case: "ipset",
            segment: "ipset",
            id_member: "ipSetId",
            ids_member: "ipSetIds",
            format: "TXT",
            deleted_status: Some(IP_SET_DELETED),
        },
        Family {
            case: "threatintelset",
            segment: "threatintelset",
            id_member: "threatIntelSetId",
            ids_member: "threatIntelSetIds",
            format: "STIX",
            deleted_status: Some(THREAT_INTEL_SET_DELETED),
        },
        Family {
            case: "threatentityset",
            segment: "threatentityset",
            id_member: "threatEntitySetId",
            ids_member: "threatEntitySetIds",
            // A format no cited case used, to show the rule is indifferent to it.
            format: "ALIEN_VAULT",
            deleted_status: None,
        },
        Family {
            case: "trustedentityset",
            segment: "trustedentityset",
            id_member: "trustedEntitySetId",
            ids_member: "trustedEntitySetIds",
            format: "PROOF_POINT",
            deleted_status: None,
        },
    ]
}

impl Family {
    /// Is this family's absence refusal the entity-set 404? It is exactly the
    /// families whose delete leaves nothing readable.
    fn refuses_with_not_found(&self) -> bool {
        self.deleted_status.is_none()
    }

    fn assert_absent(&self, case: &str, status: u16, body: &Value, id: &str) {
        if self.refuses_with_not_found() {
            assert_entity_set_absent(case, status, body, id);
        } else {
            assert_uploaded_list_absent(case, status, body, id);
        }
    }

    /// Assert how this family refuses `id`. An id outside the alphabet an entity-set
    /// id is constrained to is a CONSTRAINT violation on the two entity-set
    /// families, decided from the request before anything is resolved; everything
    /// else, and every form on the uploaded lists, is the family's own absence
    /// refusal.
    fn assert_refused(&self, case: &str, status: u16, body: &Value, id: &str, well_formed: bool) {
        if self.refuses_with_not_found() && !well_formed {
            assert_entity_set_id_constraint_violation(case, status, body, self.id_member, id);
        } else {
            self.assert_absent(case, status, body, id);
        }
    }

    async fn create(&self, router: &Router, detector_id: &str, name: &str, activate: bool) -> (u16, Value) {
        call(
            router,
            "POST",
            &format!("/detector/{detector_id}/{}", self.segment),
            Some(json!({
                "name": name,
                "format": self.format,
                "location": "s3://guardduty-lists/set.txt",
                "activate": activate,
            })),
        )
        .await
    }

    async fn created(&self, router: &Router, detector_id: &str, name: &str, activate: bool) -> String {
        let (status, body) = self.create(router, detector_id, name, activate).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the create to answer 200, got {status} {body:?}",
            self.case
        );
        body[self.id_member]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: expected a {} in {body:?}",
                    self.case, self.id_member
                )
            })
            .to_string()
    }

    async fn get(&self, router: &Router, detector_id: &str, set_id: &str) -> (u16, Value) {
        call(
            router,
            "GET",
            &format!("/detector/{detector_id}/{}/{set_id}", self.segment),
            None,
        )
        .await
    }

    async fn delete(&self, router: &Router, detector_id: &str, set_id: &str) -> (u16, Value) {
        call(
            router,
            "DELETE",
            &format!("/detector/{detector_id}/{}/{set_id}", self.segment),
            None,
        )
        .await
    }

    async fn list(&self, router: &Router, detector_id: &str) -> Vec<String> {
        let (status, body) = call(
            router,
            "GET",
            &format!("/detector/{detector_id}/{}", self.segment),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the listing to answer 200, got {status} {body:?}",
            self.case
        );
        body[self.ids_member]
            .as_array()
            .map(|ids| {
                ids.iter()
                    .map(|id| id.as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// The detector's own `UpdatedAt`, as `GetDetector` reports it.
async fn detector_updated_at(router: &Router, detector_id: &str, case: &str) -> Value {
    let (status, body) = call(router, "GET", &format!("/detector/{detector_id}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected get-detector to answer 200, got {status} {body:?}"
    );
    body["updatedAt"].clone()
}

/// The statuses a set can be in when the delete arrives. The delete's answer, and
/// what it leaves behind, must not depend on which.
fn arrival_rows() -> Vec<(&'static str, bool, bool)> {
    vec![
        ("a set in use", true, true),
        ("a set stored unused", false, true),
        ("a set still in its create's transition", true, false),
    ]
}

// ---------------------------------------------------------------------------
// Rules 1-3: a delete completes, and the id it deleted is afterwards refused.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_delete_completes_and_the_deleted_id_is_refused_afterwards() {
    for family in families() {
        for (arrival, activate, settle_first) in arrival_rows() {
            let case = format!("{} {arrival}", family.case);
            let router = new_router();
            let detector_id = enabled_detector(&router).await;
            let set_id = family
                .created(&router, &detector_id, "deleted_set-1", activate)
                .await;
            if settle_first {
                tokio::time::sleep(past_every_transition()).await;
            }

            let (status, body) = family.delete(&router, &detector_id, &set_id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the delete to answer the modelled 200, got {status} \
                 {body:?}"
            );
            let after_first_delete = detector_updated_at(&router, &detector_id, &case).await;

            // Rule 1: there is no pending span. A read STRAIGHT after the delete
            // reports the terminal status on the families that retain the record,
            // and is refused on the families that do not.
            let (status, body) = family.get(&router, &detector_id, &set_id).await;
            match family.deleted_status {
                Some(terminal) => {
                    assert_eq!(
                        status, 200,
                        "case {case:?}: a retained record stays readable, got {status} {body:?}"
                    );
                    assert_eq!(
                        body["status"].as_str().unwrap_or_default(),
                        terminal,
                        "case {case:?}: expected {terminal} straight after the delete, got {body:?}"
                    );
                    // The read reports what the create stored: the delete moved the
                    // status and nothing else.
                    assert_eq!(
                        body["name"].as_str().unwrap_or_default(),
                        "deleted_set-1",
                        "case {case:?}: a retained record must still report the stored name, got \
                         {body:?}"
                    );
                }
                None => family.assert_absent(&case, status, &body, &set_id),
            }

            // Rule 3: a second delete is the family's absence refusal, and moves
            // neither the record nor the detector.
            let (status, body) = family.delete(&router, &detector_id, &set_id).await;
            family.assert_absent(&case, status, &body, &set_id);
            let after_second_delete = detector_updated_at(&router, &detector_id, &case).await;
            assert_eq!(
                after_second_delete, after_first_delete,
                "case {case:?}: a refused second delete must not move the detector's updatedAt, \
                 expected {after_first_delete:?}, got {after_second_delete:?}"
            );

            // Rule 1 again, once every transition this service opens would have come
            // due: a terminal status does not move, and a set that is gone stays
            // gone.
            tokio::time::sleep(past_every_transition()).await;
            let (status, body) = family.get(&router, &detector_id, &set_id).await;
            match family.deleted_status {
                Some(terminal) => assert_eq!(
                    body["status"].as_str().unwrap_or_default(),
                    terminal,
                    "case {case:?}: {terminal} is terminal, got {status} {body:?}"
                ),
                None => family.assert_absent(&case, status, &body, &set_id),
            }

            // Rule 4: the collection stops naming it.
            let listed = family.list(&router, &detector_id).await;
            assert!(
                !listed.contains(&set_id),
                "case {case:?}: the listing must stop naming the deleted set, got {listed:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 2: an id no create ever minted gets the same refusal as a deleted one.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_id_the_detector_never_held_is_refused_the_same_way() {
    for family in families() {
        let case = family.case.to_string();
        let router = new_router();
        let detector_id = enabled_detector(&router).await;
        // A live set of the same family, so the refusals below are about the id in
        // the request and not about an empty collection.
        let live = family
            .created(&router, &detector_id, "live_set-1", false)
            .await;

        // The rows differ along the FORM of the id: one this service could have
        // minted, one that is far too short to be one, and one with characters a
        // minted id never carries. The last is outside the alphabet an ENTITY-SET id
        // is constrained to, so those two families answer it with the constraint
        // violation their own suite pins rather than with an absence; the uploaded
        // lists answer their one absence sentence whatever the form.
        for (form, absent, well_formed) in [
            ("well-formed but never minted", ABSENT_SET_ID, true),
            ("too short to be an id", "abc", true),
            ("characters an id never carries", "not-a-set-id!", false),
        ] {
            let row = format!("{case}: {form}");
            let (status, body) = family.get(&router, &detector_id, absent).await;
            family.assert_refused(&row, status, &body, absent, well_formed);

            let (status, body) = family.delete(&router, &detector_id, absent).await;
            family.assert_refused(&row, status, &body, absent, well_formed);

            // The refusals touched nothing: the live set is still there.
            let (status, body) = family.get(&router, &detector_id, &live).await;
            assert_eq!(
                status, 200,
                "case {row:?}: the live set must be untouched, got {status} {body:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 4: the name a deleted set held is free again.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_deleted_sets_name_can_be_created_again() {
    for family in families() {
        let case = family.case.to_string();
        let router = new_router();
        let detector_id = enabled_detector(&router).await;
        let name = "recreated_set-1";

        let first = family.created(&router, &detector_id, name, true).await;
        let (status, body) = family.delete(&router, &detector_id, &first).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the delete to answer 200, got {status} {body:?}"
        );

        // The slot the deleted set held is free, so the same name is accepted again —
        // and it is a DIFFERENT resource, with an id of its own.
        let second = family.created(&router, &detector_id, name, true).await;
        assert_ne!(
            second, first,
            "case {case:?}: a re-create must mint a new id, got {second:?} twice"
        );
        let listed = family.list(&router, &detector_id).await;
        assert!(
            listed.contains(&second) && !listed.contains(&first),
            "case {case:?}: the listing must name only the live set, got {listed:?}"
        );

        let (status, body) = family.get(&router, &detector_id, &second).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the re-created set must be readable, got {status} {body:?}"
        );
        assert_eq!(
            body["name"].as_str().unwrap_or_default(),
            name,
            "case {case:?}: the re-created set must report the name it was created with, got \
             {body:?}"
        );
    }
}
