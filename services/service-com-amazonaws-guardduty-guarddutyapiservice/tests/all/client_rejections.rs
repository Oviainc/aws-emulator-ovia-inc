// SPDX-License-Identifier: CC0-1.0

//! How GuardDuty answers a request it refuses.
//!
//! The rule under test, asserted over a SET of inputs rather than one recorded call:
//!
//! > A GuardDuty client rejection is HTTP 400 whose body names the specific error
//! > CLASS — `InvalidInputException` — in BOTH the `__type` discriminator and the
//! > exception's own `type` member, with a message classified from the REQUEST: a
//! > value that is not a well-formed id for its member is a parameter failure, a
//! > well-formed id the account does not own is an ownership failure, and neither
//! > message quotes the caller's value or the endpoint's Region back at them.
//!
//! The model folds every one of this service's client rejections into the single
//! `BadRequestException` envelope (`httpError` 400, members `message` and `type` —
//! `smithy_query(shape="BadRequestException")`), which is why naming the envelope
//! shape in `__type` is observably wrong: a client switching on the error type could
//! not tell an unacceptable input from any other 400, and an SDK's error-shape
//! unmarshalling would leave `type` null.
//!
//! Every request goes over the wire through `MultiServiceRouter::dispatch`, so these
//! tests exercise routing, restJson1 error serialization and the handlers together.

use serde_json::{json, Value};

use crate::detector_lifecycle::{
    call, call_in, create_detector, enabled_detector, error_code, error_shape, new_router,
};

/// The error class every GuardDuty client rejection is named by on the wire.
const CLASS: &str = "InvalidInputException";

/// The modeled error SHAPE the class is enveloped in — the only client error bound
/// to this service's operations, and what restJson1 resolves the error shape from
/// (the `x-amzn-errortype` header). The header names the shape, the body names the
/// class; an SDK needs the first to unmarshal and a client the second to switch on.
const ENVELOPE: &str = "BadRequestException";

