// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `PutSinkPolicy` and `GetSinkPolicy`.
//!
//! The rule under test: a sink policy is a resource-based policy document held on ONE
//! stored sink's record. `PutSinkPolicy` accepts a document that is a JSON object whose
//! `Statement` is a statement object or a non-empty array of them, each statement
//! declaring an `Effect` of `Allow` or `Deny`, a `Principal` (or `NotPrincipal`) and an
//! `Action` (or `NotAction`); it stores that document verbatim on the sink the
//! identifier resolves to, REPLACING whatever was there, and reports it back beside
//! that sink's ARN and id. Anything else is refused with the modelled
//! `InvalidParameterException` and changes nothing. `GetSinkPolicy` reports exactly the
//! stored document, and a sink that never had one — or an identifier naming no stored
//! sink at all — is the modelled `ResourceNotFoundException`.
//!
//! Every case drives the real wire path (the production `MultiServiceRouter`, restJson1
//! serialization, the modelled-constraint walk and the handlers), so what is asserted is
//! what a signed AWS SDK request observes.

use serde_json::{json, Value};
use service_com_amazonaws_oam_oamservice::state::{
    MAX_SINK_POLICY_BYTES, POLICY_VALIDATION_FAILED, RESOURCE_NOT_FOUND_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use super::sinks_and_links::{call, create_sink_ok, emulator, error_code, field, ACCOUNT};

/// The region most cases sign for.
const REGION: &str = "us-east-1";

/// A second region, so a case can ask whether a sink's policy is visible from an
/// endpoint that holds no such sink.
const OTHER_REGION: &str = "eu-west-1";

/// The documents `PutSinkPolicy` must ACCEPT, as `(case, document)`.
///
/// The rows differ along everything the rule generalises over rather than only in their
/// text: the form of `Statement` (one object, a one-entry array, a three-entry array),
/// the form of `Principal` (a wildcard string, an object keyed by principal type with a
/// single account and with a list of them, `NotPrincipal` instead), the form of `Action`
/// (one string, a list, `NotAction` instead), both `Effect` values, and the presence or
/// absence of the optional `Version`, `Sid`, `Resource` and `Condition` elements. The
/// `oam:CreateLink` / `oam:UpdateLink` actions and the `AWS::*::*` resource types are
/// the ones `PutSinkPolicy`'s own documentation names.
fn accepted_policies(sink_arn: &str) -> Vec<(&'static str, Value)> {
    vec![
        (
            "wildcard principal, single statement object",
            json!({
                "Version": "2012-10-17",
                "Statement": {
                    "Effect": "Allow",
                    "Principal": "*",
                    "Action": "oam:CreateLink",
                    "Resource": "*",
                },
            }),
        ),
        (
            "one account principal, statement array of one, no Version",
            json!({
                "Statement": [{
                    "Sid": "SharingOneAccount",
                    "Effect": "Allow",
                    "Principal": { "AWS": "111111111111" },
                    "Action": ["oam:CreateLink", "oam:UpdateLink"],
                    "Resource": sink_arn,
                }],
            }),
        ),
        (
            "principal list, condition on resource types",
            json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": { "AWS": ["111111111111", "222222222222"] },
                    "Action": ["oam:CreateLink", "oam:UpdateLink"],
                    "Resource": "*",
                    "Condition": {
                        "ForAllValues:StringEquals": {
                            "oam:ResourceTypes": [
                                "AWS::CloudWatch::Metric",
                                "AWS::Logs::LogGroup",
                            ],
                        },
                    },
                }],
            }),
        ),
        (
            "deny effect",
            json!({
                "Statement": [{
                    "Effect": "Deny",
                    "Principal": { "AWS": "333333333333" },
                    "Action": "oam:CreateLink",
                }],
            }),
        ),
        (
            "negated principal and action",
            json!({
                "Statement": [{
                    "Effect": "Deny",
                    "NotPrincipal": { "AWS": "444444444444" },
                    "NotAction": ["oam:UpdateLink"],
                    "Resource": "*",
                }],
            }),
        ),
        (
            "three statements, mixed effects",
            json!({
                "Version": "2012-10-17",
                "Statement": [
                    {
                        "Sid": "One",
                        "Effect": "Allow",
                        "Principal": { "AWS": "111111111111" },
                        "Action": "oam:CreateLink",
                    },
                    {
                        "Sid": "Two",
                        "Effect": "Deny",
                        "Principal": "*",
                        "Action": ["oam:UpdateLink"],
                        "Resource": "*",
                    },
                    {
                        "Effect": "Allow",
                        "Principal": { "Service": "cloudwatch.amazonaws.com" },
                        "Action": "oam:CreateLink",
                    },
                ],
            }),
        ),
    ]
}

