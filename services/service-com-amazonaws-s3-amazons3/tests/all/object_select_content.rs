// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `SelectObjectContent` — the `?select` subresource of an object.
//!
//! Every assertion drives a real HTTP request through the router (the harness the
//! bucket-lifecycle suite in this crate owns), so what is pinned is the wire behaviour —
//! status, error code, error message — rather than any handler's internals.
//!
//! The rules under test:
//!
//! * Amazon S3 serves no Select query for an account that did not already have S3 Select
//!   enabled, so a `?select` request is refused with HTTP 405 `MethodNotAllowed` and that
//!   code's published message — the refusal every S3 resource answers a method it does not
//!   serve with, and the same one the `?torrent` subresource already carries
//!   ([`super::object_torrent`]). It is NOT the emulator's 501 `NotImplemented`, which
//!   claims only that nobody has written the operation yet and is a code this operation
//!   models nowhere; a client that probes Select and falls back on `MethodNotAllowed`
//!   never sees a 501.
//! * The withdrawal is account-wide rather than per-object, so the refusal does not depend
//!   on what the request asks for — its expression, its expression type, its input and
//!   output serializations, a scan range, request-progress or customer-key headers, the
//!   `select-type` generation, the order of the query parameters — nor on which bucket or
//!   key it names, nor on whether the emulator holds them.
//! * The refusal reports: every object stored before it reads back byte for byte
//!   afterwards, and a key no request wrote is still absent.
//! * It is scoped to the Select data plane: a request that carries `select` where that
//!   subresource addresses no object keeps the framework's own routing refusal, so an
//!   operation that is merely unwritten still answers as unwritten.
//!
//! The status, code and message below are AWS's published ones for this refusal: the
//! `MethodNotAllowed` entry of S3's "Error responses" list of error codes
//! (<https://docs.aws.amazon.com/AmazonS3/latest/API/ErrorResponses.html#ErrorCodeList>),
//! written out literally in one place here so that a change to a crate constant cannot
//! move the expectation and the service together.

use service_com_amazonaws_s3_amazons3::state::{
    METHOD_NOT_ALLOWED_CODE, METHOD_NOT_ALLOWED_MESSAGE, METHOD_NOT_ALLOWED_STATUS,
    NO_SUCH_KEY_CODE,
};
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{router, send_bytes, Answer};
use super::object_and_tagging_roundtrip::{bucket, get_object, put_object};

/// The CSV every row of the table below selects over, so the request meets a real object
/// of the shape a Select query is written for.
const ROWS_CSV: &[u8] = b"id,name,category\n1,alice,gold\n2,bob,silver\n3,carol,bronze\n";

/// The request document S3's Select API takes, assembled from the members a case varies.
///
/// The member names and their nesting are the model's own
/// (`smithy_query(query="closure",operation="SelectObjectContent")`): `Expression`,
/// `ExpressionType`, `InputSerialization` and `OutputSerialization` are the required ones,
/// `ScanRange` and `RequestProgress` the optional ones.
fn select_request(expression: &str, expression_type: &str, input: &str, output: &str) -> Vec<u8> {
    format!(
        "<SelectObjectContentRequest xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
         <Expression>{expression}</Expression>\
         <ExpressionType>{expression_type}</ExpressionType>\
         <InputSerialization>{input}</InputSerialization>\
         <OutputSerialization>{output}</OutputSerialization>\
         </SelectObjectContentRequest>"
    )
    .into_bytes()
}

