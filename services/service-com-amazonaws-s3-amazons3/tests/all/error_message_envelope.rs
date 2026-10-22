// SPDX-License-Identifier: CC0-1.0

//! S3's XML error document always carries a `<Message>` beside its `<Code>`.
//!
//! The rule under test: an error response from this service carries a non-empty
//! `Error/Message`, and for a MODELED error — every one of whose shapes declares no
//! `message` member of its own — that message is the error code's documented text
//! (`crate::scaffolded::DOCUMENTED_ERROR_MESSAGES`). Every SDK error parser reads
//! `Error/Message`: botocore surfaces `None` (or the literal `"Unknown"`) when it is
//! absent, so a code-only document loses the whole explanation of the failure.
//!
//! Two suites:
//!
//! * [`every_refused_request_carries_its_documented_error_message`] drives real requests
//!   through the router and reads `Code` and `Message` back out of the XML, spanning
//!   four error codes on six operations plus both unmodeled channels (the documented
//!   code [`crate::scaffolded::Scaffolded::unmodeled`] carries, and the scaffold's 501).
//! * [`the_documented_message_catalog_matches_the_model`] re-derives the catalog's key
//!   set from this service's Smithy model, so neither a stale row nor a newly modeled
//!   error code can go unnoticed.

use quick_xml::events::Event;
use quick_xml::Reader;

use service_com_amazonaws_s3_amazons3::scaffolded::{
    documented_error_message, DOCUMENTED_ERROR_MESSAGES,
};

use super::bucket_lifecycle::{create_bucket, router, send, Answer};
use super::bucket_tagging::unescaped;
use super::object_and_tagging_roundtrip::{bucket, get_object, put_object};

/// The text of the `<Message>` element in a response body, or `None` when the document
/// carries none.
///
/// Read with the XML reader rather than by slicing the body, so an escaped character in
/// the text (the apostrophe in "isn't", an ampersand) is compared unescaped, the way an
/// SDK's parser hands it to a client. The reader breaks text at each entity reference and
/// emits the reference on its own, so both fragment events are accumulated — exactly as
/// [`super::bucket_tagging`]'s tag-set reader does.
fn error_message(answer: &Answer) -> Option<String> {
    let body = String::from_utf8_lossy(&answer.body).to_string();
    let mut reader = Reader::from_str(&body);
    let mut in_message = false;
    let mut text = String::new();
    loop {
        match reader
            .read_event()
            .unwrap_or_else(|error| panic!("the response body must be well-formed XML: {error}"))
        {
            Event::Start(start) => in_message = start.name().as_ref() == b"Message",
            Event::Text(chunk) if in_message => {
                let decoded = chunk
                    .xml10_content()
                    .unwrap_or_else(|error| panic!("the response body must decode: {error}"));
                text.push_str(&unescaped(&decoded));
            }
            Event::GeneralRef(reference) if in_message => {
                let name = reference
                    .decode()
                    .unwrap_or_else(|error| panic!("the response body must decode: {error}"));
                text.push_str(&unescaped(&format!("&{name};")));
            }
            Event::End(end) if end.name().as_ref() == b"Message" => return Some(text),
            Event::Eof => return None,
            _ => {}
        }
    }
}

/// One row of the refusal table: the case name, how the refusal is provoked, and the
/// status, wire code and `<Message>` it must answer with.
struct Row {
    case: &'static str,
    status: u16,
    code: &'static str,
    message: &'static str,
}

/// The documented message for `code`, or a panic naming the code — a row that names a
/// code the catalog does not hold is a bug in the row, not a test failure to report as a
/// missing message.
fn documented(code: &str) -> &'static str {
    documented_error_message(code)
        .unwrap_or_else(|| panic!("the catalog must document a message for {code:?}"))
}

