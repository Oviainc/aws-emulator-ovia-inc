// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the Entity Resolution resource-based policy loop
//! (`PutPolicy` + `GetPolicy`).
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 request signed for the `entityresolution` signing name, through the
//! harness the sibling `schema_mapping_workflow_lifecycle` suite owns — so what is
//! asserted is what an AWS SDK client would see, not an internal function's return
//! value.
//!
//! The rule under test: `PutPolicy` replaces the resource-based policy of the resource
//! its `arn` names and PERSISTS it, minting a fresh revision `token` for the revision
//! it wrote, and `GetPolicy` serves that stored revision back; `token` on the request
//! is an optimistic-concurrency check, so a write carrying a token that is not the
//! current revision's is refused with `ConflictException` (HTTP 400) and changes
//! nothing, while a write carrying none writes unconditionally. A policy belongs to a
//! resource, so an `arn` that names no resource is `ResourceNotFoundException`
//! (HTTP 404), and so is a read of a resource that carries no policy.
//!
//! Each table row differs along what the rule generalises over rather than repeating
//! one recorded request: the resource type over the WHOLE catalogue the modelled
//! `VeniceGlobalArn` admits (`schemamapping`, `matchingworkflow`, `idmappingworkflow`,
//! `idnamespace`), the policy document over its form (empty object, one statement,
//! several statements, pretty-printed) and its size up to the modelled
//! `PolicyDocument` maximum of 40960, the request's `token` over absent, current,
//! superseded and never-issued, and the ARN over the partition, region and account
//! segments that scope it.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_entityresolution_awsveniceservice::ComAmazonawsEntityresolutionAwsveniceserviceState;

use crate::schema_mapping_workflow_lifecycle::{
    account_id, assert_error_shape as assert_error, expected_arn, glue_table_arn, role_arn,
    string_member, Emulator, Reply, REGION,
};

/// The resource types a `VeniceGlobalArn` can name, i.e. the resources that can carry a
/// resource-based policy. This is the WHOLE catalogue the modelled pattern admits
/// (`^arn:(aws|aws-us-gov|aws-cn):entityresolution:<region>:<account>:((schemamapping|matchingworkflow|idmappingworkflow|idnamespace)/[a-zA-Z_0-9-]{1,255})$`,
/// `smithy_query(query="operation",operation="PutPolicy")`), and every test below spans
/// it.
const POLICY_RESOURCE_TYPES: &[&str] = &[
    "schemamapping",
    "matchingworkflow",
    "idmappingworkflow",
    "idnamespace",
];

/// The modelled `PolicyDocument` maximum (`length 1..40960`,
/// `smithy_query(query="operation",operation="PutPolicy")`).
const MAX_POLICY_DOCUMENT: usize = 40960;

/// A `PolicyToken` that is well-formed but was never issued by this service: 36
/// characters in the modelled `8-4-4-4-12` hexadecimal grouping, so the boundary
/// validator passes it and the handler is what has to refuse it.
const NEVER_ISSUED_TOKEN: &str = "00000000-0000-4000-8000-000000000000";

/// `/policies/{arn}` — the URI both policy operations bind
/// (`PUT` for `PutPolicy`, `GET` for `GetPolicy`). The `arn` is a single `@httpLabel`
/// path segment, so its `/` travels percent-encoded, exactly as an SDK sends it.
fn policy_path(arn: &str) -> String {
    format!("/policies/{}", arn.replace('/', "%2F"))
}

/// `pub(crate)` so a sibling suite can assert that deleting a resource detaches its
/// policy without cloning a second policy helper.
pub(crate) async fn put_policy(
    emulator: &Emulator,
    arn: &str,
    policy: &str,
    token: Option<&str>,
) -> Reply {
    let mut request = json!({ "policy": policy });
    if let Some(token) = token {
        request
            .as_object_mut()
            .expect("the request body is a JSON object")
            .insert("token".to_string(), json!(token));
    }
    emulator
        .call("PUT", &policy_path(arn), Some(&request))
        .await
}

