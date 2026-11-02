// SPDX-License-Identifier: CC0-1.0

//! How S3 Tables refuses a request whose value a modelled constraint rules out.
//!
//! The rule under test: a value outside a modelled constraint is refused with
//! this service's OWN modelled client-input error — `BadRequestException`, HTTP
//! 400, which every operation declares and which is the only such error the
//! model carries (there is no `ValidationException` shape in it) — and, for the
//! member classes the service words itself (a table bucket name, a table name, a
//! namespace name, a namespace naming more than one part, a table bucket ARN, a
//! taggable resource's ARN), with one FIXED sentence that takes nothing from the
//! request. The same holds of the conflict a resident namespace name answers
//! with, which is the other refusal this service states as fixed text.
//!
//! Each table below spans the class the rule generalises over rather than one
//! recorded request: which operation carries the member, which constraint the
//! value broke (character set, under the minimum length, over the maximum
//! length, too many list elements), and whether the operation behind the
//! boundary is implemented at all. Message equality against the fixed sentence
//! is what proves nothing from the request is echoed back.
//!
//! The accepted rows keep the refusals honest: a value AT each published bound
//! is served, so the refusals are of the values outside the bound and not of the
//! whole class.

use serde_json::{json, Value};

use crate::harness::{encode, expected_bucket_arn, query_string, Emulator};

/// The region every case works in; the ARNs a create mints carry it.
const REGION: &str = "us-east-1";

/// The code every refusal below must carry: the one client-input error this
/// service's model declares (`BadRequestException`, `httpError` 400).
const BAD_REQUEST: &str = "BadRequestException";

/// The fixed sentences, spelled out here rather than read from the crate, so this
/// suite pins the wording itself.
const INVALID_BUCKET_NAME: &str =
    "The specified bucket name isn't valid. Specify a different bucket name, and then try again.";
const INVALID_TABLE_NAME: &str = "The specified table name is not valid.";
const MULTIPART_NAMESPACE: &str = "Multipart namespaces are not supported.";
const INVALID_NAMESPACE_NAME: &str =
    "The specified namespace name isn’t valid. Specify a different namespace name, and then try \
     again.";
const INVALID_TABLE_BUCKET_ARN: &str = "The specified table bucket ARN is not valid.";
const INVALID_RESOURCE_ARN: &str = "The specified resourceArn is not valid.";

/// One refused request: how it is addressed, and the sentence this service must
/// answer it with.
struct RefusalCase {
    label: String,
    method: &'static str,
    path: String,
    body: Option<Value>,
    message: &'static str,
}

/// A table bucket ARN of the modelled form, for the rows that refuse something
/// OTHER than the ARN.
fn resident_bucket_arn() -> String {
    expected_bucket_arn(REGION, "constraint-refusals")
}

/// A `CreateTable` body naming `name`, with the one `format` the modelled
/// `OpenTableFormat` enum declares.
fn table_request(name: &str) -> Value {
    json!({ "name": name, "format": "ICEBERG" })
}

