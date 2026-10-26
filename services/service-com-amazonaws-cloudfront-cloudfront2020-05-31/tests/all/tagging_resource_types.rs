// SPDX-License-Identifier: CC0-1.0

//! Which RESOURCE TYPES CloudFront's tagging surface accepts, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rule under test: the tagging surface accepts only the CloudFront
//! resource types whose resources carry tags, so an ARN naming any other type is refused as
//! a bad PARAMETER — `InvalidArgument` (400), naming the resource type the submitted ARN
//! carries — decided BEFORE any lookup, which is what leaves `NoSuchResource` (404) to mean
//! "a type this surface accepts, whose resource is not there". The three operations of the
//! surface — `TagResource`, `ListTagsForResource` and `UntagResource` — all take the same
//! `Resource` ARN, so all three answer the same way.
//!
//! The rows differ along what that rule generalises over: the resource type the ARN names
//! (nine types the surface does not accept, and every type it does), whether a resource of
//! that type EXISTS (a published cache policy attached to a live distribution's default
//! cache behaviour does), the ARN's partition, and which of the three operations asks.
//!
//! The harness (the router, the request helpers, the element scanner and the tag helpers)
//! is the lifecycle and tagging suites', reused rather than copied.

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    MANAGED_CACHE_POLICIES, TAGGABLE_RESOURCE_TYPES,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{call, create_distribution, emulator, Answer, ACCOUNT_ID, API_VERSION};
use crate::distribution_tags::{list_tags_for_resource, reported_tags, tag, ReportedTags};
use crate::tag_resource::tag_resource;
use crate::untag_resource::untag_resource;

/// The resource types the tagging surface does NOT accept, as the resource part of an ARN
/// spells each of them.
///
/// Read off the model: each of these is created by an operation that takes no tags at all —
/// `CreateCachePolicy`, `CreateOriginRequestPolicy`, `CreateResponseHeadersPolicy`,
/// `CreateOriginAccessControl`, `CreatePublicKey`, `CreateRealtimeLogConfig`,
/// `CreateContinuousDeploymentPolicy`, `CreateCloudFrontOriginAccessIdentity` and
/// `CreateFieldLevelEncryptionConfig` (`smithy_query(query="operation", operation="…")`),
/// in contrast with the creates of every type in [`TAGGABLE_RESOURCE_TYPES`].
const UNTAGGABLE_RESOURCE_TYPES: [&str; 9] = [
    "cache-policy",
    "continuous-deployment-policy",
    "field-level-encryption-config",
    "origin-access-control",
    "origin-access-identity",
    "origin-request-policy",
    "public-key",
    "realtime-log-config",
    "response-headers-policy",
];

/// The error message the answer states.
fn message(answer: &Answer) -> &str {
    answer.root().find_text("Message").unwrap_or_default()
}

/// `POST /2020-05-31/streaming-distribution` with a minimal `StreamingDistributionConfig`.
///
/// The members are the ones the configuration requires, each carrying a value its own
/// constraint admits: what this suite varies is the ARN a tagging call then submits, not
/// the configuration.
async fn create_streaming_distribution(router: &MultiServiceRouter, reference: &str) -> Answer {
    let body = format!(
        "<StreamingDistributionConfig \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <CallerReference>{reference}</CallerReference>\
         <S3Origin><DomainName>media.s3.amazonaws.com</DomainName>\
         <OriginAccessIdentity></OriginAccessIdentity></S3Origin>\
         <Comment>tagging resource types suite</Comment>\
         <TrustedSigners><Enabled>false</Enabled><Quantity>0</Quantity></TrustedSigners>\
         <Enabled>true</Enabled></StreamingDistributionConfig>"
    );
    call(
        router,
        "POST",
        &format!("/{API_VERSION}/streaming-distribution"),
        Some(&body),
        None,
    )
    .await
}

/// `GET /2020-05-31/cache-policy/{Id}` — the control that proves a row's resource exists.
async fn get_cache_policy(router: &MultiServiceRouter, id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/cache-policy/{id}"),
        None,
        None,
    )
    .await
}