pub(crate) async fn get_policy(emulator: &Emulator, arn: &str) -> Reply {
    emulator.call("GET", &policy_path(arn), None).await
}

/// Create the resource `resource_type` names, under `name`, and answer the ARN the
/// create minted.
///
/// Each resource type has its own create operation and its own ARN member, so this is
/// the one place the four are spelled out; everything else in the suite works from the
/// ARN it returns.
async fn create_resource(emulator: &Emulator, resource_type: &str, name: &str) -> String {
    let (reply, arn_member) = match resource_type {
        "schemamapping" => (
            emulator
                .create_schema_mapping(&json!({
                    "schemaName": name,
                    // `mappedInputFields` is `length 2..60`, so two fields is the
                    // modelled minimum.
                    "mappedInputFields": [
                        { "fieldName": "id", "type": "UNIQUE_ID" },
                        { "fieldName": "name", "type": "NAME" },
                    ],
                }))
                .await,
            "schemaArn",
        ),
        "matchingworkflow" => {
            seed_schema(emulator, name).await;
            (
                emulator
                    .create_matching_workflow(&json!({
                        "workflowName": name,
                        "inputSourceConfig": [{
                            "inputSourceARN": glue_table_arn(name),
                            "schemaName": schema_of(name),
                        }],
                        "outputSourceConfig": [{
                            "outputS3Path": "s3://er-output/policies",
                            "output": [{ "name": "id" }],
                        }],
                        "resolutionTechniques": { "resolutionType": "ML_MATCHING" },
                        "roleArn": role_arn(),
                    }))
                    .await,
                "workflowArn",
            )
        }
        "idmappingworkflow" => {
            seed_schema(emulator, name).await;
            (
                emulator
                    .call(
                        "POST",
                        "/idmappingworkflows",
                        Some(&json!({
                            "workflowName": name,
                            "inputSourceConfig": [{
                                "inputSourceARN": glue_table_arn(name),
                                "schemaName": schema_of(name),
                                "type": "SOURCE",
                            }],
                            "idMappingTechniques": {
                                "idMappingType": "RULE_BASED",
                                "ruleBasedProperties": {
                                    "ruleDefinitionType": "SOURCE",
                                    "attributeMatchingModel": "ONE_TO_ONE",
                                    "recordMatchingModel": "ONE_SOURCE_TO_ONE_TARGET",
                                    "rules": [
                                        { "ruleName": "by email", "matchingKeys": ["email"] },
                                    ],
                                },
                            },
                        })),
                    )
                    .await,
                "workflowArn",
            )
        }
        "idnamespace" => (
            emulator
                .create_id_namespace(&json!({ "idNamespaceName": name, "type": "SOURCE" }))
                .await,
            "idNamespaceArn",
        ),
        other => panic!("{other:?} is not a resource type a VeniceGlobalArn can name"),
    };

    assert_eq!(
        reply.status, 200,
        "creating the {resource_type} {name:?} must succeed, got {reply:?}"
    );
    let arn = string_member(&reply.body, arn_member);
    assert_eq!(
        arn,
        expected_arn(resource_type, name),
        "the create must mint the ARN for this request's own region and account"
    );
    arn
}

/// The schema mapping a workflow created for `name` points its `inputSourceConfig` at.
fn schema_of(name: &str) -> String {
    format!("{name}-schema")
}

/// Create that schema mapping, so a workflow row runs against a store that holds the
/// schema it references. Idempotent: a suite that creates a workflow of the same name
/// twice (after deleting it) finds the schema already there, and a conflict on the
/// SCHEMA is not what any test here is about.
async fn seed_schema(emulator: &Emulator, name: &str) {
    let schema = schema_of(name);
    let reply = emulator
        .create_schema_mapping(&json!({
            "schemaName": schema,
            "mappedInputFields": [
                { "fieldName": "id", "type": "UNIQUE_ID" },
                { "fieldName": "email", "type": "EMAIL_ADDRESS" },
            ],
        }))
        .await;
    assert!(
        reply.status == 200 || reply.error_code.as_deref() == Some("ConflictException"),
        "seeding the schema mapping {schema:?} failed: {reply:?}"
    );
}

