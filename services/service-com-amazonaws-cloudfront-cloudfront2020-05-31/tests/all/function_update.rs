// SPDX-License-Identifier: CC0-1.0

//! `UpdateFunction`, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rules under test are the ones CloudFront's function surface states
//! for an update:
//!
//! * "You can update a function's code or the comment that describes the function. You
//!   cannot update a function's name": an update REPLACES the code and the configuration of
//!   the stored function's `DEVELOPMENT` stage, leaves its identity (its name and the ARN
//!   built from it) alone, mints a FRESH version token, and is what every later read of
//!   that stage answers from;
//! * "To update a function, you provide the function's name and version (ETag value)": the
//!   submitted version is checked against the STORED one, so a header that is no version
//!   token is "missing or not valid" (`InvalidIfMatchVersion`) and a well-formed token that
//!   is not the current one is a failed precondition (`PreconditionFailed`);
//! * a name no function holds is "the function does not exist" (`NoSuchFunctionExists`),
//!   whatever the `If-Match` carries;
//! * an update the service cannot hold — code past the published function-size quota, or a
//!   configuration describing no function it can hold — is refused with the error the model
//!   names for it, exactly as the create path refuses the same payload;
//! * every refusal leaves the stored function untouched: its code, its configuration and
//!   its version token all stand, and an update is not a publish, so the `LIVE` stage of a
//!   function that was never published still does not exist.
//!
//! The table rows differ along everything these rules generalise over: the comment's length
//! and whether it is ASCII, the runtime (each direction between the two values the modelled
//! `FunctionRuntime` enum admits), the code's size (the modelled minimum, a real script and
//! the published quota bound) and whether it is text at all, whether the configuration
//! gains or loses a key value store association, and an update that submits exactly what is
//! already stored.

use std::collections::BTreeSet;

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    FUNCTION_STAGE_DEVELOPMENT, FUNCTION_STAGE_LIVE, MAX_FUNCTION_CODE_BYTES,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{
    call, emulator, is_version_token, Answer, ACCOUNT_ID, API_VERSION,
};
// The function surface's request builder, callers and summary assertion: one `pub(crate)`
// copy, in the suite that first needed them, rather than a copy per suite.
use crate::function_lifecycle::{
    assert_summary, base64, create_function, create_key_value_store, describe_function,
    get_function, Create, VIEWER_REQUEST_CODE,
};

/// The response header `UpdateFunctionResult$ETag` is bound to.
///
/// From `smithy_query(query="operation", operation="UpdateFunction")`: this one operation
/// binds its version token to `ETtag`, where the rest of the function surface binds `ETag`.
/// Header names are compared case-insensitively, so the case here is immaterial.
const UPDATE_ETAG_HEADER: &str = "ETtag";

/// A version token that is well formed — `E` + 13 characters, the shape this service mints
/// — but that no function in a fresh emulator holds, so a precondition carrying it is STALE
/// rather than invalid.
const UNHELD_VERSION_TOKEN: &str = "E1ABCDEFGHIJKL";

// ---------------------------------------------------------------------------
// Calling the update
// ---------------------------------------------------------------------------

/// The `UpdateFunctionRequest` document for `update`: an update carries the configuration
/// and the code in the body, the name as the URI label and the version as the `If-Match`
/// header.
fn update_xml(update: &Create) -> String {
    update_xml_with_config(&update.config_xml(), &update.code)
}

/// The `UpdateFunctionRequest` document carrying `config_xml` and `code`, for the cases
/// whose configuration is written out rather than built from a [`Create`].
fn update_xml_with_config(config_xml: &str, code: &[u8]) -> String {
    format!(
        "<UpdateFunctionRequest xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         {config_xml}<FunctionCode>{}</FunctionCode></UpdateFunctionRequest>",
        base64(code)
    )
}

/// `PUT /2020-05-31/function/{Name}` with `update`'s configuration and code, under the
/// `If-Match` header `if_match` (left out of the request entirely when `None`).
async fn update_function(
    router: &MultiServiceRouter,
    name: &str,
    update: &Create,
    if_match: Option<&str>,
) -> Answer {
    update_with_body(router, name, &update_xml(update), if_match).await
}

