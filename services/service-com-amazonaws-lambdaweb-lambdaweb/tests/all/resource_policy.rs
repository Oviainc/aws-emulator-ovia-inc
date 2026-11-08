// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for a web function's resource-based policy:
//! `PutResourcePolicy`, `GetResourcePolicy` and `DeleteResourcePolicy`.
//!
//! Every assertion goes over the wire through the same [`Emulator`] harness the
//! `web_function_lifecycle` suite uses: a request is built, dispatched through the
//! production router, and only the HTTP status, the `x-amzn-errortype` header and
//! the JSON body are inspected. Nothing reaches into the service's state struct, so
//! a policy or a `revisionId` that is not computed from persisted state cannot pass.
//!
//! The tests are tables: each row is one input along the axis the rule under test
//! generalises over (the document's form and size, how many generations the policy
//! has been through, how a document fails to be JSON, which operation carries the
//! stale revision id), and the single-statement document is only one row.

use serde_json::{json, Value};

use super::web_function_lifecycle::{build_config, percent_encoded, role_arn, Emulator, Reply};

/// The region every test signs with: the Region the function, and therefore its
/// ARN, is created in.
const REGION: &str = "us-east-1";

/// The longest `policy` the model accepts: `ResourcePolicy` is `@length(1..20480)`
/// (source: `smithy_query(query="shape", shape="ResourcePolicy")`).
const POLICY_MAX_CHARS: usize = 20480;

/// The sentence both mutating resource-policy operations refuse a stale
/// `revisionId` with: ONE fixed sentence pair for ONE conflict class, carrying no
/// revision id, ARN or function name — the id the request sent is the caller's own
/// and the current one is what `GetResourcePolicy` reports.
const STALE_REVISION_MESSAGE: &str =
    "The policy was updated or deleted by another request. Try again with the latest policy \
     revisionId.";

/// The sentence the readers report for a function that carries no policy.
const POLICY_NOT_FOUND_MESSAGE: &str = "The function does not have a policy.";

/// The sentence this service refuses a `policy` that is not a policy document
/// with: fixed text, with none of the document in it.
const POLICY_SYNTAX_MESSAGE: &str = "Policy has syntax errors.";

/// An ARN as ONE URI path segment: its `:` and `/` travel percent-encoded, the way
/// an SDK sends them, or the request addresses a different route.
fn encode(arn: &str) -> String {
    percent_encoded(arn)
}

/// Whether `value` satisfies the model's `PolicyRevisionId` pattern
/// `^[0-9a-f-]{36}$` (source: `smithy_query(query="shape",
/// shape="PolicyRevisionId")`).
fn is_modelled_revision_id(value: &str) -> bool {
    value.chars().count() == 36
        && value
            .chars()
            .all(|c| matches!(c, '0'..='9' | 'a'..='f' | '-'))
}

/// A resource-based policy document of exactly `length` characters: a statement
/// whose `Sid` is padded out, so the row differs from its neighbours only in size.
fn padded_policy(length: usize) -> String {
    let prefix = concat!(
        r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","#,
        r#""Principal":{"AWS":"111122223333"},"Action":"lambda:InvokeFunction","Sid":""#
    );
    let suffix = r#""}]}"#;
    let pad = length
        .checked_sub(prefix.len() + suffix.len())
        .expect("the padded document is at least as long as its fixed text");
    format!("{prefix}{}{suffix}", "a".repeat(pad))
}

/// Create a web function with one revision and return its `functionArn` — the
/// `resourceArn` the resource-policy operations address it by.
async fn create_function(emulator: &Emulator, name: &str) -> String {
    let created = emulator
        .send(
            "PUT",
            "/web-functions",
            REGION,
            Some(json!({
                "functionName": name,
                "revisionConfig": {
                    "buildConfig": build_config("python3.13", "demo-bucket", "app.zip", None),
                    "serviceConfig": { "executionRoleArn": role_arn(name) },
                },
            })),
        )
        .await;
    assert_eq!(
        created.status, 202,
        "case {name:?}: CreateWebFunction expected 202, got {} with {:?}",
        created.status, created.body
    );
    created
        .string("functionArn")
        .unwrap_or_else(|| panic!("case {name:?}: the create reports a functionArn"))
}