/// One policy document the table spans.
struct PolicyCase {
    case: &'static str,
    document: String,
}

/// A JSON object of EXACTLY `length` bytes, so a row can sit on a published size bound.
fn document_of_length(length: usize) -> String {
    let prefix = r#"{"Version":"2012-10-17","Sid":""#;
    let suffix = r#""}"#;
    assert!(
        length >= prefix.len() + suffix.len(),
        "a JSON object cannot be shorter than {} bytes",
        prefix.len() + suffix.len()
    );
    format!(
        "{prefix}{}{suffix}",
        "p".repeat(length - prefix.len() - suffix.len())
    )
}

/// An IAM principal for a statement: this account's root user.
fn principal() -> String {
    format!("arn:aws:iam::{}:root", account_id())
}

/// The policy-document table. Rows differ in form (empty object, one statement, two
/// statements with a condition, pretty-printed with newlines) and in size (two bytes up
/// to the modelled `PolicyDocument` maximum).
fn policy_documents() -> Vec<PolicyCase> {
    let one_statement = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "AllowGetPolicy",
            "Effect": "Allow",
            "Principal": { "AWS": principal() },
            "Action": ["entityresolution:GetPolicy"],
            "Resource": "*",
        }],
    });
    let two_statements = json!({
        "Version": "2012-10-17",
        "Statement": [
            {
                "Sid": "AllowRead",
                "Effect": "Allow",
                "Principal": { "AWS": principal() },
                "Action": ["entityresolution:GetPolicy", "entityresolution:GetSchemaMapping"],
                "Resource": "*",
                "Condition": { "StringEquals": { "aws:PrincipalOrgID": "o-example" } },
            },
            {
                "Sid": "DenyPut",
                "Effect": "Deny",
                "Principal": "*",
                "Action": ["entityresolution:PutPolicy"],
                "Resource": "*",
            },
        ],
    });

    vec![
        PolicyCase {
            // The modelled `length` minimum is 1, but the smallest JSON OBJECT is `{}`:
            // a document that is not an object at all is covered by the refusal table.
            case: "the smallest JSON object",
            document: "{}".to_string(),
        },
        PolicyCase {
            case: "one Allow statement, compact",
            document: serde_json::to_string(&one_statement).expect("a document serializes"),
        },
        PolicyCase {
            case: "two statements with a condition, pretty-printed with newlines",
            document: serde_json::to_string_pretty(&two_statements)
                .expect("a document serializes"),
        },
        PolicyCase {
            case: "a document at the modelled PolicyDocument maximum of 40960 bytes",
            document: document_of_length(MAX_POLICY_DOCUMENT),
        },
        PolicyCase {
            case: "a document one byte under the modelled maximum",
            document: document_of_length(MAX_POLICY_DOCUMENT - 1),
        },
    ]
}

/// Documents the service refuses: a resource-based policy is a JSON policy document, so
/// a value that is not a JSON object is not one. Each row is inside the modelled
/// `length 1..40960`, so the boundary validator passes it and the handler is what has to
/// refuse it.
fn refused_documents() -> Vec<(&'static str, String)> {
    vec![
        (
            "the one character the modelled length minimum admits",
            "x".to_string(),
        ),
        ("a truncated object", "{\"Version\":".to_string()),
        (
            "an object with a trailing comma, which JSON does not admit",
            "{\"Version\":\"2012-10-17\",}".to_string(),
        ),
        ("a JSON array rather than an object", "[]".to_string()),
        ("a JSON string rather than an object", "\"policy\"".to_string()),
        ("a JSON number rather than an object", "42".to_string()),
        ("a JSON null rather than an object", "null".to_string()),
        (
            "a document at the modelled maximum that is not JSON",
            "p".repeat(MAX_POLICY_DOCUMENT),
        ),
    ]
}