/// `PUT /2020-05-31/function/{Name}` with the request document `body`.
async fn update_with_body(
    router: &MultiServiceRouter,
    name: &str,
    body: &str,
    if_match: Option<&str>,
) -> Answer {
    call(
        router,
        "PUT",
        &format!("/{API_VERSION}/function/{name}"),
        Some(body),
        if_match,
    )
    .await
}

/// Create the function `create` asks for and answer the version token of its `DEVELOPMENT`
/// stage.
async fn seed(router: &MultiServiceRouter, create: &Create) -> String {
    let created = create_function(router, &create.xml()).await;
    assert_eq!(
        created.status, 201,
        "case {}: the seed create must answer HTTP 201, got {} with {}",
        create.case, created.status, created.raw
    );
    created.header("etag").to_string()
}

/// One `Create` for a function named `name`, with no association and no tags.
fn function(
    case: &'static str,
    name: &str,
    comment: &str,
    runtime: &'static str,
    code: Vec<u8>,
) -> Create {
    Create {
        case,
        name: name.to_string(),
        comment: comment.to_string(),
        runtime,
        code,
        associations: Vec::new(),
        tags: Vec::new(),
    }
}

/// Assert the stored function still is `expected` with the version token `etag`: the
/// describe reports that configuration and token, and the get reports that code.
async fn assert_stored(router: &MultiServiceRouter, expected: &Create, etag: &str, context: &str) {
    let case = expected.case;
    let described = describe_function(router, &expected.name, None).await;
    assert_eq!(
        described.status, 200,
        "case {case} ({context}): expected HTTP 200 from the describe, got {} with {}",
        described.status, described.raw
    );
    assert_summary(&described, expected, FUNCTION_STAGE_DEVELOPMENT, context);
    assert_eq!(
        described.header("etag"),
        etag,
        "case {case} ({context}): expected the version token {etag:?} from the describe, got {:?}",
        described.header("etag")
    );

    let code = get_function(router, &expected.name, None).await;
    assert_eq!(
        code.status, 200,
        "case {case} ({context}): expected HTTP 200 from the get, got {}",
        code.status
    );
    assert_eq!(
        code.body, expected.code,
        "case {case} ({context}): expected {} bytes of stored code, got {} bytes",
        expected.code.len(),
        code.body.len()
    );
    assert_eq!(
        code.etag, etag,
        "case {case} ({context}): expected the version token {etag:?} from the get, got {:?}",
        code.etag
    );
}

// ---------------------------------------------------------------------------
// The updates under test
// ---------------------------------------------------------------------------

/// One row: the function to create, and the update to apply to it.
struct Row {
    case: &'static str,
    create: Create,
    update: Create,
}

