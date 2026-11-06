// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the BODY this service answers a request it refuses as invalid
//! input with — the modelled `ValidationException` (HTTP 400), whose `message` and
//! whose `fieldList` of `path` + `message`
//! (`smithy_query(query="shape",shape="ValidationException,ValidationExceptionField")`)
//! are what a client reads to learn which input it has to fix.
//!
//! Four rules, each asserted over a TABLE whose rows differ along what it generalises
//! over:
//!
//! 1. A value outside a constraint the MODEL declares is refused with the counted
//!    envelope `<n> validation error(s) detected. ` over one clause per violated
//!    member, `Value at '<json pointer>' failed to satisfy constraint: <requirement>`,
//!    and a `fieldList` entry per violated member carrying that member's pointer and
//!    its own clause. The requirement names the modelled constraint and BOTH its
//!    declared bounds where the member declares two; the rejected value is never
//!    quoted. The rows differ in the constraint class (`@range`, `@length`, `@enum`,
//!    `@required`), in which operation and member carries it, in the depth the member
//!    sits at (top level, inside a structure, a whole list), in whether the member is
//!    bound to the body or to the request URI, and in which side of the bound was
//!    broken — with each published bound accepted and one value past it refused.
//! 2. A name outside the form its kind of resource is named by is refused with ONE
//!    fixed sentence per kind, which echoes no part of the rejected name. The rows
//!    differ in HOW the name is ill-formed and in which resource kind it names.
//! 3. An `encryptionConfiguration` that breaks the `if and only if` rule its `kmsKeyArn`
//!    member documents is refused, in EITHER direction, naming the configuration.
//! 4. A vector a vector index cannot hold is refused as the RECORD it is: `fieldList`
//!    names the element of the input list and says what is wrong with the vector, and
//!    the top-level message names the record by the `key` the caller submitted it
//!    under. The rows differ in the index's dimension, the length submitted, the
//!    record's position in the batch and the form of its key.
//! 5. A `policy` document the service will not accept is refused with ONE fixed
//!    sentence whatever is wrong with it, revealing nothing about where reading it
//!    failed. The rows walk the grammar and the size ceiling.
//!
//! Every case drives real HTTP through the production front door, so what is asserted
//! is what an SDK client sees.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::vector_lifecycle::{
    call, call_reporting_errortype, encode_label, error_code, field_list, message_of, ok, router,
};

/// The wire error code every refusal in this suite carries, and the status the shape's
/// `@httpError` declares.
const VALIDATION_EXCEPTION: &str = "ValidationException";
/// See [`VALIDATION_EXCEPTION`].
const BAD_REQUEST: u16 = 400;

/// A `CreateIndex` request body for `index` in `bucket`, inside every modelled bound.
fn create_index_request(bucket: &str, index: &str) -> Value {
    json!({
        "vectorBucketName": bucket,
        "indexName": index,
        "dataType": "float32",
        "dimension": 4,
        "distanceMetric": "cosine",
    })
}

/// A router holding one vector bucket and one 4-dimension index in it.
async fn with_index(bucket: &str, index: &str) -> MultiServiceRouter {
    let router = router();
    ok(
        &router,
        "CreateVectorBucket",
        &json!({ "vectorBucketName": bucket }),
    )
    .await;
    ok(&router, "CreateIndex", &create_index_request(bucket, index)).await;
    router
}

/// The clause this service states for the member at `pointer`.
fn clause(pointer: &str, requirement: &str) -> String {
    format!("Value at '{pointer}' failed to satisfy constraint: {requirement}")
}

/// One row of the modelled-constraint table: the request, the member the refusal must
/// name by its JSON pointer, and the requirement that member had to satisfy.
struct Refused {
    case: &'static str,
    operation: &'static str,
    request: Value,
    pointer: &'static str,
    requirement: String,
}