/// The documents `PutSinkPolicy` must REFUSE, as `(case, document)`.
///
/// `Policy` is a plain string on the wire (`SinkPolicy` carries no modelled
/// constraints), so each row is sent as the exact text a caller would send — including
/// text that is not JSON at all, which `json!` could not express.
fn refused_policies() -> Vec<(&'static str, String)> {
    vec![
        ("not JSON at all", "{not valid json".to_string()),
        ("empty document", String::new()),
        ("truncated JSON", "{\"Statement\":".to_string()),
        ("a JSON array, not an object", "[]".to_string()),
        ("a JSON string, not an object", "\"a policy\"".to_string()),
        ("a JSON number, not an object", "7".to_string()),
        ("a JSON null, not an object", "null".to_string()),
        (
            "an object with no Statement",
            json!({ "Version": "2012-10-17" }).to_string(),
        ),
        (
            "an empty Statement array",
            json!({ "Version": "2012-10-17", "Statement": [] }).to_string(),
        ),
        (
            "a Statement that is a string",
            json!({ "Statement": "Allow everyone" }).to_string(),
        ),
        (
            "a statement that is not an object",
            json!({ "Statement": ["Allow"] }).to_string(),
        ),
        (
            "a statement with no Principal",
            json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Action": ["oam:CreateLink", "oam:UpdateLink"],
                    "Resource": "*",
                }],
            })
            .to_string(),
        ),
        (
            "a statement whose Principal names nobody",
            json!({
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": {},
                    "Action": "oam:CreateLink",
                }],
            })
            .to_string(),
        ),
        (
            "a statement whose Principal is blank",
            json!({
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": "   ",
                    "Action": "oam:CreateLink",
                }],
            })
            .to_string(),
        ),
        (
            "a statement with no Action",
            json!({
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": "*",
                    "Resource": "*",
                }],
            })
            .to_string(),
        ),
        (
            "a statement whose Action list is empty",
            json!({
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": "*",
                    "Action": [],
                }],
            })
            .to_string(),
        ),
        (
            "a statement with no Effect",
            json!({
                "Statement": [{
                    "Principal": "*",
                    "Action": "oam:CreateLink",
                }],
            })
            .to_string(),
        ),
        (
            "a statement whose Effect is not Allow or Deny",
            json!({
                "Statement": [{
                    "Effect": "Permit",
                    "Principal": "*",
                    "Action": "oam:CreateLink",
                }],
            })
            .to_string(),
        ),
        (
            "a statement whose Effect is miscased",
            json!({
                "Statement": [{
                    "Effect": "allow",
                    "Principal": "*",
                    "Action": "oam:CreateLink",
                }],
            })
            .to_string(),
        ),
        (
            "one bad statement among good ones",
            json!({
                "Statement": [
                    {
                        "Effect": "Allow",
                        "Principal": "*",
                        "Action": "oam:CreateLink",
                    },
                    {
                        "Effect": "Allow",
                        "Action": "oam:UpdateLink",
                    },
                ],
            })
            .to_string(),
        ),
    ]
}

/// A valid policy document rendered to exactly `bytes` bytes, by padding the `Sid` of
/// its one statement. Used to probe the stored-size bound on each side of it.
fn policy_of_size(bytes: usize) -> String {
    let document = |sid: &str| {
        json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Sid": sid,
                "Effect": "Allow",
                "Principal": "*",
                "Action": "oam:CreateLink",
                "Resource": "*",
            }],
        })
        .to_string()
    };
    let base = document("");
    let padding = bytes
        .checked_sub(base.len())
        .unwrap_or_else(|| panic!("{bytes} bytes is smaller than the {} byte base", base.len()));
    // The padding is ASCII and needs no JSON escaping, so each padding character adds
    // exactly one byte to the rendered document.
    let padded = document(&"A".repeat(padding));
    assert_eq!(
        padded.len(),
        bytes,
        "the padded document should render to exactly {bytes} bytes"
    );
    padded
}