/// The seed-and-update pairs every rule below is asserted over.
///
/// `FunctionRuntime` admits exactly `cloudfront-js-1.0` and `cloudfront-js-2.0`,
/// `FunctionName` is `length 1..64, pattern ^[a-zA-Z0-9-_]{1,64}$` and `FunctionBlob` is
/// `length 1..40960` (all from `smithy_query` on `UpdateFunction` and `FunctionRuntime`), so
/// the rows change the comment in both directions (empty to long and non-ASCII, and back),
/// the runtime in both directions, the code between the modelled minimum, a real script,
/// bytes that are not UTF-8 at all and the published quota bound, and the configuration's
/// key value store association in both directions — plus one update that submits exactly
/// what is already stored.
///
/// `kvs_arn` is the ARN of a key value store this service holds, so the rows that associate
/// one name a store that really exists.
fn update_table(kvs_arn: &str) -> Vec<Row> {
    vec![
        Row {
            case: "a script becomes the smallest admissible code, the runtime moves up",
            create: function(
                "a script becomes the smallest admissible code, the runtime moves up",
                "rewrite_uri",
                "rewrites the URI",
                "cloudfront-js-1.0",
                VIEWER_REQUEST_CODE.as_bytes().to_vec(),
            ),
            update: function(
                "a script becomes the smallest admissible code, the runtime moves up",
                "rewrite_uri",
                "",
                "cloudfront-js-2.0",
                b"f".to_vec(),
            ),
        },
        Row {
            case: "an empty comment becomes a long non-ASCII one, the runtime moves down",
            create: function(
                "an empty comment becomes a long non-ASCII one, the runtime moves down",
                "a",
                "",
                "cloudfront-js-2.0",
                b"f".to_vec(),
            ),
            update: function(
                "an empty comment becomes a long non-ASCII one, the runtime moves down",
                "a",
                "プロダクション — flags & limits",
                "cloudfront-js-1.0",
                VIEWER_REQUEST_CODE.as_bytes().to_vec(),
            ),
        },
        Row {
            case: "the longest admissible name, code that is not UTF-8",
            create: function(
                "the longest admissible name, code that is not UTF-8",
                &"n".repeat(64),
                &"c".repeat(128),
                "cloudfront-js-2.0",
                VIEWER_REQUEST_CODE.as_bytes().to_vec(),
            ),
            update: function(
                "the longest admissible name, code that is not UTF-8",
                &"n".repeat(64),
                "binary code",
                "cloudfront-js-2.0",
                vec![0x00, 0xff, 0xfe, 0x80, b'\n', 0x7f, 0x01],
            ),
        },
        Row {
            case: "the code grows to exactly the published quota",
            create: function(
                "the code grows to exactly the published quota",
                "sized-up_42",
                "grows to the quota",
                "cloudfront-js-1.0",
                b"f".to_vec(),
            ),
            update: function(
                "the code grows to exactly the published quota",
                "sized-up_42",
                "at the quota",
                "cloudfront-js-1.0",
                vec![b'x'; MAX_FUNCTION_CODE_BYTES],
            ),
        },
        Row {
            case: "the configuration gains a key value store association",
            create: function(
                "the configuration gains a key value store association",
                "gains_store",
                "no store yet",
                "cloudfront-js-2.0",
                b"f".to_vec(),
            ),
            update: Create {
                associations: vec![kvs_arn.to_string()],
                ..function(
                    "the configuration gains a key value store association",
                    "gains_store",
                    "reads the catalog",
                    "cloudfront-js-2.0",
                    b"f".to_vec(),
                )
            },
        },
        Row {
            case: "the configuration loses its key value store association",
            create: Create {
                associations: vec![kvs_arn.to_string()],
                ..function(
                    "the configuration loses its key value store association",
                    "loses_store",
                    "reads the catalog",
                    "cloudfront-js-2.0",
                    b"f".to_vec(),
                )
            },
            update: function(
                "the configuration loses its key value store association",
                "loses_store",
                "no longer reads the catalog",
                "cloudfront-js-2.0",
                b"f".to_vec(),
            ),
        },
        Row {
            case: "an update submitting exactly what is already stored",
            create: function(
                "an update submitting exactly what is already stored",
                "resubmitted",
                "unchanged",
                "cloudfront-js-2.0",
                VIEWER_REQUEST_CODE.as_bytes().to_vec(),
            ),
            update: function(
                "an update submitting exactly what is already stored",
                "resubmitted",
                "unchanged",
                "cloudfront-js-2.0",
                VIEWER_REQUEST_CODE.as_bytes().to_vec(),
            ),
        },
    ]
}

// ---------------------------------------------------------------------------
// The rules
// ---------------------------------------------------------------------------