/// Every modelled constraint of this service is refused with the service's own clause,
/// its member named by a JSON pointer, and a `fieldList` entry stating the same clause
/// — for every constraint class, at every depth, whichever operation and member carries
/// it.
#[tokio::test]
async fn a_value_outside_a_modelled_constraint_is_refused_with_the_services_clause() {
    let bucket = "constraint-bucket";
    let index = "constraint-index";
    let router = with_index(bucket, index).await;

    // The requirements, from the members' own modelled constraints
    // (`smithy_query(query="operation",
    // operation="CreateIndex,CreateVectorBucket,ListVectorBuckets,ListIndexes,ListVectors,QueryVectors,PutVectors,TagResource")`).
    let dimension = "Member must be between 1 and 4096, inclusive".to_string();
    let page = |max: u32| format!("Member must be between 1 and {max}, inclusive");
    let name_length = "Member must have length between 3 and 63, inclusive".to_string();

    let long_name = "a".repeat(64);
    let rows: Vec<Refused> = vec![
        Refused {
            case: "a dimension above the modelled ceiling",
            operation: "CreateIndex",
            request: {
                let mut request = create_index_request(bucket, "dimension-high");
                request["dimension"] = json!(5000);
                request
            },
            pointer: "/dimension",
            requirement: dimension.clone(),
        },
        Refused {
            case: "a dimension one above the modelled ceiling",
            operation: "CreateIndex",
            request: {
                let mut request = create_index_request(bucket, "dimension-edge");
                request["dimension"] = json!(4097);
                request
            },
            pointer: "/dimension",
            requirement: dimension.clone(),
        },
        Refused {
            case: "a dimension below the modelled floor",
            operation: "CreateIndex",
            request: {
                let mut request = create_index_request(bucket, "dimension-low");
                request["dimension"] = json!(0);
                request
            },
            pointer: "/dimension",
            requirement: dimension.clone(),
        },
        Refused {
            case: "a page size above the ceiling of a bucket listing",
            operation: "ListVectorBuckets",
            request: json!({ "maxResults": 501 }),
            pointer: "/maxResults",
            requirement: page(500),
        },
        Refused {
            case: "a page size below the floor of a bucket listing",
            operation: "ListVectorBuckets",
            request: json!({ "maxResults": 0 }),
            pointer: "/maxResults",
            requirement: page(500),
        },
        Refused {
            case: "a page size above the ceiling of an index listing",
            operation: "ListIndexes",
            request: json!({ "vectorBucketName": bucket, "maxResults": 501 }),
            pointer: "/maxResults",
            requirement: page(500),
        },
        Refused {
            case: "a page size above the ceiling of a vector listing, which is its own",
            operation: "ListVectors",
            request: json!({
                "vectorBucketName": bucket,
                "indexName": index,
                "maxResults": 1001,
            }),
            pointer: "/maxResults",
            requirement: page(1000),
        },
        Refused {
            case: "a segment count above its ceiling",
            operation: "ListVectors",
            request: json!({
                "vectorBucketName": bucket,
                "indexName": index,
                "segmentCount": 17,
            }),
            pointer: "/segmentCount",
            requirement: "Member must be between 1 and 16, inclusive".to_string(),
        },
        Refused {
            case: "a segment index above its ceiling, whose floor is zero",
            operation: "ListVectors",
            request: json!({
                "vectorBucketName": bucket,
                "indexName": index,
                "segmentCount": 4,
                "segmentIndex": 16,
            }),
            pointer: "/segmentIndex",
            requirement: "Member must be between 0 and 15, inclusive".to_string(),
        },
        Refused {
            // `topK` declares a floor and no ceiling, so there is no pair to state.
            case: "a result count below the floor of a member that declares one bound",
            operation: "QueryVectors",
            request: json!({
                "vectorBucketName": bucket,
                "indexName": index,
                "topK": 0,
                "queryVector": { "float32": [0.1, 0.2, 0.3, 0.4] },
            }),
            pointer: "/topK",
            requirement: "Member must be greater than or equal to 1".to_string(),
        },
        Refused {
            case: "a vector bucket name longer than the modelled length",
            operation: "CreateVectorBucket",
            request: json!({ "vectorBucketName": long_name }),
            pointer: "/vectorBucketName",
            requirement: name_length.clone(),
        },
        Refused {
            case: "an index name shorter than the modelled length",
            operation: "CreateIndex",
            request: {
                let mut request = create_index_request(bucket, "ab");
                request["indexName"] = json!("ab");
                request
            },
            pointer: "/indexName",
            requirement: name_length.clone(),
        },
        Refused {
            case: "a required member the request leaves out",
            operation: "CreateIndex",
            request: json!({
                "vectorBucketName": bucket,
                "indexName": "no-dimension",
                "dataType": "float32",
                "distanceMetric": "cosine",
            }),
            pointer: "/dimension",
            requirement: "Member must not be null".to_string(),
        },
        Refused {
            case: "a value outside an enumerated member's value set",
            operation: "CreateIndex",
            request: {
                let mut request = create_index_request(bucket, "bad-metric");
                request["distanceMetric"] = json!("manhattan");
                request
            },
            pointer: "/distanceMetric",
            // The value set in the order the model declares it
            // (`smithy_query(query="shape", shape="DistanceMetric")`).
            requirement: "Member must satisfy enum value set: [euclidean, cosine]".to_string(),
        },
        Refused {
            // Nested one level down: the pointer walks into the structure.
            case: "an enumerated member inside a structure",
            operation: "CreateIndex",
            request: {
                let mut request = create_index_request(bucket, "bad-sse");
                request["encryptionConfiguration"] = json!({ "sseType": "AES512" });
                request
            },
            pointer: "/encryptionConfiguration/sseType",
            // `smithy_query(query="shape", shape="SseType")`.
            requirement: "Member must satisfy enum value set: [AES256, aws:kms]".to_string(),
        },
        Refused {
            // The constraint is on the LIST itself, so the pointer names the list.
            case: "a batch holding fewer records than the list admits",
            operation: "PutVectors",
            request: json!({
                "vectorBucketName": bucket,
                "indexName": index,
                "vectors": [],
            }),
            pointer: "/vectors",
            requirement: "Member must have length between 1 and 500, inclusive".to_string(),
        },
    ];

    for row in rows {
        let (status, errortype, body) = call_reporting_errortype(
            &router,
            "POST",
            &format!("/{}", row.operation),
            Some(&row.request),
        )
        .await;
        let expected = clause(row.pointer, &row.requirement);
        assert_eq!(
            (status, error_code(&body).as_str(), errortype.as_deref()),
            (BAD_REQUEST, VALIDATION_EXCEPTION, Some(VALIDATION_EXCEPTION)),
            "case {:?}: {} must answer the modelled 400, got {status} {body}",
            row.case,
            row.operation
        );
        assert_eq!(
            message_of(&body),
            format!("1 validation error detected. {expected}"),
            "case {:?}: expected the counted envelope over the member's clause, got {body}",
            row.case
        );
        assert_eq!(
            field_list(&body),
            vec![(row.pointer.to_string(), expected)],
            "case {:?}: fieldList must name the member by its JSON pointer and state its \
             own clause, got {body}",
            row.case
        );
    }
}