/// `PutSinkPolicy` with `policy` sent as raw text, which is how a caller sends it.
async fn put_policy(
    router: &MultiServiceRouter,
    sink_identifier: &str,
    policy: &str,
    region: &str,
) -> (u16, Value) {
    call(
        router,
        "/PutSinkPolicy",
        &json!({ "SinkIdentifier": sink_identifier, "Policy": policy }),
        region,
    )
    .await
}

/// `GetSinkPolicy` for `sink_identifier`.
async fn get_policy(
    router: &MultiServiceRouter,
    sink_identifier: &str,
    region: &str,
) -> (u16, Value) {
    call(
        router,
        "/GetSinkPolicy",
        &json!({ "SinkIdentifier": sink_identifier }),
        region,
    )
    .await
}

/// Assert `reply` is the modelled 404 with this service's constant not-found message.
fn assert_not_found(case: &str, status: u16, reply: &Value) {
    assert_eq!(status, 404, "case {case:?}: expected 404, got {status} {reply:?}");
    assert_eq!(
        error_code(reply),
        "ResourceNotFoundException",
        "case {case:?}: expected ResourceNotFoundException, got {reply:?}"
    );
    // `ResourceNotFoundException`'s message member is modelled as `Message`
    // (`smithy_query(query="shape",shape="ResourceNotFoundException")`).
    assert_eq!(
        reply.get("Message").and_then(Value::as_str),
        Some(RESOURCE_NOT_FOUND_MESSAGE),
        "case {case:?}: expected the constant not-found message, got {reply:?}"
    );
}

/// Assert `reply` is the modelled 400 a refused policy document carries.
fn assert_policy_refused(case: &str, status: u16, reply: &Value) {
    assert_eq!(status, 400, "case {case:?}: expected 400, got {status} {reply:?}");
    assert_eq!(
        error_code(reply),
        "InvalidParameterException",
        "case {case:?}: expected InvalidParameterException, got {reply:?}"
    );
    // `InvalidParameterException`'s message member is modelled lowercase, as
    // `message` (`smithy_query(query="shape",shape="InvalidParameterException")`).
    assert_eq!(
        reply.get("message").and_then(Value::as_str),
        Some(POLICY_VALIDATION_FAILED),
        "case {case:?}: expected the constant validation message, got {reply:?}"
    );
}

#[tokio::test]
async fn every_accepted_policy_round_trips_verbatim_and_the_last_put_wins() {
    let router = emulator();
    let sink_arn = create_sink_ok(&router, "RoundTripSink", REGION).await;
    let sink_id = sink_arn
        .rsplit('/')
        .next()
        .expect("a sink ARN ends in its id")
        .to_string();

    for (case, document) in accepted_policies(&sink_arn) {
        // Sent as the text a caller sends, and compared against that same text: the
        // service reports "the policy that you specified".
        let sent = document.to_string();
        let (status, put) = put_policy(&router, &sink_arn, &sent, REGION).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from PutSinkPolicy, got {status} {put:?}"
        );
        assert_eq!(
            field(case, &put, "Policy"),
            sent,
            "case {case:?}: PutSinkPolicy should report the document it was given"
        );
        assert_eq!(
            field(case, &put, "SinkArn"),
            sink_arn,
            "case {case:?}: PutSinkPolicy should report the stored sink's ARN"
        );
        assert_eq!(
            field(case, &put, "SinkId"),
            sink_id,
            "case {case:?}: PutSinkPolicy should report the stored sink's id"
        );

        // Read back through the implemented reader: the stored document, byte for
        // byte, beside the same ARN and id.
        let (status, got) = get_policy(&router, &sink_arn, REGION).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetSinkPolicy, got {status} {got:?}"
        );
        assert_eq!(
            field(case, &got, "Policy"),
            sent,
            "case {case:?}: GetSinkPolicy should report the stored document unchanged"
        );
        assert_eq!(
            field(case, &got, "SinkArn"),
            sink_arn,
            "case {case:?}: GetSinkPolicy should report the stored sink's ARN"
        );
        assert_eq!(
            field(case, &got, "SinkId"),
            sink_id,
            "case {case:?}: GetSinkPolicy should report the stored sink's id"
        );
    }

    // Every row above was put onto the same sink, so the loop also asserts the
    // replacement rule: each put replaced the previous document entirely rather than
    // merging with it, and the document the LAST row stored is the one still there.
    let (_, last) = accepted_policies(&sink_arn)
        .pop()
        .expect("the accepted table is not empty");
    let (status, got) = get_policy(&router, &sink_arn, REGION).await;
    assert_eq!(status, 200, "after the table: expected 200, got {status} {got:?}");
    assert_eq!(
        field("last put wins", &got, "Policy"),
        last.to_string(),
        "after the table: the last document put should be the stored one"
    );
}

