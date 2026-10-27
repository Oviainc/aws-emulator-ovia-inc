// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeElasticGpus`, driven over the wire the service actually
//! speaks — `ec2Query`, through the crate's shared [`super::ec2_query`] harness.
//!
//! The rule under test: EC2 SERVES NO REPORT OF AN ELASTIC GRAPHICS ACCELERATOR. The
//! feature reached end of life, no endpoint offers it and no operation of this API can
//! bring one into being, so the call is refused for the absent FUNCTIONALITY — HTTP 400
//! `UnsupportedOperation` with EC2's sentence for functionality the endpoint does not offer
//! — rather than served. Three things that refusal is never:
//!
//! * an empty `<elasticGpuSet/>` with HTTP 200, which would say the feature exists and this
//!   account merely holds no accelerator;
//! * a 501, which would say nobody has written the operation;
//! * `DryRunOperation`, which would say the request would have succeeded.
//!
//! The refusal carries nothing of the request, because nothing a caller could change about
//! the request would make the feature appear — which is exactly what the rows below span:
//! the accelerator ids named (none, one, several, the same one twice, a value that is not an
//! id of that kind), each filter the operation documents and one it does not, a filter
//! carrying no value and one carrying several, the page size at each published bound of its
//! modelled range and between them, a continuation token, `DryRun` both ways, and every
//! member at once in either field order; then the Region the call is signed for, and what
//! the account holds when it is made. No single row is the specification, and the last two
//! dimensions pin that the answer depends on neither the Region nor the store. One further
//! test reads the response's media type, the part of the envelope the recorded response for
//! this operation also names.

use service_com_amazonaws_ec2_amazonec2 as ec2;

use super::ec2_query::{call, call_with_content_type, element, error_code, error_message, filters};
use super::ec2_query_error_envelope::{root_children, ERROR_ENVELOPE_ROOT, ERROR_REQUEST_ID};
use super::volume_lifecycle::p;

/// The operation every row calls.
const ACTION: &str = "DescribeElasticGpus";

/// The error code EC2 refuses a request for functionality it does not offer with.
const UNSUPPORTED_OPERATION: &str = "UnsupportedOperation";

/// The sentence EC2 words that refusal with.
///
/// Fixed text with no variable part: the refusal names no parameter and no resource,
/// because the whole feature is absent rather than one request being wrong. Written out
/// here rather than read from the service, so this suite pins the wording instead of
/// restating whatever the handler happens to carry.
const UNAVAILABLE_MESSAGE: &str = "The functionality you requested is not available in this region.";

/// The media type an EC2 ec2Query response declares, which a refusal of this operation
/// announces like every other response this service renders. The recorded response for
/// this operation names the `charset` parameter, where the protocol's own default is the
/// bare `text/xml`; `super::ec2_query_envelope` owns that rule across operations, and the
/// row below is this operation's.
const RESPONSE_CONTENT_TYPE: &str = "text/xml;charset=UTF-8";

/// The bounds of `DescribeElasticGpusMaxResults`, the modelled `@range` on this operation's
/// `MaxResults` — source:
/// `smithy_query(query="operation",operation="DescribeElasticGpus")`, which reports
/// `range {min: 10, max: 1000}`. A page size outside it is refused by the boundary
/// validator before the handler runs, so the rows stay inside the range: what they vary is
/// the page size a SERVABLE request could ask for.
const MIN_MAX_RESULTS: i32 = 10;
const MAX_MAX_RESULTS: i32 = 1000;

/// The filters `DescribeElasticGpus` documents — source: the `Filters` member's own
/// documentation (`smithy_query(query="doc",shape="DescribeElasticGpusRequest",
/// member="Filters")`), which names `availability-zone`, `elastic-gpu-health`,
/// `elastic-gpu-state`, `elastic-gpu-type` and `instance-id`.
const DOCUMENTED_FILTERS: &[(&str, &str)] = &[
    ("availability-zone", "us-west-2a"),
    ("elastic-gpu-health", "OK"),
    ("elastic-gpu-state", "ATTACHED"),
    ("elastic-gpu-type", "eg1.medium"),
    ("instance-id", "i-0123456789abcdef0"),
];

/// An accelerator id in the form EC2 renders one in, for the rows that name ids.
const ELASTIC_GPU_ID: &str = "egpu-0123456789abcdef0";