/// Does `value` satisfy the modelled `PolicyToken`: `length 36..36`, pattern
/// `^[a-fA-F0-9]{8}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{12}$`
/// (`smithy_query(query="operation",operation="PutPolicy")`)?
fn is_modelled_policy_token(value: &str) -> bool {
    const GROUPS: [usize; 5] = [8, 4, 4, 4, 12];
    let groups: Vec<&str> = value.split('-').collect();
    value.len() == 36
        && groups.len() == GROUPS.len()
        && groups.iter().zip(GROUPS).all(|(group, width)| {
            group.len() == width && group.chars().all(|digit| digit.is_ascii_hexdigit())
        })
}

/// Assert `reply` is a successful policy response for `arn` carrying `document`, and
/// answer the revision token it reports.
fn assert_policy_reply(case: &str, reply: &Reply, arn: &str, document: &str) -> String {
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected HTTP 200, got {reply:?}"
    );
    assert_eq!(
        string_member(&reply.body, "arn"),
        arn,
        "case {case:?}: the response must name the resource the policy is attached to, \
         got {:?}",
        reply.body
    );
    assert_eq!(
        string_member(&reply.body, "policy"),
        document,
        "case {case:?}: the policy document must come back byte for byte"
    );
    let token = string_member(&reply.body, "token");
    assert!(
        is_modelled_policy_token(&token),
        "case {case:?}: expected a PolicyToken in the modelled 8-4-4-4-12 hexadecimal \
         form, got {token:?}"
    );
    token
}

#[tokio::test]
async fn a_written_policy_reads_back_byte_for_byte_for_every_resource_type() {
    let emulator = Emulator::new();
    let mut tokens = BTreeSet::new();

    for (type_index, resource_type) in POLICY_RESOURCE_TYPES.iter().enumerate() {
        for (document_index, policy) in policy_documents().into_iter().enumerate() {
            let label = format!("{resource_type}: {}", policy.case);
            let name = format!("pol-{type_index}-{document_index}");
            let arn = create_resource(&emulator, resource_type, &name).await;

            let written = put_policy(&emulator, &arn, &policy.document, None).await;
            let token = assert_policy_reply(&label, &written, &arn, &policy.document);
            assert!(
                tokens.insert(token.clone()),
                "case {label:?}: every revision must get its own token, saw {token:?} twice"
            );

            // The round trip: the reader answers the revision the write persisted.
            // Both outputs model the same three members, so the two bodies are the same
            // JSON object.
            let read = get_policy(&emulator, &arn).await;
            assert_eq!(
                read.body, written.body,
                "case {label:?}: GetPolicy must answer exactly what PutPolicy persisted, \
                 got {:?} after {:?}",
                read.body, written.body
            );

            // `GetPolicy` is `@readonly`: reading changes nothing a later caller sees,
            // and in particular does not move the revision token.
            let again = get_policy(&emulator, &arn).await;
            assert_eq!(
                again.body, read.body,
                "case {label:?}: a second read must answer identically, got {:?} after \
                 {:?}",
                again.body, read.body
            );
        }
    }
}