#[tokio::test]
async fn a_sink_policy_is_readable_by_every_identifier_form_that_names_the_sink() {
    let router = emulator();
    let sink_arn = create_sink_ok(&router, "IdentifierFormsSink", REGION).await;
    let sink_id = sink_arn
        .rsplit('/')
        .next()
        .expect("a sink ARN ends in its id")
        .to_string();
    let document = json!({
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "AWS": "111111111111" },
            "Action": "oam:CreateLink",
        }],
    })
    .to_string();

    // The modelled `ResourceIdentifier` admits the ARN, the bare id and the sink's
    // name, and all three name the one stored sink — so a policy put through any of
    // them is readable through all of them.
    let forms: [(&str, &str); 3] = [
        ("full ARN", sink_arn.as_str()),
        ("bare id", sink_id.as_str()),
        ("sink name", "IdentifierFormsSink"),
    ];
    for (put_case, put_form) in forms {
        let (status, put) = put_policy(&router, put_form, &document, REGION).await;
        assert_eq!(
            status, 200,
            "case {put_case:?}: expected 200 from PutSinkPolicy, got {status} {put:?}"
        );
        assert_eq!(
            field(put_case, &put, "SinkArn"),
            sink_arn,
            "case {put_case:?}: every identifier form should resolve to the one sink"
        );
        for (get_case, get_form) in forms {
            let case = format!("put by {put_case}, read by {get_case}");
            let (status, got) = get_policy(&router, get_form, REGION).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200 from GetSinkPolicy, got {status} {got:?}"
            );
            assert_eq!(
                field(&case, &got, "Policy"),
                document,
                "case {case:?}: every identifier form should read the one stored policy"
            );
        }
    }
}

#[tokio::test]
async fn a_sink_that_was_never_given_a_policy_has_none_to_report() {
    let router = emulator();
    let sink_arn = create_sink_ok(&router, "FreshSink", REGION).await;
    let sink_id = sink_arn
        .rsplit('/')
        .next()
        .expect("a sink ARN ends in its id")
        .to_string();

    // A policy exists only once one has been put, whichever form the reader names the
    // sink by.
    for (case, identifier) in [
        ("fresh sink by ARN", sink_arn.as_str()),
        ("fresh sink by id", sink_id.as_str()),
        ("fresh sink by name", "FreshSink"),
    ] {
        let (status, got) = get_policy(&router, identifier, REGION).await;
        assert_not_found(case, status, &got);
    }
}

#[tokio::test]
async fn an_identifier_naming_no_stored_sink_is_refused_by_both_operations() {
    let router = emulator();
    let sink_arn = create_sink_ok(&router, "RealSink", REGION).await;
    let document = json!({
        "Statement": [{
            "Effect": "Allow",
            "Principal": "*",
            "Action": "oam:CreateLink",
        }],
    })
    .to_string();
    let (status, put) = put_policy(&router, &sink_arn, &document, REGION).await;
    assert_eq!(status, 200, "seeding the policy: got {status} {put:?}");

    // Each row is an identifier the modelled pattern admits that names no sink stored
    // in the Region the request is signed for.
    let absent: Vec<(&str, String)> = vec![
        ("another sink's name", "NoSuchSink".to_string()),
        (
            "an id no sink carries",
            "11111111-2222-3333-4444-555555555555".to_string(),
        ),
        (
            "an ARN in another account",
            format!("arn:aws:oam:{REGION}:999999999999:sink/{}", "abc"),
        ),
        (
            "an ARN of another service",
            format!("arn:aws:logs:{REGION}:{ACCOUNT}:sink/{}", "abc"),
        ),
        (
            "an ARN of another resource kind",
            sink_arn.replace("sink/", "link/"),
        ),
    ];
    for (case, identifier) in &absent {
        let (status, got) = get_policy(&router, identifier, REGION).await;
        assert_not_found(&format!("GetSinkPolicy {case}"), status, &got);
        let (status, put) = put_policy(&router, identifier, &document, REGION).await;
        assert_not_found(&format!("PutSinkPolicy {case}"), status, &put);
    }

    // The sink exists, but not at the endpoint the request is signed for: a regional
    // endpoint sees only its own Region's sinks.
    let (status, got) = get_policy(&router, &sink_arn, OTHER_REGION).await;
    assert_not_found("another Region's endpoint", status, &got);
    let (status, put) = put_policy(&router, &sink_arn, &document, OTHER_REGION).await;
    assert_not_found("PutSinkPolicy at another Region's endpoint", status, &put);

    // The policy lives on the sink, so deleting the sink takes it with it.
    let (status, deleted) = call(
        &router,
        "/DeleteSink",
        &json!({ "Identifier": sink_arn }),
        REGION,
    )
    .await;
    assert_eq!(status, 200, "DeleteSink: got {status} {deleted:?}");
    let (status, got) = get_policy(&router, &sink_arn, REGION).await;
    assert_not_found("deleted sink", status, &got);
    let (status, put) = put_policy(&router, &sink_arn, &document, REGION).await;
    assert_not_found("PutSinkPolicy on a deleted sink", status, &put);
}

