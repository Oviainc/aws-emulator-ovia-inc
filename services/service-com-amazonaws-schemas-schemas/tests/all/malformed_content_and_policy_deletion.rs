// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the two things this service must REFUSE rather than
//! quietly accept: a `CreateSchema` whose `Content` is not a document of the declared
//! `Type`, and a read of a resource the store does not hold — including a resource
//! policy a delete has already removed.
//!
//! Every case drives the real wire path: a signed `restJson1` request through
//! `MultiServiceRouter::dispatch`, exactly as the production router serves it. Nothing
//! here touches the state struct directly, so a handler that answered from its input
//! instead of from the store would still have to make a later read agree.
//!
//! The HTTP harness is the crate's one harness, imported from
//! `registry_schema_lifecycle`.

use serde_json::{json, Value};
use service_com_amazonaws_schemas_schemas::ops::types::TYPE_VALUES;
use service_com_amazonaws_schemas_schemas::state::{
    CODE_MALFORMED_REQUEST, CODE_RESOURCE_NOT_FOUND,
};

use crate::registry_schema_lifecycle::{
    create_registry, registry_uri, schema_names, schema_uri, Answer, Emulator,
};
// `/v1/policy` and its `registryName` binding are encoded once, in the sibling policy
// suite, rather than a second time here.
use crate::resource_policy::policy_uri;

/// Assert an answer is the 404 this service gives for a resource it does not hold:
/// the shape a client branches on in `__type`, and the CONDITION in the error's own
/// `Code` member, with a message that says something.
fn assert_resource_not_found(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 404,
        "case {case:?}: expected 404 for a resource the store does not hold, got {} {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "NotFoundException",
        "case {case:?}: expected the wire shape NotFoundException, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.str_at("Code"),
        CODE_RESOURCE_NOT_FOUND,
        "case {case:?}: expected Code {CODE_RESOURCE_NOT_FOUND}, got {:?}",
        answer.body
    );
    assert!(
        !answer.str_at("Message").is_empty(),
        "case {case:?}: a 404 must say what was not found, got {:?}",
        answer.body
    );
}

/// One `Content` the create table sends, and whether a schema registry can store it.
#[derive(Debug)]
struct ContentCase {
    case: &'static str,
    content: String,
    /// `true` where the content is a JSON object document, which is what both
    /// modelled `Type`s are.
    storable: bool,
}