/// `PUT /resource-policy/{resourceArn}`, optionally carrying `RevisionId`.
async fn put_policy(
    emulator: &Emulator,
    resource_arn: &str,
    policy: &str,
    revision_id: Option<&str>,
) -> Reply {
    let mut body = json!({ "Policy": policy });
    if let Some(revision_id) = revision_id {
        body["RevisionId"] = json!(revision_id);
    }
    emulator
        .send(
            "PUT",
            &format!("/resource-policy/{}", encode(resource_arn)),
            REGION,
            Some(body),
        )
        .await
}

/// `GET /resource-policy/{resourceArn}`.
async fn get_policy(emulator: &Emulator, resource_arn: &str) -> Reply {
    emulator
        .send(
            "GET",
            &format!("/resource-policy/{}", encode(resource_arn)),
            REGION,
            None,
        )
        .await
}

/// `DELETE /resource-policy/{resourceArn}`, optionally carrying the `RevisionId`
/// query parameter.
async fn delete_policy(
    emulator: &Emulator,
    resource_arn: &str,
    revision_id: Option<&str>,
) -> Reply {
    let path = match revision_id {
        Some(revision_id) => format!(
            "/resource-policy/{}?RevisionId={revision_id}",
            encode(resource_arn)
        ),
        None => format!("/resource-policy/{}", encode(resource_arn)),
    };
    emulator.send("DELETE", &path, REGION, None).await
}

/// The `(Policy, RevisionId)` a successful put or get reports.
fn reported(case: &str, reply: &Reply) -> (String, String) {
    let policy = reply
        .string("Policy")
        .unwrap_or_else(|| panic!("case {case}: expected a Policy member, got {:?}", reply.body));
    let revision_id = reply.string("RevisionId").unwrap_or_else(|| {
        panic!(
            "case {case}: expected a RevisionId member, got {:?}",
            reply.body
        )
    });
    (policy, revision_id)
}

/// One row of the document table: what makes it different, and the document.
struct DocumentCase {
    case: &'static str,
    document: String,
}

/// The documents a put must accept: every one is a JSON object, and they differ
/// along the axis the rule generalises over — one statement or several, the
/// statement given as an object or as an array, which elements it carries, and the
/// document's size, including exactly the longest the model allows.
fn accepted_documents(resource_arn: &str) -> Vec<DocumentCase> {
    vec![
        DocumentCase {
            case: "one statement in an array",
            document: json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Sid": "invoke",
                    "Effect": "Allow",
                    "Principal": { "AWS": "111122223333" },
                    "Action": "lambda:InvokeFunction",
                    "Resource": resource_arn,
                }],
            })
            .to_string(),
        },
        DocumentCase {
            case: "the statement given as a single object",
            document: json!({
                "Version": "2012-10-17",
                "Statement": {
                    "Effect": "Allow",
                    "Principal": { "Service": "apigateway.amazonaws.com" },
                    "Action": "lambda:InvokeFunction",
                    "Resource": resource_arn,
                },
            })
            .to_string(),
        },
        DocumentCase {
            case: "two statements, one of them a Deny with a Condition",
            document: json!({
                "Version": "2012-10-17",
                "Statement": [
                    {
                        "Effect": "Allow",
                        "Principal": "*",
                        "Action": ["lambda:InvokeFunction", "lambda:GetWebFunction"],
                        "Resource": resource_arn,
                    },
                    {
                        "Effect": "Deny",
                        "Principal": "*",
                        "Action": "lambda:InvokeFunction",
                        "Resource": resource_arn,
                        "Condition": { "StringNotEquals": { "aws:PrincipalOrgID": "o-abc123" } },
                    },
                ],
            })
            .to_string(),
        },
        DocumentCase {
            case: "a JSON object carrying no elements",
            document: "{}".to_string(),
        },
        DocumentCase {
            case: "a document of exactly the longest length the model allows",
            document: padded_policy(POLICY_MAX_CHARS),
        },
    ]
}

#[tokio::test]
async fn a_put_policy_is_reported_back_by_get_verbatim() {
    // RULE: a put stores the document the caller sent and reports it with the
    // `revisionId` of the generation it wrote; a read reports that same pair until
    // the next mutation. The rows span the documents a JSON-formatted policy can
    // take, including the longest the model allows.
    for DocumentCase { case, document } in accepted_documents("arn:aws:lambda:*:*:web-function/*") {
        let emulator = Emulator::new();
        let arn = create_function(&emulator, "policy-roundtrip").await;

        let put = put_policy(&emulator, &arn, &document, None).await;
        assert_eq!(
            put.status, 200,
            "case {case}: PutResourcePolicy expected 200, got {} with {:?}",
            put.status, put.body
        );
        let (stored, revision_id) = reported(case, &put);
        assert_eq!(
            stored, document,
            "case {case}: the put reports the document it stored, got {stored:?}"
        );
        assert!(
            is_modelled_revision_id(&revision_id),
            "case {case}: expected a modelled PolicyRevisionId, got {revision_id:?}"
        );

        // Read it back, twice: a read reports the stored pair and moves nothing.
        for round in 1..=2 {
            let got = get_policy(&emulator, &arn).await;
            assert_eq!(
                got.status, 200,
                "case {case} read {round}: GetResourcePolicy expected 200, got {} with {:?}",
                got.status, got.body
            );
            assert_eq!(
                reported(case, &got),
                (document.clone(), revision_id.clone()),
                "case {case} read {round}: a read reports the stored policy and its revision, \
                 got {:?}",
                got.body
            );
        }
    }
}

