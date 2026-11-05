// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the BODY and the STATUS of every client-error refusal this service
//! raises, driven over the wire (awsJson1_0, `x-amz-target: B2BI.<Operation>`) through the
//! same router the emulator boots, so each assertion is about what an SDK client reads.
//!
//! Four rules are under test, each over a TABLE whose rows differ along what the rule
//! generalises over:
//!
//! * every modelled client error of this service travels at one client-error status with
//!   its class named by the FULLY-QUALIFIED shape id of the namespace this service's wire
//!   responses use, and its message in the member its own shape declares — `Message` on
//!   `ValidationException`, `message` on `ResourceNotFoundException`
//!   (`smithy_query(query="shape",shape="ValidationException,ResourceNotFoundException")`).
//!   The rows span both classes over every operation that raises one.
//! * a capability's input and output locations must be Amazon S3 buckets THIS account
//!   owns, and a configuration naming one it does not is refused as invalid input before
//!   any reference in that configuration is resolved. The rows span which of the two
//!   buckets is not owned, a location naming no bucket at all, and a request that is wrong
//!   in both ways at once.
//! * an Amazon S3 object a request names and object storage does not hold is reported by
//!   its `s3://` URI, with no dependency error code in the sentence — the same wording
//!   whichever operation read it. The rows span the operation, a missing key, a missing
//!   bucket and the shape of the key.
//! * a transformer job that resolves to nothing is reported by the job id the request
//!   carried and the account the job would have belonged to. The rows span the job id and
//!   whether the transformer it was paired with exists.
//!
//! The tagging operations' own ARN classification is covered by `tagging.rs`,
//! `tag_resource.rs` and `untag_resource.rs`; this suite adds only the cross-operation
//! claim that all three answer it identically.

use serde_json::{json, Value};
use service_com_amazonaws_b2bi_b2bi::state::{invalid_arn, s3_object_not_found, INVALID_INPUT_ARN};

use crate::onboarding_loop::{
    b2bi_router, call, error_code, error_type, given_bucket, given_capability_buckets,
    given_transformer, text, ACCOUNT, CAPABILITY_INPUT_BUCKET, CAPABILITY_OUTPUT_BUCKET,
    SIGNING_REGION,
};

/// The one status every client-error refusal of this service carries on this envelope
/// (`scaffolded::NOT_FOUND_STATUS` restates the model's 404 onto it).
const CLIENT_ERROR_STATUS: u16 = 400;

/// The qualified shape id this service's recorded error bodies name a missing resource by.
const QUALIFIED_NOT_FOUND: &str = "com.amazonaws.visby#ResourceNotFoundException";

/// The qualified shape id this service's recorded error bodies name a refused input by.
const QUALIFIED_VALIDATION: &str = "com.amazonaws.visby#ValidationException";

/// Well-formed identifiers no store holds: each satisfies the modelled `len 1..64` /
/// `^[a-zA-Z0-9_-]+$` every b2bi id carries, so what refuses them is the lookup.
const UNKNOWN_PROFILE: &str = "p-0123456789abcdef0";
const UNKNOWN_CAPABILITY: &str = "ca-0123456789abcdef0";
const UNKNOWN_PARTNERSHIP: &str = "ps-0123456789abcdef0";
const UNKNOWN_TRANSFORMER: &str = "tr-0123456789abcdef0";

/// A `transformerJobId` of the modelled `len 25..25` / `^[a-zA-Z0-9_-]+$`
/// (`smithy_query(query="shape",shape="TransformerJobId")`), so it reaches the handler.
const UNKNOWN_JOB: &str = "0123456789abcdef012345678";

/// A well-formed b2bi ARN for `kind`/`id` in the region these tests sign for.
fn b2bi_arn(kind: &str, id: &str) -> String {
    format!("arn:aws:b2bi:{SIGNING_REGION}:{ACCOUNT}:{kind}/{id}")
}