#[tokio::test]
async fn every_refused_request_carries_its_documented_error_message() {
    let router = router();

    // The bucket and object the "it exists, the key does not" rows need.
    bucket(&router, "message-bucket").await;
    put_object(&router, "message-bucket", "present.txt", b"stored", &[]).await;

    // A bucket outside us-east-1, so a second create of it is refused rather than
    // answering the documented legacy 200.
    let created = create_bucket(&router, "message-recreate", Some("us-west-2"), None).await;
    assert_eq!(
        created.status, 200,
        "the first create must succeed, got {} with body {:?}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );

    // Each answer is paired with the row that describes it. The rows differ along what
    // the rule generalises over: the error CODE (four modeled ones, plus the two
    // channels whose message the handler words), the operation that answers it (create,
    // list, read, write, delete), and whether the failure is a missing container, a
    // missing member of one, a name already taken or a header that does not hold.
    let answers: [(Row, Answer); 8] = [
        (
            Row {
                case: "a duplicate CreateBucket by the owner",
                status: 409,
                code: "BucketAlreadyOwnedByYou",
                message: documented("BucketAlreadyOwnedByYou"),
            },
            create_bucket(&router, "message-recreate", Some("us-west-2"), None).await,
        ),
        (
            Row {
                case: "ListObjectsV2 on a bucket that does not exist",
                status: 404,
                code: "NoSuchBucket",
                message: documented("NoSuchBucket"),
            },
            send(&router, "GET", "/message-absent?list-type=2", &[], "").await,
        ),
        (
            Row {
                case: "DeleteBucket on a bucket that does not exist",
                status: 404,
                code: "NoSuchBucket",
                message: documented("NoSuchBucket"),
            },
            send(&router, "DELETE", "/message-absent", &[], "").await,
        ),
        (
            Row {
                case: "PutObject into a bucket that does not exist",
                status: 404,
                code: "NoSuchBucket",
                message: documented("NoSuchBucket"),
            },
            put_object(&router, "message-absent", "k.txt", b"payload", &[]).await,
        ),
        (
            Row {
                case: "GetObject of a key the bucket does not hold",
                status: 404,
                code: "NoSuchKey",
                message: documented("NoSuchKey"),
            },
            get_object(&router, "message-bucket", "absent.txt", "", &[]).await,
        ),
        (
            Row {
                case: "PutObject whose Content-MD5 does not match its payload",
                status: 400,
                code: "InvalidRequest",
                message: documented("InvalidRequest"),
            },
            put_object(
                &router,
                "message-bucket",
                "digest.txt",
                b"payload",
                // Base64 of some other payload's digest — a well-formed header whose
                // value cannot be this body's.
                &[("content-md5", "rL0Y20zC+Fzt72VPzMSk2A==")],
            )
            .await,
        ),
        // The two channels whose message the handler itself words are serialized through
        // the same wrapper, so they are pinned here too: the fix must not drop the text
        // they already carried.
        (
            Row {
                case: "GetBucketPolicy on a bucket with no policy (an unmodeled code)",
                status: 404,
                code: "NoSuchBucketPolicy",
                message: "",
            },
            send(&router, "GET", "/message-bucket?policy", &[], "").await,
        ),
        (
            Row {
                case: "a scaffolded operation's 501",
                status: 501,
                code: "NotImplemented",
                message: "",
            },
            send(&router, "GET", "/message-bucket?requestPayment", &[], "").await,
        ),
    ];

    for (row, answer) in &answers {
        let Row {
            case,
            status,
            code,
            message,
        } = row;
        assert_eq!(
            answer.status, *status,
            "case {case:?}: expected HTTP {status}, got {} with body {:?}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.error_code(),
            *code,
            "case {case:?}: expected error code {code:?}, got {:?}",
            answer.error_code()
        );
        let actual = error_message(answer);
        match actual.as_deref() {
            None => panic!(
                "case {case:?}: the error document carries no <Message> at all; body {:?}",
                String::from_utf8_lossy(&answer.body)
            ),
            Some(text) if message.is_empty() => assert!(
                !text.trim().is_empty(),
                "case {case:?}: expected a non-empty <Message>, got {text:?}"
            ),
            Some(text) => assert_eq!(
                text, *message,
                "case {case:?}: expected <Message>{message}</Message>, got {text:?}"
            ),
        }
    }
}

#[tokio::test]
async fn an_unrefused_request_carries_no_error_message() {
    // The rule is about ERROR documents: a success answers no `<Message>`, so the
    // wrapper cannot be writing one unconditionally.
    let router = router();
    let created = create_bucket(&router, "message-success", Some("eu-west-2"), None).await;
    assert_eq!(
        created.status, 200,
        "the create must succeed, got {}",
        created.status
    );
    assert_eq!(
        error_message(&created),
        None,
        "a successful response must carry no <Message>, got {:?}",
        error_message(&created)
    );
}

#[test]
fn the_documented_message_catalog_matches_the_model() {
    // Every structure in this service's model that carries `smithy.api#error`, read from
    // the model itself rather than from a second hand-written list: an error code the
    // model gains must gain a documented message with it, and a row naming a shape the
    // model no longer declares is stale.
    let model_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/smithy-model/com-amazonaws-s3-amazons3.json"
    );
    let model: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(model_path).expect("the service's Smithy model must be readable"),
    )
    .expect("the Smithy model must parse as JSON");
    let shapes = model["shapes"]
        .as_object()
        .expect("the model must carry a shapes object");

    let mut modeled: Vec<(&str, bool)> = shapes
        .iter()
        .filter(|(_, shape)| shape["traits"].get("smithy.api#error").is_some())
        .map(|(id, shape)| {
            let name = id.split('#').next_back().expect("a shape id names a shape");
            let has_message = shape["members"]
                .as_object()
                .is_some_and(|members| members.keys().any(|m| m.eq_ignore_ascii_case("message")));
            (name, has_message)
        })
        .collect();
    modeled.sort_unstable();
    assert!(
        !modeled.is_empty(),
        "the model must declare at least one error shape; found none, so this test is \
         reading the wrong file"
    );

    for (name, has_message) in &modeled {
        let documented = documented_error_message(name);
        if *has_message {
            // Its own member carries the handler's text; a catalog row would be written
            // beside it.
            assert!(
                documented.is_none(),
                "case {name:?}: the shape declares its own message member, so the \
                 catalog must not document one; got {documented:?}"
            );
            continue;
        }
        let Some(text) = documented else {
            panic!(
                "case {name:?}: the model declares this error code and the shape carries \
                 no message member, so DOCUMENTED_ERROR_MESSAGES must name it"
            );
        };
        assert!(
            !text.trim().is_empty(),
            "case {name:?}: the documented message must not be blank, got {text:?}"
        );
    }

    let names: Vec<&str> = modeled.iter().map(|(name, _)| *name).collect();
    for (code, _) in DOCUMENTED_ERROR_MESSAGES {
        assert!(
            names.contains(code),
            "case {code:?}: the catalog documents a message for an error code this \
             service's model does not declare; modeled codes are {names:?}"
        );
    }

    let mut sorted = DOCUMENTED_ERROR_MESSAGES.to_vec();
    sorted.sort_unstable();
    assert_eq!(
        sorted,
        DOCUMENTED_ERROR_MESSAGES.to_vec(),
        "the catalog must stay sorted by error code so a new row is easy to place"
    );
}