/// The AMI the instance-holding row launches from, in the long id form.
const AMI: &str = "ami-0123456789abcdef0";

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// Assert that `status` and `body` are the refusal of the absent feature, and that they
/// report no accelerator and no page of any kind.
fn assert_refused_for_the_absent_feature(case: &str, status: u16, body: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(body),
        UNSUPPORTED_OPERATION,
        "case {case:?}: expected the code {UNSUPPORTED_OPERATION}, got {body}"
    );
    assert_eq!(
        error_message(body),
        UNAVAILABLE_MESSAGE,
        "case {case:?}: expected the refusal to be worded for the absent functionality, \
         got {:?} in {body}",
        error_message(body)
    );
    // Not an empty report dressed as a refusal either: none of the result shape's elements
    // is in the body, so nothing says "the feature exists and you hold none of it".
    for reported in [
        "elasticGpuSet",
        "elasticGpuId",
        "elasticGpuState",
        "nextToken",
        "maxResults",
    ] {
        assert!(
            element(body, reported).is_none(),
            "case {case:?}: a refusal reports no <{reported}>, got {body}"
        );
    }
    // And it is a refusal of this service's own envelope: exactly one `<RequestID>`, the
    // element `super::ec2_query_error_envelope` owns the rule for.
    let request_ids = root_children(case, body, ERROR_ENVELOPE_ROOT, ERROR_REQUEST_ID);
    assert_eq!(
        request_ids.len(),
        1,
        "case {case:?}: expected exactly one <{ERROR_REQUEST_ID}> naming the answered \
         request, got {request_ids:?} in {body}"
    );
    assert!(
        !request_ids[0].trim().is_empty(),
        "case {case:?}: expected <{ERROR_REQUEST_ID}> to name the answered request, got \
         {request_ids:?} in {body}"
    );
}

/// Everything the store holds that a served report — or a handler that wrote while reading
/// — would move: the instances the account has, and the caller-supplied bytes charged
/// against the store. Compared before and against after, so a refusal that quietly wrote
/// something is visible.
fn stored(engine: &engine::InMemoryEngine) -> (usize, usize) {
    let store = engine.state::<ec2::state::ComAmazonawsEc2Amazonec2State>();
    let guard = store.lock();
    (guard.instances.len(), guard.stored_bytes())
}

