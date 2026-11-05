// SPDX-License-Identifier: CC0-1.0

//! The ENVELOPE FIS's tagging surface answers in, end to end over HTTP: the status, the
//! error code a client branches on and the sentence each refusal carries.
//!
//! Three rules, each asserted over a table rather than over one request:
//!
//! 1. A tag MUTATION that is carried out answers **HTTP 204 No Content** and reports no
//!    member: `TagResourceResponse` and `UntagResourceResponse` carry no members
//!    (`smithy_query(query="operation", operation="TagResource,UntagResource")`), so there
//!    is nothing to report, and what the call left is read through `ListTagsForResource`
//!    and the resource's own reader instead.
//! 2. A `resourceArn` that is not an ARN this service could have ISSUED for a resource
//!    carrying caller tags — not an ARN at all, another vendor's, a resource type this
//!    service holds no caller tags for, or a resource id outside the form it mints — is
//!    refused by the ARN layer, before any lookup, with **HTTP 400** under the error code
//!    `BadRequestException` and the fixed sentence `Invalid input resource arn`, which
//!    echoes no part of the request.
//! 3. An ARN that IS one of this service's, naming a resource the store holds no record
//!    of, is a miss: **HTTP 404** under the error code `NotFoundException` with the
//!    fixed sentence `Resource was not found`. The tagging surface addresses its resource
//!    by ARN rather than by a typed id, so this sentence names no resource type and
//!    interpolates nothing — and the code is the one that surface answers for every
//!    resource class it addresses, not the modelled `ResourceNotFoundException` of
//!    `GetExperimentTemplate` or `GetExperiment`, which were asked for a resource of one
//!    named kind (`smithy_query(query="operation", operation="TagResource,
//!    ListTagsForResource,UntagResource")` lists NO error for any of the three, so no
//!    modelled shape is theirs to answer).
//!
//! The rows differ along everything the three rules generalise over: WHICH of the three
//! tag operations serves the call, which kind of taggable resource it addresses (an
//! experiment template and an experiment, the two records that carry caller tags), how
//! many tags the request carries or names, whether a named key is carried or absent, and —
//! for rule 2 — which FIELD of the ARN makes it one this service never issued (its syntax,
//! its vendor, its resource type, its id's prefix, width, alphabet or emptiness).
//!
//! The resources, the ARN tables and the tag-map helpers come from the tag WRITE suite
//! (`resource_tagging`), so the envelope asserted here is the envelope of the very same
//! calls whose persisted effect that suite asserts.

use std::collections::BTreeMap;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::experiment_lifecycle::{emulator, send, signed_for, FALLBACK_REGION};
use crate::resource_tagging::{
    label, new_resource, new_template, pairs, read_resource, tag_map, tags_of, unaddressable_arns,
    unminted_arns, Kind, TAGS_PER_RESOURCE,
};

/// The header a restJson1 client resolves an error SHAPE from.
pub(crate) const ERROR_TYPE_HEADER: &str = "x-amzn-errortype";

/// The one sentence the ARN layer refuses a `resourceArn` with.
pub(crate) const INVALID_RESOURCE_ARN: &str = "Invalid input resource arn";

/// The one sentence a tag call answers for an ARN of this service naming no record.
pub(crate) const NOT_FOUND: &str = "Resource was not found";

/// What the emulator answered: the status, the header a client resolves an error shape
/// from, and the parsed body.
pub(crate) struct Envelope {
    pub(crate) status: u16,
    pub(crate) error_type: Option<String>,
    pub(crate) body: Value,
}

impl Envelope {
    /// The wire error code in the body (`__type`), or the empty string.
    pub(crate) fn code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    /// The refusal's sentence, or the empty string.
    pub(crate) fn message(&self) -> &str {
        self.body["message"].as_str().unwrap_or_default()
    }

    /// True when the answer reports NO member — an absent body, or the empty JSON object
    /// the framework serializes an output shape carrying no members into. What rule 1 is
    /// about is the status and the absence of any reported member; the two framings of
    /// "nothing" are the same answer to a client, which reads members and finds none.
    fn reports_no_member(&self) -> bool {
        match &self.body {
            Value::Null => true,
            Value::Object(members) => members.is_empty(),
            _ => false,
        }
    }
}

