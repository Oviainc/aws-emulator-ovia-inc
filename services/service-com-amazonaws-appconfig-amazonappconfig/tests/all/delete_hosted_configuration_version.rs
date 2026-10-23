// SPDX-License-Identifier: CC0-1.0

//! `DeleteHostedConfigurationVersion`.
//!
//! The rule under test, in one sentence: a delete of a hosted configuration version that
//! answers its modelled 204 has taken exactly the addressed version out of the addressed
//! configuration profile's hosted store — so the reader and a second delete of that
//! version are refused with `ResourceNotFoundException` (HTTP 404), every other version
//! and every other profile still reads back byte for byte, the content budget the write
//! reserved is given back, and the number it freed is never handed out again — while a
//! delete naming an application, a profile or a version number the store does not hold
//! removes nothing and answers the same 404.
//!
//! Every case drives real HTTP requests through the router the emulator's front door
//! uses, so what is asserted is what an AppConfig client sees on the wire. The one
//! assertion made against state directly is the service's hosted-content accounting,
//! which no operation reports.
//!
//! The tables span what the rule generalises over rather than one recorded version: how
//! many versions the profile holds, which of them is deleted (the only one, the first,
//! one in the middle, the last), the payload's size, content type and byte values, and —
//! for the misses — which level of the path is unresolvable and the form of the version
//! number (never written, the `Integer` member's zero default, negative, `i32::MAX`).

use std::sync::Arc;

use service_com_amazonaws_appconfig_amazonappconfig::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::appconfig_workflow::{create_application, create_profile, router_on, send, Answer};

/// One payload a hosted configuration version is written from: its content type and its
/// bytes.
struct Payload {
    content_type: &'static str,
    content: Vec<u8>,
}

/// Where the hosted configuration versions of one profile live (`POST`/`GET`/`DELETE`
/// `…/hostedconfigurationversions`), from this operation's modelled URI
/// (`smithy_query(query="operation",operation="DeleteHostedConfigurationVersion")`).
fn hosted_uri(application: &str, profile: &str) -> String {
    format!("/applications/{application}/configurationprofiles/{profile}/hostedconfigurationversions")
}

/// Bytes of hosted configuration content this service is holding, across every profile.
/// No operation reports it, so the budget a delete frees is read off the state the
/// handlers share.
fn held_bytes(engine: &Arc<engine::InMemoryEngine>) -> usize {
    state::service_state(&**engine)
        .expect("the test engine must carry this service's state")
        .lock()
        .hosted_configuration_bytes()
}

/// `POST` one hosted configuration version and answer the version number it was given.
async fn create_version(
    router: &MultiServiceRouter,
    application: &str,
    profile: &str,
    case: &str,
    payload: &Payload,
) -> i32 {
    let created = send(
        router,
        "POST",
        &hosted_uri(application, profile),
        payload.content_type,
        payload.content.clone(),
        &[],
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "case {case:?}: CreateHostedConfigurationVersion must answer 201, got {} / {}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    created
        .header("Version-Number")
        .unwrap_or_else(|| panic!("case {case:?}: CreateHostedConfigurationVersion answered no Version-Number"))
        .parse()
        .unwrap_or_else(|error| {
            panic!("case {case:?}: Version-Number must be an integer, got {error}")
        })
}

/// `GET` one hosted configuration version.
async fn get_version(
    router: &MultiServiceRouter,
    application: &str,
    profile: &str,
    version: &str,
) -> Answer {
    send(
        router,
        "GET",
        &format!("{}/{version}", hosted_uri(application, profile)),
        "application/json",
        Vec::new(),
        &[],
    )
    .await
}

/// `DELETE` one hosted configuration version. The version number travels as text so a
/// case can address one no write could have produced.
async fn delete_version(
    router: &MultiServiceRouter,
    application: &str,
    profile: &str,
    version: &str,
) -> Answer {
    send(
        router,
        "DELETE",
        &format!("{}/{version}", hosted_uri(application, profile)),
        "application/json",
        Vec::new(),
        &[],
    )
    .await
}

/// A delete that must have succeeded: the modelled 204, with no body.
fn assert_deleted(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status,
        204,
        "case {case:?}: DeleteHostedConfigurationVersion must answer the modelled 204, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    assert!(
        answer.body.is_empty(),
        "case {case:?}: DeleteHostedConfigurationVersion answers no body, got {:?}",
        String::from_utf8_lossy(&answer.body)
    );
}

