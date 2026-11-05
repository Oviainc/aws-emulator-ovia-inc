// SPDX-License-Identifier: CC0-1.0

//! `ListTagsForResource`, end to end over HTTP.
//!
//! The rule under test is the one FIS's tagging surface states for a tag READ: the call
//! names its resource by ARN, that ARN is resolved in this service's OWN namespace, and
//! the answer is the tag map that resource PERSISTS — the same map a tag call wrote and
//! the resource's own reader (`GetExperimentTemplate`, `GetExperiment`) reports, always
//! present and empty when the resource carries no tags. An ARN that addresses nothing in
//! this namespace is a bad request (400 `BadRequestException`) and an ARN naming a
//! resource of this service the store holds no record of is a miss (404
//! `NotFoundException`) — the same two refusals a tag WRITE of that ARN earns, so
//! a read and a write cannot disagree about which ARNs this service addresses. Being a
//! read, it changes nothing: a second call, and the resource's own reader afterwards,
//! report exactly what the first call did.
//!
//! The rows below differ along everything that rule generalises over: WHICH kind of
//! resource is read (an experiment template and an experiment, the two records of this
//! service that carry caller tags), how many tags it carries (none, one, three, the most a
//! `TagMap` holds), HOW they got there (the create request, later tag calls, or both), and
//! the shape of a key and a value (plain, empty, whitespace, non-ASCII, and each at the
//! ceiling `TagKey` and `TagValue` publish).
//!
//! The resources, the tag-map helpers, the modelled ceilings and the table of ARNs this
//! service cannot address come from the tag WRITE suite (`resource_tagging`), so the two
//! halves of the rule are asserted over the same inputs.

use std::collections::BTreeMap;

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use crate::experiment_lifecycle::{emulator, send, Answer, FALLBACK_REGION};
use crate::resource_tagging::{
    generated, label, new_resource, new_template, pairs, read_resource, tag_map, tag_resource,
    tags_of, unaddressable_arns, unminted_arns, Kind, Resource, Row, TAGS_PER_RESOURCE,
    TAG_KEY_CEILING, TAG_VALUE_CEILING,
};

/// `ListTagsForResource`: report the tags of the resource `resource_arn` names.
async fn list_tags(router: &MultiServiceRouter, resource_arn: &str) -> Answer {
    send(
        router,
        "GET",
        &format!("/tags/{}", label(resource_arn)),
        None,
        FALLBACK_REGION,
    )
    .await
}

/// The tags the resource's OWN reader reports.
async fn tags_from_reader(
    router: &MultiServiceRouter,
    resource: &Resource,
    case: &str,
) -> BTreeMap<String, String> {
    tags_of(&read_resource(router, resource, case).await, case)
}

/// The rows the reporting rule is asserted over: the tags the resource is created with,
/// the tag maps applied to it afterwards, and the tags a tag read must report.
fn rows() -> Vec<Row> {
    let key_at_ceiling = "k".repeat(TAG_KEY_CEILING);
    let value_at_ceiling = "v".repeat(TAG_VALUE_CEILING);
    vec![
        Row {
            case: "a resource carrying no tags at all".to_string(),
            initial: Vec::new(),
            applies: Vec::new(),
            expected: Vec::new(),
        },
        Row {
            case: "one tag, carried since the resource was created".to_string(),
            initial: pairs(&[("Env", "test")]),
            applies: Vec::new(),
            expected: pairs(&[("Env", "test")]),
        },
        Row {
            case: "one tag, applied after the resource was created".to_string(),
            initial: Vec::new(),
            applies: vec![pairs(&[("Env", "test")])],
            expected: pairs(&[("Env", "test")]),
        },
        Row {
            case: "three tags, from the create request and two later calls".to_string(),
            initial: pairs(&[("Suite", "tag-read")]),
            applies: vec![pairs(&[("Env", "test")]), pairs(&[("Owner", "fis")])],
            expected: pairs(&[("Env", "test"), ("Owner", "fis"), ("Suite", "tag-read")]),
        },
        Row {
            case: "a key whose value a later call replaced".to_string(),
            initial: pairs(&[("Env", "test"), ("Suite", "tag-read")]),
            applies: vec![pairs(&[("Env", "prod")])],
            expected: pairs(&[("Env", "prod"), ("Suite", "tag-read")]),
        },
        Row {
            case: "an empty value, which TagValue's pattern admits".to_string(),
            initial: pairs(&[("Env", "")]),
            applies: Vec::new(),
            expected: pairs(&[("Env", "")]),
        },
        Row {
            case: "a key and a value carrying whitespace".to_string(),
            initial: pairs(&[("two words", "a b"), (" ", "just a space")]),
            applies: Vec::new(),
            expected: pairs(&[("two words", "a b"), (" ", "just a space")]),
        },
        Row {
            case: "a non-ASCII key and value".to_string(),
            initial: pairs(&[("環境", "本番"), ("naïve", "café")]),
            applies: Vec::new(),
            expected: pairs(&[("環境", "本番"), ("naïve", "café")]),
        },
        Row {
            case: "a key and a value at the ceilings TagKey and TagValue publish".to_string(),
            initial: vec![(key_at_ceiling.clone(), value_at_ceiling.clone())],
            applies: Vec::new(),
            expected: vec![(key_at_ceiling, value_at_ceiling)],
        },
        Row {
            case: "the most tags a resource carries".to_string(),
            initial: generated(TAGS_PER_RESOURCE, "created"),
            applies: Vec::new(),
            expected: generated(TAGS_PER_RESOURCE, "created"),
        },
        Row {
            case: "the most tags a resource carries, reached across two calls".to_string(),
            initial: generated(TAGS_PER_RESOURCE - 10, "created"),
            applies: vec![generated(TAGS_PER_RESOURCE, "applied")],
            expected: generated(TAGS_PER_RESOURCE, "applied"),
        },
    ]
}