/// One call to this service, answered with its whole envelope.
pub(crate) async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> Envelope {
    let payload = body
        .map(|value| Bytes::from(serde_json::to_vec(&value).expect("the test body serializes")))
        .unwrap_or_default();
    let request = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "fis.us-east-1.amazonaws.com")
        .header("authorization", signed_for(FALLBACK_REGION))
        .header("content-type", "application/json")
        .body(payload)
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get(ERROR_TYPE_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Envelope {
        status,
        error_type,
        body,
    }
}

/// Which of the three tag operations a row calls, and how it spells that call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Operation {
    Tag,
    Untag,
    List,
}

impl Operation {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Tag => "TagResource",
            Self::Untag => "UntagResource",
            Self::List => "ListTagsForResource",
        }
    }

    /// The same request every rule's table sends through this operation: one that would
    /// succeed if the ARN named a live resource.
    pub(crate) async fn call_on(self, router: &MultiServiceRouter, resource_arn: &str) -> Envelope {
        match self {
            Self::Tag => {
                call(
                    router,
                    "POST",
                    &format!("/tags/{}", label(resource_arn)),
                    Some(json!({ "tags": { "Env": "test" } })),
                )
                .await
            }
            Self::Untag => {
                call(
                    router,
                    "DELETE",
                    &format!("/tags/{}?tagKeys=Env", label(resource_arn)),
                    None,
                )
                .await
            }
            Self::List => {
                call(
                    router,
                    "GET",
                    &format!("/tags/{}", label(resource_arn)),
                    None,
                )
                .await
            }
        }
    }
}

/// Every operation of the tagging surface, so each table runs over all three.
pub(crate) fn operations() -> [Operation; 3] {
    [Operation::Tag, Operation::Untag, Operation::List]
}

/// What a row of the success table does to a resource's tags.
#[derive(Debug, Clone)]
enum Mutation {
    /// `TagResource` with these pairs.
    Tag(Vec<(String, String)>),
    /// `UntagResource` naming these keys — `None` sends no `tagKeys` parameter at all.
    Untag(Option<Vec<String>>),
}

/// One row of the success table: what the resource starts with, the one mutation applied,
/// and the tags its readers must report afterwards.
struct Success {
    case: &'static str,
    initial: Vec<(String, String)>,
    mutation: Mutation,
    expected: Vec<(String, String)>,
}

fn names(keys: &[&str]) -> Vec<String> {
    keys.iter().map(|key| (*key).to_string()).collect()
}

/// `count` distinct tags, for the rows about how many a call carries.
fn many(count: usize) -> Vec<(String, String)> {
    (0..count)
        .map(|index| (format!("key-{index:02}"), format!("value-{index:02}")))
        .collect()
}

/// The rows rule 1 is asserted over: both mutating operations, carrying none, one, several
/// and the most tags a resource holds, naming keys the resource carries and keys it does
/// not, and naming no key at all.
fn success_rows() -> Vec<Success> {
    vec![
        Success {
            case: "tag one key onto a resource carrying none",
            initial: Vec::new(),
            mutation: Mutation::Tag(pairs(&[("Env", "test")])),
            expected: pairs(&[("Env", "test")]),
        },
        Success {
            case: "tag three keys in one call",
            initial: Vec::new(),
            mutation: Mutation::Tag(pairs(&[("Env", "test"), ("Owner", "fis"), ("Team", "chaos")])),
            expected: pairs(&[("Env", "test"), ("Owner", "fis"), ("Team", "chaos")]),
        },
        Success {
            case: "tag an empty map, which changes nothing",
            initial: pairs(&[("Env", "test")]),
            mutation: Mutation::Tag(Vec::new()),
            expected: pairs(&[("Env", "test")]),
        },
        Success {
            case: "tag the most tags a resource carries",
            initial: Vec::new(),
            mutation: Mutation::Tag(many(TAGS_PER_RESOURCE)),
            expected: many(TAGS_PER_RESOURCE),
        },
        Success {
            case: "untag one key the resource carries",
            initial: pairs(&[("Env", "test"), ("Owner", "fis")]),
            mutation: Mutation::Untag(Some(names(&["Env"]))),
            expected: pairs(&[("Owner", "fis")]),
        },
        Success {
            case: "untag a key the resource does not carry",
            initial: pairs(&[("Env", "test")]),
            mutation: Mutation::Untag(Some(names(&["Absent"]))),
            expected: pairs(&[("Env", "test")]),
        },
        Success {
            case: "untag naming no key at all",
            initial: pairs(&[("Env", "test")]),
            mutation: Mutation::Untag(None),
            expected: pairs(&[("Env", "test")]),
        },
        Success {
            case: "untag every key the resource carries",
            initial: pairs(&[("Env", "test"), ("Owner", "fis")]),
            mutation: Mutation::Untag(Some(names(&["Env", "Owner"]))),
            expected: Vec::new(),
        },
    ]
}