/// A `CapabilityConfiguration` naming `transformer_id` whose locations are `input` and
/// `output` — `None` for a location that names no bucket at all.
fn configuration_over(transformer_id: &str, input: Option<&str>, output: Option<&str>) -> Value {
    let location = |bucket: Option<&str>| match bucket {
        Some(bucket) => json!({ "bucketName": bucket, "key": "edi/" }),
        None => json!({ "key": "edi/" }),
    };
    json!({
        "edi": {
            "type": { "x12Details": { "transactionSet": "X12_850", "version": "VERSION_4010" } },
            "inputLocation": location(input),
            "outputLocation": location(output),
            "transformerId": transformer_id
        }
    })
}

/// A `TestParsing` request for the object at `bucket`/`key`.
fn test_parsing_request(bucket: &str, key: &str) -> Value {
    json!({
        "inputFile": { "bucketName": bucket, "key": key },
        "fileFormat": "JSON",
        "ediType": { "x12Details": { "transactionSet": "X12_850", "version": "VERSION_4010" } }
    })
}

/// RULE: every modelled client-error refusal of this service answers at the one
/// client-error status this envelope carries, names its class with the fully-qualified
/// shape id its recorded bodies use, and carries its sentence in the member its OWN error
/// shape declares — so a client branching on the class takes the right branch and a client
/// reading the message finds it.
#[tokio::test]
async fn every_client_error_names_its_class_by_qualified_shape_id() {
    let router = b2bi_router();
    given_capability_buckets(&router).await;
    let transformer_id = given_transformer(&router, "qualified-type-transformer").await;

    // Each row: the case, the operation, the request, the `__type` the body must carry, and
    // the member that shape spells its message in.
    let cases: Vec<(&str, &str, Value, &str, &str)> = vec![
        (
            "GetProfile for a profileId nobody holds",
            "GetProfile",
            json!({ "profileId": UNKNOWN_PROFILE }),
            QUALIFIED_NOT_FOUND,
            "message",
        ),
        (
            "GetCapability for a capabilityId nobody holds",
            "GetCapability",
            json!({ "capabilityId": UNKNOWN_CAPABILITY }),
            QUALIFIED_NOT_FOUND,
            "message",
        ),
        (
            "DeleteCapability for a capabilityId nobody holds",
            "DeleteCapability",
            json!({ "capabilityId": UNKNOWN_CAPABILITY }),
            QUALIFIED_NOT_FOUND,
            "message",
        ),
        (
            "GetPartnership for a partnershipId nobody holds",
            "GetPartnership",
            json!({ "partnershipId": UNKNOWN_PARTNERSHIP }),
            QUALIFIED_NOT_FOUND,
            "message",
        ),
        (
            "DeletePartnership for a partnershipId nobody holds",
            "DeletePartnership",
            json!({ "partnershipId": UNKNOWN_PARTNERSHIP }),
            QUALIFIED_NOT_FOUND,
            "message",
        ),
        (
            "CreatePartnership naming a profile nobody holds",
            "CreatePartnership",
            json!({
                "profileId": UNKNOWN_PROFILE,
                "name": "partner-under-an-unknown-profile",
                "email": "partner@example.com",
                "capabilities": []
            }),
            QUALIFIED_NOT_FOUND,
            "message",
        ),
        (
            "GetTransformerJob for a job nobody holds",
            "GetTransformerJob",
            json!({ "transformerId": UNKNOWN_TRANSFORMER, "transformerJobId": UNKNOWN_JOB }),
            QUALIFIED_NOT_FOUND,
            "message",
        ),
        (
            "TestParsing whose inputFile names no stored object",
            "TestParsing",
            test_parsing_request(CAPABILITY_INPUT_BUCKET, "absent/850.edi"),
            QUALIFIED_NOT_FOUND,
            "message",
        ),
        (
            "ListTagsForResource with an ARN that resolves to nothing",
            "ListTagsForResource",
            json!({ "ResourceARN": b2bi_arn("profile", UNKNOWN_PROFILE) }),
            QUALIFIED_VALIDATION,
            "Message",
        ),
        (
            "TagResource with an ARN that resolves to nothing",
            "TagResource",
            json!({
                "ResourceARN": b2bi_arn("profile", UNKNOWN_PROFILE),
                "Tags": [{ "Key": "env", "Value": "prod" }]
            }),
            QUALIFIED_VALIDATION,
            "Message",
        ),
        (
            "UntagResource with an ARN that resolves to nothing",
            "UntagResource",
            json!({
                "ResourceARN": b2bi_arn("profile", UNKNOWN_PROFILE),
                "TagKeys": ["env"]
            }),
            QUALIFIED_VALIDATION,
            "Message",
        ),
        (
            "CreateCapability over a bucket this account does not own",
            "CreateCapability",
            json!({
                "name": "capability-over-an-unowned-bucket",
                "type": "edi",
                "configuration": configuration_over(
                    &transformer_id,
                    Some("a-bucket-this-account-does-not-own"),
                    Some(CAPABILITY_OUTPUT_BUCKET)
                )
            }),
            QUALIFIED_VALIDATION,
            "Message",
        ),
        (
            "ListProfiles with a nextToken this service cannot use",
            "ListProfiles",
            json!({ "nextToken": "not-a-token" }),
            QUALIFIED_VALIDATION,
            "Message",
        ),
    ];

    for (case, operation, request, qualified, member) in cases {
        let (status, body) = call(&router, operation, request).await;
        assert_eq!(
            status, CLIENT_ERROR_STATUS,
            "case {case:?}: a client-error refusal of this service is \
             {CLIENT_ERROR_STATUS}, got {status} {body}"
        );
        assert_eq!(
            error_type(&body),
            qualified,
            "case {case:?}: expected __type {qualified:?}, got {body}"
        );
        // The qualified id still names the class a client resolves out of it.
        assert_eq!(
            error_code(&body),
            qualified.split('#').next_back().unwrap_or_default(),
            "case {case:?}: the shape name in __type must be the error class, got {body}"
        );
        assert!(
            !text(&body, member).is_empty(),
            "case {case:?}: the sentence belongs in the {member:?} member, got {body}"
        );
        // ...and in that member ONLY: the two classes spell it differently, so a body
        // carrying both spellings would let one client read nothing.
        let other = if member == "Message" {
            "message"
        } else {
            "Message"
        };
        assert!(
            body.get(other).is_none(),
            "case {case:?}: the body must not also carry {other:?}, got {body}"
        );
    }
}