/// `POST /<bucket>/<key>?<query>` with a Select request document — the route the model
/// binds `SelectObjectContent` at (`POST /{Bucket}/{Key+}?select&select-type=2`), with
/// whatever query string and headers the case names.
async fn select_object_content(
    router: &MultiServiceRouter,
    bucket: &str,
    key: &str,
    query: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> Answer {
    let mut all: Vec<(&str, &str)> = vec![("content-type", "application/xml")];
    all.extend_from_slice(headers);
    send_bytes(
        router,
        "POST",
        &format!("/{bucket}/{key}?{query}"),
        &all,
        body,
    )
    .await
}

/// One row of the Select table: the case, the key the request names, whether the bucket is
/// made to hold that key first, the query string after the `?`, the request's headers, and
/// the request document it carries.
struct Row {
    case: &'static str,
    key: &'static str,
    stored: bool,
    query: &'static str,
    headers: Vec<(&'static str, &'static str)>,
    body: Vec<u8>,
}

/// The rows: one Select request per thing the rule generalises over — the expression and
/// its type, the input and output serializations, the optional members, the SSE-C headers,
/// the `select-type` generation and the query order, where the key sits and how it is
/// spelled, and whether the bucket holds it at all.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a CSV projection with a WHERE clause, the published request shape",
            key: "data/rows.csv",
            stored: true,
            query: "select&select-type=2",
            headers: Vec::new(),
            body: select_request(
                "SELECT s.id, s.name, s.category FROM S3Object s WHERE s.id = '2'",
                "SQL",
                "<CSV><FileHeaderInfo>USE</FileHeaderInfo></CSV>",
                "<CSV></CSV>",
            ),
        },
        Row {
            case: "a whole-object SELECT * with no header row declared",
            key: "data/headerless.csv",
            stored: true,
            query: "select&select-type=2",
            headers: Vec::new(),
            body: select_request(
                "SELECT * FROM S3Object",
                "SQL",
                "<CSV><FileHeaderInfo>NONE</FileHeaderInfo></CSV>",
                "<CSV></CSV>",
            ),
        },
        Row {
            case: "an aggregate over a JSON-lines input answered as JSON",
            key: "data/records.jsonl",
            stored: true,
            query: "select&select-type=2",
            headers: Vec::new(),
            body: select_request(
                "SELECT COUNT(*) FROM S3Object s",
                "SQL",
                "<JSON><Type>LINES</Type></JSON>",
                "<JSON></JSON>",
            ),
        },
        Row {
            case: "a Parquet input with a GZIP-compressed CSV output",
            key: "data/table.parquet",
            stored: true,
            query: "select&select-type=2",
            headers: Vec::new(),
            body: select_request(
                "SELECT s._1 FROM S3Object s LIMIT 1",
                "SQL",
                "<Parquet></Parquet>",
                "<CSV><FieldDelimiter>;</FieldDelimiter></CSV>",
            ),
        },
        Row {
            case: "a request carrying the optional scan range and progress members",
            key: "data/scanned.csv",
            stored: true,
            query: "select&select-type=2",
            headers: Vec::new(),
            body: {
                let mut document = String::from_utf8(select_request(
                    "SELECT * FROM S3Object",
                    "SQL",
                    "<CSV><FileHeaderInfo>USE</FileHeaderInfo></CSV>",
                    "<CSV></CSV>",
                ))
                .expect("the request document is UTF-8");
                document = document.replace(
                    "</SelectObjectContentRequest>",
                    "<RequestProgress><Enabled>true</Enabled></RequestProgress>\
                     <ScanRange><Start>0</Start><End>16</End></ScanRange>\
                     </SelectObjectContentRequest>",
                );
                document.into_bytes()
            },
        },
        Row {
            case: "a request whose customer-provided key headers are set",
            key: "data/sse-c.csv",
            stored: true,
            query: "select&select-type=2",
            headers: vec![
                ("x-amz-server-side-encryption-customer-algorithm", "AES256"),
                (
                    "x-amz-server-side-encryption-customer-key",
                    "MDEyMzQ1Njc4OTAxMjM0NTY3ODkwMTIzNDU2Nzg5MDE=",
                ),
                (
                    "x-amz-server-side-encryption-customer-key-MD5",
                    "rL0Y20zC+Fzt72VPzMSk2A==",
                ),
            ],
            body: select_request(
                "SELECT * FROM S3Object",
                "SQL",
                "<CSV></CSV>",
                "<CSV></CSV>",
            ),
        },
        Row {
            case: "a request naming the bucket's own expected owner",
            key: "data/owned.csv",
            stored: true,
            query: "select&select-type=2",
            headers: vec![(
                "x-amz-expected-bucket-owner",
                // `emulator_account_id` is this emulator's one account, borrowed for the
                // program's lifetime, so it sits in a table of `&'static str` headers.
                engine::emulator_account_id(),
            )],
            body: select_request(
                "SELECT * FROM S3Object",
                "SQL",
                "<CSV></CSV>",
                "<CSV></CSV>",
            ),
        },
        Row {
            case: "the subresource flag with the two query parameters reversed",
            key: "data/reversed.csv",
            stored: true,
            query: "select-type=2&select",
            headers: Vec::new(),
            body: select_request(
                "SELECT * FROM S3Object",
                "SQL",
                "<CSV></CSV>",
                "<CSV></CSV>",
            ),
        },
        Row {
            case: "the subresource flag alone, with no select-type beside it",
            key: "data/untyped.csv",
            stored: true,
            query: "select",
            headers: Vec::new(),
            body: select_request(
                "SELECT * FROM S3Object",
                "SQL",
                "<CSV></CSV>",
                "<CSV></CSV>",
            ),
        },
        Row {
            case: "a select-type generation this model does not bind",
            key: "data/other-generation.csv",
            stored: true,
            query: "select&select-type=1",
            headers: Vec::new(),
            body: select_request(
                "SELECT * FROM S3Object",
                "SQL",
                "<CSV></CSV>",
                "<CSV></CSV>",
            ),
        },
        Row {
            case: "a key at the bucket root",
            key: "rows.csv",
            stored: true,
            query: "select&select-type=2",
            headers: Vec::new(),
            body: select_request(
                "SELECT * FROM S3Object",
                "SQL",
                "<CSV></CSV>",
                "<CSV></CSV>",
            ),
        },
        Row {
            case: "a key the request URI carries percent-encoded",
            key: "spaced%20rows.csv",
            stored: true,
            query: "select&select-type=2",
            headers: Vec::new(),
            body: select_request(
                "SELECT * FROM S3Object",
                "SQL",
                "<CSV></CSV>",
                "<CSV></CSV>",
            ),
        },
        Row {
            case: "a key spelled like the subresource itself",
            key: "select",
            stored: true,
            query: "select&select-type=2",
            headers: Vec::new(),
            body: select_request(
                "SELECT * FROM S3Object",
                "SQL",
                "<CSV></CSV>",
                "<CSV></CSV>",
            ),
        },
        Row {
            case: "a key the bucket does not hold",
            key: "data/never-written.csv",
            stored: false,
            query: "select&select-type=2",
            headers: Vec::new(),
            body: select_request(
                "SELECT * FROM S3Object",
                "SQL",
                "<CSV></CSV>",
                "<CSV></CSV>",
            ),
        },
        Row {
            case: "a request carrying no document at all",
            key: "data/bodyless.csv",
            stored: true,
            query: "select&select-type=2",
            headers: Vec::new(),
            body: Vec::new(),
        },
    ]
}

