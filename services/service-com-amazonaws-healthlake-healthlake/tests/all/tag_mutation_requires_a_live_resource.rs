// SPDX-License-Identifier: CC0-1.0

//! Which resources the MUTATING tagging operations (`TagResource`,
//! `UntagResource`) may change.
//!
//! One rule, asserted over a SET of inputs rather than one recorded request: a
//! tagging call that CHANGES a resource reaches only a resource this service
//! still holds, so a data store that has reached the terminal `DELETED` status
//! — retained for a while after its deletion only so a reader can observe it —
//! is refused with the data store's `ResourceNotFoundException`, and the stored
//! tag set is left exactly as it was; a record in any state the service still
//! serves takes the mutation.
//!
//! The rows span the axis the rule generalises over: the target's lifecycle
//! status (every state this slice reaches, taken from the one table in
//! `tests/all/datastore_state_conflict.rs`) crossed with the two mutating
//! tagging operations, plus the other resource form the modelled
//! `AmazonResourceName` admits (a data transformation profile, which carries no
//! lifecycle status) as the control that the gate is about the STATE and not
//! about the tagging path.
//!
//! Every status is reached through the real lifecycle rather than by poking the
//! state, and every call goes through
//! [`MultiServiceRouter::dispatch`](smithy_server_runtime::MultiServiceRouter)
//! by way of the shared harness in `fhir_datastore_lifecycle`, so routing,
//! deserialization and the error envelope are exercised too.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_healthlake_healthlake::state::{
    DatastoreStatus, DATASTORE_NOT_FOUND_MESSAGE,
};

use crate::datastore_state_conflict::{datastore_in, states};
use crate::fhir_datastore_lifecycle::{
    error_code, error_message, string_at, HealthLake, PAST_TRANSITION,
};
use crate::resource_tagging::{create_datastore, create_profile, profile_arn, tag_list};

/// The name every data store below is created with.
const CREATED_NAME: &str = "tag-gated.store";

/// The tag key each row writes, and the one it then removes.
const WRITTEN_KEY: &str = "env";

/// The tags `ListTagsForResource` reports for `arn`, as a comparable map.
async fn listed_tags(service: &HealthLake, case: &str, arn: &str) -> BTreeMap<String, String> {
    let payload = service
        .ok("ListTagsForResource", &json!({ "ResourceARN": arn }))
        .await;
    crate::list_tags_for_resource::reported_tags(case, &payload)
}

/// The `DatastoreArn` the service minted for `id`, read off the reporting
/// operation so no test spells an ARN of its own.
async fn datastore_arn(service: &HealthLake, id: &str) -> String {
    let described = service
        .ok("DescribeFHIRDatastore", &json!({ "DatastoreId": id }))
        .await;
    string_at(&described["DatastoreProperties"], "DatastoreArn")
}

/// Assert that `operation` on `arn` is refused as the rule requires: the
/// service's fixed not-found sentence for `expected_message`, quoting nothing
/// from the request.
async fn assert_refused_as_not_found(
    service: &HealthLake,
    case: &str,
    operation: &str,
    request: &Value,
    expected_message: &str,
) {
    let (status, payload) = service.call(operation, request).await;
    // Every modelled client error of this service goes on the wire as 400,
    // whatever 4xx its shape declares: see `tests/all/error_envelopes.rs`.
    assert_eq!(
        status, 400,
        "case {case:?}: {operation} must answer 400, got {status}: {payload}"
    );
    assert_eq!(
        error_code(&payload),
        "ResourceNotFoundException",
        "case {case:?}: {operation} must answer ResourceNotFoundException, got {payload}"
    );
    let message = error_message(&payload);
    assert_eq!(
        message, expected_message,
        "case {case:?}: {operation} must state {expected_message:?}, got {message:?}"
    );
    let arn = string_at(request, "ResourceARN");
    assert!(
        !payload.to_string().contains(arn.as_str()),
        "case {case:?}: a refusal must quote nothing from the request, got {payload}"
    );
}