#[tokio::test]
async fn only_the_current_revision_token_is_accepted() {
    let emulator = Emulator::new();
    let mut tokens = BTreeSet::new();

    for (index, resource_type) in POLICY_RESOURCE_TYPES.iter().enumerate() {
        let name = format!("rev-{index}");
        let arn = create_resource(&emulator, resource_type, &name).await;
        let documents = policy_documents();

        // A write that carries no token writes unconditionally.
        let first = put_policy(&emulator, &arn, &documents[0].document, None).await;
        let first_token =
            assert_policy_reply(resource_type, &first, &arn, &documents[0].document);

        // A write that carries the CURRENT revision's token is accepted, and produces a
        // new revision with its own token.
        let second = put_policy(
            &emulator,
            &arn,
            &documents[1].document,
            Some(&first_token),
        )
        .await;
        let second_token =
            assert_policy_reply(resource_type, &second, &arn, &documents[1].document);
        assert_ne!(
            second_token, first_token,
            "{resource_type}: a new revision must get a new token, got {second_token:?} twice"
        );

        // A write that carries the SUPERSEDED token, and one that carries a token this
        // service never issued, are both refused — and change nothing.
        for (case, stale) in [
            ("the superseded revision's token", first_token.as_str()),
            ("a token this service never issued", NEVER_ISSUED_TOKEN),
        ] {
            let label = format!("{resource_type}: {case}");
            let refused =
                put_policy(&emulator, &arn, &documents[2].document, Some(stale)).await;
            assert_error(&label, &refused, 400, "ConflictException");

            let read = get_policy(&emulator, &arn).await;
            assert_eq!(
                read.body, second.body,
                "case {label:?}: a refused write must leave the standing revision \
                 exactly as it was, got {:?} after {:?}",
                read.body, second.body
            );
        }

        // ...and the token the standing revision reports is still accepted, so the
        // refusals above are about the TOKEN and not about the resource.
        let third = put_policy(
            &emulator,
            &arn,
            &documents[2].document,
            Some(&second_token),
        )
        .await;
        let third_token =
            assert_policy_reply(resource_type, &third, &arn, &documents[2].document);

        for token in [first_token, second_token, third_token] {
            assert!(
                tokens.insert(token.clone()),
                "{resource_type}: every revision of every resource must get its own \
                 token, saw {token:?} twice"
            );
        }
    }
}

#[tokio::test]
async fn a_token_is_stale_on_a_resource_that_carries_no_policy() {
    let emulator = Emulator::new();

    for (index, resource_type) in POLICY_RESOURCE_TYPES.iter().enumerate() {
        let name = format!("notoken-{index}");
        let arn = create_resource(&emulator, resource_type, &name).await;
        let document = &policy_documents()[1].document;

        // There is no current revision to match, so any token is stale.
        let refused = put_policy(&emulator, &arn, document, Some(NEVER_ISSUED_TOKEN)).await;
        assert_error(resource_type, &refused, 400, "ConflictException");

        // The refused write persisted nothing, so the resource still carries no policy.
        let read = get_policy(&emulator, &arn).await;
        assert_error(resource_type, &read, 404, "ResourceNotFoundException");

        // And the same write without a token is accepted, so the refusal was the token's
        // doing.
        let written = put_policy(&emulator, &arn, document, None).await;
        assert_policy_reply(resource_type, &written, &arn, document);
    }
}

#[tokio::test]
async fn reading_a_resource_that_carries_no_policy_is_not_found() {
    let emulator = Emulator::new();

    for (index, resource_type) in POLICY_RESOURCE_TYPES.iter().enumerate() {
        let name = format!("unset-{index}");
        let arn = create_resource(&emulator, resource_type, &name).await;

        let missing = get_policy(&emulator, &arn).await;
        assert_error(resource_type, &missing, 404, "ResourceNotFoundException");

        // The same ARN, once a policy is written, is served — so the 404 is decided by
        // what the store holds, not by the ARN's shape.
        let document = &policy_documents()[0].document;
        let written = put_policy(&emulator, &arn, document, None).await;
        assert_policy_reply(resource_type, &written, &arn, document);
        let found = get_policy(&emulator, &arn).await;
        assert_eq!(
            found.status, 200,
            "{resource_type}: expected HTTP 200 from GetPolicy, got {found:?}"
        );
    }
}