/// An update PERSISTS the submitted code and configuration into the function's
/// `DEVELOPMENT` stage and mints a FRESH version token: the answer, a later describe and a
/// later get all report what this call stored, the function keeps the name and the ARN its
/// create minted, and no two updates across the table hand back the same token.
#[tokio::test]
async fn an_update_persists_the_submitted_code_and_configuration() {
    let router = emulator();
    let kvs_arn = create_key_value_store(&router, "update_table_store").await;
    let mut etags = BTreeSet::new();

    for Row {
        case,
        create,
        update,
    } in update_table(&kvs_arn)
    {
        let created_etag = seed(&router, &create).await;
        assert!(
            etags.insert(created_etag.clone()),
            "case {case}: the create's version token {created_etag} repeated"
        );

        let answer = update_function(&router, &create.name, &update, Some(&created_etag)).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected HTTP 200 from the update, got {} with {}",
            answer.status, answer.raw
        );
        // The answer is the stage this call wrote, under the name and ARN the create
        // minted: "you cannot update a function's name".
        assert_summary(&answer, &update, FUNCTION_STAGE_DEVELOPMENT, "the update");

        let updated_etag = answer.header(UPDATE_ETAG_HEADER).to_string();
        assert!(
            is_version_token(&updated_etag),
            "case {case}: expected a version token in the {UPDATE_ETAG_HEADER} header, got \
             {updated_etag:?}"
        );
        assert_ne!(
            updated_etag, created_etag,
            "case {case}: expected a fresh version token, got the create's {created_etag}"
        );
        assert!(
            etags.insert(updated_etag.clone()),
            "case {case}: the update's version token {updated_etag} repeated"
        );

        // Both reads answer from the one record this update wrote.
        assert_stored(&router, &update, &updated_etag, "after the update").await;

        // The version the update replaced no longer satisfies the precondition.
        let stale = update_function(&router, &create.name, &create, Some(&created_etag)).await;
        assert_eq!(
            stale.status, 412,
            "case {case}: expected HTTP 412 for the replaced version, got {} with {}",
            stale.status, stale.raw
        );
        assert_eq!(
            stale.error_code(),
            "PreconditionFailed",
            "case {case}: expected PreconditionFailed, got {}",
            stale.raw
        );
        assert_stored(&router, &update, &updated_etag, "after a stale update").await;

        // An update is not a publish: the LIVE stage still does not exist.
        let live = describe_function(&router, &create.name, Some(FUNCTION_STAGE_LIVE)).await;
        assert_eq!(
            live.status, 404,
            "case {case}: expected HTTP 404 describing the LIVE stage, got {} with {}",
            live.status, live.raw
        );
    }
}

/// The precondition is evaluated against the STORED version: a header that is no version
/// token at all is `InvalidIfMatchVersion` (HTTP 400) and a well-formed token that is not
/// the current one is `PreconditionFailed` (HTTP 412), while the current token — bare or in
/// the quoted form entity tags normally take — is the one precondition an update lands
/// under. Every refusal leaves the stored function exactly as it was.
#[tokio::test]
async fn a_precondition_that_is_not_the_stored_version_is_refused() {
    let router = emulator();
    let stored = function(
        "the function under test",
        "preconditioned",
        "the stored function",
        "cloudfront-js-2.0",
        VIEWER_REQUEST_CODE.as_bytes().to_vec(),
    );
    let etag = seed(&router, &stored).await;

    // A second function, so one function's CURRENT token is a stale token for another.
    let other = function(
        "another function",
        "other_function",
        "another function",
        "cloudfront-js-1.0",
        b"f".to_vec(),
    );
    let other_etag = seed(&router, &other).await;

    let attempt = function(
        "the function under test",
        "preconditioned",
        "an update that must not land",
        "cloudfront-js-1.0",
        b"must not land".to_vec(),
    );

    // Rows: an empty header, a value that is no version token, a token one character short
    // of the shape this service mints, a well-formed token no function holds, and another
    // function's current token.
    let cases: [(&str, &str, u16, &str); 5] = [
        ("an empty If-Match", "", 400, "InvalidIfMatchVersion"),
        (
            "an If-Match that is no version token",
            "not-a-version",
            400,
            "InvalidIfMatchVersion",
        ),
        (
            "an If-Match one character short of a version token",
            "E2QWRUHAPOMQZ",
            400,
            "InvalidIfMatchVersion",
        ),
        (
            "a well-formed version token no function holds",
            UNHELD_VERSION_TOKEN,
            412,
            "PreconditionFailed",
        ),
        (
            "another function's current version token",
            other_etag.as_str(),
            412,
            "PreconditionFailed",
        ),
    ];

    for (case, if_match, status, error_code) in cases {
        let answer = update_function(&router, &stored.name, &attempt, Some(if_match)).await;
        assert_eq!(
            answer.status, status,
            "case {case}: expected HTTP {status} for If-Match={if_match:?}, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            error_code,
            "case {case}: expected {error_code}, got {}",
            answer.raw
        );
        assert_stored(&router, &stored, &etag, case).await;
    }

    // `IfMatch` is a required member bound to the `If-Match` header, so a request carrying
    // no header at all never reaches this operation: the framework refuses it before the
    // handler runs, which is why the rows above all carry one.

    // The current token in the quoted form HTTP entity tags normally take is the same
    // precondition, so this one update LANDS.
    let quoted = format!("\"{etag}\"");
    let accepted = update_function(&router, &stored.name, &attempt, Some(&quoted)).await;
    assert_eq!(
        accepted.status, 200,
        "case the current version token, quoted: expected HTTP 200, got {} with {}",
        accepted.status, accepted.raw
    );
    let accepted_etag = accepted.header(UPDATE_ETAG_HEADER).to_string();
    assert_stored(&router, &attempt, &accepted_etag, "after the quoted update").await;
}