/// A call that must miss: the modelled `ResourceNotFoundException`, HTTP 404, with the
/// error type on the wire where an SDK classifies it from.
fn assert_not_found(case: &str, operation: &str, answer: &Answer) {
    assert_eq!(
        answer.status,
        404,
        "case {case:?}: {operation} must answer 404 for a version that is not held, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    assert!(
        answer.error_code().contains("ResourceNotFoundException"),
        "case {case:?}: {operation} must answer ResourceNotFoundException, got {:?}",
        answer.error_code()
    );
    assert!(
        answer
            .header("x-amzn-errortype")
            .is_some_and(|value| value.contains("ResourceNotFoundException")),
        "case {case:?}: {operation} must set x-amzn-errortype, got {:?}",
        answer.header("x-amzn-errortype")
    );
    assert!(
        answer
            .string("Message")
            .is_some_and(|message| !message.is_empty()),
        "case {case:?}: {operation} must carry a Message, got {}",
        answer.json()
    );
}

/// The payload table every delete case is built from: content types and byte values that
/// differ along what the rule generalises over, including bytes no text encoding carries
/// and a payload three orders of magnitude larger than the rest.
fn payloads() -> Vec<Payload> {
    vec![
        Payload {
            content_type: "application/json",
            content: br#"{"enabled":true}"#.to_vec(),
        },
        Payload {
            content_type: "application/x-yaml",
            content: b"enabled: true\nlimit: 7\n".to_vec(),
        },
        Payload {
            content_type: "text/plain",
            content: vec![b'x'],
        },
        Payload {
            content_type: "application/octet-stream",
            content: vec![0, 1, 2, 255, 0, 128],
        },
        Payload {
            content_type: "text/plain",
            content: vec![b'a'; 64 * 1024],
        },
    ]
}

/// Deleting a hosted configuration version removes exactly that version: the reader and a
/// second delete miss on it, every sibling version and the sibling profile are untouched,
/// the freed content is no longer charged against the service's budget, and the next write
/// is numbered past the deleted version rather than reusing its number.
#[tokio::test]
async fn deleting_a_hosted_configuration_version_removes_only_that_version() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let application = create_application(&router, "deleting").await;

    // A sibling profile under the same application, written once and never deleted from:
    // whatever the rows below do, this version must still read back.
    let witness_payload = Payload {
        content_type: "application/json",
        content: br#"{"witness":1}"#.to_vec(),
    };
    let witness_profile = create_profile(&router, &application, "witness").await;
    let witness_version =
        create_version(&router, &application, &witness_profile, "witness", &witness_payload).await;

    // Rows differ along how many versions the profile holds and which of them is
    // deleted: the only one, the first, one in the middle, the last, and the largest.
    let cases: Vec<(&str, usize, usize)> = vec![
        ("only version", 1, 1),
        ("first of three", 3, 1),
        ("middle of three", 3, 2),
        ("last of three", 3, 3),
        ("binary payload among five", 5, 4),
        ("64 KiB payload among five", 5, 5),
    ];

    let all_payloads = payloads();
    for (case, version_count, delete_position) in cases {
        let profile = create_profile(&router, &application, case).await;
        let written: Vec<&Payload> = all_payloads.iter().take(version_count).collect();
        assert_eq!(
            written.len(),
            version_count,
            "case {case:?}: the payload table must cover {version_count} versions, got {}",
            written.len()
        );

        let mut numbers = Vec::new();
        for payload in &written {
            numbers.push(create_version(&router, &application, &profile, case, payload).await);
        }

        let doomed_number = numbers[delete_position - 1];
        let doomed_payload = written[delete_position - 1];
        let bytes_before = held_bytes(&engine);

        let deleted = delete_version(
            &router,
            &application,
            &profile,
            &doomed_number.to_string(),
        )
        .await;
        assert_deleted(case, &deleted);

        // The budget the write reserved for exactly these bytes is given back, and
        // nothing else is.
        assert_eq!(
            held_bytes(&engine),
            bytes_before - doomed_payload.content.len(),
            "case {case:?}: deleting {} bytes must free exactly those, held {} before and {} after",
            doomed_payload.content.len(),
            bytes_before,
            held_bytes(&engine)
        );

        // The reader misses on the deleted version…
        let read_back = get_version(
            &router,
            &application,
            &profile,
            &doomed_number.to_string(),
        )
        .await;
        assert_not_found(case, "GetHostedConfigurationVersion", &read_back);

        // …and so does a second delete: the first one took the record out, so there is
        // nothing left for this call to remove.
        let again = delete_version(
            &router,
            &application,
            &profile,
            &doomed_number.to_string(),
        )
        .await;
        assert_not_found(case, "a repeated DeleteHostedConfigurationVersion", &again);

        // Every version this delete did not address is still there, byte for byte.
        for (index, payload) in written.iter().enumerate() {
            let number = numbers[index];
            if number == doomed_number {
                continue;
            }
            let survivor = get_version(&router, &application, &profile, &number.to_string()).await;
            assert_eq!(
                survivor.status,
                200,
                "case {case:?}: version {number} must survive the delete, got {} / {}",
                survivor.status,
                String::from_utf8_lossy(&survivor.body)
            );
            assert_eq!(
                survivor.body.as_ref(),
                payload.content.as_slice(),
                "case {case:?}: version {number} must read back byte for byte, got {} bytes instead of {}",
                survivor.body.len(),
                payload.content.len()
            );
            assert_eq!(
                survivor.header("Content-Type").as_deref(),
                Some(payload.content_type),
                "case {case:?}: version {number} must keep its ContentType, got {:?}",
                survivor.header("Content-Type")
            );
        }

        // A version number is an identity, not a slot: the next write is numbered past
        // every number ever assigned to this profile, so the deleted number cannot come
        // back holding different content.
        let next_number = create_version(
            &router,
            &application,
            &profile,
            case,
            &all_payloads[0],
        )
        .await;
        assert!(
            next_number > *numbers.iter().max().expect("each row writes a version"),
            "case {case:?}: the write after a delete must be numbered past {:?}, got {next_number}",
            numbers
        );
    }

    let witness = get_version(
        &router,
        &application,
        &witness_profile,
        &witness_version.to_string(),
    )
    .await;
    assert_eq!(
        witness.status,
        200,
        "case \"witness\": a sibling profile's version must be untouched by every delete, got {} / {}",
        witness.status,
        String::from_utf8_lossy(&witness.body)
    );
    assert_eq!(
        witness.body.as_ref(),
        witness_payload.content.as_slice(),
        "case \"witness\": the sibling profile's content must read back byte for byte, got {:?}",
        String::from_utf8_lossy(&witness.body)
    );
}