#[tokio::test]
async fn only_a_data_store_the_service_still_holds_takes_a_tag_mutation() {
    for (status, _settled) in states() {
        // A service per row, so neither the lifecycle waits nor an accepted
        // mutation of one row can be observed by another.
        let service = HealthLake::new();
        let id = datastore_in(&service, status, CREATED_NAME).await;
        let arn = datastore_arn(&service, &id).await;
        let case = format!("a {} data store", status.as_wire());

        let tag_request =
            json!({ "ResourceARN": &arn, "Tags": tag_list(&[(WRITTEN_KEY, "test")]) });
        let untag_request = json!({ "ResourceARN": &arn, "TagKeys": [WRITTEN_KEY] });

        if status == DatastoreStatus::Deleted {
            assert_refused_as_not_found(
                &service,
                &case,
                "TagResource",
                &tag_request,
                DATASTORE_NOT_FOUND_MESSAGE,
            )
            .await;
            assert_refused_as_not_found(
                &service,
                &case,
                "UntagResource",
                &untag_request,
                DATASTORE_NOT_FOUND_MESSAGE,
            )
            .await;
            // The refused mutations wrote nothing: the retained record reports
            // exactly the tag set it had, which is the empty one its create
            // left.
            let actual = listed_tags(&service, &case, &arn).await;
            assert!(
                actual.is_empty(),
                "case {case:?}: a refused tag mutation must store nothing, got {actual:?}"
            );
            continue;
        }

        // Every other state is one the service still serves, so the mutation
        // lands and is observable through the reporting operation.
        let (status_code, payload) = service.call("TagResource", &tag_request).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: TagResource must answer 200, got {status_code}: {payload}"
        );
        let actual = listed_tags(&service, &case, &arn).await;
        assert_eq!(
            actual,
            [(WRITTEN_KEY.to_string(), "test".to_string())].into(),
            "case {case:?}: the written tag must be reported, got {actual:?}"
        );

        let (status_code, payload) = service.call("UntagResource", &untag_request).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: UntagResource must answer 200, got {status_code}: {payload}"
        );
        let actual = listed_tags(&service, &case, &arn).await;
        assert!(
            actual.is_empty(),
            "case {case:?}: the removed tag must be gone, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn a_tag_set_written_before_the_deletion_landed_survives_the_refusals_after_it() {
    // The sequence the rule is about end to end: a data store tagged while the
    // service still serves it keeps exactly that set once its deletion has
    // landed, and neither mutating tagging operation may change it after.
    let service = HealthLake::new();
    let arn = create_datastore(
        &service,
        CREATED_NAME,
        Some(&tag_list(&[("owner", "platform")])),
    )
    .await;
    let case = "a data store tagged before its deletion landed";
    tokio::time::sleep(PAST_TRANSITION).await;

    // The create above answers the ARN, and the delete below addresses the
    // record by id, which is that ARN's last segment
    // (`datastore/fhir/{DatastoreId}`).
    let datastore_id = arn
        .rsplit('/')
        .next()
        .expect("a data store ARN ends in its id")
        .to_string();

    // Written while the data store is ACTIVE: accepted, and reported.
    let (status_code, payload) = service
        .call(
            "TagResource",
            &json!({ "ResourceARN": &arn, "Tags": tag_list(&[(WRITTEN_KEY, "test")]) }),
        )
        .await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: TagResource on an ACTIVE data store must answer 200, \
         got {status_code}: {payload}"
    );

    service
        .ok("DeleteFHIRDatastore", &json!({ "DatastoreId": &datastore_id }))
        .await;
    tokio::time::sleep(PAST_TRANSITION).await;
    let described = service
        .ok("DescribeFHIRDatastore", &json!({ "DatastoreId": &datastore_id }))
        .await;
    assert_eq!(
        described["DatastoreProperties"]["DatastoreStatus"], "DELETED",
        "case {case:?}: the deletion must have landed, got {described}"
    );

    let expected: BTreeMap<String, String> = [
        (WRITTEN_KEY.to_string(), "test".to_string()),
        ("owner".to_string(), "platform".to_string()),
    ]
    .into();
    for (operation, request) in [
        (
            "TagResource",
            json!({ "ResourceARN": &arn, "Tags": tag_list(&[("after.delete", "no")]) }),
        ),
        (
            "UntagResource",
            json!({ "ResourceARN": &arn, "TagKeys": ["owner"] }),
        ),
    ] {
        assert_refused_as_not_found(
            &service,
            case,
            operation,
            &request,
            DATASTORE_NOT_FOUND_MESSAGE,
        )
        .await;
        let actual = listed_tags(&service, case, &arn).await;
        assert_eq!(
            actual, expected,
            "case {case:?}: {operation} must leave the stored tag set as it was, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn a_data_transformation_profile_carries_no_lifecycle_and_takes_both_mutations() {
    // The control for the rule: the gate is about the TARGET's lifecycle state,
    // not about the tagging path, so the other resource form the modelled
    // `AmazonResourceName` admits — which carries no `DatastoreStatus` at all —
    // still takes both mutations through the same shared resolver.
    let service = HealthLake::new();
    let datastore_arn = create_datastore(&service, CREATED_NAME, None).await;
    let profile_id = create_profile(&service, "ungated profile", None).await;
    let arn = profile_arn(&datastore_arn, &profile_id);
    let case = "a data transformation profile ARN";

    let (status_code, payload) = service
        .call(
            "TagResource",
            &json!({ "ResourceARN": &arn, "Tags": tag_list(&[(WRITTEN_KEY, "test")]) }),
        )
        .await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: TagResource must answer 200, got {status_code}: {payload}"
    );
    let actual = listed_tags(&service, case, &arn).await;
    assert_eq!(
        actual,
        [(WRITTEN_KEY.to_string(), "test".to_string())].into(),
        "case {case:?}: the written tag must be reported, got {actual:?}"
    );

    let (status_code, payload) = service
        .call(
            "UntagResource",
            &json!({ "ResourceARN": &arn, "TagKeys": [WRITTEN_KEY] }),
        )
        .await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: UntagResource must answer 200, got {status_code}: {payload}"
    );
    let actual = listed_tags(&service, case, &arn).await;
    assert!(
        actual.is_empty(),
        "case {case:?}: the removed tag must be gone, got {actual:?}"
    );
}