/// Every refusal class this service words with a fixed sentence, across every
/// operation that carries the member.
fn fixed_sentence_cases() -> Vec<RefusalCase> {
    let arn = resident_bucket_arn();
    // Values that cannot be a table bucket ARN: nothing ARN-shaped at all, an
    // account id one digit short of the modelled twelve, a bucket segment under
    // the modelled three characters, and the ARN of a table rather than a bucket.
    let malformed_bucket_arns = [
        "not-an-arn".to_string(),
        format!("arn:aws:s3tables:{REGION}:00000000000:bucket/resident"),
        format!("arn:aws:s3tables:{REGION}:000000000000:bucket/ab"),
        format!("{arn}/table/0123456789abcdef"),
    ];
    // Values that cannot be a taggable resource's ARN: the recorded one, an
    // account id one digit short of the modelled twelve, one naming no resource
    // after the bucket segment, and one whose resource is not a bucket at all.
    let malformed_resource_arns = [
        "not-a-valid-s3tables-arn".to_string(),
        format!("arn:aws:s3tables:{REGION}:00000000000:bucket/resident"),
        format!("arn:aws:s3tables:{REGION}:000000000000:bucket/"),
        format!("arn:aws:s3tables:{REGION}:000000000000:table/resident"),
    ];

    let mut cases = vec![
        RefusalCase {
            label: "a table bucket name holding an uppercase letter".to_string(),
            method: "PUT",
            path: "/buckets".to_string(),
            body: Some(json!({ "name": "Mixed-Case-Bucket" })),
            message: INVALID_BUCKET_NAME,
        },
        RefusalCase {
            label: "a table bucket name holding an underscore".to_string(),
            method: "PUT",
            path: "/buckets".to_string(),
            body: Some(json!({ "name": "has_underscore" })),
            message: INVALID_BUCKET_NAME,
        },
        RefusalCase {
            label: "a table bucket name one character under the modelled minimum".to_string(),
            method: "PUT",
            path: "/buckets".to_string(),
            body: Some(json!({ "name": "ab" })),
            message: INVALID_BUCKET_NAME,
        },
        RefusalCase {
            label: "a table bucket name one character over the modelled maximum".to_string(),
            method: "PUT",
            path: "/buckets".to_string(),
            body: Some(json!({ "name": "a".repeat(64) })),
            message: INVALID_BUCKET_NAME,
        },
        RefusalCase {
            // Inside the modelled pattern and length; refused by the naming rule
            // the pattern cannot express, and stated with the same sentence.
            label: "a table bucket name opening with a hyphen".to_string(),
            method: "PUT",
            path: "/buckets".to_string(),
            body: Some(json!({ "name": "-leading-hyphen" })),
            message: INVALID_BUCKET_NAME,
        },
        RefusalCase {
            label: "a table bucket name closing with a hyphen".to_string(),
            method: "PUT",
            path: "/buckets".to_string(),
            body: Some(json!({ "name": "trailing-hyphen-" })),
            message: INVALID_BUCKET_NAME,
        },
        RefusalCase {
            label: "a table name holding an uppercase letter".to_string(),
            method: "PUT",
            path: format!("/tables/{}/resident", encode(&arn)),
            body: Some(table_request("Mixed_Case_Table")),
            message: INVALID_TABLE_NAME,
        },
        RefusalCase {
            label: "a table name holding a hyphen".to_string(),
            method: "PUT",
            path: format!("/tables/{}/resident", encode(&arn)),
            body: Some(table_request("has-hyphen")),
            message: INVALID_TABLE_NAME,
        },
        RefusalCase {
            label: "a table name under the modelled minimum length".to_string(),
            method: "PUT",
            path: format!("/tables/{}/resident", encode(&arn)),
            body: Some(table_request("")),
            message: INVALID_TABLE_NAME,
        },
        RefusalCase {
            label: "a table name one character over the modelled maximum".to_string(),
            method: "PUT",
            path: format!("/tables/{}/resident", encode(&arn)),
            body: Some(table_request(&"t".repeat(256))),
            message: INVALID_TABLE_NAME,
        },
        RefusalCase {
            label: "a namespace naming two parts".to_string(),
            method: "PUT",
            path: format!("/namespaces/{}", encode(&arn)),
            body: Some(json!({ "namespace": ["first", "second"] })),
            message: MULTIPART_NAMESPACE,
        },
        RefusalCase {
            label: "a namespace naming three parts".to_string(),
            method: "PUT",
            path: format!("/namespaces/{}", encode(&arn)),
            body: Some(json!({ "namespace": ["first", "second", "third"] })),
            message: MULTIPART_NAMESPACE,
        },
        RefusalCase {
            // The one part a namespace has, outside the character set a
            // namespace name is drawn from: the element of the list, not its
            // length.
            label: "a namespace part holding an uppercase letter".to_string(),
            method: "PUT",
            path: format!("/namespaces/{}", encode(&arn)),
            body: Some(json!({ "namespace": ["Mixed_Case"] })),
            message: INVALID_NAMESPACE_NAME,
        },
        RefusalCase {
            label: "a namespace part holding a hyphen".to_string(),
            method: "PUT",
            path: format!("/namespaces/{}", encode(&arn)),
            body: Some(json!({ "namespace": ["has-hyphen"] })),
            message: INVALID_NAMESPACE_NAME,
        },
        RefusalCase {
            label: "a namespace part one character over the modelled maximum".to_string(),
            method: "PUT",
            path: format!("/namespaces/{}", encode(&arn)),
            body: Some(json!({ "namespace": ["n".repeat(256)] })),
            message: INVALID_NAMESPACE_NAME,
        },
        RefusalCase {
            // The same shape reached as a path label by another operation.
            label: "GetNamespace addressed by a namespace label outside the character set"
                .to_string(),
            method: "GET",
            path: format!("/namespaces/{}/Uppercase", encode(&arn)),
            body: None,
            message: INVALID_NAMESPACE_NAME,
        },
        RefusalCase {
            label: "CreateTable addressed by a namespace label outside the character set"
                .to_string(),
            method: "PUT",
            path: format!("/tables/{}/Uppercase", encode(&arn)),
            body: Some(table_request("resident")),
            message: INVALID_NAMESPACE_NAME,
        },
    ];

    for malformed in &malformed_bucket_arns {
        // Four operations carrying the same member: a read, a delete, a create
        // that also has a body, and one whose handler is not implemented at all —
        // the refusal is of the VALUE, so it cannot depend on either.
        cases.push(RefusalCase {
            label: format!("GetTableBucket addressed by {malformed:?}"),
            method: "GET",
            path: format!("/buckets/{}", encode(malformed)),
            body: None,
            message: INVALID_TABLE_BUCKET_ARN,
        });
        cases.push(RefusalCase {
            label: format!("DeleteTableBucket addressed by {malformed:?}"),
            method: "DELETE",
            path: format!("/buckets/{}", encode(malformed)),
            body: None,
            message: INVALID_TABLE_BUCKET_ARN,
        });
        cases.push(RefusalCase {
            label: format!("CreateNamespace addressed by {malformed:?}"),
            method: "PUT",
            path: format!("/namespaces/{}", encode(malformed)),
            body: Some(json!({ "namespace": ["resident"] })),
            message: INVALID_TABLE_BUCKET_ARN,
        });
        cases.push(RefusalCase {
            label: format!("ListNamespaces addressed by {malformed:?}"),
            method: "GET",
            path: format!("/namespaces/{}", encode(malformed)),
            body: None,
            message: INVALID_TABLE_BUCKET_ARN,
        });
    }

    for malformed in &malformed_resource_arns {
        cases.push(RefusalCase {
            label: format!("ListTagsForResource addressed by {malformed:?}"),
            method: "GET",
            path: format!("/tag/{}", encode(malformed)),
            body: None,
            message: INVALID_RESOURCE_ARN,
        });
        cases.push(RefusalCase {
            label: format!("TagResource addressed by {malformed:?}"),
            method: "POST",
            path: format!("/tag/{}", encode(malformed)),
            body: Some(json!({ "tags": { "owner": "platform" } })),
            message: INVALID_RESOURCE_ARN,
        });
        cases.push(RefusalCase {
            label: format!("UntagResource addressed by {malformed:?}"),
            method: "DELETE",
            path: format!(
                "/tag/{}{}",
                encode(malformed),
                query_string(&[("tagKeys", "owner".to_string())])
            ),
            body: None,
            message: INVALID_RESOURCE_ARN,
        });
    }

    cases
}