/// A delete that cannot resolve the application, the configuration profile or the version
/// number answers 404 `ResourceNotFoundException` and removes nothing.
#[tokio::test]
async fn deleting_a_version_the_store_does_not_hold_changes_nothing() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let application = create_application(&router, "misses").await;
    let profile = create_profile(&router, &application, "held").await;
    let payload = Payload {
        content_type: "application/json",
        content: br#"{"held":true}"#.to_vec(),
    };
    let held = create_version(&router, &application, &profile, "held", &payload).await;
    let bytes_before = held_bytes(&engine);

    // An id of the modelled `Id` shape that nothing was created under, so the miss is
    // about existence rather than about the id's spelling.
    let unknown_application = format!("{application}x");
    let unknown_profile = format!("{profile}x");

    // Rows differ along which level of the path is unresolvable and along the form of
    // the version number: one past the last written, the `Integer` member's zero
    // default, a negative number, and the largest value an `Integer` carries.
    let cases: Vec<(&str, &str, &str, String)> = vec![
        (
            "unknown application",
            unknown_application.as_str(),
            profile.as_str(),
            held.to_string(),
        ),
        (
            "unknown configuration profile",
            application.as_str(),
            unknown_profile.as_str(),
            held.to_string(),
        ),
        (
            "version one past the last written",
            application.as_str(),
            profile.as_str(),
            (held + 1).to_string(),
        ),
        (
            "version zero",
            application.as_str(),
            profile.as_str(),
            "0".to_string(),
        ),
        (
            "negative version",
            application.as_str(),
            profile.as_str(),
            "-1".to_string(),
        ),
        (
            "the largest Integer",
            application.as_str(),
            profile.as_str(),
            i32::MAX.to_string(),
        ),
    ];

    for (case, target_application, target_profile, version) in cases {
        let answer = delete_version(&router, target_application, target_profile, &version).await;
        assert_not_found(case, "DeleteHostedConfigurationVersion", &answer);

        // Nothing was removed: the version that IS held still reads back, and the
        // service is still charged for exactly its bytes.
        let still_there = get_version(&router, &application, &profile, &held.to_string()).await;
        assert_eq!(
            still_there.status,
            200,
            "case {case:?}: a refused delete must leave the held version readable, got {} / {}",
            still_there.status,
            String::from_utf8_lossy(&still_there.body)
        );
        assert_eq!(
            still_there.body.as_ref(),
            payload.content.as_slice(),
            "case {case:?}: a refused delete must leave the content intact, got {:?}",
            String::from_utf8_lossy(&still_there.body)
        );
        assert_eq!(
            held_bytes(&engine),
            bytes_before,
            "case {case:?}: a refused delete must free no budget, expected {bytes_before} bytes held, got {}",
            held_bytes(&engine)
        );
    }
}