#[tokio::test]
async fn each_generation_mints_a_fresh_revision_id_and_the_reader_follows_it() {
    // RULE: every successful put is a mutation: it mints a fresh `revisionId` and
    // the reader reports the new pair. The rows are the successive generations of
    // ONE policy, each a different document.
    let emulator = Emulator::new();
    let arn = create_function(&emulator, "policy-generations").await;

    let documents = accepted_documents(&arn);
    let mut seen_revision_ids: Vec<String> = Vec::new();
    let mut carry: Option<String> = None;

    for (generation, DocumentCase { case, document }) in documents.into_iter().enumerate() {
        // Half the generations assert the current revision id, half send none:
        // "If you don't specify a value, the update proceeds without checking the
        // current revision."
        let supplied = if generation % 2 == 0 {
            None
        } else {
            carry.clone()
        };
        let put = put_policy(&emulator, &arn, &document, supplied.as_deref()).await;
        assert_eq!(
            put.status, 200,
            "case generation {generation} ({case}): PutResourcePolicy expected 200, got {} \
             with {:?}",
            put.status, put.body
        );
        let (stored, revision_id) = reported(case, &put);
        assert_eq!(
            stored, document,
            "case generation {generation} ({case}): the put reports the document it stored, \
             got {stored:?}"
        );
        assert!(
            !seen_revision_ids.contains(&revision_id),
            "case generation {generation} ({case}): every generation mints a fresh revision id, \
             got {revision_id:?} after {seen_revision_ids:?}"
        );

        let got = get_policy(&emulator, &arn).await;
        assert_eq!(
            reported(case, &got),
            (document.clone(), revision_id.clone()),
            "case generation {generation} ({case}): the reader follows the newest generation, \
             got {:?}",
            got.body
        );

        seen_revision_ids.push(revision_id.clone());
        carry = Some(revision_id);
    }
}

#[tokio::test]
async fn a_stale_revision_id_is_a_conflict_that_changes_nothing() {
    // RULE: a request that names a `revisionId` may act only on the generation that
    // id identifies; any other id is a 409 conflict and the stored policy is left
    // as it was. The rows span which operation carries the stale id and which stale
    // id it is — a superseded generation's, and one this service never minted.
    let emulator = Emulator::new();
    let arn = create_function(&emulator, "policy-stale-revision").await;

    let documents = accepted_documents(&arn);
    // The id below is 36 characters of the modelled alphabet, so the boundary
    // validator passes it and the handler is what refuses it.
    let never_minted = "ffffffff-ffff-ffff-ffff-ffffffffffff";
    let mut superseded: Vec<String> = Vec::new();
    let mut current = String::new();

    for (generation, DocumentCase { case, document }) in documents.into_iter().enumerate() {
        let put = put_policy(&emulator, &arn, &document, None).await;
        assert_eq!(
            put.status, 200,
            "case generation {generation} ({case}): PutResourcePolicy expected 200, got {} \
             with {:?}",
            put.status, put.body
        );
        let (_, revision_id) = reported(case, &put);
        if !current.is_empty() {
            superseded.push(current.clone());
        }
        current = revision_id;

        let mut stale: Vec<String> = superseded.clone();
        stale.push(never_minted.to_string());
        for id in &stale {
            for (operation, refused) in [
                (
                    "PutResourcePolicy",
                    put_policy(&emulator, &arn, "{}", Some(id)).await,
                ),
                (
                    "DeleteResourcePolicy",
                    delete_policy(&emulator, &arn, Some(id)).await,
                ),
            ] {
                assert_eq!(
                    refused.status, 409,
                    "case generation {generation} ({operation} with the stale id {id:?}): \
                     expected HTTP 409, got {} with {:?}",
                    refused.status, refused.body
                );
                assert_eq!(
                    refused.error(),
                    "ConflictException",
                    "case generation {generation} ({operation} with the stale id {id:?}): \
                     expected ConflictException, got {:?}",
                    refused.body
                );
                assert_eq!(
                    refused.string("message").as_deref(),
                    Some(STALE_REVISION_MESSAGE),
                    "case generation {generation} ({operation} with the stale id {id:?}): \
                     expected the stale-revision sentence, got {:?}",
                    refused.body
                );

                // ... and the stored policy is untouched by the refusal.
                let got = get_policy(&emulator, &arn).await;
                assert_eq!(
                    reported(case, &got),
                    (document.clone(), current.clone()),
                    "case generation {generation} ({operation} with the stale id {id:?}): \
                     a refused request leaves the policy as it was, got {:?}",
                    got.body
                );
            }
        }
    }
}