#[tokio::test]
async fn a_value_a_modelled_constraint_rules_out_is_the_services_own_bad_request() {
    let emulator = Emulator::new();

    for case in fixed_sentence_cases() {
        let refused = emulator
            .call(case.method, &case.path, REGION, case.body.as_ref())
            .await;
        assert_eq!(
            refused.status, 400,
            "case {label:?}: expected status 400, got {status} {body}",
            label = case.label,
            status = refused.status,
            body = refused.body
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some(BAD_REQUEST),
            "case {label:?}: expected error type {BAD_REQUEST}, got {actual:?}",
            label = case.label,
            actual = refused.error_code
        );
        assert_eq!(
            refused.message(),
            case.message,
            "case {label:?}: expected the service's fixed sentence, got {actual:?}",
            label = case.label,
            actual = refused.message()
        );
    }
}

#[tokio::test]
async fn a_refusal_this_service_states_no_sentence_for_still_carries_its_own_error() {
    let emulator = Emulator::new();
    let arn = resident_bucket_arn();

    // A member class with no fixed sentence of its own keeps the clause that
    // names the member and the requirement it broke — but the error type is
    // still this service's, because the type the framework would otherwise name
    // is a shape this model does not declare.
    let cases = [
        (
            "a table format outside the modelled enum",
            "PUT",
            format!("/tables/{}/resident", encode(&arn)),
            Some(json!({ "name": "resident", "format": "PARQUET" })),
            "format",
        ),
        (
            "a create missing the required table bucket name",
            "PUT",
            "/buckets".to_string(),
            Some(json!({})),
            "name",
        ),
    ];

    for (label, method, path, body, member) in &cases {
        let refused = emulator.call(method, path, REGION, body.as_ref()).await;
        assert_eq!(
            refused.status, 400,
            "case {label:?}: expected status 400, got {status} {body}",
            status = refused.status,
            body = refused.body
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some(BAD_REQUEST),
            "case {label:?}: expected error type {BAD_REQUEST}, got {actual:?}",
            actual = refused.error_code
        );
        assert!(
            refused.message().contains(member),
            "case {label:?}: the refusal must name the member {member:?}, got {actual:?}",
            actual = refused.message()
        );
    }
}

