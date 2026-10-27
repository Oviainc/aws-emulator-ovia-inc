// SPDX-License-Identifier: CC0-1.0

//! How this service frames an answer that carries no body, end to end over the router.
//!
//! THE RULE under test: a response with NO body still has a LENGTH, and this service
//! states it — `content-length: 0` — rather than leaving the framing to the transport.
//! `DeleteApplication` is modelled with an empty (`Unit`) output at HTTP 204
//! (`smithy_query(query="operation", operation="DeleteApplication")`), so it is the one
//! answer of this service that carries no body; every other answer carries one and is
//! framed from the body it carries, which is what the control rows below hold it to.
//!
//! The framing is asserted on the response the router produces, which is the response this
//! service's serializer states and what the in-process front door and the SDK connector
//! both deliver. It is that serializer seam this suite pins, and only that: an answer
//! served over HTTP/1.1 reaches a client without the header, because the HTTP writer the
//! vendored transport uses removes `content-length` from a `204` (RFC 9110 §15.3.5 forbids
//! a body there), and that writer is below anything this crate decides.
//!
//! The harness is the `applications` suite's `Repo`/`Answer`, which carries the stated
//! length beside the body, so these cases drive the same wire path every other suite does.

use serde_json::{json, Value};

use super::applications::{minimal, path_label, Repo};

/// THE RULE: an answer with no body states `content-length: 0`.
///
/// The rows differ along what the rule generalises over: whether the answer has a body at
/// all, which status it carries, and whether it is a success or one of the modelled
/// failures — the 404 for an application the catalogue does not hold, and the 400 for a
/// parameter outside the contract. An answer that carries a body is the control: it is
/// framed from the body it carries, never with the empty answer's `0`. Each row states
/// which of the two it expects, so a bodyless answer that grew a body (or the reverse)
/// fails the case instead of falling into the other branch.
#[tokio::test]
async fn an_answer_with_no_body_states_its_length() {
    let repo = Repo::new();

    let created = repo.create_application(minimal("framing-app")).await;
    assert_eq!(
        created.status, 201,
        "creating: got {} with {}",
        created.status, created.body
    );
    let application_id = created
        .at("applicationId")
        .as_str()
        .expect("a minted applicationId")
        .to_string();
    let label = path_label(&application_id);

    // (case, method, path, request body, the status expected, whether the answer carries
    //  no body at all)
    #[allow(clippy::type_complexity)]
    let cases: Vec<(&str, &str, String, Option<Value>, u16, bool)> = vec![
        (
            "the delete of a published application: the modelled empty output",
            "DELETE",
            format!("/applications/{label}"),
            None,
            204,
            true,
        ),
        (
            "the delete of an application the catalogue does not hold",
            "DELETE",
            format!("/applications/{label}"),
            None,
            404,
            false,
        ),
        (
            "a read of an application the catalogue does not hold",
            "GET",
            format!("/applications/{label}"),
            None,
            404,
            false,
        ),
        (
            "a create whose name is outside the documented pattern",
            "POST",
            "/applications".to_string(),
            Some(json!({
                "name": "framing app",
                "author": "example-author",
                "description": "a demo app"
            })),
            400,
            false,
        ),
        (
            "a list, which always carries a body",
            "GET",
            "/applications".to_string(),
            None,
            200,
            false,
        ),
    ];

    for (case, method, path, request, expected, bodyless) in &cases {
        let answer = repo.call(method, path, request.clone()).await;
        assert_eq!(
            answer.status, *expected,
            "case {case:?}: expected {expected}, got {} with {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.body.is_null(),
            *bodyless,
            "case {case:?}: expected a body present={}, got {}",
            !bodyless,
            answer.body
        );
        if *bodyless {
            assert_eq!(
                answer.content_length.as_deref(),
                Some("0"),
                "case {case:?}: an answer with no body states content-length 0, got {:?}",
                answer.content_length
            );
        } else {
            // A body is framed from the body: whatever states the length, it is that
            // body's own and never the empty answer's.
            let stated_length = answer.body.to_string().len();
            assert!(
                answer
                    .content_length
                    .as_ref()
                    .is_none_or(|stated| *stated != "0" && !stated.is_empty()),
                "case {case:?}: an answer of {stated_length} bytes may not state the empty \
                 answer's length, got {:?}",
                answer.content_length
            );
        }
    }
}