/// The table: rows differ along what the rule generalises over — whether the body
/// parses as JSON at all, where the parse fails, which JSON type it turns out to be,
/// its size and its whitespace. Each row is sent under BOTH modelled `Type`s, because
/// the requirement is the same for both.
fn content_cases() -> Vec<ContentCase> {
    let rows: Vec<(&'static str, String, bool)> = vec![
        // Not JSON at all. The first row is the recorded instance's shape: a body
        // starting with a letter a JSON parser reads as the start of a literal.
        ("not-json-text", "not a schema".to_string(), false),
        ("not-json-prose", "Allow everyone, please".to_string(), false),
        ("only-whitespace", "   \n\t ".to_string(), false),
        ("one-character", "x".to_string(), false),
        // Parses partway and then fails.
        ("truncated-object", "{\"openapi\":".to_string(), false),
        ("unterminated-string", "{\"openapi\":\"3.0.0".to_string(), false),
        ("unquoted-key", "{openapi: 3}".to_string(), false),
        ("trailing-garbage", "{} }".to_string(), false),
        ("two-documents-in-a-row", "{}{}".to_string(), false),
        // Well-formed JSON of every type that is not an object.
        ("json-array", "[{\"type\":\"object\"}]".to_string(), false),
        ("json-number", "7".to_string(), false),
        ("json-string", "\"object\"".to_string(), false),
        ("json-boolean", "true".to_string(), false),
        ("json-null", "null".to_string(), false),
        // A large body that is still not a document: size is not what decides.
        ("large-non-json", "x".repeat(50_000), false),
        // And the other side of the rule: JSON object documents, whatever they hold.
        ("empty-object", "{}".to_string(), true),
        (
            "openapi-document",
            r#"{"openapi":"3.0.0","info":{"title":"t","version":"1"},"paths":{}}"#.to_string(),
            true,
        ),
        (
            "draft4-document",
            r#"{"$schema":"http://json-schema.org/draft-04/schema#","type":"object"}"#.to_string(),
            true,
        ),
        (
            "object-padded-with-whitespace",
            "  {\n  \"openapi\" : \"3.0.0\"\n}  ".to_string(),
            true,
        ),
        (
            "object-near-the-modelled-upper-length-bound",
            // `Content` is modelled `len 1..100000`; this row sits just under the top.
            format!(r#"{{"type":"object","description":"{}"}}"#, "x".repeat(99_900)),
            true,
        ),
    ];
    rows.into_iter()
        .map(|(case, content, storable)| ContentCase {
            case,
            content,
            storable,
        })
        .collect()
}

/// RULE: a schema registry stores schema SOURCE, so `CreateSchema` stores `Content`
/// only when it parses as a document of the `Type` the request declares — a JSON
/// object. Content that does not is refused with HTTP 400 `BadRequestException`,
/// `Code` `MalformedRequest`, and NOTHING is written: the schema the request named
/// does not come into existence, so every later read of it is the service's 404, and
/// the name it asked for is still free.
#[tokio::test]
async fn create_schema_stores_only_content_that_is_a_document_of_the_declared_type() {
    let emulator = Emulator::new();
    let registry = "validated-creates";
    create_registry(&emulator, registry).await;

    // Source: `com.amazonaws.schemas#Type`, re-exported as `TYPE_VALUES` — the rule is
    // the same for every value the enum admits, so every value is tried.
    for schema_type in TYPE_VALUES {
        for row in content_cases() {
            let case = format!("{schema_type}/{}", row.case);
            // One name per row, so a row's outcome cannot depend on an earlier one.
            let name = format!("schema-{schema_type}-{}", row.case);
            let uri = schema_uri(registry, &name);

            let created = emulator
                .call(
                    "POST",
                    &uri,
                    Some(json!({"Content": row.content, "Type": schema_type})),
                )
                .await;

            if row.storable {
                assert_eq!(
                    created.status, 201,
                    "case {case:?}: a JSON object document must be stored, got {} {:?}",
                    created.status,
                    created.body.get("Message")
                );
                let described = emulator.call("GET", &uri, None).await;
                assert_eq!(
                    described.status, 200,
                    "case {case:?}: a stored schema must read back, got {} {:?}",
                    described.status, described.body
                );
                assert_eq!(
                    described.str_at("Content"),
                    row.content,
                    "case {case:?}: the stored Content must come back verbatim"
                );
                continue;
            }

            assert_eq!(
                created.status, 400,
                "case {case:?}: content that is not a document must be refused, got {} {:?}",
                created.status,
                created.body.get("Message")
            );
            assert_eq!(
                created.error_code(),
                "BadRequestException",
                "case {case:?}: expected the wire shape BadRequestException, got {:?}",
                created.body
            );
            assert_eq!(
                created.str_at("Code"),
                CODE_MALFORMED_REQUEST,
                "case {case:?}: expected Code {CODE_MALFORMED_REQUEST}, got {:?}",
                created.body
            );
            let message = created.str_at("Message");
            assert!(
                message.contains("Content"),
                "case {case:?}: the refusal must name the member it is about, got {message:?}"
            );
            assert!(
                row.content.len() < 8 || !message.contains(&row.content),
                "case {case:?}: the refusal must not echo the rejected body back, got \
                 {message:?}"
            );

            // Nothing was written: the schema does not exist, for any reader of it.
            for (operation, method, read_uri) in [
                ("DescribeSchema", "GET", uri.clone()),
                ("DeleteSchema", "DELETE", uri.clone()),
                ("ListSchemaVersions", "GET", format!("{uri}/versions")),
                (
                    "ExportSchema",
                    "GET",
                    format!("{uri}/export?type=JSONSchemaDraft4"),
                ),
            ] {
                let answer = emulator.call(method, &read_uri, None).await;
                assert_resource_not_found(&format!("{case} / {operation}"), &answer);
            }
            let listed = emulator
                .call("GET", &format!("{}/schemas", registry_uri(registry)), None)
                .await;
            assert!(
                !schema_names(&listed).contains(&name),
                "case {case:?}: a refused create must not appear in ListSchemas, got {:?}",
                schema_names(&listed)
            );

            // And the name the refused request asked for is still free.
            let accepted = emulator
                .call(
                    "POST",
                    &uri,
                    Some(json!({"Content": "{}", "Type": schema_type})),
                )
                .await;
            assert_eq!(
                accepted.status, 201,
                "case {case:?}: the refused name must still be free, got {} {:?}",
                accepted.status, accepted.body
            );
            let deleted = emulator.call("DELETE", &uri, None).await;
            assert_eq!(
                deleted.status, 204,
                "case {case:?}: the tidy-up delete must succeed, got {} {:?}",
                deleted.status, deleted.body
            );

            // Whether the body is a document is a property of the REQUEST — both
            // members are modelled `required` here — so it is decided before any
            // resource is resolved: the same body naming a registry that does not
            // exist is this same refusal, not a 404 about the registry.
            let ghost = emulator
                .call(
                    "POST",
                    &schema_uri("ghost-registry", &name),
                    Some(json!({"Content": row.content, "Type": schema_type})),
                )
                .await;
            assert_eq!(
                ghost.status, 400,
                "case {case:?}: a malformed body is refused on the request, whatever \
                 registry it names, got {} {:?}",
                ghost.status,
                ghost.body.get("Message")
            );
            assert_eq!(
                ghost.str_at("Code"),
                CODE_MALFORMED_REQUEST,
                "case {case:?}: expected Code {CODE_MALFORMED_REQUEST} for a malformed \
                 body naming an absent registry, got {:?}",
                ghost.body
            );
        }
    }
}

/// RULE: a request that names a resource the store does not hold is answered 404 with
/// the shape `NotFoundException` and the condition code `ResourceNotFound` in the
/// error's own `Code` member — the same pair for every operation, whichever resource
/// (registry, schema, schema version, code binding, resource policy) is the missing
/// one. `DeleteSchema`'s and `DeleteResourcePolicy`'s successes are an empty 204, so
/// the status and that body are all a client has to go on.
#[tokio::test]
async fn naming_a_resource_the_store_does_not_hold_is_resource_not_found() {
    let emulator = Emulator::new();
    let registry = "present-registry";
    create_registry(&emulator, registry).await;
    let present = schema_uri(registry, "present-schema");
    let created = emulator
        .call(
            "POST",
            &present,
            Some(json!({"Content": r#"{"openapi":"3.0.0"}"#, "Type": "OpenApi3"})),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "the seed create must succeed, got {} {:?}",
        created.status, created.body
    );

    // Rows: each kind of resource this service names, missing at each level — the
    // registry, the schema inside a registry that exists, a version the schema does
    // not hold, a code binding nothing started, and a resource policy nobody wrote.
    let ghost = schema_uri("ghost-registry", "any");
    let absent = schema_uri(registry, "ghost-schema");
    let cases: Vec<(&str, &str, String)> = vec![
        ("DescribeRegistry", "GET", registry_uri("ghost-registry")),
        ("DeleteRegistry", "DELETE", registry_uri("ghost-registry")),
        (
            "ListSchemas",
            "GET",
            format!("{}/schemas", registry_uri("ghost-registry")),
        ),
        (
            "SearchSchemas",
            "GET",
            format!(
                "{}/schemas/search?keywords=any",
                registry_uri("ghost-registry")
            ),
        ),
        ("DescribeSchema / absent registry", "GET", ghost.clone()),
        ("DeleteSchema / absent registry", "DELETE", ghost.clone()),
        ("DescribeSchema / absent schema", "GET", absent.clone()),
        ("DeleteSchema / absent schema", "DELETE", absent.clone()),
        (
            "ListSchemaVersions / absent schema",
            "GET",
            format!("{absent}/versions"),
        ),
        (
            "DescribeSchema / absent version",
            "GET",
            format!("{present}?schemaVersion=7"),
        ),
        (
            "DescribeCodeBinding / nothing generated",
            "GET",
            format!("{present}/language/Java8"),
        ),
        (
            "GetResourcePolicy / absent registry",
            "GET",
            policy_uri(Some("ghost-registry")),
        ),
        (
            "DeleteResourcePolicy / absent registry",
            "DELETE",
            policy_uri(Some("ghost-registry")),
        ),
        (
            "GetResourcePolicy / no policy written",
            "GET",
            policy_uri(Some(registry)),
        ),
        (
            "DeleteResourcePolicy / no policy written",
            "DELETE",
            policy_uri(Some(registry)),
        ),
    ];

    for (case, method, uri) in cases {
        let answer = emulator.call(method, &uri, None).await;
        assert_resource_not_found(case, &answer);
    }
}

/// RULE: `DeleteResourcePolicy` removes the policy from the registry it names, so the
/// deletion is observable — every later `GetResourcePolicy` of that registry reports
/// that the policy does not exist (404 `ResourceNotFound`), a second delete has
/// nothing left to remove, and a fresh `PutResourcePolicy` creates a new policy at a
/// revision no earlier one used.
#[tokio::test]
async fn a_deleted_resource_policy_is_gone_from_every_later_read() {
    let emulator = Emulator::new();

    // Rows: documents differing along what the rule generalises over — size, nesting,
    // member count and whitespace. Each row gets its own registry, so a row cannot
    // see another's policy. The third element is the bytes a READ reports where they
    // differ from the bytes sent: the store keeps a policy document's JSON CONTENT, so
    // insignificant whitespace is gone from it (the rule `resource_policy.rs` pins);
    // every other row here is already compact and reads back as it was sent.
    let documents: Vec<(&str, String, Option<String>)> = vec![
        ("the smallest JSON object there is", "{}".to_string(), None),
        (
            "a single-statement policy",
            json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Sid": "AllowDescribe",
                    "Effect": "Allow",
                    "Principal": { "AWS": "000000000000" },
                    "Action": ["schemas:DescribeRegistry"],
                    "Resource": "*"
                }]
            })
            .to_string(),
            None,
        ),
        (
            "pretty-printed, with newlines and tabs",
            "{\n\t\"Version\" : \"2012-10-17\",\n\t\"Statement\" : [ ]\n}".to_string(),
            Some(r#"{"Version":"2012-10-17","Statement":[]}"#.to_string()),
        ),
        (
            "a document of several kilobytes",
            json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Deny",
                    "Principal": "*",
                    "Action": "schemas:*",
                    "Resource": "*",
                    "Sid": "x".repeat(4_000)
                }]
            })
            .to_string(),
            None,
        ),
    ];

    let mut revisions = Vec::new();
    for (index, (case, document, read_as)) in documents.iter().enumerate() {
        let registry = format!("policy-registry-{index}");
        create_registry(&emulator, &registry).await;

        let put = emulator
            .call(
                "PUT",
                &policy_uri(Some(&registry)),
                Some(json!({ "Policy": document })),
            )
            .await;
        assert_eq!(
            put.status, 200,
            "case {case:?}: expected 200 from PutResourcePolicy, got {} {:?}",
            put.status, put.body
        );
        let first_revision = put.str_at("RevisionId");
        revisions.push(first_revision.clone());

        let got = emulator.call("GET", &policy_uri(Some(&registry)), None).await;
        assert_eq!(
            got.str_at("Policy"),
            *read_as.as_ref().unwrap_or(document),
            "case {case:?}: the policy must be readable before the delete"
        );

        let deleted = emulator
            .call("DELETE", &policy_uri(Some(&registry)), None)
            .await;
        assert_eq!(
            deleted.status, 204,
            "case {case:?}: expected 204 from DeleteResourcePolicy, got {} {:?}",
            deleted.status, deleted.body
        );
        assert_eq!(
            deleted.body,
            Value::Null,
            "case {case:?}: a 204 carries no body, got {:?}",
            deleted.body
        );

        // The deletion is observable: two reads in a row both report it gone, so the
        // 404 is the store's answer and not a one-shot.
        for read in 0..2 {
            let gone = emulator.call("GET", &policy_uri(Some(&registry)), None).await;
            assert_resource_not_found(&format!("{case} / read {read} after delete"), &gone);
        }
        // And a second delete has nothing left to remove.
        let again = emulator
            .call("DELETE", &policy_uri(Some(&registry)), None)
            .await;
        assert_resource_not_found(&format!("{case} / second delete"), &again);

        // The registry itself survived: only its policy was deleted.
        let registry_read = emulator.call("GET", &registry_uri(&registry), None).await;
        assert_eq!(
            registry_read.status, 200,
            "case {case:?}: deleting a policy must not delete the registry, got {} {:?}",
            registry_read.status, registry_read.body
        );

        // A write after the delete creates a new policy at a revision no earlier
        // policy used, and it reads back.
        let rewritten = emulator
            .call(
                "PUT",
                &policy_uri(Some(&registry)),
                Some(json!({ "Policy": "{}" })),
            )
            .await;
        assert_eq!(
            rewritten.status, 200,
            "case {case:?}: a write after the delete must create the policy, got {} {:?}",
            rewritten.status, rewritten.body
        );
        let new_revision = rewritten.str_at("RevisionId");
        assert!(
            !revisions.contains(&new_revision),
            "case {case:?}: a write after a delete must mint an unused revision, got \
             {new_revision:?} among {revisions:?}"
        );
        revisions.push(new_revision.clone());
        let reread = emulator.call("GET", &policy_uri(Some(&registry)), None).await;
        assert_eq!(
            reread.str_at("RevisionId"),
            new_revision,
            "case {case:?}: the rewritten policy reads back at its own revision"
        );
        // The stale precondition is the old revision: the delete did not preserve it.
        let stale = emulator
            .call(
                "PUT",
                &policy_uri(Some(&registry)),
                Some(json!({ "Policy": "{}", "RevisionId": first_revision })),
            )
            .await;
        assert_eq!(
            stale.status, 412,
            "case {case:?}: the deleted policy's revision must no longer satisfy a \
             precondition, got {} {:?}",
            stale.status, stale.body
        );
    }
}