/// The reporting rule, over both kinds of taggable resource: a tag read answers the tags
/// the resource persists, the resource's own reader agrees, and reading changes nothing.
#[tokio::test]
async fn a_tag_read_reports_the_tags_the_resource_persists() {
    for kind in [Kind::ExperimentTemplate, Kind::Experiment] {
        for row in rows() {
            let case = format!("{} / {}", kind.as_str(), row.case);
            // One emulator per row, so each row judges a resource of its own.
            let router = emulator();
            let resource = new_resource(&router, kind, &case, &tag_map(&row.initial)).await;

            for (index, apply) in row.applies.iter().enumerate() {
                let answer = tag_resource(&router, &resource.arn, &tag_map(apply)).await;
                assert_eq!(
                    answer.status, 204,
                    "case {case:?} call {index}: expected HTTP 204 from TagResource, got {} \
                     with {:?}",
                    answer.status, answer.body
                );
            }

            let expected: BTreeMap<String, String> = row.expected.iter().cloned().collect();
            let answer = list_tags(&router, &resource.arn).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected HTTP 200 from ListTagsForResource, got {} with {:?}",
                answer.status, answer.body
            );
            assert!(
                answer.member("tags").is_object(),
                "case {case:?}: expected a tags map in the answer, got {:?}",
                answer.body
            );
            assert_eq!(
                tags_of(&answer.body, &case),
                expected,
                "case {case:?}: expected the persisted tags, got {:?}",
                answer.member("tags")
            );

            // The one map: the resource's own reader reports exactly what the tag read did.
            assert_eq!(
                tags_from_reader(&router, &resource, &case).await,
                expected,
                "case {case:?}: the resource's own reader must report the same tags as the \
                 tag read, got {:?}",
                answer.member("tags")
            );

            // A read changes nothing a later caller sees.
            let again = list_tags(&router, &resource.arn).await;
            assert_eq!(
                again.status, 200,
                "case {case:?}: expected HTTP 200 from a second ListTagsForResource, got {} \
                 with {:?}",
                again.status, again.body
            );
            assert_eq!(
                tags_of(&again.body, &case),
                expected,
                "case {case:?}: a second tag read must report the same tags, got {:?}",
                again.member("tags")
            );
        }
    }
}

/// An ARN this service cannot address is a bad request, and the resource that IS live is
/// unaffected by having been asked about by the wrong name.
#[tokio::test]
async fn an_arn_this_service_cannot_address_is_a_bad_request() {
    let router = emulator();
    let case = "the suite's own template";
    let stored = pairs(&[("Suite", "tag-read"), ("Env", "test")]);
    let resource = new_resource(&router, Kind::ExperimentTemplate, case, &tag_map(&stored)).await;

    for (case, arn) in unaddressable_arns() {
        let answer = list_tags(&router, &arn).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400 for {arn:?}, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "BadRequestException",
            "case {case:?}: expected BadRequestException for {arn:?}, got {:?}",
            answer.body
        );
    }

    assert_eq!(
        tags_from_reader(&router, &resource, case).await,
        stored.iter().cloned().collect::<BTreeMap<_, _>>(),
        "case {case:?}: a refused tag read must leave the stored tags alone"
    );
}

/// An ARN this service CAN address, naming a resource the store holds no record of, is a
/// miss — the same refusal a tag WRITE of that ARN earns.
#[tokio::test]
async fn an_arn_naming_no_record_is_a_miss() {
    let router = emulator();

    // A template this suite creates and then deletes: the ARN stays well formed, and the
    // record behind it is gone.
    let (deleted_id, deleted_arn) = new_template(&router, "deleted", &json!({})).await;
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

    for (case, arn) in misses {
        let answer = list_tags(&router, &arn).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected HTTP 404 for {arn:?}, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "NotFoundException",
            "case {case:?}: expected NotFoundException for {arn:?}, got {:?}",
            answer.body
        );

        // A read and a write of the same ARN agree about the miss.
        let write = tag_resource(&router, &arn, &tag_map(&pairs(&[("Env", "test")]))).await;
        assert_eq!(
            write.status, answer.status,
            "case {case:?}: a tag write of {arn:?} must answer the same status as the read, \
             got {} with {:?}",
            write.status, write.body
        );
        assert_eq!(
            write.error_code(),
            answer.error_code(),
            "case {case:?}: a tag write of {arn:?} must answer the same error code as the \
             read, got {:?}",
            write.body
        );
    }
}