#[tokio::test]
async fn a_namespace_name_already_taken_is_refused_with_the_services_fixed_sentence() {
    let emulator = Emulator::new();
    let bucket = emulator
        .create_table_bucket(REGION, &json!({ "name": "conflicting-namespaces" }))
        .await;
    let arn = bucket.text("arn");

    // Two names, so the one sentence is shown to carry nothing of either: a
    // conflict states that the name is taken, not which name was asked for.
    for name in ["resident", "another_resident"] {
        let request = json!({ "namespace": [name] });
        let created = emulator.create_namespace(REGION, &arn, &request).await;
        assert_eq!(
            created.status, 200,
            "case {name:?}: the first create must be served, got {status} {body}",
            status = created.status,
            body = created.body
        );

        let repeat = emulator.create_namespace(REGION, &arn, &request).await;
        assert_eq!(
            repeat.status, 409,
            "case {name:?}: a second create of a resident name must conflict, got {status} {body}",
            status = repeat.status,
            body = repeat.body
        );
        assert_eq!(
            repeat.error_code.as_deref(),
            Some("ConflictException"),
            "case {name:?}: expected the modelled conflict, got {actual:?}",
            actual = repeat.error_code
        );
        assert_eq!(
            repeat.message(),
            "A namespace with an identical name already exists in the bucket.",
            "case {name:?}: expected the service's fixed sentence, got {actual:?}",
            actual = repeat.message()
        );
    }
}

#[tokio::test]
async fn a_value_at_every_published_bound_is_served_and_round_trips() {
    let emulator = Emulator::new();

    // The published bounds themselves: `TableBucketName` is @length(3, 63), so
    // both ends are names this service serves.
    for name in ["abc".to_string(), "b".repeat(63)] {
        let created = emulator
            .create_table_bucket(REGION, &json!({ "name": name }))
            .await;
        assert_eq!(
            created.status, 200,
            "case {name:?}: a name at a published bound must be served, got {status} {body}",
            status = created.status,
            body = created.body
        );
        let arn = created.text("arn");
        let read = emulator.get_table_bucket(REGION, &arn).await;
        assert_eq!(
            read.text("name"),
            name,
            "case {name:?}: the read must report the stored bucket"
        );
    }

    let bucket = emulator
        .create_table_bucket(REGION, &json!({ "name": "bounds-under-test" }))
        .await;
    let arn = bucket.text("arn");

    // `CreateNamespaceRequest.namespace` is @length(min: 1, max: 1): one part is
    // the whole admissible range, and it is served.
    let namespace = emulator
        .create_namespace(REGION, &arn, &json!({ "namespace": ["resident"] }))
        .await;
    assert_eq!(
        namespace.status, 200,
        "a namespace naming one part must be served, got {status} {body}",
        status = namespace.status,
        body = namespace.body
    );

    // `TableName` is @length(1, 255): both ends are names this service serves,
    // and a create is proved by reading the table back.
    for name in ["t".to_string(), "u".repeat(255)] {
        let created = emulator
            .create_table(REGION, &arn, "resident", &table_request(&name))
            .await;
        assert_eq!(
            created.status, 200,
            "case {name:?}: a table name at a published bound must be served, got {status} {body}",
            status = created.status,
            body = created.body
        );
        let read = emulator.get_table(REGION, &arn, "resident", &name).await;
        assert_eq!(
            read.text("tableARN"),
            created.text("tableARN"),
            "case {name:?}: the read must report the table the create minted"
        );
    }
}