#[tokio::test]
async fn an_arn_that_names_no_resource_is_refused_by_both_operations() {
    let emulator = Emulator::new();
    let document = policy_documents()[1].document.clone();

    // A resource of each type that DOES exist, so the ARNs below differ from a resolving
    // one in exactly one segment.
    let local = create_resource(&emulator, "schemamapping", "scoped").await;

    let mut cases: Vec<(String, String, u16, &str)> = POLICY_RESOURCE_TYPES
        .iter()
        .map(|resource_type| {
            (
                format!("a {resource_type} that was never created"),
                expected_arn(resource_type, "never-created"),
                404,
                "ResourceNotFoundException",
            )
        })
        .collect();
    cases.extend([
        (
            "another partition".to_string(),
            format!(
                "arn:aws-cn:entityresolution:{REGION}:{}:schemamapping/scoped",
                account_id()
            ),
            404,
            "ResourceNotFoundException",
        ),
        (
            "another region".to_string(),
            format!(
                "arn:aws:entityresolution:eu-west-1:{}:schemamapping/scoped",
                account_id()
            ),
            404,
            "ResourceNotFoundException",
        ),
        (
            "another account".to_string(),
            format!("arn:aws:entityresolution:{REGION}:210987654321:schemamapping/scoped"),
            404,
            "ResourceNotFoundException",
        ),
        (
            // The modelled `VeniceGlobalArn` pattern admits only the four resource
            // types and only the `entityresolution` service, so an ARN outside THAT is
            // refused at the boundary with the modelled `ValidationException` instead.
            "a resource type the modelled pattern does not admit".to_string(),
            format!("arn:aws:entityresolution:{REGION}:{}:workflow/scoped", account_id()),
            400,
            "ValidationException",
        ),
        (
            "another service, which the modelled pattern does not admit".to_string(),
            format!("arn:aws:glue:{REGION}:{}:schemamapping/scoped", account_id()),
            400,
            "ValidationException",
        ),
    ]);

    for (case, arn, status, code) in cases {
        let written = put_policy(&emulator, &arn, &document, None).await;
        assert_error(&format!("PutPolicy on {case}"), &written, status, code);
        let read = get_policy(&emulator, &arn).await;
        assert_error(&format!("GetPolicy on {case}"), &read, status, code);
    }

    // ...while the ARN that differs in none of those segments resolves, and nothing the
    // refused writes above carried was persisted under it.
    let unset = get_policy(&emulator, &local).await;
    assert_error(
        "the resource that exists, before any write",
        &unset,
        404,
        "ResourceNotFoundException",
    );
    let written = put_policy(&emulator, &local, &document, None).await;
    assert_policy_reply("the resource that exists", &written, &local, &document);
}

#[tokio::test]
async fn a_policy_document_that_is_not_a_json_object_is_refused_and_changes_nothing() {
    let emulator = Emulator::new();
    let standing = policy_documents()[1].document.clone();

    for (index, (case, document)) in refused_documents().into_iter().enumerate() {
        // Once on a resource that carries no policy, and once on one that carries a
        // standing revision: a refusal must never create a policy, nor replace one.
        let empty = create_resource(&emulator, "idnamespace", &format!("bad-{index}")).await;
        let refused = put_policy(&emulator, &empty, &document, None).await;
        assert_error(case, &refused, 400, "ValidationException");
        let read = get_policy(&emulator, &empty).await;
        assert_error(case, &read, 404, "ResourceNotFoundException");

        let held = create_resource(&emulator, "schemamapping", &format!("bad-{index}")).await;
        let written = put_policy(&emulator, &held, &standing, None).await;
        let token = assert_policy_reply(case, &written, &held, &standing);
        let refused = put_policy(&emulator, &held, &document, Some(&token)).await;
        assert_error(case, &refused, 400, "ValidationException");
        let read = get_policy(&emulator, &held).await;
        assert_eq!(
            read.body, written.body,
            "case {case:?}: a refused write must leave the standing revision exactly as \
             it was, got {:?} after {:?}",
            read.body, written.body
        );

        // The resource is resolved FIRST: an ARN that names no resource answers 404
        // whatever the body carries, so a caller is told which of the two is wrong.
        let absent = expected_arn("idnamespace", &format!("bad-absent-{index}"));
        let refused = put_policy(&emulator, &absent, &document, None).await;
        assert_error(case, &refused, 404, "ResourceNotFoundException");
    }
}