#[tokio::test]
async fn a_document_that_is_not_a_sink_policy_is_refused_and_stores_nothing() {
    let router = emulator();
    let sink_arn = create_sink_ok(&router, "RefusalSink", REGION).await;

    // Before anything is stored, a refused put must leave the sink with no policy at
    // all rather than a half-stored one.
    for (case, document) in refused_policies() {
        let (status, refused) = put_policy(&router, &sink_arn, &document, REGION).await;
        assert_policy_refused(case, status, &refused);
        let (status, got) = get_policy(&router, &sink_arn, REGION).await;
        assert_not_found(&format!("after refusing {case}"), status, &got);
    }

    // With a policy already attached, a refused put must leave THAT document exactly
    // as it was: the document is judged before anything is written.
    let stored = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "Keep",
            "Effect": "Allow",
            "Principal": { "AWS": "111111111111" },
            "Action": ["oam:CreateLink", "oam:UpdateLink"],
            "Resource": "*",
        }],
    })
    .to_string();
    let (status, put) = put_policy(&router, &sink_arn, &stored, REGION).await;
    assert_eq!(status, 200, "seeding the policy: got {status} {put:?}");
    for (case, document) in refused_policies() {
        let (status, refused) = put_policy(&router, &sink_arn, &document, REGION).await;
        assert_policy_refused(case, status, &refused);
        let (status, got) = get_policy(&router, &sink_arn, REGION).await;
        assert_eq!(
            status, 200,
            "after refusing {case}: expected the stored policy, got {status} {got:?}"
        );
        assert_eq!(
            field(case, &got, "Policy"),
            stored,
            "after refusing {case}: the stored document should be untouched"
        );
    }
}

#[tokio::test]
async fn a_policy_document_past_the_stored_size_bound_is_refused() {
    let router = emulator();
    let sink_arn = create_sink_ok(&router, "SizeBoundSink", REGION).await;

    // Each side of the bound, and well inside it: the bound itself is admitted, one
    // byte past it is not.
    let at_bound = policy_of_size(MAX_SINK_POLICY_BYTES);
    for (case, document) in [
        ("well inside the bound", policy_of_size(1_024)),
        ("exactly at the bound", at_bound.clone()),
    ] {
        let (status, put) = put_policy(&router, &sink_arn, &document, REGION).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {put:?}"
        );
        assert_eq!(
            field(case, &put, "Policy"),
            document,
            "case {case:?}: the document should be stored verbatim"
        );
    }

    for (case, document) in [
        ("one byte past the bound", policy_of_size(MAX_SINK_POLICY_BYTES + 1)),
        ("far past the bound", policy_of_size(MAX_SINK_POLICY_BYTES * 2)),
    ] {
        let (status, refused) = put_policy(&router, &sink_arn, &document, REGION).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "InvalidParameterException",
            "case {case:?}: expected InvalidParameterException, got {refused:?}"
        );
        // Nothing past the bound is stored, so the document admitted at the bound is
        // still the one attached.
        let (status, got) = get_policy(&router, &sink_arn, REGION).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the previous policy, got {status} {got:?}"
        );
        assert_eq!(
            field(case, &got, "Policy"),
            at_bound,
            "case {case:?}: an oversized document should not replace the stored one"
        );
    }
}