/// Every operation that declares a `@required` member answers the SAME modelled body
/// when that member is absent — the counted envelope over `Member must not be null` and a
/// `fieldList` naming the member by its JSON pointer — so the rule is one refusal across
/// the whole service rather than one per operation, whichever request form carries the
/// member.
#[tokio::test]
async fn every_operation_states_an_absent_required_member_the_same_way() {
    let bucket = "required-members-bucket";
    let index = "required-members-index";
    let router = with_index(bucket, index).await;
    let arn = crate::vector_lifecycle::expected_bucket_arn(bucket);

    // One row per `@required` member the model declares on a body of this service
    // (`smithy_query(query="operation", operation="...")`), each omitting exactly that
    // member and supplying every other, so the member the refusal must name is the one
    // the row left out. (`UntagResource`'s required `tagKeys` is an `@httpQuery` member,
    // whose absence the router's own route lookup answers before any operation is named,
    // so it has no row here.)
    let full_index = create_index_request(bucket, "required-probe");
    let full_query = json!({
        "vectorBucketName": bucket,
        "indexName": index,
        "topK": 1,
        "queryVector": { "float32": [0.1, 0.2, 0.3, 0.4] },
    });
    let without = |request: &Value, member: &str| {
        let mut request = request.clone();
        request
            .as_object_mut()
            .expect("the request body is an object")
            .remove(member);
        request
    };
    // (case, method, path and query, body, the member the refusal must name)
    let rows: Vec<(&str, &str, String, Value, &str)> = vec![
        (
            "CreateVectorBucket without its bucket name",
            "POST",
            "/CreateVectorBucket".to_string(),
            json!({}),
            "/vectorBucketName",
        ),
        (
            "CreateIndex without its index name",
            "POST",
            "/CreateIndex".to_string(),
            without(&full_index, "indexName"),
            "/indexName",
        ),
        (
            "CreateIndex without its data type",
            "POST",
            "/CreateIndex".to_string(),
            without(&full_index, "dataType"),
            "/dataType",
        ),
        (
            "CreateIndex without its distance metric",
            "POST",
            "/CreateIndex".to_string(),
            without(&full_index, "distanceMetric"),
            "/distanceMetric",
        ),
        (
            "PutVectors without its batch",
            "POST",
            "/PutVectors".to_string(),
            json!({ "vectorBucketName": bucket, "indexName": index }),
            "/vectors",
        ),
        (
            "GetVectors without its keys",
            "POST",
            "/GetVectors".to_string(),
            json!({ "vectorBucketName": bucket, "indexName": index }),
            "/keys",
        ),
        (
            "DeleteVectors without its keys",
            "POST",
            "/DeleteVectors".to_string(),
            json!({ "vectorBucketName": bucket, "indexName": index }),
            "/keys",
        ),
        (
            "QueryVectors without its result count",
            "POST",
            "/QueryVectors".to_string(),
            without(&full_query, "topK"),
            "/topK",
        ),
        (
            "QueryVectors without its query vector",
            "POST",
            "/QueryVectors".to_string(),
            without(&full_query, "queryVector"),
            "/queryVector",
        ),
        (
            "PutVectorBucketPolicy without its policy",
            "POST",
            "/PutVectorBucketPolicy".to_string(),
            json!({ "vectorBucketName": bucket }),
            "/policy",
        ),
        (
            "UpdateIndexMode without its mode",
            "POST",
            "/UpdateIndexMode".to_string(),
            json!({ "vectorBucketName": bucket, "indexName": index }),
            "/indexMode",
        ),
        (
            "PutVectorBucketDefaultIndexMode without its mode",
            "POST",
            "/PutVectorBucketDefaultIndexMode".to_string(),
            json!({ "vectorBucketName": bucket }),
            "/defaultIndexMode",
        ),
        (
            "TagResource without its tags, which names its resource in the URI",
            "POST",
            format!("/tags/{}", encode_label(&arn)),
            json!({}),
            "/tags",
        ),
    ];

    for (case, method, path_and_query, body, pointer) in rows {
        let (status, errortype, answered) =
            call_reporting_errortype(&router, method, &path_and_query, Some(&body)).await;
        let expected = clause(pointer, "Member must not be null");
        assert_eq!(
            (status, error_code(&answered).as_str(), errortype.as_deref()),
            (BAD_REQUEST, VALIDATION_EXCEPTION, Some(VALIDATION_EXCEPTION)),
            "case {case:?}: must be the modelled 400, got {status} {answered}"
        );
        assert_eq!(
            (message_of(&answered), field_list(&answered)),
            (
                format!("1 validation error detected. {expected}"),
                vec![(pointer.to_string(), expected.clone())]
            ),
            "case {case:?}: every operation must state an absent required member the same \
             way, got {answered}"
        );
    }
}

