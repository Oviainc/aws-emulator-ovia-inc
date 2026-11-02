// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the account rule `CreateLink` enforces, and for what
//! follows from it on the sink it was asked about.
//!
//! The rules under test:
//!
//! * a monitoring link joins a SOURCE account to a sink in a MONITORING account
//!   ("Creates a link between a source account and a sink that you have created in a
//!   monitoring account", `CreateLink`'s own documentation), so a `SinkIdentifier`
//!   resolving to a sink the calling account itself owns is refused with the modelled
//!   `InvalidParameterException` (HTTP 400) and nothing is stored for it — whatever
//!   form the identifier takes and whatever else the request carries;
//! * because nothing was stored, the sink the refused call named still has no links:
//!   `ListAttachedLinks` reports an empty `Items`, `DeleteSink` succeeds with an empty
//!   body, and `GetSink` for the deleted sink is the modelled
//!   `ResourceNotFoundException` (HTTP 404);
//! * a `DeleteSink` on a sink that DOES still have links attached reports the rule the
//!   operation documents, in one fixed sentence that does not depend on how many links
//!   are attached.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers — so what is
//! asserted is what a signed AWS SDK request observes.

use serde_json::{json, Value};
use service_com_amazonaws_oam_oamservice::state::{
    RESOURCE_NOT_FOUND_MESSAGE, SAME_ACCOUNT_LINK_MESSAGE, SINK_HAS_LINKS_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use super::sinks_and_links::{
    attached_link_arns, call_as, create_link_ok, emulator, error_code, field, request,
    RESOURCE_TYPES,
};

/// Status, `x-amzn-errortype` header and parsed body of one call.
///
/// The error header is read off the response itself rather than folded into the body,
/// because a modelled refusal of this service binds the exception name to
/// `x-amzn-ErrorType` and that header is part of what a caller observes.
async fn call_full(
    router: &MultiServiceRouter,
    uri: &str,
    body: &Value,
    akid: &str,
    region: &str,
) -> (u16, Option<String>, Value) {
    let response = router.dispatch(request(uri, body, akid, region)).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(|value| {
            // An error type may travel namespaced (`com.amazonaws.oam#Foo`).
            value
                .rsplit('#')
                .next()
                .unwrap_or(value)
                .trim()
                .to_string()
        });
    let raw = response.into_body();
    let parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    (status, error_type, parsed)
}

/// The `message`/`Message` member of a refusal, whichever casing the answering shape
/// models it with (`InvalidParameterException` models `message`, `ConflictException`
/// and `ResourceNotFoundException` model `Message`).
fn refusal_message(case: &str, body: &Value) -> String {
    for member in ["Message", "message"] {
        if let Some(text) = body.get(member).and_then(Value::as_str) {
            return text.to_string();
        }
    }
    panic!("case {case:?}: expected a message member, got {body:?}")
}

/// Create a sink owned by `owner_akid`, which must succeed, and answer its ARN, its id
/// and its name.
async fn sink_owned_by(
    router: &MultiServiceRouter,
    name: &str,
    owner_akid: &str,
    region: &str,
) -> (String, String, String) {
    let (status, created) = call_as(
        router,
        "/CreateSink",
        &json!({ "Name": name }),
        owner_akid,
        region,
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateSink {name} as {owner_akid}: expected 200, got {status} {created:?}"
    );
    let arn = field(name, &created, "Arn");
    let id = field(name, &created, "Id");
    (arn, id, name.to_string())
}

/// How a request may name a sink: every form the modelled `ResourceIdentifier` admits
/// (`CreateLink`'s `SinkIdentifier` pattern
/// `^[a-zA-Z0-9][a-zA-Z0-9_:\.\-\/]{0,2047}$`, per
/// `smithy_query(query="operation",operation="CreateLink")`) — the full ARN, the bare
/// id, and the sink's own name.
#[derive(Debug, Clone, Copy)]
enum Form {
    Arn,
    Id,
    Name,
}

impl Form {
    /// The three forms, so a table spans all of them.
    const ALL: [Form; 3] = [Form::Arn, Form::Id, Form::Name];

    /// The identifier text of this form for the sink `sink_owned_by` answered.
    fn of(self, sink: &(String, String, String)) -> String {
        match self {
            Form::Arn => sink.0.clone(),
            Form::Id => sink.1.clone(),
            Form::Name => sink.2.clone(),
        }
    }
}

/// One `CreateLink` body shape, named, built around the identifier under test.
///
/// Rows differ along what the rule does NOT depend on: the label template, how many
/// resource types are shared, whether tags are supplied and whether the request
/// carries a `LinkConfiguration`.
fn link_bodies(identifier: &str) -> Vec<(&'static str, Value)> {
    vec![
        (
            "one resource type, no tags",
            json!({
                "LabelTemplate": "$AccountName",
                "ResourceTypes": [RESOURCE_TYPES[0]],
                "SinkIdentifier": identifier,
            }),
        ),
        (
            "every modelled resource type",
            json!({
                "LabelTemplate": "monitoring-$AccountEmail",
                "ResourceTypes": RESOURCE_TYPES,
                "SinkIdentifier": identifier,
            }),
        ),
        (
            "a custom label with tags",
            json!({
                "LabelTemplate": "team-observability",
                "ResourceTypes": [RESOURCE_TYPES[1], RESOURCE_TYPES[2]],
                "SinkIdentifier": identifier,
                "Tags": { "env": "prod" },
            }),
        ),
        (
            "a link configuration",
            json!({
                "LabelTemplate": "$AccountEmailNoDomain",
                "ResourceTypes": [RESOURCE_TYPES[1]],
                "SinkIdentifier": identifier,
                "LinkConfiguration": {
                    "LogGroupConfiguration": { "Filter": "LogGroupName LIKE 'app/%'" },
                },
            }),
        ),
    ]
}

#[tokio::test]
async fn create_link_refuses_a_sink_the_calling_account_owns() {
    // Rows differ along the account that owns the sink (so the decision is an equality
    // on the identity the request speaks for, never a fixed value), the Region the
    // sink lives in, how the request names it, and what else the request carries.
    let long_akid = format!("AKID{}", "Z".repeat(16));
    let owners: Vec<(&str, &str, &str)> = vec![
        ("a plain key id", "AKIDOAMOWNER1", "us-west-2"),
        ("a punctuated key id", "akid-oam-owner-2", "eu-west-1"),
        ("a long key id", long_akid.as_str(), "ap-southeast-2"),
    ];

    for (owner_case, owner_akid, region) in owners {
        let router = emulator();
        let sink = sink_owned_by(&router, "demo-sink", owner_akid, region).await;

        for form in Form::ALL {
            let identifier = form.of(&sink);
            for (body_case, body) in link_bodies(&identifier) {
                let case = format!("{owner_case}, {form:?} identifier, {body_case}");
                let (status, error_type, got) =
                    call_full(&router, "/CreateLink", &body, owner_akid, region).await;
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected 400, got {status} {got:?}"
                );
                assert_eq!(
                    error_code(&got),
                    "InvalidParameterException",
                    "case {case:?}: got {got:?}"
                );
                assert_eq!(
                    error_type.as_deref(),
                    Some("InvalidParameterException"),
                    "case {case:?}: expected the exception name on x-amzn-errortype, got \
                     {error_type:?}"
                );
                assert_eq!(
                    refusal_message(&case, &got),
                    SAME_ACCOUNT_LINK_MESSAGE,
                    "case {case:?}: got {got:?}"
                );
                assert_eq!(
                    got.get("Arn"),
                    None,
                    "case {case:?}: a refusal reports no link, got {got:?}"
                );

                // Nothing was stored for any of them.
                assert_eq!(
                    attached_link_arns(&router, &case, &sink.0, None, region).await,
                    Vec::<String>::new(),
                    "case {case:?}: a refused CreateLink must not have attached a link"
                );
            }
        }

        // The control: ANOTHER account's source account links to the very same sink,
        // by each identifier form, and each link is attached.
        let mut expected = Vec::new();
        for (index, form) in Form::ALL.into_iter().enumerate() {
            let identifier = form.of(&sink);
            let source_akid = format!("AKIDOAMSOURCE{index}");
            let case = format!("{owner_case}, control with a {form:?} identifier");
            let arn = create_link_ok(&router, &identifier, &source_akid, region).await;
            expected.push(arn);
            expected.sort();
            let mut attached = attached_link_arns(&router, &case, &sink.0, None, region).await;
            attached.sort();
            assert_eq!(
                attached, expected,
                "case {case:?}: a cross-account link must be attached to the sink"
            );
        }
    }
}