/// The two SYNTHETIC error shapes this crate's error channel carries are not in this
/// service's model, so neither HAS a shape id of this service to qualify, and both travel
/// under their bare names: the refusal the boundary validator answers a request that does
/// not satisfy a modelled constraint with, and the 501 an operation nobody has implemented
/// answers.
///
/// For the 501 that is the requirement — nothing in this service's model describes a
/// request it has not implemented. For the boundary refusal it is a KNOWN LIMITATION, not
/// AWS's behaviour: the real service answers input validation with its own
/// `com.amazonaws.visby#ValidationException` and a `Message`, while the framework renders
/// that refusal from a shape in its own namespace with a lowercase `message`, outside the
/// seam a service crate can reach. It is pinned here so the divergence is observed and
/// deliberate rather than silent, and so the fix that gives the framework refusal this
/// service's own shape has a row to change.
#[tokio::test]
async fn the_synthetic_error_shapes_carry_no_shape_id_of_this_service() {
    let router = b2bi_router();

    // Each row: the case, the operation, the request, the status and the bare `__type`.
    let cases: Vec<(&str, &str, Value, u16, &str)> = vec![
        (
            // `profileId` is `@required`, so the boundary validator refuses this before
            // any handler runs.
            "a request missing a required member",
            "GetProfile",
            json!({}),
            CLIENT_ERROR_STATUS,
            "ValidationException",
        ),
        (
            // No operation of this service carries the scaffold marker any more, and the
            // synthetic 501 is still what a request this emulator has no ENGINE for is
            // answered with: an XML input document needs an XSLT engine `TestMapping` does
            // not have (see `tests/all/test_mapping.rs`, which asserts that refusal itself).
            "an operation nobody has implemented",
            "TestMapping",
            json!({
                "inputFileContent": "<transactionSets><set/></transactionSets>",
                "mappingTemplate": "$",
                "fileFormat": "XML"
            }),
            501,
            "NotImplemented",
        ),
    ];

    for (case, operation, request, expected_status, bare) in cases {
        let (status, body) = call(&router, operation, request).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status}, got {status} {body}"
        );
        assert_eq!(
            error_type(&body),
            bare,
            "case {case:?}: expected the bare __type {bare:?}, got {body}"
        );
    }
}

