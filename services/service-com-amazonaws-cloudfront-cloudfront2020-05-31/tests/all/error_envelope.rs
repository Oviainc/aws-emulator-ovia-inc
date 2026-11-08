// SPDX-License-Identifier: CC0-1.0

//! CloudFront's error envelope, end to end over HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing, the
//! request boundary, the handler and serialization — the same path a client takes. The
//! rules under test are about how this service states a FAILURE:
//!
//! * the restXml error document names the request it answers: `<ErrorResponse>` carries a
//!   `RequestId` element beside `<Error>`, on every error this service answers — a
//!   modelled failure a handler raised and a refusal the request boundary made alike —
//!   and each response carries its own id;
//! * a value a modelled query parameter cannot take is refused with the operation's own
//!   modelled error for a bad argument (`InvalidArgument`, 400), stating the constraint
//!   on the parameter, never with a code the model does not declare or a codec's internal
//!   parse text;
//! * a body-bearing request that carries no body at all has a null PAYLOAD, so it is
//!   refused as `MalformedXML` (400) naming the payload member — not a member nested
//!   inside it;
//! * an error message is the service's own: the sentence the model states for the shape
//!   where the shape has one, and the sentence CloudFront builds from the request where
//!   it builds one — never an internal identifier appended to it.
//!
//! The error documents are read through the suite harness's XML reader (shared with
//! `distribution_lifecycle`), so each assertion is about the document a client would
//! parse rather than about a substring of it.

use std::collections::BTreeSet;

use smithy_server_runtime::MultiServiceRouter;

// The one harness this test binary has: the router, the request helper and the parsed
// answer all come from the suite beside this one rather than from a second copy.
use super::distribution_lifecycle::{call, emulator, Answer, API_VERSION};

// The sentences the service states, spelled out here rather than imported from the
// implementation: each is the text the recorded responses carry (and, for the two
// not-found shapes, the sentence the model states on the shape), so a constant edited to
// some other wording fails these rows.
/// `NoSuchDistribution` — the model's own sentence for the shape.
const NO_SUCH_DISTRIBUTION: &str = "The specified distribution does not exist.";

/// `NoSuchInvalidation` — the model's own sentence for the shape.
const NO_SUCH_INVALIDATION: &str = "The specified invalidation does not exist.";

/// `PreconditionFailed`, as the recorded responses carry it.
const PRECONDITION_FAILED: &str = "The request failed because it didn't meet the \
     preconditions in one or more request-header fields.";

/// `InvalidArgument` for a `MaxItems` no page size can be taken from, as the recorded
/// responses carry it.
const MAX_ITEMS_INVALID: &str = "MaxItems must be a non-zero positive integer.";

/// A distribution id of the shape this service mints (`E` + 13) that no store holds, so
/// every read of it is a not-found.
const ABSENT_DISTRIBUTION: &str = "EDFDVBD6EXAMPLE";

/// An invalidation id of the shape this service mints (`I` + 13) that no distribution
/// holds.
const ABSENT_INVALIDATION: &str = "I2J0I21PCKYOOK";

/// A well-formed version token (`E` + 13, as `ETag` values are shaped) that is not the
/// current version of anything, so an `If-Match` carrying it fails the precondition
/// rather than being rejected as no version at all.
const STALE_VERSION: &str = "E1PDK09ESLDGHD";

/// The error message the answer states, with the entities the envelope escapes already
/// resolved by the reader.
fn message(answer: &Answer) -> &str {
    answer.root().find_text("Message").unwrap_or_default()
}

/// The request id the envelope names, read only where the envelope puts it: a
/// `RequestId` element that is a DIRECT child of the document root, beside `Error`. An
/// answer whose id is anywhere else reads as no id at all.
fn request_id(answer: &Answer) -> Option<&str> {
    let root = answer.root();
    root.child("Error")?;
    root.child("RequestId").map(super::distribution_lifecycle::Node::text)
}