#[tokio::test]
async fn policies_of_different_resources_are_independent() {
    let emulator = Emulator::new();
    let documents = policy_documents();
    let mut written: Vec<(&str, String, String, Value)> = Vec::new();

    // The same NAME under each resource type: the resource types are separate
    // namespaces, so each of these is its own resource and carries its own policy.
    for (index, resource_type) in POLICY_RESOURCE_TYPES.iter().enumerate() {
        let arn = create_resource(&emulator, resource_type, "shared").await;
        let document = documents[index % documents.len()].document.clone();
        let reply = put_policy(&emulator, &arn, &document, None).await;
        assert_policy_reply(resource_type, &reply, &arn, &document);
        written.push((resource_type, arn, document, reply.body));
    }

    for (resource_type, arn, document, body) in &written {
        let read = get_policy(&emulator, arn).await;
        assert_eq!(
            &read.body, body,
            "{resource_type}: each resource must read back its OWN revision, got {:?} \
             for the policy {document:?}",
            read.body
        );
    }
}

#[tokio::test]
async fn a_resource_that_was_deleted_and_created_again_carries_no_policy() {
    let emulator = Emulator::new();
    let document = policy_documents()[2].document.clone();

    // The resource types whose delete operation is implemented; the rule is about the
    // resource going away, not about which type it was.
    for resource_type in ["schemamapping", "matchingworkflow", "idnamespace"] {
        let name = "recreated";
        let arn = create_resource(&emulator, resource_type, name).await;
        let written = put_policy(&emulator, &arn, &document, None).await;
        let token = assert_policy_reply(resource_type, &written, &arn, &document);

        let deleted = match resource_type {
            "schemamapping" => emulator.delete_schema_mapping(name).await,
            "idnamespace" => emulator.delete_id_namespace(name).await,
            _ => emulator.delete_matching_workflow(name).await,
        };
        assert_eq!(
            deleted.status, 200,
            "{resource_type}: deleting {name:?} must succeed, got {deleted:?}"
        );

        // The resource is gone, so there is nothing to read a policy on.
        let gone = get_policy(&emulator, &arn).await;
        assert_error(resource_type, &gone, 404, "ResourceNotFoundException");

        // A resource created again under the same name is a NEW resource: it starts with
        // no policy, so the deleted one's revision is neither served nor accepted as the
        // current revision.
        let recreated = create_resource(&emulator, resource_type, name).await;
        assert_eq!(
            recreated, arn,
            "{resource_type}: the re-created resource has the same ARN, so the policy \
             store cannot tell the two apart by ARN alone"
        );
        let unset = get_policy(&emulator, &arn).await;
        assert_error(resource_type, &unset, 404, "ResourceNotFoundException");

        let refused = put_policy(&emulator, &arn, &document, Some(&token)).await;
        assert_error(resource_type, &refused, 400, "ConflictException");

        let fresh = put_policy(&emulator, &arn, &document, None).await;
        let fresh_token = assert_policy_reply(resource_type, &fresh, &arn, &document);
        assert_ne!(
            fresh_token, token,
            "{resource_type}: the re-created resource's first revision must be its own, \
             got the deleted resource's token {token:?} back"
        );
    }
}