/// A member bound to the request URI rather than to the body is held to its modelled
/// constraint the same way, and a member declaring ONE bound states that bound alone.
#[tokio::test]
async fn a_uri_bound_member_outside_its_constraint_is_refused_the_same_way() {
    let router = router();
    // `resourceArn` is an `@httpLabel` whose `@length` declares a maximum only
    // (`smithy_query(query="operation",operation="TagResource")`).
    let over_long = format!("arn:aws:s3vectors:us-east-1:000000000000:bucket/{}", "a".repeat(1011));
    let (status, body) = crate::vector_lifecycle::call_uri(
        &router,
        "POST",
        &format!("/tags/{}", encode_label(&over_long)),
        Some(&json!({ "tags": { "team": "web" } })),
    )
    .await;
    let expected = clause(
        "/resourceArn",
        "Member must have length less than or equal to 1011",
    );
    assert_eq!(
        (status, error_code(&body).as_str()),
        (BAD_REQUEST, VALIDATION_EXCEPTION),
        "an over-long resource ARN must be the modelled 400, got {status} {body}"
    );
    assert_eq!(
        (message_of(&body), field_list(&body)),
        (
            format!("1 validation error detected. {expected}"),
            vec![("/resourceArn".to_string(), expected.clone())]
        ),
        "a URI-bound member must be stated like any other, got {body}"
    );
}