/// A `DistributionConfig` body: `origins` names one `(Id, DomainName)` per origin, and
/// `stated_origins` is the `Quantity` the document claims — which a well-formed request
/// states as the number of origins it carries.
fn config_xml(
    caller_reference: &str,
    stated_origins: usize,
    origins: &[(&str, &str)],
    aliases: &[&str],
    stated_aliases: usize,
    enabled: bool,
) -> String {
    let mut out = format!(
        "<DistributionConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <CallerReference>{caller_reference}</CallerReference>"
    );
    if !aliases.is_empty() || stated_aliases > 0 {
        // `AliasList`'s member is named `CNAME` in XML.
        out.push_str(&format!("<Aliases><Quantity>{stated_aliases}</Quantity><Items>"));
        for alias in aliases {
            out.push_str(&format!("<CNAME>{alias}</CNAME>"));
        }
        out.push_str("</Items></Aliases>");
    }
    out.push_str(&format!("<Origins><Quantity>{stated_origins}</Quantity><Items>"));
    for (id, domain) in origins {
        out.push_str(&format!(
            "<Origin><Id>{id}</Id><DomainName>{domain}</DomainName>\
             <CustomOriginConfig><HTTPPort>80</HTTPPort><HTTPSPort>443</HTTPSPort>\
             <OriginProtocolPolicy>https-only</OriginProtocolPolicy>\
             </CustomOriginConfig></Origin>"
        ));
    }
    out.push_str("</Items></Origins>");
    out.push_str(&format!(
        "<DefaultCacheBehavior><TargetOriginId>{}</TargetOriginId>\
         <ViewerProtocolPolicy>allow-all</ViewerProtocolPolicy></DefaultCacheBehavior>\
         <Comment>a distribution for {caller_reference}</Comment><Enabled>{enabled}</Enabled>\
         </DistributionConfig>",
        origins.first().map(|(id, _)| *id).unwrap_or_default(),
    ));
    out
}

/// A well-formed configuration for `reference`, with one origin.
fn config(reference: &str) -> String {
    config_xml(reference, 1, &[("primary", "origin.example.com")], &[], 0, true)
}

/// Create one distribution and return its id.
async fn create(router: &MultiServiceRouter, reference: &str) -> String {
    let answer = call(
        router,
        "POST",
        &format!("/{API_VERSION}/distribution"),
        Some(&config(reference)),
        None,
    )
    .await;
    assert_eq!(
        answer.status, 201,
        "creating {reference} must succeed, got {} {}",
        answer.status, answer.raw
    );
    answer
        .root()
        .find_text("Id")
        .expect("the created distribution reports its id")
        .to_string()
}

/// Whether `value` has the shape of a v4 UUID: 8-4-4-4-12 hex digits.
fn is_uuid(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == 5
        && [8usize, 4, 4, 4, 12]
            .iter()
            .zip(&groups)
            .all(|(len, group)| {
                group.len() == *len && group.chars().all(|c| c.is_ascii_hexdigit())
            })
}

// ---------------------------------------------------------------------------
// The envelope names the request, on every error
// ---------------------------------------------------------------------------

/// One failing call: how it is made, and what the service answers.
struct Case {
    name: &'static str,
    method: &'static str,
    uri: String,
    body: Option<String>,
    if_match: Option<String>,
    status: u16,
    code: &'static str,
}