/// RULE: the policy a delete removes is the one belonging to the registry the request
/// names — every other registry's policy is untouched — and a delete that names no
/// registry names no resource, so it is refused 400 `BadRequestException`.
#[tokio::test]
async fn deleting_one_registrys_policy_leaves_every_other_policy_alone() {
    let emulator = Emulator::new();
    let registries = [
        ("registry-a", r#"{"Sid":"a"}"#),
        ("registry-b", r#"{"Sid":"b"}"#),
        ("registry-c", r#"{"Sid":"c"}"#),
    ];
    for (registry, document) in registries {
        create_registry(&emulator, registry).await;
        let put = emulator
            .call(
                "PUT",
                &policy_uri(Some(registry)),
                Some(json!({ "Policy": document })),
            )
            .await;
        assert_eq!(
            put.status, 200,
            "case {registry:?}: expected 200 from PutResourcePolicy, got {} {:?}",
            put.status, put.body
        );
    }

    // A delete without `RegistryName` names no resource at all, so it must refuse
    // rather than pick one.
    let unnamed = emulator.call("DELETE", &policy_uri(None), None).await;
    assert_eq!(
        unnamed.status, 400,
        "a delete naming no registry must be refused, got {} {:?}",
        unnamed.status, unnamed.body
    );
    assert_eq!(
        unnamed.error_code(),
        "BadRequestException",
        "expected BadRequestException, got {:?}",
        unnamed.body
    );

    let deleted = emulator
        .call("DELETE", &policy_uri(Some("registry-b")), None)
        .await;
    assert_eq!(
        deleted.status, 204,
        "expected 204 from DeleteResourcePolicy, got {} {:?}",
        deleted.status, deleted.body
    );

    for (registry, document) in registries {
        let got = emulator.call("GET", &policy_uri(Some(registry)), None).await;
        if registry == "registry-b" {
            assert_resource_not_found("the deleted registry's policy", &got);
            continue;
        }
        assert_eq!(
            got.status, 200,
            "case {registry:?}: another registry's delete must not remove this policy, \
             got {} {:?}",
            got.status, got.body
        );
        assert_eq!(
            got.str_at("Policy"),
            document,
            "case {registry:?}: each registry still reports its own policy"
        );
    }
}