/// Each published bound is ACCEPTED: the refusals above are of values past the bound,
/// not of the bound itself.
#[tokio::test]
async fn every_published_bound_is_accepted() {
    let bucket = "accepted-bounds";
    let router = router();
    ok(
        &router,
        "CreateVectorBucket",
        &json!({ "vectorBucketName": bucket }),
    )
    .await;

    // (case, operation, request)
    let rows: Vec<(&str, &str, Value)> = vec![
        ("the floor of the dimension range", "CreateIndex", {
            let mut request = create_index_request(bucket, "dimension-floor");
            request["dimension"] = json!(1);
            request
        }),
        ("the ceiling of the dimension range", "CreateIndex", {
            let mut request = create_index_request(bucket, "dimension-ceiling");
            request["dimension"] = json!(4096);
            request
        }),
        (
            "the floor of a bucket listing's page size",
            "ListVectorBuckets",
            json!({ "maxResults": 1 }),
        ),
        (
            "the ceiling of a bucket listing's page size",
            "ListVectorBuckets",
            json!({ "maxResults": 500 }),
        ),
        (
            "the ceiling of an index listing's page size",
            "ListIndexes",
            json!({ "vectorBucketName": bucket, "maxResults": 500 }),
        ),
        ("the shortest accepted index name", "CreateIndex", {
            let mut request = create_index_request(bucket, "abc");
            request["indexName"] = json!("abc");
            request
        }),
        ("the longest accepted index name", "CreateIndex", {
            let name = format!("i{}", "0".repeat(62));
            let mut request = create_index_request(bucket, &name);
            request["indexName"] = json!(name);
            request
        }),
    ];

    for (case, operation, request) in rows {
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: {operation} must accept a value at the published bound, \
             got {status} {body}"
        );
    }
}

/// Names that are within the modelled length but outside the form the ARN patterns
/// encode, so the refusal is the service's own rather than a modelled constraint's.
fn ill_formed_names() -> Vec<(&'static str, String)> {
    vec![
        ("upper case and underscores", "Invalid_Bucket_Name".to_string()),
        ("an underscore", "probe_name".to_string()),
        ("a leading hyphen", "-probename".to_string()),
        ("a trailing hyphen", "probename-".to_string()),
        ("a leading dot", ".probename".to_string()),
        ("a trailing dot", "probename.".to_string()),
        ("a space", "probe name".to_string()),
        ("a slash", "probe/name".to_string()),
        ("a colon", "probe:name".to_string()),
        ("non-ASCII letters", "pröbename".to_string()),
    ]
}

/// A name outside the form its kind of resource is named by is refused with ONE fixed
/// sentence for that kind — in `message` and in the `fieldList` entry naming the member
/// the name travelled in — and the sentence carries no part of the rejected name.
#[tokio::test]
async fn an_ill_formed_resource_name_reports_one_fixed_sentence_per_kind() {
    // (the resource kind, the operation, the member the name travels in, the sentence)
    let kinds: [(&str, &str, &str, &str); 2] = [
        (
            "a vector bucket",
            "CreateVectorBucket",
            "vectorBucketName",
            "Invalid vector bucket name",
        ),
        ("an index", "CreateIndex", "indexName", "Invalid index name"),
    ];

    for (kind, operation, member, sentence) in kinds {
        for (why, name) in ill_formed_names() {
            let router = router();
            ok(
                &router,
                "CreateVectorBucket",
                &json!({ "vectorBucketName": "name-rules-bucket" }),
            )
            .await;
            let request = match operation {
                "CreateVectorBucket" => json!({ "vectorBucketName": name }),
                _ => {
                    let mut request = create_index_request("name-rules-bucket", &name);
                    request["indexName"] = json!(name);
                    request
                }
            };
            let case = format!("{kind} named with {why}");
            let (status, body) = call(&router, operation, &request).await;
            let message = message_of(&body);
            assert_eq!(
                (status, error_code(&body).as_str(), message.as_str()),
                (BAD_REQUEST, VALIDATION_EXCEPTION, sentence),
                "case {case:?}: must be the modelled 400 carrying this kind's fixed \
                 sentence, got {status} {body}"
            );
            assert_eq!(
                field_list(&body),
                vec![(member.to_string(), sentence.to_string())],
                "case {case:?}: fieldList must name the member the name travelled in, \
                 got {body}"
            );
            assert!(
                !message.contains(&name),
                "case {case:?}: the sentence must echo no part of the rejected name, \
                 got {message:?}"
            );
        }
    }
}