/// Every error class this service's distribution surface answers, as one request each:
/// a 404 from three different operations, a 409 conflict, a 409 state refusal, a 412
/// precondition, a 400 raised by a handler, a 400 the request boundary raised on a query
/// value, and a 400 it raised on an absent payload.
async fn error_cases(router: &MultiServiceRouter) -> Vec<Case> {
    let existing = create(router, "envelope-existing").await;
    let disabled = create(router, "envelope-conflict").await;
    vec![
        Case {
            name: "GetDistribution on an id the store does not hold",
            method: "GET",
            uri: format!("/{API_VERSION}/distribution/{ABSENT_DISTRIBUTION}"),
            body: None,
            if_match: None,
            status: 404,
            code: "NoSuchDistribution",
        },
        Case {
            name: "GetInvalidation on an id the distribution does not hold",
            method: "GET",
            uri: format!(
                "/{API_VERSION}/distribution/{existing}/invalidation/{ABSENT_INVALIDATION}"
            ),
            body: None,
            if_match: None,
            status: 404,
            code: "NoSuchInvalidation",
        },
        Case {
            name: "CreateInvalidation under a distribution that does not exist",
            method: "POST",
            uri: format!("/{API_VERSION}/distribution/{ABSENT_DISTRIBUTION}/invalidation"),
            body: Some(
                "<InvalidationBatch><Paths><Quantity>1</Quantity><Items><Path>/*</Path>\
                 </Items></Paths><CallerReference>envelope-batch</CallerReference>\
                 </InvalidationBatch>"
                    .to_string(),
            ),
            if_match: None,
            status: 404,
            code: "NoSuchDistribution",
        },
        Case {
            name: "CreateDistribution reusing a caller reference for another configuration",
            method: "POST",
            uri: format!("/{API_VERSION}/distribution"),
            // A repeat under the same reference that submits the SAME configuration is
            // the same request again, so the conflict is a reference the store holds
            // being pointed at a DIFFERENT configuration.
            body: Some(config_xml(
                "envelope-existing",
                1,
                &[("primary", "elsewhere.example.com")],
                &[],
                0,
                true,
            )),
            if_match: None,
            status: 409,
            code: "DistributionAlreadyExists",
        },
        Case {
            name: "DeleteDistribution under an If-Match that is not the current version",
            method: "DELETE",
            uri: format!("/{API_VERSION}/distribution/{disabled}"),
            body: None,
            if_match: Some(STALE_VERSION.to_string()),
            status: 412,
            code: "PreconditionFailed",
        },
        Case {
            name: "CreateDistribution whose Origins.Quantity disagrees with its Items",
            method: "POST",
            uri: format!("/{API_VERSION}/distribution"),
            body: Some(config_xml(
                "envelope-quantities",
                2,
                &[("primary", "origin.example.com")],
                &[],
                0,
                true,
            )),
            if_match: None,
            status: 400,
            code: "InconsistentQuantities",
        },
        Case {
            name: "ListDistributions with a MaxItems that is no integer",
            method: "GET",
            uri: format!("/{API_VERSION}/distribution?MaxItems=not-a-number"),
            body: None,
            if_match: None,
            status: 400,
            code: "InvalidArgument",
        },
        Case {
            name: "CreateDistribution with no body",
            method: "POST",
            uri: format!("/{API_VERSION}/distribution"),
            body: Some(String::new()),
            if_match: None,
            status: 400,
            code: "MalformedXML",
        },
    ]
}

#[tokio::test]
async fn every_error_envelope_names_the_request_it_answers() {
    let router = emulator();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for case in error_cases(&router).await {
        let answer = call(
            &router,
            case.method,
            &case.uri,
            case.body.as_deref(),
            case.if_match.as_deref(),
        )
        .await;
        let name = case.name;
        assert_eq!(
            answer.status, case.status,
            "case {name}: expected HTTP {}, got {} {}",
            case.status, answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            case.code,
            "case {name}: expected code {}, got {:?} in {}",
            case.code,
            answer.error_code(),
            answer.raw
        );
        let id = request_id(&answer);
        assert!(
            id.is_some_and(is_uuid),
            "case {name}: expected a RequestId element beside Error, got {id:?} in {}",
            answer.raw
        );
        let id = id.expect("a request id").to_string();
        assert!(
            seen.insert(id.clone()),
            "case {name}: every response names its own request, but {id} was seen before"
        );
    }
}

#[tokio::test]
async fn a_successful_answer_carries_no_error_envelope() {
    let router = emulator();
    let id = create(&router, "envelope-success").await;
    let answer = call(
        &router,
        "GET",
        &format!("/{API_VERSION}/distribution/{id}"),
        None,
        None,
    )
    .await;
    assert_eq!(answer.status, 200, "the read must succeed: {}", answer.raw);
    assert!(
        !answer.raw.contains("<RequestId>") && !answer.raw.contains("<ErrorResponse"),
        "a success must not be dressed as an error envelope: {}",
        answer.raw
    );
}