/// A `DistributionConfig` whose default cache behaviour attaches the cache policy `id`, so
/// the policy a row names is in use by a live distribution.
fn config_attaching(reference: &str, cache_policy_id: &str) -> String {
    format!(
        "<DistributionConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <CallerReference>{reference}</CallerReference>\
         <Origins><Quantity>1</Quantity><Items>\
         <Origin><Id>primary</Id><DomainName>origin.example.com</DomainName></Origin>\
         </Items></Origins>\
         <DefaultCacheBehavior><TargetOriginId>primary</TargetOriginId>\
         <ViewerProtocolPolicy>allow-all</ViewerProtocolPolicy>\
         <CachePolicyId>{cache_policy_id}</CachePolicyId></DefaultCacheBehavior>\
         <Comment>{reference}</Comment><Enabled>true</Enabled></DistributionConfig>"
    )
}

/// Ask all three operations of the tagging surface about `arn`, in the order a client that
/// tags, reads and untags would.
async fn ask_each_operation(router: &MultiServiceRouter, arn: &str) -> Vec<(&'static str, Answer)> {
    vec![
        (
            "TagResource",
            tag_resource(router, arn, Some(&[tag("environment", Some("test"))])).await,
        ),
        ("ListTagsForResource", list_tags_for_resource(router, arn).await),
        (
            "UntagResource",
            untag_resource(router, arn, Some(&["environment".to_string()])).await,
        ),
    ]
}

#[tokio::test]
async fn a_resource_type_the_tagging_surface_does_not_accept_is_an_invalid_argument() {
    let router = emulator();

    // The row the rule has to hold for even though the resource EXISTS: a published cache
    // policy, attached to a live distribution's default cache behaviour.
    let published = MANAGED_CACHE_POLICIES
        .first()
        .expect("the published cache policy catalogue is not empty");
    let read = get_cache_policy(&router, published.id).await;
    assert_eq!(
        read.status, 200,
        "the cache policy {} must exist for this row to be about its TYPE, got {} ({})",
        published.id, read.status, read.raw
    );
    let attached = create_distribution(
        &router,
        &config_attaching("ref-tagging-types", published.id),
    )
    .await;
    assert_eq!(
        attached.status, 201,
        "the distribution attaching {} must be created, got {} ({})",
        published.id, attached.status, attached.raw
    );

    // Every row is a well-formed CloudFront ARN (`ResourceARN`'s pattern is satisfied), so
    // what makes each a refusal is the resource TYPE it names. The last two rows pair a
    // type the surface does not accept with the other partition the pattern admits, and
    // with a resource that exists.
    let mut rows: Vec<(String, String, &str)> = UNTAGGABLE_RESOURCE_TYPES
        .iter()
        .map(|kind| {
            (
                format!("a {kind} the store holds none of"),
                format!("arn:aws:cloudfront::{ACCOUNT_ID}:{kind}/absent-resource"),
                *kind,
            )
        })
        .collect();
    rows.push((
        "a cache policy that exists and is attached to a distribution".to_string(),
        format!("arn:aws:cloudfront::{ACCOUNT_ID}:cache-policy/{}", published.id),
        "cache-policy",
    ));
    rows.push((
        "a response headers policy in the other partition".to_string(),
        format!("arn:aws-cn:cloudfront::{ACCOUNT_ID}:response-headers-policy/absent-resource"),
        "response-headers-policy",
    ));

    for (case, arn, kind) in &rows {
        let expected = format!("The parameter resource type: {kind} is invalid.");
        for (operation, answer) in ask_each_operation(&router, arn).await {
            assert_eq!(
                answer.status, 400,
                "case {case}/{operation}: expected HTTP 400, got {} ({})",
                answer.status, answer.raw
            );
            assert_eq!(
                answer.error_code(),
                "InvalidArgument",
                "case {case}/{operation}: expected InvalidArgument, got {:?} ({})",
                answer.error_code(),
                answer.raw
            );
            assert_eq!(
                message(&answer),
                expected,
                "case {case}/{operation}: expected message {expected:?}, got {:?}",
                message(&answer)
            );
        }
    }

    // The refusals touched nothing: the cache policy is still readable, so none of them
    // was an existence answer in disguise.
    let after = get_cache_policy(&router, published.id).await;
    assert_eq!(
        after.status, 200,
        "case the cache policy after the refusals: expected HTTP 200, got {} ({})",
        after.status, after.raw
    );
}