/// The two kinds say different things, so a client branching on the text tells a
/// rejected bucket name from a rejected index name.
#[tokio::test]
async fn the_two_name_kinds_report_different_sentences() {
    let router = router();
    ok(
        &router,
        "CreateVectorBucket",
        &json!({ "vectorBucketName": "two-kinds-bucket" }),
    )
    .await;
    let (_, bucket_body) = call(
        &router,
        "CreateVectorBucket",
        &json!({ "vectorBucketName": "Bad_Name" }),
    )
    .await;
    let mut request = create_index_request("two-kinds-bucket", "Bad_Name");
    request["indexName"] = json!("Bad_Name");
    let (_, index_body) = call(&router, "CreateIndex", &request).await;
    assert_ne!(
        message_of(&bucket_body),
        message_of(&index_body),
        "each resource kind must have its own sentence, got {bucket_body} and {index_body}"
    );
}

/// `kmsKeyArn` "is allowed if and only if `sseType` is set to `aws:kms`", so BOTH
/// directions of that biconditional are refused — the KMS type with no key, and a key
/// with any other type — with the refusal naming the CONFIGURATION rather than either
/// member, on every operation that takes one; and a configuration that satisfies the
/// rule is stored and reads back.
#[tokio::test]
async fn an_encryption_configuration_outside_the_kms_rule_is_refused_both_ways() {
    let key = "arn:aws:kms:us-east-1:000000000000:key/1234abcd-12ab-34cd-56ef-1234567890ab";
    let required = "kmsKeyArn must be specified when sseType is set to aws:kms";
    let not_allowed = "kmsKeyArn is not allowed when sseType is not set to aws:kms";

    // (case, the configuration, the sentence)
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "the KMS type with no key for it",
            json!({ "sseType": "aws:kms" }),
            required,
        ),
        (
            "a key beside the S3-managed type",
            json!({ "sseType": "AES256", "kmsKeyArn": key }),
            not_allowed,
        ),
        (
            "a key beside no declared type at all, which defaults to the S3-managed one",
            json!({ "kmsKeyArn": key }),
            not_allowed,
        ),
    ];

    for (case, configuration, sentence) in rows {
        let router = router();
        ok(
            &router,
            "CreateVectorBucket",
            &json!({ "vectorBucketName": "encryption-bucket" }),
        )
        .await;

        // The bucket create and the index create both take an encryption configuration,
        // so the rule holds of both.
        let mut index_request = create_index_request("encryption-bucket", "encryption-index");
        index_request["encryptionConfiguration"] = configuration.clone();
        let probes: Vec<(&str, Value)> = vec![
            (
                "CreateVectorBucket",
                json!({
                    "vectorBucketName": "encryption-probe",
                    "encryptionConfiguration": configuration.clone(),
                }),
            ),
            ("CreateIndex", index_request),
        ];

        for (operation, request) in probes {
            let case = format!("{case} on {operation}");
            let (status, errortype, body) =
                call_reporting_errortype(&router, "POST", &format!("/{operation}"), Some(&request))
                    .await;
            assert_eq!(
                (status, error_code(&body).as_str(), errortype.as_deref()),
                (BAD_REQUEST, VALIDATION_EXCEPTION, Some(VALIDATION_EXCEPTION)),
                "case {case:?}: must be the modelled 400, got {status} {body}"
            );
            assert_eq!(
                (message_of(&body).as_str(), field_list(&body)),
                (
                    sentence,
                    vec![("EncryptionConfiguration".to_string(), sentence.to_string())]
                ),
                "case {case:?}: must carry the service's sentence and name the \
                 configuration, got {body}"
            );
        }

        // The refused creates stored nothing.
        let listed = ok(&router, "ListVectorBuckets", &json!({})).await;
        assert_eq!(
            listed["vectorBuckets"].as_array().map(Vec::len),
            Some(1),
            "case {case:?}: a refused create must store no bucket, got {listed}"
        );
    }

    // Both configurations the rule admits are accepted, and read back as stored.
    for (case, configuration) in [
        ("the S3-managed type alone", json!({ "sseType": "AES256" })),
        (
            "the KMS type with its key",
            json!({ "sseType": "aws:kms", "kmsKeyArn": key }),
        ),
    ] {
        let router = router();
        ok(
            &router,
            "CreateVectorBucket",
            &json!({
                "vectorBucketName": "encryption-accepted",
                "encryptionConfiguration": configuration.clone(),
            }),
        )
        .await;
        let got = ok(
            &router,
            "GetVectorBucket",
            &json!({ "vectorBucketName": "encryption-accepted" }),
        )
        .await;
        assert_eq!(
            got["vectorBucket"]["encryptionConfiguration"]["sseType"],
            configuration["sseType"],
            "case {case:?}: the stored configuration must read back, got {got}"
        );
        assert_eq!(
            got["vectorBucket"]["encryptionConfiguration"]["kmsKeyArn"],
            configuration
                .get("kmsKeyArn")
                .cloned()
                .unwrap_or(Value::Null),
            "case {case:?}: the stored key must read back, got {got}"
        );
    }
}