#[tokio::test]
async fn every_stale_revision_conflict_reports_the_same_fixed_sentence() {
    // RULE: the refusal of a `revisionId` that is not the stored policy's current
    // generation is ONE conflict class, so both mutating resource-policy operations
    // report the same fixed sentence for it, however the id came to be stale and
    // whatever the request carries — the sentence templates nothing, so neither the
    // supplied id, nor the current one, nor the ARN or function name appears in it.
    // The rows span how stale the id is: one generation back, two, three, an id this
    // service never minted, and the id of a policy that is gone altogether (the
    // "deleted" half of the class).
    let emulator = Emulator::new();
    let name = "policy-stale-sentence";
    let arn = create_function(&emulator, name).await;

    // Four successive generations of the same policy: each put mints a fresh id, so
    // the ids of the earlier ones are stale by one, two and three generations once
    // the chain is complete.
    let mut chain: Vec<String> = Vec::new();
    for (generation, DocumentCase { case, document }) in
        accepted_documents(&arn).into_iter().take(4).enumerate()
    {
        let put = put_policy(&emulator, &arn, &document, None).await;
        assert_eq!(
            put.status, 200,
            "case generation {generation} ({case}): PutResourcePolicy expected 200, got {} \
             with {:?}",
            put.status, put.body
        );
        let (_, revision_id) = reported(case, &put);
        chain.push(revision_id);
    }
    let current = chain
        .pop()
        .expect("the chain holds every generation that was put");

    // 36 characters of the modelled alphabet, so the boundary validator passes it
    // and the handler is what refuses it.
    let never_minted = "ffffffff-ffff-ffff-ffff-ffffffffffff".to_string();
    let mut stale: Vec<(String, String)> = Vec::new();
    for (back, id) in chain.iter().rev().enumerate() {
        stale.push((format!("{} generation(s) stale", back + 1), id.clone()));
    }
    stale.push((
        "an id this service never minted".to_string(),
        never_minted.clone(),
    ));

    for (case, id) in &stale {
        for (operation, refused) in [
            (
                "PutResourcePolicy",
                put_policy(&emulator, &arn, "{}", Some(id)).await,
            ),
            (
                "DeleteResourcePolicy",
                delete_policy(&emulator, &arn, Some(id)).await,
            ),
        ] {
            assert_conflict(case, operation, &refused, &[id, &current, &arn, name]);
        }
    }

    // The other half of the class: the policy is GONE, so the id the caller holds
    // identifies no generation at all and a put naming it is the same conflict.
    let removed = delete_policy(&emulator, &arn, Some(&current)).await;
    assert_eq!(
        removed.status, 204,
        "case the policy is deleted: DeleteResourcePolicy expected 204, got {} with {:?}",
        removed.status, removed.body
    );
    for (case, id) in [
        ("the id of a policy that was deleted", &current),
        ("an id this service never minted", &never_minted),
    ] {
        let refused = put_policy(&emulator, &arn, "{}", Some(id)).await;
        assert_conflict(case, "PutResourcePolicy", &refused, &[id, &arn, name]);
    }
}

/// Assert `refused` is the modelled stale-revision conflict: HTTP 409,
/// `ConflictException`, and exactly the fixed sentence, with none of `values`
/// (the supplied id, the current id, the ARN, the function name) templated into it.
fn assert_conflict(case: &str, operation: &str, refused: &Reply, values: &[&str]) {
    assert_eq!(
        refused.status, 409,
        "case {case:?} ({operation}): expected HTTP 409, got {} with {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error(),
        "ConflictException",
        "case {case:?} ({operation}): expected ConflictException, got {:?}",
        refused.body
    );
    let message = refused.string("message").unwrap_or_default();
    assert_eq!(
        message, STALE_REVISION_MESSAGE,
        "case {case:?} ({operation}): expected the stale-revision sentence, got {message:?}"
    );
    for value in values {
        assert!(
            !message.contains(*value),
            "case {case:?} ({operation}): the sentence templates nothing, yet it carries \
             {value:?}: {message:?}"
        );
    }
}