/// The one property a response cannot show: a policy is not a resource of its own and
/// dies with the resource it is attached to, so the policy collection is bounded by the
/// LIVE resources rather than by how many have ever been policied and deleted.
///
/// The loop is the growth attempt — create a resource, attach a policy at the modelled
/// `PolicyDocument` maximum, delete the resource — repeated with a fresh name each
/// time, so every live-resource quota stays satisfied while the number of policies ever
/// written grows.
#[tokio::test]
async fn the_policy_store_stays_bounded_however_many_resources_are_policied_and_deleted() {
    let emulator = Emulator::new();
    let document = document_of_length(MAX_POLICY_DOCUMENT);

    for cycle in 0..25 {
        let name = format!("churn-{cycle}");
        let arn = create_resource(&emulator, "schemamapping", &name).await;
        let written = put_policy(&emulator, &arn, &document, None).await;
        assert_policy_reply(&name, &written, &arn, &document);

        let held = emulator
            .engine()
            .state::<ComAmazonawsEntityresolutionAwsveniceserviceState>()
            .lock()
            .policies
            .len();
        assert_eq!(
            held, 1,
            "cycle {cycle}: the store must hold only the policy of the one live \
             resource, got {held} entries"
        );

        let deleted = emulator.delete_schema_mapping(&name).await;
        assert_eq!(
            deleted.status, 200,
            "cycle {cycle}: deleting {name:?} must succeed, got {deleted:?}"
        );
    }
}

/// The same bound along the other axis a request controls: the REGION it is signed for.
///
/// A `VeniceGlobalArn` carries a region segment, and the region a request is served in
/// comes from the caller's own SigV4 credential scope rather than from the stored
/// record, so a policy collection keyed by the ARN would hold one entry per region
/// string a client chooses to sign with — each one re-validating against the same live
/// resource, so never prunable. The entry is keyed by the resource's own
/// `<resourceType>/<name>` identity instead, so ONE live resource holds ONE policy
/// however many Regions write it.
///
/// Each row is a different region spelling (AWS Region ids, plus the harness default the
/// resource was created in), writing the same resource's policy from that Region.
#[tokio::test]
async fn one_resource_holds_one_policy_whatever_region_the_write_is_signed_for() {
    // SOURCE: the region is any value a client can put in its credential scope; these
    // are AWS Region ids, with `REGION` the harness default `create_resource` used.
    const REGIONS: &[&str] = &[
        REGION,
        "us-west-2",
        "eu-central-1",
        "ap-southeast-1",
        "sa-east-1",
    ];

    let emulator = Emulator::new();
    let name = "cross-region";
    create_resource(&emulator, "schemamapping", name).await;

    let mut tokens = BTreeSet::new();
    for region in REGIONS {
        // The ARN a client configured for that Region addresses the resource with: this
        // service mints an ARN scoped to the request's own Region and account.
        let arn = format!(
            "arn:aws:entityresolution:{region}:{}:schemamapping/{name}",
            account_id()
        );
        let document = json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Sid": region,
                "Effect": "Allow",
                "Principal": { "AWS": account_id() },
                "Action": "entityresolution:GetSchemaMapping",
                "Resource": arn,
            }],
        })
        .to_string();

        let written = emulator
            .call_in_region(
                region,
                "PUT",
                &policy_path(&arn),
                Some(&json!({ "policy": document })),
            )
            .await;
        assert_eq!(
            written.status, 200,
            "case {region:?}: writing the policy from that Region must succeed, got {written:?}"
        );
        tokens.insert(string_member(&written.body, "token"));

        let keys: Vec<String> = emulator
            .engine()
            .state::<ComAmazonawsEntityresolutionAwsveniceserviceState>()
            .lock()
            .policies
            .keys()
            .cloned()
            .collect();
        assert_eq!(
            keys,
            vec![format!("schemamapping/{name}")],
            "case {region:?}: the store must hold exactly one entry, keyed by the \
             resource's own identity and carrying no region segment, got {keys:?}"
        );

        // One entry means one policy: the revision just written is what a read in that
        // Region serves back.
        let read = emulator
            .call_in_region(region, "GET", &policy_path(&arn), None)
            .await;
        assert_eq!(
            read.status, 200,
            "case {region:?}: reading the policy back must succeed, got {read:?}"
        );
        assert_eq!(
            string_member(&read.body, "policy"),
            document,
            "case {region:?}: the read must serve the revision this Region wrote"
        );
    }

    assert_eq!(
        tokens.len(),
        REGIONS.len(),
        "each write is its own revision, so each must mint its own token, got {tokens:?}"
    );
}