#[tokio::test]
async fn a_type_the_tagging_surface_accepts_is_answered_by_existence() {
    let router = emulator();

    // Every type the surface accepts, with an ARN naming no resource of it: the answer is
    // the EXISTENCE refusal, never the parameter one, which is what distinguishes the two.
    for kind in TAGGABLE_RESOURCE_TYPES {
        let arn = format!("arn:aws:cloudfront::{ACCOUNT_ID}:{kind}/absent-resource");
        for (operation, answer) in ask_each_operation(&router, &arn).await {
            assert_eq!(
                answer.status, 404,
                "case {kind}/{operation}: expected HTTP 404, got {} ({})",
                answer.status, answer.raw
            );
            assert_eq!(
                answer.error_code(),
                "NoSuchResource",
                "case {kind}/{operation}: expected NoSuchResource, got {:?} ({})",
                answer.error_code(),
                answer.raw
            );
        }
    }

    // And a type the surface accepts whose resource IS there round-trips: a streaming
    // distribution — one of the types the model makes taggable, through
    // `CreateStreamingDistributionWithTags` — takes tags, reports them back and loses them
    // again.
    let created = create_streaming_distribution(&router, "ref-tagging-streaming").await;
    assert_eq!(
        created.status, 201,
        "case the streaming distribution: expected HTTP 201, got {} ({})",
        created.status, created.raw
    );
    let arn = created
        .root()
        .find_text("ARN")
        .unwrap_or_default()
        .to_string();
    assert!(
        arn.contains(":streaming-distribution/"),
        "case the streaming distribution: expected a streaming distribution ARN, got {arn:?}"
    );

    // A resource that carries no tags yet reports the empty set rather than a refusal.
    let empty = list_tags_for_resource(&router, &arn).await;
    assert_eq!(
        empty.status, 200,
        "case the streaming distribution before tagging: expected HTTP 200, got {} ({})",
        empty.status, empty.raw
    );
    assert_eq!(
        reported_tags(empty.root()),
        ReportedTags::new(),
        "case the streaming distribution before tagging: expected no tags, got {}",
        empty.raw
    );

    let tagged = tag_resource(
        &router,
        &arn,
        Some(&[tag("environment", Some("test")), tag("owner", None)]),
    )
    .await;
    assert_eq!(
        tagged.status, 204,
        "case the streaming distribution tagged: expected HTTP 204, got {} ({})",
        tagged.status, tagged.raw
    );
    let read = list_tags_for_resource(&router, &arn).await;
    assert_eq!(
        read.status, 200,
        "case the streaming distribution read back: expected HTTP 200, got {} ({})",
        read.status, read.raw
    );
    let expected: ReportedTags = [
        ("environment".to_string(), Some("test".to_string())),
        ("owner".to_string(), None),
    ]
    .into_iter()
    .collect();
    assert_eq!(
        reported_tags(read.root()),
        expected,
        "case the streaming distribution read back: expected {expected:?}, got {}",
        read.raw
    );

    let untagged = untag_resource(&router, &arn, Some(&["environment".to_string()])).await;
    assert_eq!(
        untagged.status, 204,
        "case the streaming distribution untagged: expected HTTP 204, got {} ({})",
        untagged.status, untagged.raw
    );
    let after = list_tags_for_resource(&router, &arn).await;
    let remaining: ReportedTags = [("owner".to_string(), None)].into_iter().collect();
    assert_eq!(
        reported_tags(after.root()),
        remaining,
        "case the streaming distribution untagged: expected {remaining:?}, got {}",
        after.raw
    );
}