#[tokio::test]
async fn a_document_that_is_not_a_json_object_is_refused_and_changes_nothing() {
    // RULE: `policy` is "The JSON-formatted resource-based policy to attach to the
    // web function", so a document that is not JSON — or is JSON that is not an
    // object — is a 400 and nothing is written. The rows span the ways a document
    // fails that: truncated JSON, text that is no JSON at all, and each other JSON
    // type a top-level value can have.
    let malformed: [(&str, &str); 8] = [
        ("a truncated object", "{"),
        ("an unterminated string member", r#"{"Statement": "#),
        ("text that is not JSON at all", "this is not a policy"),
        ("the empty-looking document", " "),
        ("a JSON array", r#"[{"Effect":"Allow"}]"#),
        ("a JSON string", r#""Allow""#),
        ("a JSON number", "17"),
        ("a JSON null", "null"),
    ];

    let emulator = Emulator::new();
    let arn = create_function(&emulator, "policy-malformed").await;
    let held = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "AWS": "111122223333" },
            "Action": "lambda:InvokeFunction",
            "Resource": arn,
        }],
    })
    .to_string();
    let attached = put_policy(&emulator, &arn, &held, None).await;
    let (_, held_revision_id) = reported("the held policy", &attached);

    for (case, document) in malformed {
        let refused = put_policy(&emulator, &arn, document, None).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400, got {} with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            refused.body
        );
        assert_eq!(
            refused.string("message").as_deref(),
            Some(POLICY_SYNTAX_MESSAGE),
            "case {case:?}: expected the service's policy-syntax sentence, got {:?}",
            refused.body
        );

        let got = get_policy(&emulator, &arn).await;
        assert_eq!(
            reported(case, &got),
            (held.clone(), held_revision_id.clone()),
            "case {case:?}: a refused put leaves the attached policy as it was, got {:?}",
            got.body
        );
    }

    // And one character past the modelled maximum is refused the same way, by the
    // boundary validator, with nothing written.
    let too_long = padded_policy(POLICY_MAX_CHARS + 1);
    let refused = put_policy(&emulator, &arn, &too_long, None).await;
    assert_eq!(
        refused.status, 400,
        "case a document one character past the modelled maximum: expected HTTP 400, got {} \
         with {:?}",
        refused.status, refused.body
    );
    let got = get_policy(&emulator, &arn).await;
    assert_eq!(
        reported("after the over-long document", &got),
        (held, held_revision_id),
        "case a document one character past the modelled maximum: the attached policy is \
         untouched, got {:?}",
        got.body
    );
}