/// The tags `ListTagsForResource` reports for `resource_arn`.
async fn listed_tags(
    router: &MultiServiceRouter,
    resource_arn: &str,
    case: &str,
) -> BTreeMap<String, String> {
    let answer = send(
        router,
        "GET",
        &format!("/tags/{}", label(resource_arn)),
        None,
        FALLBACK_REGION,
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200 from ListTagsForResource, got {} with {:?}",
        answer.status, answer.body
    );
    tags_of(&answer.body, case)
}

/// RULE 1: a tag mutation that is carried out answers 204 reporting nothing, and what it
/// left is reported by the tag read and by the resource's own reader.
#[tokio::test]
async fn a_carried_out_tag_mutation_answers_204_reporting_nothing() {
    for kind in [Kind::ExperimentTemplate, Kind::Experiment] {
        for row in success_rows() {
            let case = format!("{} / {}", kind.as_str(), row.case);
            // One emulator per row, so each row judges a resource of its own.
            let router = emulator();
            let resource = new_resource(&router, kind, &case, &tag_map(&row.initial)).await;

            let answer = match &row.mutation {
                Mutation::Tag(applied) => {
                    call(
                        &router,
                        "POST",
                        &format!("/tags/{}", label(&resource.arn)),
                        Some(json!({ "tags": tag_map(applied) })),
                    )
                    .await
                }
                Mutation::Untag(removed) => {
                    let query = match removed {
                        None => String::new(),
                        Some(keys) => format!(
                            "?{}",
                            keys.iter()
                                .map(|key| format!("tagKeys={key}"))
                                .collect::<Vec<_>>()
                                .join("&")
                        ),
                    };
                    call(
                        &router,
                        "DELETE",
                        &format!("/tags/{}{query}", label(&resource.arn)),
                        None,
                    )
                    .await
                }
            };

            assert_eq!(
                answer.status, 204,
                "case {case:?}: expected HTTP 204 No Content, got {} with {:?}",
                answer.status, answer.body
            );
            assert!(
                answer.reports_no_member(),
                "case {case:?}: a carried-out mutation reports no member, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.error_type, None,
                "case {case:?}: a success names no error type, got {:?}",
                answer.error_type
            );

            // What the call left is persisted: the tag read and the resource's own reader
            // report the same map.
            let expected: BTreeMap<String, String> = row.expected.iter().cloned().collect();
            assert_eq!(
                listed_tags(&router, &resource.arn, &case).await,
                expected,
                "case {case:?}: expected the tag read to report what the call left"
            );
            let record = read_resource(&router, &resource, &case).await;
            assert_eq!(
                tags_of(&record, &case),
                expected,
                "case {case:?}: expected the resource's own reader to agree, got {:?}",
                record["tags"]
            );
        }
    }
}

/// RULE 2: every ARN this service could not have issued earns the SAME bad request, from
/// every operation of the tagging surface, and the live resource is untouched by it.
#[tokio::test]
async fn an_arn_this_service_never_issued_is_one_fixed_bad_request() {
    for operation in operations() {
        for (reason, arn) in unaddressable_arns() {
            let case = format!("{} / {reason}", operation.as_str());
            // One emulator per case, so the "nothing was written" assertion below is
            // about this case's call alone.
            let router = emulator();
            let stored = pairs(&[("Suite", "envelope")]);
            let resource = new_resource(
                &router,
                Kind::ExperimentTemplate,
                &case,
                &tag_map(&stored),
            )
            .await;

            let answer = operation.call_on(&router, &arn).await;
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected HTTP 400 for {arn:?}, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_type.as_deref(),
                Some("BadRequestException"),
                "case {case:?}: expected the BadRequestException error type for {arn:?}, \
                 got {:?}",
                answer.error_type
            );
            assert_eq!(
                answer.code(),
                "BadRequestException",
                "case {case:?}: expected BadRequestException in the body for {arn:?}, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.message(),
                INVALID_RESOURCE_ARN,
                "case {case:?}: expected the fixed ARN refusal for {arn:?}, got {:?}",
                answer.message()
            );
            // Fixed text: no part of the refused request comes back in it.
            assert!(
                !answer.message().contains(arn.as_str()),
                "case {case:?}: the refused ARN must not be echoed back, got {:?}",
                answer.message()
            );

            assert_eq!(
                tags_of(&read_resource(&router, &resource, &case).await, &case),
                stored.iter().cloned().collect::<BTreeMap<_, _>>(),
                "case {case:?}: a refused call must leave the stored tags alone"
            );
        }
    }
}