/// Assert that `answer` is the published Select refusal, naming `case` and the actual
/// values on every failure.
fn assert_is_the_select_refusal(case: &str, answer: &Answer) {
    let code = answer.error_code();
    let message = answer.error_message();
    assert_eq!(
        answer.status, METHOD_NOT_ALLOWED_STATUS,
        "case {case:?}: expected HTTP {METHOD_NOT_ALLOWED_STATUS}, got {} with body {:?}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    assert_eq!(
        code, METHOD_NOT_ALLOWED_CODE,
        "case {case:?}: expected the error code {METHOD_NOT_ALLOWED_CODE:?}, got {code:?} with \
         body {:?}",
        String::from_utf8_lossy(&answer.body)
    );
    assert_eq!(
        message, METHOD_NOT_ALLOWED_MESSAGE,
        "case {case:?}: expected the message {METHOD_NOT_ALLOWED_MESSAGE:?}, got {message:?}"
    );
}

#[tokio::test]
async fn a_select_request_is_refused_as_a_method_the_resource_does_not_allow() {
    let router = router();
    let name = "object-select";
    bucket(&router, name).await;

    let rows = rows();

    // Every row that names a stored object is written first, so the Select request meets a
    // real object of the shape its expression is written for.
    for row in &rows {
        if !row.stored {
            continue;
        }
        let stored = put_object(
            &router,
            name,
            row.key,
            ROWS_CSV,
            &[("content-type", "text/csv")],
        )
        .await;
        assert_eq!(
            stored.status,
            200,
            "case {:?}: the object must be stored first, got {} with body {:?}",
            row.case,
            stored.status,
            String::from_utf8_lossy(&stored.body)
        );
    }

    for row in &rows {
        let answer =
            select_object_content(&router, name, row.key, row.query, &row.headers, &row.body).await;
        assert_is_the_select_refusal(row.case, &answer);

        // The refusal carries nothing from the request: not the key it named and not a word
        // of the expression it asked for.
        for fragment in [row.key, "S3Object", "SELECT"] {
            assert!(
                !answer.error_message().contains(fragment),
                "case {:?}: the message must carry nothing from the request, but {fragment:?} \
                 is in {:?}",
                row.case,
                answer.error_message()
            );
        }

        // Asking twice answers the same way: the call refused, it did not consume or enable
        // anything.
        let again =
            select_object_content(&router, name, row.key, row.query, &row.headers, &row.body).await;
        assert_eq!(
            again.status, answer.status,
            "case {:?}: a second Select request must answer the same status, got {} then {}",
            row.case, answer.status, again.status
        );
        assert_eq!(
            again.error_code(),
            answer.error_code(),
            "case {:?}: a second Select request must answer the same code, got {:?} then {:?}",
            row.case,
            answer.error_code(),
            again.error_code()
        );
    }

    // The wire contract spelled out, independent of the constants the table above reads:
    // the status, code and message AWS's published `ErrorCodeList` carries for this
    // refusal.
    let literal = select_object_content(
        &router,
        name,
        "data/rows.csv",
        "select&select-type=2",
        &[],
        &select_request(
            "SELECT s.id, s.name, s.category FROM S3Object s WHERE s.id = '2'",
            "SQL",
            "<CSV><FileHeaderInfo>USE</FileHeaderInfo></CSV>",
            "<CSV></CSV>",
        ),
    )
    .await;
    assert_eq!(
        literal.status,
        405,
        "the published status for this refusal is 405, got {} with body {:?}",
        literal.status,
        String::from_utf8_lossy(&literal.body)
    );
    assert_eq!(
        literal.error_code(),
        "MethodNotAllowed",
        "the published code for this refusal is \"MethodNotAllowed\", got {:?}",
        literal.error_code()
    );
    assert_eq!(
        literal.error_message(),
        "The specified method is not allowed against this resource.",
        "the published message for this refusal is the one its list entry carries, got {:?}",
        literal.error_message()
    );

    // The refusals reported nothing and changed nothing: each stored object reads back byte
    // for byte, and the key no request wrote is still absent.
    for row in &rows {
        let read = get_object(&router, name, row.key, "", &[]).await;
        if row.stored {
            assert_eq!(
                read.status,
                200,
                "case {:?}: the object must still be readable, got {} with body {:?}",
                row.case,
                read.status,
                String::from_utf8_lossy(&read.body)
            );
            assert_eq!(
                read.body.as_ref(),
                ROWS_CSV,
                "case {:?}: the stored bytes must be unchanged, got {:?}",
                row.case,
                String::from_utf8_lossy(&read.body)
            );
        } else {
            let code = read.error_code();
            assert_eq!(
                code, NO_SUCH_KEY_CODE,
                "case {:?}: the key must still be absent, got {code:?} with status {}",
                row.case, read.status
            );
        }
    }
}