/// RULE: a capability READS the documents its input location holds and WRITES the
/// transformed ones to its output location, so both buckets have to be ones this account
/// owns; a configuration naming a bucket it does not own is refused as invalid input,
/// before the references that configuration names are resolved, and nothing is minted.
///
/// The rows span which location is at fault (input, output, both), a location naming no
/// bucket at all, the positive case, and a request that names BOTH an unowned bucket and a
/// transformer nobody holds — which answers the input it got wrong rather than the
/// reference it could not find.
#[tokio::test]
async fn a_capability_is_refused_unless_this_account_owns_both_of_its_buckets() {
    let router = b2bi_router();
    let transformer_id = given_transformer(&router, "bucket-ownership-transformer").await;
    // Only these two buckets exist, so every other name below is one nobody owns.
    given_capability_buckets(&router).await;
    given_bucket(&router, "a-third-bucket-this-account-owns").await;

    let unowned = "a-bucket-this-account-does-not-own";

    // Each row: the case, the transformerId, the input bucket, the output bucket, and
    // whether the capability may be created.
    #[allow(clippy::type_complexity)]
    let cases: Vec<(&str, &str, Option<&str>, Option<&str>, bool)> = vec![
        (
            "both buckets owned",
            transformer_id.as_str(),
            Some(CAPABILITY_INPUT_BUCKET),
            Some(CAPABILITY_OUTPUT_BUCKET),
            true,
        ),
        (
            "one bucket for both locations",
            transformer_id.as_str(),
            Some("a-third-bucket-this-account-owns"),
            Some("a-third-bucket-this-account-owns"),
            true,
        ),
        (
            "the input bucket is not owned",
            transformer_id.as_str(),
            Some(unowned),
            Some(CAPABILITY_OUTPUT_BUCKET),
            false,
        ),
        (
            "the output bucket is not owned",
            transformer_id.as_str(),
            Some(CAPABILITY_INPUT_BUCKET),
            Some(unowned),
            false,
        ),
        (
            "neither bucket is owned",
            transformer_id.as_str(),
            Some(unowned),
            Some("another-bucket-this-account-does-not-own"),
            false,
        ),
        (
            "the input location names no bucket",
            transformer_id.as_str(),
            None,
            Some(CAPABILITY_OUTPUT_BUCKET),
            false,
        ),
        (
            "the output location names no bucket",
            transformer_id.as_str(),
            Some(CAPABILITY_INPUT_BUCKET),
            None,
            false,
        ),
        (
            // Wrong in both ways: the buckets are validated first, so this is the bucket
            // refusal and not the reference one.
            "an unowned bucket and a transformer nobody holds",
            UNKNOWN_TRANSFORMER,
            Some(unowned),
            Some(CAPABILITY_OUTPUT_BUCKET),
            false,
        ),
    ];

    let (_, before) = call(&router, "ListCapabilities", json!({})).await;
    let mut expected_count = before["capabilities"].as_array().map(Vec::len).unwrap_or(0);

    for (case, transformer, input, output, accepted) in cases {
        let (status, body) = call(
            &router,
            "CreateCapability",
            json!({
                "name": format!("capability for {case}"),
                "type": "edi",
                "configuration": configuration_over(transformer, input, output)
            }),
        )
        .await;

        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: a capability over buckets this account owns must be \
                 created, got {status} {body}"
            );
            expected_count += 1;
        } else {
            assert_eq!(
                status, CLIENT_ERROR_STATUS,
                "case {case:?}: expected {CLIENT_ERROR_STATUS}, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: expected a ValidationException, got {body}"
            );
            assert_eq!(
                text(&body, "Message"),
                "Customer does not own input and/or output buckets.",
                "case {case:?}: expected the bucket-ownership refusal, got {body}"
            );
        }

        // Nothing a refused create names is persisted, and nothing an accepted one names
        // is lost.
        let (_, listed) = call(&router, "ListCapabilities", json!({})).await;
        assert_eq!(
            listed["capabilities"].as_array().map(Vec::len).unwrap_or(0),
            expected_count,
            "case {case:?}: the store must hold {expected_count} capabilities, got {listed}"
        );
    }
}