/// A vector whose data length is not the index's dimension is refused as the RECORD it
/// is: `fieldList` names the element of `vectors` it was submitted in and says what the
/// vector had to be, and the top-level message names the record by its own `key`.
/// Nothing in the batch is written.
#[tokio::test]
async fn a_vector_the_index_cannot_hold_is_refused_as_the_record_it_is() {
    // (case, the index dimension, the submitted length, the position in the batch,
    // the record's key)
    let rows: [(&str, usize, usize, usize, &str); 6] = [
        ("the only record, too short", 4, 2, 0, "bad-dim"),
        ("the only record, too long", 4, 7, 0, "bad-dim"),
        ("the second record of a batch", 4, 3, 1, "second"),
        ("the third record of a batch", 8, 1, 2, "third-record"),
        ("a one-dimension index", 1, 2, 0, "k"),
        (
            "a key of the opaque generated form",
            3,
            4,
            1,
            "9f41c2ab-7de0-4a11-8f3e-0b1d2c3e4f55",
        ),
    ];

    for (case, dimension, length, position, key) in rows {
        let bucket = "record-bucket";
        let index = "record-index";
        let router = router();
        ok(
            &router,
            "CreateVectorBucket",
            &json!({ "vectorBucketName": bucket }),
        )
        .await;
        let mut create = create_index_request(bucket, index);
        create["dimension"] = json!(dimension);
        ok(&router, "CreateIndex", &create).await;

        // The batch holds well-formed records at every other position, so the refusal
        // is of the one record and the atomicity of the batch is what is left to check.
        let good: Vec<f32> = (0..dimension).map(|nth| 0.5 + nth as f32).collect();
        let offending: Vec<f32> = (0..length).map(|nth| 0.25 + nth as f32).collect();
        let vectors: Vec<Value> = (0..=position)
            .map(|nth| {
                if nth == position {
                    json!({ "key": key, "data": { "float32": offending } })
                } else {
                    json!({ "key": format!("good-{nth}"), "data": { "float32": good } })
                }
            })
            .collect();

        let (status, body) = call(
            &router,
            "PutVectors",
            &json!({
                "vectorBucketName": bucket,
                "indexName": index,
                "vectors": vectors,
            }),
        )
        .await;
        let defect = format!("vector must have length {dimension}, but has length {length}");
        assert_eq!(
            (status, error_code(&body).as_str()),
            (BAD_REQUEST, VALIDATION_EXCEPTION),
            "case {case:?}: must be the modelled 400, got {status} {body}"
        );
        assert_eq!(
            field_list(&body),
            vec![(format!("vectors[{position}]"), defect.clone())],
            "case {case:?}: fieldList must name the list ELEMENT and state what the \
             vector had to be, got {body}"
        );
        assert_eq!(
            message_of(&body),
            format!("Invalid record for key '{key}': {defect}"),
            "case {case:?}: the message must name the record by its key, got {body}"
        );

        // A refused batch writes nothing, not even the records that were well formed.
        let listed = ok(
            &router,
            "ListVectors",
            &json!({ "vectorBucketName": bucket, "indexName": index }),
        )
        .await;
        assert_eq!(
            listed["vectors"].as_array().map(Vec::len),
            Some(0),
            "case {case:?}: a refused batch must write nothing, got {listed}"
        );
    }
}

/// The same rule about a vector holds of the vector a QUERY is run with, where the
/// member it travelled in is what the refusal names — there is no record to name.
#[tokio::test]
async fn a_query_vector_the_index_cannot_hold_names_the_member_it_travelled_in() {
    let router = with_index("query-vector-bucket", "query-vector-index").await;
    for length in [1_usize, 3, 5, 8] {
        let data: Vec<f32> = (0..length).map(|nth| 0.5 + nth as f32).collect();
        let (status, body) = call(
            &router,
            "QueryVectors",
            &json!({
                "vectorBucketName": "query-vector-bucket",
                "indexName": "query-vector-index",
                "topK": 1,
                "queryVector": { "float32": data },
            }),
        )
        .await;
        let defect = format!("vector must have length 4, but has length {length}");
        assert_eq!(
            (status, error_code(&body).as_str()),
            (BAD_REQUEST, VALIDATION_EXCEPTION),
            "case a {length}-value query vector: must be the modelled 400, got {status} {body}"
        );
        assert_eq!(
            (message_of(&body), field_list(&body)),
            (
                defect.clone(),
                vec![("queryVector".to_string(), defect.clone())]
            ),
            "case a {length}-value query vector: must name the member it travelled in, \
             got {body}"
        );
    }
}