/// `(case, the parameters)` for every shape a `DescribeElasticGpus` request can take, each
/// row differing along what the rule generalises over.
fn request_rows() -> Vec<(String, Params)> {
    let mut rows: Vec<(String, Params)> = vec![
        ("a request that narrows nothing".to_string(), Vec::new()),
        (
            "one accelerator id".to_string(),
            vec![p("ElasticGpuId.1", ELASTIC_GPU_ID)],
        ),
        (
            "several accelerator ids".to_string(),
            vec![
                p("ElasticGpuId.1", ELASTIC_GPU_ID),
                p("ElasticGpuId.2", "egpu-00000000000000001"),
                p("ElasticGpuId.3", "egpu-fffffffffffffffff"),
            ],
        ),
        (
            "a value that is not an accelerator id at all".to_string(),
            vec![p("ElasticGpuId.1", "not-an-id")],
        ),
        (
            "an empty accelerator id".to_string(),
            vec![p("ElasticGpuId.1", "")],
        ),
        (
            "the same accelerator id named twice".to_string(),
            vec![
                p("ElasticGpuId.1", ELASTIC_GPU_ID),
                p("ElasticGpuId.2", ELASTIC_GPU_ID),
            ],
        ),
        (
            "a continuation token from a page nobody was served".to_string(),
            vec![p("NextToken", "AAAAAAAAAAE=")],
        ),
        (
            "a continuation token that is not a token".to_string(),
            vec![p("NextToken", "%%%not-a-token%%%")],
        ),
        (
            "the DryRun probe, which asks whether the request would have succeeded"
                .to_string(),
            vec![p("DryRun", "true")],
        ),
        (
            "DryRun asked for and declined".to_string(),
            vec![p("DryRun", "false")],
        ),
    ];

    // The page size at each published bound of the modelled range and one value between
    // them: a page of a report that is not served is still not served.
    for max_results in [MIN_MAX_RESULTS, 500, MAX_MAX_RESULTS] {
        rows.push((
            format!("a page size of {max_results}"),
            vec![p("MaxResults", max_results.to_string())],
        ));
    }

    // Each filter the operation documents, one at a time, and one it does not document:
    // an unsupported filter is not reported as "no results" either, because no result set
    // is reached at all.
    for (name, value) in DOCUMENTED_FILTERS {
        rows.push((
            format!("the documented filter {name}"),
            filters(&[(name, &[value])]),
        ));
    }
    rows.push((
        "a filter this operation does not document".to_string(),
        filters(&[("tag:Name", &["graphics"])]),
    ));
    // The number of values one filter carries is another thing a caller can vary: none, and
    // several at once. Neither turns the absent feature into a result set to narrow.
    rows.push((
        "a documented filter carrying no value at all".to_string(),
        filters(&[("elastic-gpu-state", &[])]),
    ));
    rows.push((
        "a documented filter carrying several values".to_string(),
        filters(&[(
            "elastic-gpu-type",
            &["eg1.medium", "eg1.large", "eg1.xlarge", "eg1.2xlarge"],
        )]),
    ));
    rows.push((
        "every documented filter at once".to_string(),
        filters(
            &DOCUMENTED_FILTERS
                .iter()
                .map(|(name, value)| (*name, std::slice::from_ref(value)))
                .collect::<Vec<_>>(),
        ),
    ));

    // Every member at once, which is the request a client that paged through a report it
    // had been served would send.
    let mut everything = vec![
        p("ElasticGpuId.1", ELASTIC_GPU_ID),
        p("MaxResults", MAX_MAX_RESULTS.to_string()),
        p("NextToken", "AAAAAAAAAAI="),
        p("DryRun", "true"),
    ];
    everything.extend(filters(&[("elastic-gpu-state", &["ATTACHED"])]));
    rows.push(("every member at once".to_string(), everything.clone()));

    // The same members, sent in the reverse order: a form field's position is not part of
    // the request either, so the answer cannot depend on the order they arrive in.
    everything.reverse();
    rows.push((
        "every member at once, its fields in the reverse order".to_string(),
        everything,
    ));

    rows
}

/// Every shape of request is refused for the absent feature, and none of them is served,
/// probed or answered with a page.
#[tokio::test]
async fn every_describe_request_is_refused_for_the_absent_feature() {
    let (router, engine) = super::ec2_query::emulator();
    let baseline = stored(&engine);

    for (case, params) in request_rows() {
        let (status, body) = call(&router, "us-west-2", ACTION, &params).await;
        assert_refused_for_the_absent_feature(&case, status, &body);
    }

    assert_eq!(
        stored(&engine),
        baseline,
        "a reporting operation writes nothing: expected the store to end where it started \
         at {baseline:?}, got {:?}",
        stored(&engine)
    );
}

/// The answer does not depend on the Region the call is signed for: the feature is offered
/// in none of them, so the same refusal comes back from every endpoint.
#[tokio::test]
async fn the_refusal_is_the_same_in_every_region() {
    let (router, _engine) = super::ec2_query::emulator();

    for region in [
        "us-east-1",
        "us-west-2",
        "eu-central-1",
        "ap-southeast-2",
        "sa-east-1",
    ] {
        for (label, params) in [
            ("narrowing nothing", Vec::new()),
            ("naming an accelerator", vec![p("ElasticGpuId.1", ELASTIC_GPU_ID)]),
        ] {
            let case = format!("{label}, signed for {region}");
            let (status, body) = call(&router, region, ACTION, &params).await;
            assert_refused_for_the_absent_feature(&case, status, &body);
        }
    }
}