/// One tagging operation under test: its name and the request it takes for an ARN.
type TaggingCall = (&'static str, fn(&str) -> Value);

/// RULE: every tagging operation of this service validates its `ResourceARN` the same way
/// — the grammar first, then by RESOLVING what parses — so all three answer the modelled
/// `ValidationException` with the same sentence for the same kind of bad ARN: the value
/// itself when it is not an ARN at all, and the one fixed sentence when it parses and
/// addresses no resource of this account.
///
/// The rows span the three operations against both ways an ARN fails.
#[tokio::test]
async fn every_tagging_operation_refuses_an_unresolvable_arn_the_same_way() {
    let router = b2bi_router();

    // The two recorded sentences, pinned literally here and asserted to be the ones the
    // handlers share, so neither the tests nor the three operations can drift from them.
    assert_eq!(
        INVALID_INPUT_ARN, "Invalid input ARN!",
        "the refusal for an ARN that addresses nothing must be the recorded sentence"
    );
    assert_eq!(
        invalid_arn("not-an-arn"),
        "Invalid ARN not-an-arn",
        "the refusal for a value that is not an ARN must be the recorded sentence over it"
    );

    // (how the ARN fails, the ARN, the sentence the refusal carries)
    let candidates = [
        (
            "not an ARN at all",
            "this-is-not-an-arn".to_string(),
            invalid_arn("this-is-not-an-arn"),
        ),
        (
            "a well-formed ARN naming nothing",
            b2bi_arn("capability", UNKNOWN_CAPABILITY),
            INVALID_INPUT_ARN.to_string(),
        ),
    ];
    let requests: [TaggingCall; 3] = [
        ("ListTagsForResource", |arn| json!({ "ResourceARN": arn })),
        ("TagResource", |arn| {
            json!({ "ResourceARN": arn, "Tags": [{ "Key": "env", "Value": "prod" }] })
        }),
        ("UntagResource", |arn| {
            json!({ "ResourceARN": arn, "TagKeys": ["env"] })
        }),
    ];

    for (operation, request) in requests {
        for (class, arn, expected) in &candidates {
            let case = format!("{operation} / {class}");
            let (status, body) = call(&router, operation, request(arn)).await;
            assert_eq!(
                status, CLIENT_ERROR_STATUS,
                "case {case:?}: expected {CLIENT_ERROR_STATUS}, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: expected a ValidationException, got {body}"
            );
            assert_eq!(
                text(&body, "Message"),
                *expected,
                "case {case:?}: expected {expected:?}, got {body}"
            );
        }
    }
}

/// RULE: an Amazon S3 object a request names and object storage does not hold is reported
/// as the `s3://<bucket>/<key>` URI the request addressed and the fact that it is not
/// there — one wording for every operation that reads an input object, carrying nothing of
/// the error code the storage service answered with.
///
/// The rows span the operation reading the object, a key an owned bucket does not hold, a
/// bucket nobody holds, and the shape of the key (a deep prefix, a name with no prefix).
#[tokio::test]
async fn a_missing_input_object_is_reported_as_the_s3_uri_that_does_not_exist() {
    let router = b2bi_router();
    given_bucket(&router, "edi-objects").await;

    // The recorded sentence, pinned literally and asserted to be the one every reader
    // states, so the template cannot drift from it.
    assert_eq!(
        s3_object_not_found("a-bucket", "a/key.edi"),
        "The S3 object s3://a-bucket/a/key.edi does not exist",
        "a missing object must be reported with the recorded sentence over its URI"
    );

    // Each row: the case, the operation, the bucket and the key.
    let cases: Vec<(&str, &str, &str, &str)> = vec![
        (
            "TestParsing, a key the bucket does not hold",
            "TestParsing",
            "edi-objects",
            "inbound/850.edi",
        ),
        (
            "TestParsing, a bucket nobody holds",
            "TestParsing",
            "no-such-edi-bucket",
            "inbound/850.edi",
        ),
        (
            "TestParsing, a deeply prefixed key",
            "TestParsing",
            "edi-objects",
            "year/2026/month/01/interchange-0001.edi",
        ),
        (
            "TestParsing, a key with no prefix at all",
            "TestParsing",
            "edi-objects",
            "850",
        ),
        (
            "CreateStarterMappingTemplate, a sample the bucket does not hold",
            "CreateStarterMappingTemplate",
            "edi-objects",
            "samples/850.edi",
        ),
        (
            "CreateStarterMappingTemplate, a bucket nobody holds",
            "CreateStarterMappingTemplate",
            "no-such-edi-bucket",
            "samples/850.edi",
        ),
    ];

    for (case, operation, bucket, key) in cases {
        let request = if operation == "TestParsing" {
            test_parsing_request(bucket, key)
        } else {
            json!({
                "mappingType": "JSONATA",
                "templateDetails": {
                    "x12": { "transactionSet": "X12_850", "version": "VERSION_4010" }
                },
                "outputSampleLocation": { "bucketName": bucket, "key": key }
            })
        };
        let (status, body) = call(&router, operation, request).await;
        assert_eq!(
            status, CLIENT_ERROR_STATUS,
            "case {case:?}: expected {CLIENT_ERROR_STATUS}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected a ResourceNotFoundException, got {body}"
        );
        assert_eq!(
            text(&body, "message"),
            s3_object_not_found(bucket, key),
            "case {case:?}: expected the missing-object sentence, got {body}"
        );
        assert!(
            !text(&body, "message").contains("NoSuch"),
            "case {case:?}: the sentence must carry no storage error code, got {body}"
        );
    }
}

/// RULE: a transformer job this service holds no run for is reported by the job id the
/// request carried and the account the job would have belonged to — a job id is unique to
/// an account, so the account is what scopes the lookup, and the transformer the request
/// paired it with is not part of the answer.
///
/// The rows span the job id (every character class the modelled `len 25..25` /
/// `^[a-zA-Z0-9_-]+$` admits) and whether the transformer it is paired with exists, a live
/// transformer that simply never ran that job included.
#[tokio::test]
async fn a_transformer_job_that_resolves_to_nothing_names_the_job_and_the_customer() {
    let router = b2bi_router();
    let live_transformer = given_transformer(&router, "job-lookup-transformer").await;

    // Each row: the case, the transformerId the request pairs the job with, and the
    // transformerJobId — each exactly 25 characters, as the model requires.
    let cases: Vec<(&str, &str, &str)> = vec![
        (
            "a job under a transformer nobody holds",
            UNKNOWN_TRANSFORMER,
            UNKNOWN_JOB,
        ),
        (
            "a job under a live transformer that never ran it",
            live_transformer.as_str(),
            "abcdefghijklmnopqrstuvwxy",
        ),
        (
            "a job id of underscores and dashes",
            live_transformer.as_str(),
            "job_id-with-0000000000000",
        ),
        (
            "a job id in upper case",
            live_transformer.as_str(),
            "ABCDEFGHIJKLMNOPQRSTUVWXY",
        ),
    ];

    for (case, transformer_id, job_id) in cases {
        let (status, body) = call(
            &router,
            "GetTransformerJob",
            json!({ "transformerId": transformer_id, "transformerJobId": job_id }),
        )
        .await;
        assert_eq!(
            status, CLIENT_ERROR_STATUS,
            "case {case:?}: expected {CLIENT_ERROR_STATUS}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected a ResourceNotFoundException, got {body}"
        );
        assert_eq!(
            text(&body, "message"),
            format!("Found no transformer job {job_id} for customer {ACCOUNT}"),
            "case {case:?}: expected the job-and-customer sentence, got {body}"
        );
        assert!(
            !text(&body, "message").contains(transformer_id),
            "case {case:?}: the sentence names the customer, not the transformer, got {body}"
        );
    }
}