#[tokio::test]
async fn a_select_request_is_refused_whatever_it_addresses() {
    // The withdrawal is account-wide, so the Select refusal precedes the address: a bucket
    // this emulator does not hold is answered the same way, and so is every key form under
    // it.
    let router = router();
    let document = select_request(
        "SELECT * FROM S3Object",
        "SQL",
        "<CSV></CSV>",
        "<CSV></CSV>",
    );
    for (case, named, key) in [
        (
            "a bucket that was never created",
            "object-select-absent-bucket",
            "data/rows.csv",
        ),
        (
            "a key at the root of a bucket that was never created",
            "object-select-absent-bucket",
            "rows.csv",
        ),
        (
            "a percent-encoded key of a bucket that was never created",
            "object-select-absent-bucket",
            "spaced%20rows.csv",
        ),
    ] {
        let answer =
            select_object_content(&router, named, key, "select&select-type=2", &[], &document).await;
        assert_is_the_select_refusal(case, &answer);
    }
}

#[tokio::test]
async fn the_select_refusal_is_scoped_to_the_select_subresource() {
    // The seam that answers Select claims only the `select` subresource OF AN OBJECT.
    // A `select` flag that addresses no object — a bucket path, or the service root —
    // names no Select request, so it must keep the framework's own routing refusal rather
    // than being reworded as a method this resource does not allow.
    let router = router();
    let name = "object-select-scope";
    bucket(&router, name).await;
    let document = select_request(
        "SELECT * FROM S3Object",
        "SQL",
        "<CSV></CSV>",
        "<CSV></CSV>",
    );

    for (case, uri) in [
        ("the select flag on a bucket path", "/object-select-scope?select&select-type=2"),
        ("the select flag on the service root", "/?select&select-type=2"),
    ] {
        let answer = send_bytes(
            &router,
            "POST",
            uri,
            &[("content-type", "application/xml")],
            &document,
        )
        .await;
        assert_ne!(
            answer.status, METHOD_NOT_ALLOWED_STATUS,
            "case {case:?}: this addresses no object, so it must not answer the Select \
             refusal, got {} with body {:?}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_ne!(
            answer.error_code(),
            METHOD_NOT_ALLOWED_CODE,
            "case {case:?}: this addresses no object, so it must not answer the Select \
             refusal's code, got {:?}",
            answer.error_code()
        );
    }
}