#[tokio::test]
async fn a_refused_same_account_link_leaves_the_sink_link_free_and_deletable() {
    // The lifecycle the rule implies, over rows differing along the Region and along
    // how the delete and the read name the sink.
    let cases: &[(&str, &str, Form)] = &[
        ("by ARN", "us-west-2", Form::Arn),
        ("by id", "eu-central-1", Form::Id),
        ("by name", "us-east-1", Form::Name),
    ];

    for (case, region, form) in cases {
        let owner_akid = "AKIDOAMLIFECYCLE";
        let router = emulator();
        let sink = sink_owned_by(&router, "lifecycle-sink", owner_akid, region).await;
        let identifier = form.of(&sink);

        let (status, _, refused) = call_full(
            &router,
            "/CreateLink",
            &json!({
                "LabelTemplate": "$AccountName",
                "ResourceTypes": [RESOURCE_TYPES[0]],
                "SinkIdentifier": identifier.clone(),
            }),
            owner_akid,
            region,
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 from the same-account CreateLink, got {status} \
             {refused:?}"
        );

        // `Items` is @required, so an unattached sink reports an EMPTY list.
        let (status, _, listed) = call_full(
            &router,
            "/ListAttachedLinks",
            &json!({ "SinkIdentifier": identifier.clone() }),
            owner_akid,
            region,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from ListAttachedLinks, got {status} {listed:?}"
        );
        assert_eq!(
            listed.get("Items").and_then(Value::as_array),
            Some(&Vec::<Value>::new()),
            "case {case:?}: expected an empty Items, got {listed:?}"
        );

        // With no link attached, the delete succeeds and models no output members.
        let (status, _, deleted) = call_full(
            &router,
            "/DeleteSink",
            &json!({ "Identifier": identifier.clone() }),
            owner_akid,
            region,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from DeleteSink, got {status} {deleted:?}"
        );
        assert!(
            deleted.is_null() || deleted == json!({}),
            "case {case:?}: DeleteSinkOutput has no members, got {deleted:?}"
        );

        // And the deleted sink is gone from every reader of it.
        for (reader, uri, body) in [
            (
                "GetSink",
                "/GetSink",
                json!({ "Identifier": identifier.clone() }),
            ),
            (
                "ListAttachedLinks",
                "/ListAttachedLinks",
                json!({ "SinkIdentifier": identifier.clone() }),
            ),
        ] {
            let case = format!("{case}, {reader} after the delete");
            let (status, error_type, got) =
                call_full(&router, uri, &body, owner_akid, region).await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected 404, got {status} {got:?}"
            );
            assert_eq!(
                error_code(&got),
                "ResourceNotFoundException",
                "case {case:?}: got {got:?}"
            );
            assert_eq!(
                error_type.as_deref(),
                Some("ResourceNotFoundException"),
                "case {case:?}: expected the exception name on x-amzn-errortype, got \
                 {error_type:?}"
            );
            assert_eq!(
                refusal_message(&case, &got),
                RESOURCE_NOT_FOUND_MESSAGE,
                "case {case:?}: got {got:?}"
            );
        }
    }
}