/// A name no stored function holds is `NoSuchFunctionExists` (HTTP 404) whatever shape the
/// name takes and whatever the `If-Match` carries — the existence of the function is decided
/// before its version can be compared — and the update stores nothing under that name.
#[tokio::test]
async fn a_name_no_function_holds_is_not_found() {
    let router = emulator();
    let present = function(
        "the one function that exists",
        "present_function",
        "the only function",
        "cloudfront-js-2.0",
        VIEWER_REQUEST_CODE.as_bytes().to_vec(),
    );
    let etag = seed(&router, &present).await;

    // Rows: a name never created, the same name in another case (names are compared as
    // given), a prefix of the name that exists, and the longest name the model admits —
    // each with an `If-Match` that is current for the function that DOES exist, one that is
    // no version token, and none at all.
    let names: [(&str, String); 4] = [
        ("a name never created", "absent_function".to_string()),
        (
            "the same name in another case",
            "PRESENT_FUNCTION".to_string(),
        ),
        ("a prefix of the existing name", "present".to_string()),
        ("the longest admissible name", "z".repeat(64)),
    ];
    let preconditions: [(&str, Option<&str>); 3] = [
        ("another function's current token", Some(etag.as_str())),
        ("a value that is no version token", Some("not-a-version")),
        ("a well-formed token no function holds", Some(UNHELD_VERSION_TOKEN)),
    ];

    let attempt = function(
        "an update of a function that does not exist",
        "absent_function",
        "must not be stored",
        "cloudfront-js-1.0",
        b"must not be stored".to_vec(),
    );

    for (case, name) in &names {
        for (precondition, if_match) in &preconditions {
            let answer = update_function(&router, name, &attempt, *if_match).await;
            assert_eq!(
                answer.status, 404,
                "case {case} with {precondition}: expected HTTP 404 for {name:?}, got {} with {}",
                answer.status, answer.raw
            );
            assert_eq!(
                answer.error_code(),
                "NoSuchFunctionExists",
                "case {case} with {precondition}: expected NoSuchFunctionExists, got {}",
                answer.raw
            );
        }

        let described = describe_function(&router, name, None).await;
        assert_eq!(
            described.status, 404,
            "case {case}: the refused update must store nothing, got {} with {}",
            described.status, described.raw
        );
    }

    // The function that does exist is untouched by every refusal above.
    assert_stored(&router, &present, &etag, "after the refused updates").await;
}