/// The eight operations that take a `DetectorId` in the URI and are implemented in
/// this slice: (operation, method, path suffix after the id, request body).
const DETECTOR_ID_OPERATIONS: [(&str, &str, &str, Option<&str>); 8] = [
    ("GetDetector", "GET", "", None),
    ("UpdateDetector", "POST", "", Some(r#"{"enable":true}"#)),
    ("DeleteDetector", "DELETE", "", None),
    ("ListFindings", "POST", "/findings", Some("{}")),
    (
        "GetFindings",
        "POST",
        "/findings/get",
        Some(r#"{"findingIds":[]}"#),
    ),
    (
        "CreateSampleFindings",
        "POST",
        "/findings/create",
        Some(r#"{"findingTypes":["Recon:EC2/PortProbeUnprotectedPort"]}"#),
    ),
    ("GetMalwareScanSettings", "GET", "/malware-scan-settings", None),
    (
        "UpdateMalwareScanSettings",
        "POST",
        "/malware-scan-settings",
        Some(r#"{"ebsSnapshotPreservation":"NO_RETENTION"}"#),
    ),
];

/// Values that cannot be a `DetectorId` at all, each with why.
///
/// A detector id is the 32 hexadecimal characters this service mints, so the rows
/// differ along LENGTH (including both sides of the 32 boundary) and along the
/// ALPHABET. The model's own constraint is `len 1..300`
/// (`smithy_query(shape="DetectorId")`) — far wider than the id format — so every
/// row below reaches the handler instead of being refused at the boundary.
pub(crate) const MALFORMED_DETECTOR_IDS: [(&str, &str); 7] = [
    (
        // 62 characters: the length the recorded rejection carried.
        "7a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8",
        "62 hexadecimal characters",
    ),
    (
        "deadbeefdeadbeefdeadbeefdeadbee",
        "31 characters: one short of the id length",
    ),
    (
        "deadbeefdeadbeefdeadbeefdeadbeef0",
        "33 characters: one past the id length",
    ),
    (
        "deadbeefdeadbeefdeadbeefdeadbeeg",
        "32 characters, but 'g' is not hexadecimal",
    ),
    ("not-a-detector-id", "not hexadecimal at all"),
    (
        "12345678-1234-1234-1234-123456789012",
        "a dashed UUID, which is 36 characters",
    ),
    ("0", "a single character"),
];

/// Well-formed detector ids no create in the Region under test ever minted. Case is
/// not significant to the id FORM, so an upper-cased id is well formed and is
/// refused as not owned rather than as a bad parameter.
const UNOWNED_DETECTOR_IDS: [(&str, &str); 3] = [
    (
        "deadbeefdeadbeefdeadbeefdeadbeef",
        "32 lowercase hexadecimal characters",
    ),
    (
        "DEADBEEFDEADBEEFDEADBEEFDEADBEEF",
        "32 uppercase hexadecimal characters",
    ),
    (
        "00000000000000000000000000000000",
        "32 zeros: the low end of the id space",
    ),
];

/// The message a value that cannot be a value of `member` is refused with.
pub(crate) fn invalid_parameter(member: &str) -> String {
    format!("The request is rejected because the parameter {member} has an invalid value.")
}

/// The message a LIST of account ids carrying a value that cannot be an account id
/// at all is refused with: GuardDuty's fixed format-rejection sentence for a batch,
/// naming the list `member` the request carried the ids in.
pub(crate) fn account_ids_format(member: &str) -> String {
    format!(
        "The request failed because parameter {member} has accountId values that are formatted incorrectly."
    )
}

/// The same format rejection where the request carried the account id in a STRUCTURE
/// member of its own rather than in a list: GuardDuty single-quotes the member name
/// and states the class.
pub(crate) const ACCOUNT_ID_FORMAT: &str =
    "The request failed because the 'accountId' was formatted incorrectly.";

/// The message GuardDuty refuses an unusable input value with where it does NOT
/// name the member at fault: its generic input-parameter sentence. Written out here
/// rather than read from the service so the test pins the words themselves.
pub(crate) const UNUSABLE_INPUT_VALUE: &str =
    "The request is rejected because an invalid or out-of-range value is specified as an input parameter.";

/// The message a page token this service did not issue is refused with, on every
/// paginated operation of this service: [`UNUSABLE_INPUT_VALUE`], which names
/// neither `nextToken` nor the token.
pub(crate) fn page_token_refusal() -> String {
    UNUSABLE_INPUT_VALUE.to_string()
}

/// The message a well-formed id this account does not own is refused with.
pub(crate) fn not_owned(member: &str) -> String {
    format!(
        "The request is rejected because the input {member} is not owned by the current account."
    )
}

/// Assert one rejection: the status, the error class in all three places it appears
/// (the body's `__type`, the body's `type` member, the `x-amzn-errortype` header) and
/// the classified message.
fn assert_rejection(case: &str, status: u16, body: &Value, expected_message: &str) {
    assert_eq!(
        status, 400,
        "case {case}: expected HTTP 400, got {status} {body:?}"
    );
    assert_eq!(
        body["__type"].as_str(),
        Some(CLASS),
        "case {case}: expected the error class in __type, got {body:?}"
    );
    assert_eq!(
        body["type"].as_str(),
        Some(CLASS),
        "case {case}: expected the error class in the type member, got {body:?}"
    );
    assert_eq!(
        error_code(body),
        CLASS,
        "case {case}: expected the error class on the wire, got {body:?}"
    );
    assert_eq!(
        error_shape(body),
        ENVELOPE,
        "case {case}: expected the modeled envelope shape in x-amzn-errortype, got {body:?}"
    );
    assert_eq!(
        body["message"].as_str(),
        Some(expected_message),
        "case {case}: expected the classified message, got {body:?}"
    );
}

/// Assert a rejection quotes neither the caller's value nor the endpoint's Region.
fn assert_quotes_nothing_back(case: &str, body: &Value, value: &str, region: &str) {
    let message = body["message"].as_str().unwrap_or_default();
    assert!(
        !message.contains(value),
        "case {case}: a rejection must not echo the caller's value {value:?}, got {message:?}"
    );
    assert!(
        !message.contains(region),
        "case {case}: a rejection must not name the endpoint's Region {region:?}, got {message:?}"
    );
}

/// Run one `DETECTOR_ID_OPERATIONS` row against `detector_id` in `region`.
async fn call_with_detector_id(
    router: &smithy_server_runtime::MultiServiceRouter,
    region: &str,
    operation: &(&str, &str, &str, Option<&str>),
    detector_id: &str,
) -> (u16, Value) {
    let (_, method, suffix, body) = operation;
    let body: Option<Value> = body.map(|raw| serde_json::from_str(raw).expect("test body"));
    call_in(
        router,
        region,
        method,
        &format!("/detector/{detector_id}{suffix}"),
        body,
    )
    .await
}

// ---------------------------------------------------------------------------
// A value that cannot be a detector id is a PARAMETER failure.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_malformed_detector_id_is_refused_as_an_invalid_parameter() {
    let region = "us-west-2";
    // Both with an empty store and with a detector present, so the classification
    // depends on the input and not on what happens to be stored.
    for populated in [false, true] {
        let router = new_router();
        if populated {
            let (status, created) =
                call_in(&router, region, "POST", "/detector", Some(json!({"enable": true}))).await;
            assert_eq!(
                status, 200,
                "setting up a detector in {region}: got {status} {created:?}"
            );
        }
        for (detector_id, why) in MALFORMED_DETECTOR_IDS {
            for operation in &DETECTOR_ID_OPERATIONS {
                let case =
                    format!("{} with {why} (store populated: {populated})", operation.0);
                let (status, body) =
                    call_with_detector_id(&router, region, operation, detector_id).await;
                assert_rejection(&case, status, &body, &invalid_parameter("detectorId"));
                assert_quotes_nothing_back(&case, &body, detector_id, region);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// A well-formed id the account does not own is an OWNERSHIP failure.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_well_formed_but_unowned_detector_id_is_refused_as_not_owned() {
    let region = "us-west-2";
    for populated in [false, true] {
        let router = new_router();
        if populated {
            let (status, created) =
                call_in(&router, region, "POST", "/detector", Some(json!({"enable": true}))).await;
            assert_eq!(
                status, 200,
                "setting up a detector in {region}: got {status} {created:?}"
            );
        }
        for (detector_id, why) in UNOWNED_DETECTOR_IDS {
            for operation in &DETECTOR_ID_OPERATIONS {
                let case =
                    format!("{} with {why} (store populated: {populated})", operation.0);
                let (status, body) =
                    call_with_detector_id(&router, region, operation, detector_id).await;
                assert_rejection(&case, status, &body, &not_owned("detectorId"));
                assert_quotes_nothing_back(&case, &body, detector_id, region);
            }
        }
    }
}

/// The two ways an id this service really minted stops being owned here: the detector
/// was deleted, or it belongs to another Region's endpoint. Both are well-formed ids,
/// so both are the ownership failure — and neither is a parameter failure.
#[tokio::test]
async fn an_id_this_service_minted_is_not_owned_once_deleted_or_read_from_elsewhere() {
    let home = "eu-central-1";
    let elsewhere = "ap-southeast-2";
    let router = new_router();

    let (status, created) =
        call_in(&router, home, "POST", "/detector", Some(json!({"enable": true}))).await;
    assert_eq!(status, 200, "create-detector in {home}: got {status} {created:?}");
    let detector_id = created["detectorId"]
        .as_str()
        .unwrap_or_else(|| panic!("create-detector: expected a detectorId, got {created:?}"))
        .to_string();

    // Another Region's endpoint does not own it.
    for operation in &DETECTOR_ID_OPERATIONS {
        let case = format!("{} from {elsewhere} on a detector of {home}", operation.0);
        let (status, body) =
            call_with_detector_id(&router, elsewhere, operation, &detector_id).await;
        assert_rejection(&case, status, &body, &not_owned("detectorId"));
        assert_quotes_nothing_back(&case, &body, &detector_id, elsewhere);
    }

    // Its own endpoint still does, until the delete.
    let (status, body) = call_in(
        &router,
        home,
        "DELETE",
        &format!("/detector/{detector_id}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "delete-detector in {home}: got {status} {body:?}");

    for operation in &DETECTOR_ID_OPERATIONS {
        let case = format!("{} on a deleted detector", operation.0);
        let (status, body) = call_with_detector_id(&router, home, operation, &detector_id).await;
        assert_rejection(&case, status, &body, &not_owned("detectorId"));
        assert_quotes_nothing_back(&case, &body, &detector_id, home);
    }
}

// ---------------------------------------------------------------------------
// The duplicate-detector rejection.
// ---------------------------------------------------------------------------

/// "You can have only one detector per account per Region"
/// (`smithy_query(query="doc",shape="CreateDetector")`). The second create is the
/// same rejection in every Region, and names neither the Region nor the detector the
/// first call minted.
#[tokio::test]
async fn a_second_create_detector_is_refused_as_an_existing_detector() {
    // Rows differ along the Region and along the body the second create sends.
    let regions = ["us-east-1", "us-west-2", "eu-central-1", "ap-southeast-2"];
    let second_bodies = [
        json!({"enable": true}),
        json!({"enable": false}),
        json!({"enable": true, "findingPublishingFrequency": "ONE_HOUR"}),
        json!({"enable": true, "clientToken": "a-token-the-first-create-did-not-use"}),
        json!({"enable": true, "tags": {"team": "security"}}),
    ];

    for region in regions {
        for second in &second_bodies {
            let router = new_router();
            let (status, created) = call_in(
                &router,
                region,
                "POST",
                "/detector",
                Some(json!({"enable": true})),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {region}: the first create must succeed, got {status} {created:?}"
            );
            let first_id = created["detectorId"].as_str().unwrap_or_default().to_string();

            let case = format!("second create-detector in {region} with {second}");
            let (status, body) =
                call_in(&router, region, "POST", "/detector", Some(second.clone())).await;
            assert_rejection(
                &case,
                status,
                &body,
                "The request is rejected because a detector already exists for the current account.",
            );
            assert_quotes_nothing_back(&case, &body, &first_id, region);
        }
    }
}

// ---------------------------------------------------------------------------
// A page token this service never issued.
// ---------------------------------------------------------------------------

/// A `NextToken` is opaque: the only thing a caller may do with it is hand back one
/// this service issued. Anything else is a value the service cannot use, refused
/// with its generic input-parameter sentence, which names neither the member nor the
/// token.
#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused_as_an_unusable_input_value() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;

    // Rows differ along the FORM of the token: not a number at all, negative,
    // fractional, out of range, and past the end of a list this service did issue
    // tokens into.
    let tokens = [
        "abc",
        "-1",
        "1.5",
        "99999999999999999999999999",
        "9999",
        "1e3",
    ];

    for token in tokens {
        for (operation, path, body) in [
            ("ListDetectors", "/detector".to_string(), None),
            (
                "ListFindings",
                format!("/detector/{detector_id}/findings"),
                Some(json!({})),
            ),
        ] {
            let case = format!("{operation} with nextToken {token:?}");
            let (status, response) = match body {
                None => {
                    call(
                        &router,
                        "GET",
                        &format!("{path}?nextToken={token}"),
                        None,
                    )
                    .await
                }
                Some(mut body) => {
                    body["nextToken"] = json!(token);
                    call(&router, "POST", &path, Some(body)).await
                }
            };
            assert_rejection(&case, status, &response, &page_token_refusal());
            assert_quotes_nothing_back(&case, &response, token, "us-east-1");
        }
    }
}

// ---------------------------------------------------------------------------
// A rejection the service models but does not classify by id.
// ---------------------------------------------------------------------------

/// Every other refusal this slice answers carries the same error class, so a client
/// has ONE error type to switch on for "this request was not acceptable". The rows
/// are a feature list the service refuses, a criterion key that names no field of a
/// finding and a sort attribute that names none either.
#[tokio::test]
async fn every_client_rejection_carries_the_same_error_class() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;

    let (status, body) = create_detector(
        &router,
        json!({
            "enable": true,
            "features": [
                {"name": "EKS_RUNTIME_MONITORING", "status": "ENABLED"},
                {"name": "RUNTIME_MONITORING", "status": "ENABLED"}
            ]
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "create-detector with both runtime features: got {status} {body:?}"
    );
    assert_eq!(
        body["type"].as_str(),
        Some(CLASS),
        "create-detector with both runtime features: got {body:?}"
    );
    assert_eq!(
        error_code(&body),
        CLASS,
        "create-detector with both runtime features: got {body:?}"
    );

    for (case, request) in [
        (
            "a criterion key that names no finding field",
            json!({"findingCriteria": {"criterion": {"service.actionType": {"equals": ["DNS_REQUEST"]}}}}),
        ),
        (
            "a sort attribute that names a detail document rather than a field in it",
            json!({"sortCriteria": {"attributeName": "resource.instanceDetails"}}),
        ),
    ] {
        let (status, body) = call(
            &router,
            "POST",
            &format!("/detector/{detector_id}/findings"),
            Some(request),
        )
        .await;
        assert_eq!(status, 400, "case {case}: got {status} {body:?}");
        assert_eq!(
            body["__type"].as_str(),
            Some(CLASS),
            "case {case}: expected the error class in __type, got {body:?}"
        );
        assert_eq!(
            body["type"].as_str(),
            Some(CLASS),
            "case {case}: expected the error class in the type member, got {body:?}"
        );
    }
}