#[tokio::test]
async fn delete_sink_reports_the_documented_rule_while_links_remain() {
    // Rows differ along HOW MANY links are still attached, which the refusal must not
    // report: the message is the rule the operation documents and nothing else.
    let region = "us-west-2";
    let owner_akid = "AKIDOAMDELETEOWNER";
    for attached in [1usize, 2, 3] {
        let case = format!("{attached} attached link(s)");
        let router = emulator();
        let sink = sink_owned_by(&router, "delete-sink", owner_akid, region).await;
        let mut links = Vec::new();
        for index in 0..attached {
            links.push(
                create_link_ok(
                    &router,
                    &sink.0,
                    &format!("AKIDOAMDELETESOURCE{index}"),
                    region,
                )
                .await,
            );
        }

        let (status, error_type, refused) = call_full(
            &router,
            "/DeleteSink",
            &json!({ "Identifier": sink.0.clone() }),
            owner_akid,
            region,
        )
        .await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected 409, got {status} {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "ConflictException",
            "case {case:?}: got {refused:?}"
        );
        assert_eq!(
            error_type.as_deref(),
            Some("ConflictException"),
            "case {case:?}: expected the exception name on x-amzn-errortype, got {error_type:?}"
        );
        assert_eq!(
            refusal_message(&case, &refused),
            SINK_HAS_LINKS_MESSAGE,
            "case {case:?}: got {refused:?}"
        );

        // The refused delete kept the sink and its links.
        let mut expected = links.clone();
        expected.sort();
        let mut still_attached = attached_link_arns(&router, &case, &sink.0, None, region).await;
        still_attached.sort();
        assert_eq!(
            still_attached, expected,
            "case {case:?}: the refused delete must have kept every link"
        );

        // Deleting every link clears the precondition, and the sink then deletes.
        for link in &links {
            let (status, _, body) = call_full(
                &router,
                "/DeleteLink",
                &json!({ "Identifier": link.clone() }),
                owner_akid,
                region,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200 from DeleteLink, got {status} {body:?}"
            );
        }
        let (status, _, deleted) = call_full(
            &router,
            "/DeleteSink",
            &json!({ "Identifier": sink.0.clone() }),
            owner_akid,
            region,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from DeleteSink once every link is gone, got \
             {status} {deleted:?}"
        );
    }
}