#[tokio::test]
async fn a_deleted_policy_is_gone_and_a_deleted_function_takes_its_policy_with_it() {
    // RULE: a delete that passes the revision check removes the policy, and a
    // function's own delete takes its policy with it; a function that carries no
    // policy answers the readers 404. The rows span how the delete is addressed —
    // naming the current revision id and naming none ("the policy is deleted
    // regardless of its current revision") — and how the policy goes: by its own
    // delete, and by the function's.
    let cases: [(&str, bool); 2] = [
        ("the delete names the current revision id", true),
        ("the delete names no revision id", false),
    ];

    for (case, name_revision) in cases {
        let emulator = Emulator::new();
        let arn = create_function(&emulator, "policy-removal").await;
        let document = json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Principal": { "AWS": "111122223333" },
                "Action": "lambda:InvokeFunction",
                "Resource": arn,
            }],
        })
        .to_string();

        let put = put_policy(&emulator, &arn, &document, None).await;
        let (_, revision_id) = reported(case, &put);
        let supplied = if name_revision {
            Some(revision_id.as_str())
        } else {
            None
        };

        let deleted = delete_policy(&emulator, &arn, supplied).await;
        assert_eq!(
            deleted.status, 204,
            "case {case:?}: DeleteResourcePolicy expected 204, got {} with {:?}",
            deleted.status, deleted.body
        );

        // The policy is gone: both readers answer the modelled not-found, and so
        // does a second delete.
        for (operation, gone) in [
            ("GetResourcePolicy", get_policy(&emulator, &arn).await),
            (
                "DeleteResourcePolicy",
                delete_policy(&emulator, &arn, None).await,
            ),
        ] {
            assert_eq!(
                gone.status, 404,
                "case {case:?} ({operation} after the delete): expected HTTP 404, got {} \
                 with {:?}",
                gone.status, gone.body
            );
            assert_eq!(
                gone.error(),
                "ResourceNotFoundException",
                "case {case:?} ({operation} after the delete): expected \
                 ResourceNotFoundException, got {:?}",
                gone.body
            );
            assert_eq!(
                gone.string("message").as_deref(),
                Some(POLICY_NOT_FOUND_MESSAGE),
                "case {case:?} ({operation} after the delete): expected the no-policy sentence, \
                 got {:?}",
                gone.body
            );
        }

        // Attach a policy again, then delete the FUNCTION: the policy goes with it,
        // so a function created again under the same name carries none.
        let reattached = put_policy(&emulator, &arn, &document, None).await;
        assert_eq!(
            reattached.status, 200,
            "case {case:?}: the policy can be attached again, got {} with {:?}",
            reattached.status, reattached.body
        );
        let function_deleted = emulator
            .send(
                "DELETE",
                &format!("/web-functions/{}", encode(&arn)),
                REGION,
                None,
            )
            .await;
        assert_eq!(
            function_deleted.status, 204,
            "case {case:?}: DeleteWebFunction expected 204, got {} with {:?}",
            function_deleted.status, function_deleted.body
        );

        let recreated = create_function(&emulator, "policy-removal").await;
        assert_eq!(
            recreated, arn,
            "case {case:?}: the function is created again under the same ARN, got {recreated:?}"
        );
        let cascaded = get_policy(&emulator, &arn).await;
        assert_eq!(
            cascaded.status, 404,
            "case {case:?}: a function's delete takes its resource policy with it, got {} \
             with {:?}",
            cascaded.status, cascaded.body
        );
    }
}

#[tokio::test]
async fn a_resource_arn_that_names_no_stored_function_is_not_found() {
    // RULE: the resource-policy operations address a web function by the
    // `web-function/<name>` segment of the ARN they carry — the same resolution
    // every other reader in this crate applies — so an ARN whose segment names no
    // stored function is the modelled not-found, on every one of them. The rows
    // differ along how that name misses: a name never created, one differing only
    // in case, one that extends the stored name and one that is a prefix of it.
    let emulator = Emulator::new();
    let held = create_function(&emulator, "policy-arn-resolution").await;

    let absent: Vec<(&str, String)> = vec![
        (
            "a function that was never created",
            held.replace("policy-arn-resolution", "never-created"),
        ),
        (
            "a name that differs only in case",
            held.replace("policy-arn-resolution", "Policy-Arn-Resolution"),
        ),
        (
            "a name that extends the stored one",
            held.replace("policy-arn-resolution", "policy-arn-resolution-2"),
        ),
        (
            "a name that is a prefix of the stored one",
            held.replace("policy-arn-resolution", "policy-arn"),
        ),
    ];

    for (case, arn) in &absent {
        for (operation, reply) in [
            (
                "PutResourcePolicy",
                put_policy(&emulator, arn, "{}", None).await,
            ),
            ("GetResourcePolicy", get_policy(&emulator, arn).await),
            (
                "DeleteResourcePolicy",
                delete_policy(&emulator, arn, None).await,
            ),
        ] {
            assert_eq!(
                reply.status, 404,
                "case {case:?} ({operation} on {arn}): expected HTTP 404, got {} with {:?}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error(),
                "ResourceNotFoundException",
                "case {case:?} ({operation} on {arn}): expected ResourceNotFoundException, \
                 got {:?}",
                reply.body
            );
            assert_eq!(
                reply
                    .field("resourceType")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                "WebFunction",
                "case {case:?} ({operation} on {arn}): the refusal names the resource that is \
                 missing, got {:?}",
                reply.body
            );
        }
    }

    // The function that WAS created still has no policy, which is a different
    // not-found: the policy sub-resource, not the function.
    let no_policy = get_policy(&emulator, &held).await;
    assert_eq!(
        no_policy.status, 404,
        "a function with no policy attached: expected HTTP 404, got {} with {:?}",
        no_policy.status, no_policy.body
    );
    assert_eq!(
        no_policy
            .field("resourceType")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        "ResourcePolicy",
        "a function with no policy attached: the refusal names the policy, got {:?}",
        no_policy.body
    );
}