/// The refusal is announced as the XML media type the rest of this service's responses
/// are, `charset` parameter and all — a refused call is still an EC2 response, so a client
/// decodes its envelope the same way whether the operation served anything or not.
#[tokio::test]
async fn the_refusal_declares_the_xml_media_type_with_its_charset() {
    let (router, _engine) = super::ec2_query::emulator();

    // `(case, the parameters)`: the shapes that would have narrowed a served report, so the
    // media type is read off more than the one empty request.
    for (case, params) in [
        ("narrowing nothing".to_string(), Vec::new()),
        (
            "naming an accelerator".to_string(),
            vec![p("ElasticGpuId.1", ELASTIC_GPU_ID)],
        ),
        (
            "a page size and a filter".to_string(),
            {
                let mut params = vec![p("MaxResults", MAX_MAX_RESULTS.to_string())];
                params.extend(filters(&[("elastic-gpu-type", &["eg1.large"])]));
                params
            },
        ),
    ] {
        let (status, content_type, body) =
            call_with_content_type(&router, "eu-west-1", ACTION, &params).await;
        assert_refused_for_the_absent_feature(&case, status, &body);
        assert_eq!(
            content_type.as_deref(),
            Some(RESPONSE_CONTENT_TYPE),
            "case {case:?}: expected the refusal to declare {RESPONSE_CONTENT_TYPE:?}, got \
             {content_type:?} with body {body}"
        );
    }
}

/// Nor does it depend on what the account holds: with an instance of its own running — the
/// resource an accelerator would have been attached to — the report is still not served,
/// and that instance is left exactly as it was.
#[tokio::test]
async fn the_refusal_is_the_same_whatever_the_account_holds() {
    let (router, engine) = super::ec2_query::emulator();
    let region = "us-west-2";

    let (status, body) = call(
        &router,
        region,
        "RunInstances",
        &[p("ImageId", AMI), p("MinCount", "1"), p("MaxCount", "1")],
    )
    .await;
    assert_eq!(
        status, 200,
        "the instance the accelerator rows name must launch, got {status}: {body}"
    );
    let instance_id = element(&body, "instanceId")
        .expect("RunInstances must report the instance it minted");

    let held = stored(&engine);
    assert_eq!(
        held.0, 1,
        "the account must hold the one launched instance, got {held:?}"
    );

    for (label, params) in [
        ("an account holding one instance".to_string(), Vec::new()),
        (
            "the filter naming that instance".to_string(),
            filters(&[("instance-id", &[instance_id.as_str()])]),
        ),
        (
            "an accelerator id beside that instance".to_string(),
            vec![p("ElasticGpuId.1", ELASTIC_GPU_ID)],
        ),
    ] {
        let (status, body) = call(&router, region, ACTION, &params).await;
        assert_refused_for_the_absent_feature(&label, status, &body);
    }

    assert_eq!(
        stored(&engine),
        held,
        "the refused report leaves the account's instances and the store's charge as it \
         found them, expected {held:?}, got {:?}",
        stored(&engine)
    );

    // The instance is still reportable by the operation that does report it, so the
    // refusal above is this operation's and not the service losing its state.
    let (status, body) = call(
        &router,
        region,
        "DescribeInstances",
        &[p("InstanceId.1", &instance_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeInstances must still report the launched instance, got {status}: {body}"
    );
    assert_eq!(
        element(&body, "instanceId").as_deref(),
        Some(instance_id.as_str()),
        "DescribeInstances must still report {instance_id}, got {body}"
    );
}

/// The same request asked twice is answered the same way twice: the refusal consumes
/// nothing and remembers nothing, so there is no first call that behaves differently from
/// the second.
#[tokio::test]
async fn the_refusal_repeats_itself() {
    let (router, engine) = super::ec2_query::emulator();
    let baseline = stored(&engine);

    for (case, params) in [
        ("narrowing nothing".to_string(), Vec::new()),
        (
            "naming an accelerator and a page size".to_string(),
            vec![
                p("ElasticGpuId.1", ELASTIC_GPU_ID),
                p("MaxResults", MIN_MAX_RESULTS.to_string()),
            ],
        ),
    ] {
        let mut answers = Vec::new();
        for attempt in 0..3 {
            let attempted = format!("{case} (attempt {attempt})");
            let (status, body) = call(&router, "us-east-1", ACTION, &params).await;
            assert_refused_for_the_absent_feature(&attempted, status, &body);
            answers.push((status, error_code(&body), error_message(&body)));
        }
        assert!(
            answers.windows(2).all(|pair| pair[0] == pair[1]),
            "case {case:?}: every attempt is answered the same way, got {answers:?}"
        );
    }

    assert_eq!(
        stored(&engine),
        baseline,
        "repeating the call stores nothing either, expected {baseline:?}, got {:?}",
        stored(&engine)
    );
}