#[tokio::test]
async fn a_request_that_names_no_operation_also_names_the_request() {
    // The router refuses a request that resolves to no modelled operation in FRONT of
    // the service, so its envelope is rendered by the service definition rather than by
    // an operation — and it names the request just the same.
    let router = emulator();
    let answer = call(
        &router,
        "GET",
        &format!("/{API_VERSION}/no-such-resource"),
        None,
        None,
    )
    .await;
    assert!(
        answer.status >= 400,
        "an unroutable request must be refused, got {} {}",
        answer.status,
        answer.raw
    );
    let id = request_id(&answer);
    assert!(
        id.is_some_and(is_uuid),
        "expected a RequestId element beside Error, got {id:?} in {}",
        answer.raw
    );
}

// ---------------------------------------------------------------------------
// A query value the parameter cannot take
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_max_items_the_page_size_cannot_take_is_invalid_argument() {
    let router = emulator();
    create(&router, "max-items-row").await;

    // `MaxItems` is an `integer` `@httpQuery` member (the model's list operations), and
    // a page size is a non-negative integer: the rows differ in TYPE (no integer at all, a
    // decimal, an empty value), in VALUE (negative) and in SIZE (past i32), and the
    // usable rows differ in value — zero included, which bounds the page at no items — and
    // in whether another parameter travels with them.
    let unusable = [
        "not-a-number",
        "",
        "-1",
        "3.5",
        "99999999999",
        "1e2",
        "%20",
        // Decodes to `a&b`, which the envelope escapes into the refusal it renders.
        "a%26b",
        // Decodes to `<two>`, escaped the same way.
        "%3Ctwo%3E",
    ];
    for value in unusable {
        let answer = call(
            &router,
            "GET",
            &format!("/{API_VERSION}/distribution?MaxItems={value}"),
            None,
            None,
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case MaxItems={value:?}: expected HTTP 400, got {} {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "InvalidArgument",
            "case MaxItems={value:?}: expected code InvalidArgument, got {:?} in {}",
            answer.error_code(),
            answer.raw
        );
        assert_eq!(
            message(&answer),
            MAX_ITEMS_INVALID,
            "case MaxItems={value:?}: expected the constraint on the parameter, got {:?}",
            message(&answer)
        );
        assert!(
            request_id(&answer).is_some(),
            "case MaxItems={value:?}: the refusal must name the request: {}",
            answer.raw
        );
    }

    for value in ["0", "1", "100", "2147483647"] {
        let answer = call(
            &router,
            "GET",
            &format!("/{API_VERSION}/distribution?MaxItems={value}"),
            None,
            None,
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case MaxItems={value:?}: a non-negative integer page size is usable, got {} {}",
            answer.status, answer.raw
        );
    }
}

#[tokio::test]
async fn the_max_items_refusal_is_in_front_of_every_operation_that_binds_it() {
    // `ListDistributionsByWebACLId` binds the same `MaxItems` member: a value the
    // parameter cannot take is refused at the request boundary, as AWS refuses it, while a
    // usable one reaches the operation — which answers from the store.
    let router = emulator();
    let answer = call(
        &router,
        "GET",
        &format!("/{API_VERSION}/distributionsByWebACLId/abc123?MaxItems=not-a-number"),
        None,
        None,
    )
    .await;
    assert_eq!(
        answer.status, 400,
        "an unusable MaxItems is refused before the operation runs: {}",
        answer.raw
    );
    assert_eq!(
        answer.error_code(),
        "InvalidArgument",
        "expected code InvalidArgument, got {:?} in {}",
        answer.error_code(),
        answer.raw
    );
    assert_eq!(
        message(&answer),
        MAX_ITEMS_INVALID,
        "expected the constraint on the parameter, got {:?}",
        message(&answer)
    );

    let served = call(
        &router,
        "GET",
        &format!("/{API_VERSION}/distributionsByWebACLId/abc123?MaxItems=10"),
        None,
        None,
    )
    .await;
    assert_eq!(
        served.status, 200,
        "a usable MaxItems reaches the operation, which answers from the store: {}",
        served.raw
    );
}

// ---------------------------------------------------------------------------
// A request that carries no body at all
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_absent_payload_is_malformed_xml_naming_the_payload_member() {
    let router = emulator();
    let id = create(&router, "absent-payload").await;

    // Every row is an operation whose input binds a REQUIRED structured `@httpPayload`
    // member (`smithy_query(query="operation")`), and the member differs per row — so
    // the refusal names the payload the operation declares rather than one member name
    // written into the service. The last row is an operation this emulator has not
    // implemented: the body is refused in front of it either way.
    let cases = [
        (
            "CreateDistribution",
            "POST",
            format!("/{API_VERSION}/distribution"),
            "distributionConfig",
        ),
        (
            "UpdateDistribution",
            "PUT",
            format!("/{API_VERSION}/distribution/{id}/config"),
            "distributionConfig",
        ),
        (
            "CreateInvalidation",
            "POST",
            format!("/{API_VERSION}/distribution/{id}/invalidation"),
            "invalidationBatch",
        ),
        (
            "CreateDistributionWithTags",
            "POST",
            format!("/{API_VERSION}/distribution?WithTags"),
            "distributionConfigWithTags",
        ),
    ];
    for (name, method, uri, payload) in cases {
        for body in ["", "   \n"] {
            let answer = call(&router, method, &uri, Some(body), Some(STALE_VERSION)).await;
            assert_eq!(
                answer.status, 400,
                "case {name} with body {body:?}: expected HTTP 400, got {} {}",
                answer.status, answer.raw
            );
            assert_eq!(
                answer.error_code(),
                "MalformedXML",
                "case {name} with body {body:?}: expected code MalformedXML, got {:?} in {}",
                answer.error_code(),
                answer.raw
            );
            let expected = format!(
                "1 validation error detected: Value null at '{payload}' failed to satisfy \
                 constraint: Member must not be null"
            );
            assert_eq!(
                message(&answer),
                expected,
                "case {name} with body {body:?}: expected the payload member to be the null \
                 one, got {:?}",
                message(&answer)
            );
            assert!(
                request_id(&answer).is_some(),
                "case {name} with body {body:?}: the refusal must name the request: {}",
                answer.raw
            );
        }
    }
}

#[tokio::test]
async fn a_body_that_is_present_but_incomplete_is_not_an_absent_payload() {
    // The rule is about a request with NO body: one that carries a document the service
    // can read keeps the member-level refusal, so a client can tell the two apart.
    let router = emulator();
    let answer = call(
        &router,
        "POST",
        &format!("/{API_VERSION}/distribution"),
        Some(
            "<DistributionConfig><Comment>no caller reference</Comment>\
             </DistributionConfig>",
        ),
        None,
    )
    .await;
    assert_eq!(
        answer.status, 400,
        "an incomplete configuration is refused: {}",
        answer.raw
    );
    assert_ne!(
        answer.error_code(),
        "MalformedXML",
        "a body that was read is not an absent payload: {}",
        answer.raw
    );
}

// ---------------------------------------------------------------------------
// The wording of each message
// ---------------------------------------------------------------------------

/// One failing call, and the exact message the service states for it.
struct Wording {
    name: &'static str,
    method: &'static str,
    uri: String,
    body: Option<String>,
    if_match: Option<String>,
    message: String,
}

#[tokio::test]
async fn each_error_message_is_the_service_s_own_wording() {
    let router = emulator();
    let existing = create(&router, "wording-existing").await;
    let other = create(&router, "wording-other").await;

    // (how the call is made) -> the message the service states.
    //
    // A shape whose message the model states carries exactly that sentence, and no
    // request detail is appended to it; a message CloudFront builds from the request
    // carries exactly the detail CloudFront puts in it.
    let expected_already_exists = format!(
        "The caller reference that you are using to create a distribution is associated \
         with another distribution. Already exists: {existing}"
    );
    let cases = vec![
        Wording {
            name: "GetDistribution/404",
            method: "GET",
            uri: format!("/{API_VERSION}/distribution/{ABSENT_DISTRIBUTION}"),
            body: None,
            if_match: None,
            message: NO_SUCH_DISTRIBUTION.to_string(),
        },
        Wording {
            name: "DeleteDistribution/404",
            method: "DELETE",
            uri: format!("/{API_VERSION}/distribution/{ABSENT_DISTRIBUTION}"),
            body: None,
            if_match: Some(STALE_VERSION.to_string()),
            message: NO_SUCH_DISTRIBUTION.to_string(),
        },
        Wording {
            name: "GetInvalidation/404 on the distribution",
            method: "GET",
            uri: format!(
                "/{API_VERSION}/distribution/{ABSENT_DISTRIBUTION}/invalidation/\
                 {ABSENT_INVALIDATION}"
            ),
            body: None,
            if_match: None,
            message: NO_SUCH_DISTRIBUTION.to_string(),
        },
        Wording {
            name: "GetInvalidation/404 on the invalidation",
            method: "GET",
            uri: format!(
                "/{API_VERSION}/distribution/{existing}/invalidation/{ABSENT_INVALIDATION}"
            ),
            body: None,
            if_match: None,
            message: NO_SUCH_INVALIDATION.to_string(),
        },
        Wording {
            name: "UpdateDistribution/412",
            method: "PUT",
            uri: format!("/{API_VERSION}/distribution/{other}/config"),
            body: Some(config("wording-other")),
            if_match: Some(STALE_VERSION.to_string()),
            message: PRECONDITION_FAILED.to_string(),
        },
        Wording {
            name: "DeleteDistribution/412",
            method: "DELETE",
            uri: format!("/{API_VERSION}/distribution/{other}"),
            body: None,
            if_match: Some(STALE_VERSION.to_string()),
            message: PRECONDITION_FAILED.to_string(),
        },
        Wording {
            name: "CreateDistribution/409",
            method: "POST",
            uri: format!("/{API_VERSION}/distribution"),
            // A different configuration under a caller reference the store holds: the
            // same configuration would be the same request again.
            body: Some(config_xml(
                "wording-existing",
                1,
                &[("primary", "elsewhere.example.com")],
                &[],
                0,
                true,
            )),
            if_match: None,
            message: expected_already_exists,
        },
        Wording {
            name: "CreateDistribution/400 Origins",
            method: "POST",
            uri: format!("/{API_VERSION}/distribution"),
            body: Some(config_xml(
                "wording-origins",
                2,
                &[("primary", "origin.example.com")],
                &[],
                0,
                true,
            )),
            if_match: None,
            message: "The specified quantity of Origins (2) does not match the actual quantity \
                      supplied (1)."
                .to_string(),
        },
        Wording {
            name: "CreateDistribution/400 Aliases",
            method: "POST",
            uri: format!("/{API_VERSION}/distribution"),
            body: Some(config_xml(
                "wording-aliases",
                1,
                &[("primary", "origin.example.com")],
                &["one.example.com", "two.example.com", "three.example.com"],
                5,
                true,
            )),
            if_match: None,
            message: "The specified quantity of Aliases (5) does not match the actual quantity \
                      supplied (3)."
                .to_string(),
        },
    ];

    for case in cases {
        let Wording {
            name,
            method,
            uri,
            body,
            if_match,
            message: expected,
        } = case;
        let answer = call(&router, method, &uri, body.as_deref(), if_match.as_deref()).await;
        assert_eq!(
            message(&answer),
            expected,
            "case {name}: expected message {expected:?}, got {:?} in {}",
            message(&answer),
            answer.raw
        );
        assert!(
            !message(&answer).contains(ABSENT_DISTRIBUTION)
                && !message(&answer).contains(ABSENT_INVALIDATION)
                && !message(&answer).contains(STALE_VERSION),
            "case {name}: the message must not echo an identifier the caller sent: {:?}",
            message(&answer)
        );
    }
}