/// A `policy` document the service will not accept is refused with ONE fixed sentence,
/// whichever rule the document broke: nothing about where reading it failed reaches the
/// client, and the document is not stored.
#[tokio::test]
async fn a_policy_document_the_service_refuses_reports_one_fixed_sentence() {
    const MALFORMED: &str = "The provided policy is malformed or invalid";
    let bucket = "policy-sentence-bucket";
    let router = router();
    ok(
        &router,
        "CreateVectorBucket",
        &json!({ "vectorBucketName": bucket }),
    )
    .await;

    // The rows walk the grammar — and the size ceiling, one byte past it — so the rule
    // is shown to hold whatever is wrong with the document.
    let statement = json!({
        "Effect": "Allow",
        "Principal": { "AWS": "arn:aws:iam::000000000000:root" },
        "Action": "s3vectors:GetVectors",
        "Resource": "*",
    });
    let oversized = json!({
        "Version": "2012-10-17",
        "Id": "x".repeat(21 * 1024),
        "Statement": [statement],
    })
    .to_string();
    let rows: Vec<(&str, String)> = vec![
        ("text that is not JSON at all", "this is not a policy".to_string()),
        ("JSON that stops halfway", "{\"Statement\":".to_string()),
        ("an empty document", String::new()),
        ("a JSON array at the top level", "[]".to_string()),
        ("a JSON number at the top level", "7".to_string()),
        ("JSON null", "null".to_string()),
        ("an object with no Statement", json!({ "Version": "2012-10-17" }).to_string()),
        (
            "a Statement naming no statement",
            json!({ "Version": "2012-10-17", "Statement": [] }).to_string(),
        ),
        (
            "a statement that is not an object",
            json!({ "Statement": ["s3vectors:*"] }).to_string(),
        ),
        (
            "a statement with no Effect",
            json!({ "Statement": [{ "Action": "s3vectors:*" }] }).to_string(),
        ),
        (
            "an Effect outside Allow and Deny",
            json!({ "Statement": [{ "Effect": "allow", "Action": "s3vectors:*" }] }).to_string(),
        ),
        (
            "a statement naming no action",
            json!({ "Statement": [{ "Effect": "Allow", "Resource": "*" }] }).to_string(),
        ),
        ("a document past the size ceiling", oversized),
    ];

    for (case, policy) in rows {
        let (status, errortype, body) = call_reporting_errortype(
            &router,
            "POST",
            "/PutVectorBucketPolicy",
            Some(&json!({ "vectorBucketName": bucket, "policy": policy })),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str(), errortype.as_deref()),
            (BAD_REQUEST, VALIDATION_EXCEPTION, Some(VALIDATION_EXCEPTION)),
            "case {case:?}: must be the modelled 400, got {status} {body}"
        );
        assert_eq!(
            message_of(&body),
            MALFORMED,
            "case {case:?}: must carry the service's one fixed sentence, got {body}"
        );

        // A refused put stores nothing, so the policy is still absent.
        let (status, body) = call(
            &router,
            "GetVectorBucketPolicy",
            &json!({ "vectorBucketName": bucket }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (404, "NotFoundException"),
            "case {case:?}: a refused put must store nothing, got {status} {body}"
        );
    }

    // A document the grammar admits is still accepted and reads back as submitted.
    let accepted = json!({ "Version": "2012-10-17", "Statement": [statement] }).to_string();
    ok(
        &router,
        "PutVectorBucketPolicy",
        &json!({ "vectorBucketName": bucket, "policy": accepted }),
    )
    .await;
    let got = ok(
        &router,
        "GetVectorBucketPolicy",
        &json!({ "vectorBucketName": bucket }),
    )
    .await;
    assert_eq!(
        got["policy"].as_str(),
        Some(accepted.as_str()),
        "an accepted policy must read back as submitted, got {got}"
    );
}