/// RULE 3: an ARN of this service naming no stored record earns the same generic miss from
/// every operation of the tagging surface — including the ARN of a resource that existed
/// and has since been deleted.
#[tokio::test]
async fn an_arn_naming_no_record_is_one_generic_miss() {
    for operation in operations() {
        let router = emulator();

        // A template this case creates and then deletes: the ARN stays one this service
        // issued, and the record behind it is gone.
        let (deleted_id, deleted_arn) =
            new_template(&router, operation.as_str(), &json!({})).await;
        let deleted = send(
            &router,
            "DELETE",
            &format!("/experimentTemplates/{deleted_id}"),
            None,
            FALLBACK_REGION,
        )
        .await;
        assert_eq!(
            deleted.status, 200,
            "expected HTTP 200 from DeleteExperimentTemplate, got {} with {:?}",
            deleted.status, deleted.body
        );

        let mut misses = unminted_arns();
        misses.push(("a template that has since been deleted", deleted_arn));

        for (reason, arn) in misses {
            let case = format!("{} / {reason}", operation.as_str());
            let answer = operation.call_on(&router, &arn).await;
            assert_eq!(
                answer.status, 404,
                "case {case:?}: expected HTTP 404 for {arn:?}, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.code(),
                "NotFoundException",
                "case {case:?}: expected the tagging surface's 404 shape for {arn:?}, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.error_type.as_deref(),
                Some("NotFoundException"),
                "case {case:?}: expected the NotFoundException error type for {arn:?}, got {:?}",
                answer.error_type
            );
            assert_eq!(
                answer.message(),
                NOT_FOUND,
                "case {case:?}: expected the tagging surface's generic miss for {arn:?}, \
                 got {:?}",
                answer.message()
            );
            // Generic: the sentence names neither the resource type asked about nor the
            // id it was asked for.
            for named in ["ExperimentTemplate", "Experiment", "experiment-template"] {
                assert!(
                    !answer.message().contains(named),
                    "case {case:?}: the miss names no resource type, got {:?}",
                    answer.message()
                );
            }
            assert!(
                !answer.message().contains(arn.as_str()),
                "case {case:?}: the miss echoes nothing of the request, got {:?}",
                answer.message()
            );
        }
    }
}

/// The boundary of rule 2: what the ARN must carry is this service's vendor, a taggable
/// resource type and an id of the form it mints — the REGION and ACCOUNT segments are not
/// a second key, so an ARN differing only in those still addresses the same record, which
/// is what makes the id what names it.
#[tokio::test]
async fn the_arns_region_and_account_segments_are_not_a_second_key() {
    let router = emulator();
    let case = "another region and account for the same id";
    let resource = new_resource(
        &router,
        Kind::ExperimentTemplate,
        case,
        &tag_map(&pairs(&[("Suite", "envelope")])),
    )
    .await;
    let id = resource
        .arn
        .rsplit('/')
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: no id in {:?}", resource.arn))
        .to_string();

    for (reason, arn) in [
        (
            "another Region",
            format!("arn:aws:fis:eu-west-3:000000000000:experiment-template/{id}"),
        ),
        (
            "another account",
            format!("arn:aws:fis:{FALLBACK_REGION}:123456789012:experiment-template/{id}"),
        ),
    ] {
        let case = format!("{case} / {reason}");
        let answer = call(
            &router,
            "POST",
            &format!("/tags/{}", label(&arn)),
            Some(json!({ "tags": { reason: "yes" } })),
        )
        .await;
        assert_eq!(
            answer.status, 204,
            "case {case:?}: expected the same record to be tagged through {arn:?}, got {} \
             with {:?}",
            answer.status, answer.body
        );
    }

    // All three calls reached the ONE record the id names.
    let record = read_resource(&router, &resource, case).await;
    assert_eq!(
        tags_of(&record, case),
        pairs(&[
            ("Suite", "envelope"),
            ("another Region", "yes"),
            ("another account", "yes"),
        ])
        .into_iter()
        .collect::<BTreeMap<_, _>>(),
        "case {case:?}: expected every call to have reached the one record, got {:?}",
        record["tags"]
    );
}