/// CloudFront's published quota bounds the code an update stores exactly as it bounds the
/// code a create stores: code at the bound is accepted, code past it is
/// `FunctionSizeLimitExceeded` (HTTP 413), and code past the MODELLED bound (`FunctionBlob`,
/// `length 1..40960`) is refused at the boundary with HTTP 400. Every refusal leaves the
/// stored code and version token as they were.
#[tokio::test]
async fn code_past_the_published_quota_is_refused() {
    let router = emulator();

    // Rows: the published bound itself, one byte past it, and one byte past the modelled
    // maximum of `FunctionBlob`.
    let cases: [(&str, usize, u16, &str); 3] = [
        (
            "exactly the published quota",
            MAX_FUNCTION_CODE_BYTES,
            200,
            "",
        ),
        (
            "one byte past the published quota",
            MAX_FUNCTION_CODE_BYTES + 1,
            413,
            "FunctionSizeLimitExceeded",
        ),
        ("one byte past the modelled maximum", 40_961, 400, ""),
    ];

    for (case, size, status, error_code) in cases {
        let stored = function(
            case,
            &format!("sized_{size}"),
            "the stored function",
            "cloudfront-js-2.0",
            b"f".to_vec(),
        );
        let etag = seed(&router, &stored).await;

        let attempt = Create {
            code: vec![b'y'; size],
            ..stored.clone()
        };
        let answer = update_function(&router, &stored.name, &attempt, Some(&etag)).await;
        assert_eq!(
            answer.status, status,
            "case {case}: expected HTTP {status} for {size} bytes, got {} with {}",
            answer.status, answer.raw
        );
        if !error_code.is_empty() {
            assert_eq!(
                answer.error_code(),
                error_code,
                "case {case}: expected {error_code}, got {}",
                answer.raw
            );
        }

        if status == 200 {
            let accepted_etag = answer.header(UPDATE_ETAG_HEADER).to_string();
            assert_stored(&router, &attempt, &accepted_etag, "the accepted size").await;
        } else {
            // The refused update spent no version token and replaced no code.
            assert_stored(&router, &stored, &etag, "the refused size").await;
        }
    }
}

/// A configuration this service cannot hold is refused with the modelled `InvalidArgument`
/// (HTTP 400) and stores nothing — the same rule the create path applies: a
/// `KeyValueStoreAssociations` wrapper whose `Quantity` disagrees with its `Items`, more
/// associations than a function may carry, and an association naming a key value store this
/// service does not hold.
#[tokio::test]
async fn a_configuration_the_service_cannot_hold_is_refused() {
    let router = emulator();
    let first = create_key_value_store(&router, "update_store_one").await;
    let second = create_key_value_store(&router, "update_store_two").await;
    // A well-formed key value store ARN (the modelled pattern demands one) that no store in
    // this service holds.
    let absent = format!(
        "arn:aws:cloudfront::{ACCOUNT_ID}:key-value-store/\
         00000000-0000-4000-8000-000000000000"
    );

    let stored = function(
        "the function under test",
        "refused_config",
        "the stored function",
        "cloudfront-js-2.0",
        VIEWER_REQUEST_CODE.as_bytes().to_vec(),
    );
    let etag = seed(&router, &stored).await;

    // Rows: a wrapper that overstates its length, one that understates it, two associations
    // where one is the most a function may carry, and one association naming a store that
    // does not exist.
    let overstated = format!(
        "<FunctionConfig><Comment>overstated</Comment><Runtime>cloudfront-js-2.0</Runtime>\
         <KeyValueStoreAssociations><Quantity>2</Quantity><Items>\
         <KeyValueStoreAssociation><KeyValueStoreARN>{first}</KeyValueStoreARN>\
         </KeyValueStoreAssociation></Items></KeyValueStoreAssociations></FunctionConfig>"
    );
    let understated = format!(
        "<FunctionConfig><Comment>understated</Comment><Runtime>cloudfront-js-2.0</Runtime>\
         <KeyValueStoreAssociations><Quantity>0</Quantity><Items>\
         <KeyValueStoreAssociation><KeyValueStoreARN>{first}</KeyValueStoreARN>\
         </KeyValueStoreAssociation></Items></KeyValueStoreAssociations></FunctionConfig>"
    );
    let cases: [(&str, String); 4] = [
        ("a Quantity above the Items length", overstated),
        ("a Quantity below the Items length", understated),
        (
            "more associations than a function may carry",
            Create {
                associations: vec![first.clone(), second.clone()],
                ..stored.clone()
            }
            .config_xml(),
        ),
        (
            "an association naming a store that does not exist",
            Create {
                associations: vec![absent.clone()],
                ..stored.clone()
            }
            .config_xml(),
        ),
    ];

    for (case, config_xml) in cases {
        let body = update_xml_with_config(&config_xml, b"must not land");
        let answer = update_with_body(&router, &stored.name, &body, Some(&etag)).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected HTTP 400, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "InvalidArgument",
            "case {case}: expected InvalidArgument, got {}",
            answer.raw
        );
        assert_stored(&router, &stored, &etag, case).await;
    }
}
