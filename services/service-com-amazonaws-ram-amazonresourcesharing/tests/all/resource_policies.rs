// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `GetResourcePolicies`.
//!
//! The rule the rows below pin: a read reports, for each resource ARN it names, the
//! resource-based policy documents RAM has instantiated on that resource — one per
//! share that shares it, with `Resource` taken from the resource's ARN and
//! `Principal` from the identities added to the share, and only for the identities
//! outside the owning account's organization, because an identity inside it reaches
//! the resource through Organizations rather than through a policy on the resource.
//! A resource ARN no LIVE share currently shares is refused, because policies are
//! reported only for the resources the caller owns and has shared.
//!
//! So the rows vary along what the rule generalises over:
//!
//! * WHICH identities a share adds — none, the owning account, the owner's own
//!   organization, an organizational unit, an organization ARN, an account outside
//!   the organization, a role outside it, several of them, a mix of inside and
//!   outside — and how many resources and shares are involved;
//! * WHETHER a live share still shares the resource: never shared, shared,
//!   disassociated, re-associated, shared by a second share, every share deleted;
//! * the paging and the `principal` filter over a set of documents larger than one;
//! * the values this read refuses outright: an empty list, a value that is not an
//!   ARN, an empty principal, a `nextToken` it never issued, and a `maxResults`
//!   outside the modelled range.

use serde_json::{json, Value};
use service_com_amazonaws_ram_amazonresourcesharing::state::{
    MAX_ENTITIES_PER_CALL, MAX_PAGE_SIZE, POLICY_EFFECT_ALLOW, POLICY_LANGUAGE_VERSION,
};
use smithy_server_runtime::MultiServiceRouter;

// Every suite in this crate calls the service the one way: through `tests/all/harness.rs`.
use super::harness::*;

/// An account outside the one that owns the shares, so a principal in it is external.
const FOREIGN_ACCOUNT: &str = "111122223333";
/// A second one, so a row can share with more than one external account.
const OTHER_FOREIGN_ACCOUNT: &str = "444455556666";
/// An IAM role in the first of them: an external principal that names a role rather
/// than a whole account.
const FOREIGN_ROLE: &str = "arn:aws:iam::111122223333:role/Reader";
/// An organization in the owner's own organization, which RAM shares with directly
/// rather than through a policy on the resource.
const OWN_ORGANIZATION: &str = "o-abc1234567";
/// An organizational unit of it, named by its bare id.
const OWN_ORGANIZATIONAL_UNIT: &str = "ou-abc1-12345678";

/// A row of the reported-documents table.
struct ShareCase {
    case: &'static str,
    /// The identities the share is created with.
    principals: Vec<String>,
    /// The resources the share is created with.
    resources: Vec<String>,
    /// What the read asks about, which may be fewer resources than the share holds,
    /// or one of them twice.
    asked: Vec<String>,
    /// The `principal` filter the read names, or `None` for every principal.
    principal: Option<String>,
    /// The documents the read must report: one `(resource ARN, principals)` pair per
    /// document, in the order reported.
    expected: Vec<(String, Vec<String>)>,
}

#[tokio::test]
async fn a_shared_resource_reports_the_policy_its_share_instantiates() {
    let cases = vec![
        ShareCase {
            case: "a share with no principal",
            principals: vec![],
            resources: vec![LOCAL_RESOURCE.to_string()],
            asked: vec![LOCAL_RESOURCE.to_string()],
            principal: None,
            expected: vec![],
        },
        ShareCase {
            case: "a share with the owning account itself",
            principals: vec![ACCOUNT.to_string()],
            resources: vec![LOCAL_RESOURCE.to_string()],
            asked: vec![LOCAL_RESOURCE.to_string()],
            principal: None,
            expected: vec![],
        },
        ShareCase {
            case: "a share with the owner's own organization",
            principals: vec![OWN_ORGANIZATION.to_string()],
            resources: vec![LOCAL_RESOURCE.to_string()],
            asked: vec![LOCAL_RESOURCE.to_string()],
            principal: None,
            expected: vec![],
        },
        ShareCase {
            case: "a share with an organizational unit of it",
            principals: vec![OWN_ORGANIZATIONAL_UNIT.to_string()],
            resources: vec![LOCAL_RESOURCE.to_string()],
            asked: vec![LOCAL_RESOURCE.to_string()],
            principal: None,
            expected: vec![],
        },
        ShareCase {
            case: "a share with an organization named by its ARN",
            principals: vec![format!(
                "arn:aws:organizations::{ACCOUNT}:organization/{OWN_ORGANIZATION}"
            )],
            resources: vec![LOCAL_RESOURCE.to_string()],
            asked: vec![LOCAL_RESOURCE.to_string()],
            principal: None,
            expected: vec![],
        },
        ShareCase {
            case: "a share with an account outside the organization",
            principals: vec![FOREIGN_ACCOUNT.to_string()],
            resources: vec![LOCAL_RESOURCE.to_string()],
            asked: vec![LOCAL_RESOURCE.to_string()],
            principal: None,
            expected: vec![(
                LOCAL_RESOURCE.to_string(),
                vec![FOREIGN_ACCOUNT.to_string()],
            )],
        },
        ShareCase {
            case: "a share with a role outside the organization",
            principals: vec![FOREIGN_ROLE.to_string()],
            resources: vec![LOCAL_RESOURCE.to_string()],
            asked: vec![LOCAL_RESOURCE.to_string()],
            principal: None,
            expected: vec![(LOCAL_RESOURCE.to_string(), vec![FOREIGN_ROLE.to_string()])],
        },
        ShareCase {
            case: "a share with two accounts outside the organization",
            principals: vec![
                FOREIGN_ACCOUNT.to_string(),
                OTHER_FOREIGN_ACCOUNT.to_string(),
            ],
            resources: vec![LOCAL_RESOURCE.to_string()],
            asked: vec![LOCAL_RESOURCE.to_string()],
            principal: None,
            expected: vec![(
                LOCAL_RESOURCE.to_string(),
                vec![
                    FOREIGN_ACCOUNT.to_string(),
                    OTHER_FOREIGN_ACCOUNT.to_string(),
                ],
            )],
        },
        ShareCase {
            case: "a share with identities inside and outside the organization",
            principals: vec![
                OWN_ORGANIZATION.to_string(),
                ACCOUNT.to_string(),
                FOREIGN_ACCOUNT.to_string(),
            ],
            resources: vec![LOCAL_RESOURCE.to_string()],
            asked: vec![LOCAL_RESOURCE.to_string()],
            principal: None,
            // Only the identity the owner's organization does not reach is named.
            expected: vec![(
                LOCAL_RESOURCE.to_string(),
                vec![FOREIGN_ACCOUNT.to_string()],
            )],
        },
        ShareCase {
            case: "two resources shared outside the organization",
            principals: vec![FOREIGN_ACCOUNT.to_string()],
            resources: vec![LOCAL_RESOURCE.to_string(), LOCAL_RESOURCE_TWO.to_string()],
            asked: vec![LOCAL_RESOURCE.to_string(), LOCAL_RESOURCE_TWO.to_string()],
            principal: None,
            expected: vec![
                (
                    LOCAL_RESOURCE.to_string(),
                    vec![FOREIGN_ACCOUNT.to_string()],
                ),
                (
                    LOCAL_RESOURCE_TWO.to_string(),
                    vec![FOREIGN_ACCOUNT.to_string()],
                ),
            ],
        },
        ShareCase {
            case: "two resources, one of them read",
            principals: vec![FOREIGN_ACCOUNT.to_string()],
            resources: vec![LOCAL_RESOURCE.to_string(), LOCAL_RESOURCE_TWO.to_string()],
            asked: vec![LOCAL_RESOURCE_TWO.to_string()],
            principal: None,
            expected: vec![(
                LOCAL_RESOURCE_TWO.to_string(),
                vec![FOREIGN_ACCOUNT.to_string()],
            )],
        },
        ShareCase {
            case: "one resource named twice",
            principals: vec![FOREIGN_ACCOUNT.to_string()],
            resources: vec![LOCAL_RESOURCE.to_string()],
            asked: vec![LOCAL_RESOURCE.to_string(), LOCAL_RESOURCE.to_string()],
            principal: None,
            // Naming a resource twice asks about one resource, so it is reported once.
            expected: vec![(
                LOCAL_RESOURCE.to_string(),
                vec![FOREIGN_ACCOUNT.to_string()],
            )],
        },
        ShareCase {
            case: "read for a principal the share shares with",
            principals: vec![
                FOREIGN_ACCOUNT.to_string(),
                OTHER_FOREIGN_ACCOUNT.to_string(),
            ],
            resources: vec![LOCAL_RESOURCE.to_string()],
            asked: vec![LOCAL_RESOURCE.to_string()],
            principal: Some(FOREIGN_ACCOUNT.to_string()),
            expected: vec![(
                LOCAL_RESOURCE.to_string(),
                vec![
                    FOREIGN_ACCOUNT.to_string(),
                    OTHER_FOREIGN_ACCOUNT.to_string(),
                ],
            )],
        },
        ShareCase {
            case: "read for a principal the share does not share with",
            principals: vec![FOREIGN_ACCOUNT.to_string()],
            resources: vec![LOCAL_RESOURCE.to_string()],
            asked: vec![LOCAL_RESOURCE.to_string()],
            principal: Some(OTHER_FOREIGN_ACCOUNT.to_string()),
            expected: vec![],
        },
        ShareCase {
            case: "a resource in another account, shared outside the organization",
            principals: vec![FOREIGN_ACCOUNT.to_string()],
            resources: vec![FOREIGN_RESOURCE.to_string()],
            asked: vec![FOREIGN_RESOURCE.to_string()],
            principal: None,
            expected: vec![(
                FOREIGN_RESOURCE.to_string(),
                vec![FOREIGN_ACCOUNT.to_string()],
            )],
        },
    ];

    for case in cases {
        let router = emulator();
        let mut create_body = json!({
            "name": "policies",
            "resourceArns": case.resources,
        });
        if !case.principals.is_empty() {
            create_body["principals"] = json!(case.principals);
        }
        create_ok(&router, case.case, &create_body).await;

        let mut read = json!({"resourceArns": case.asked});
        if let Some(principal) = &case.principal {
            read["principal"] = json!(principal);
        }
        let (status, body) = get_policies(&router, &read).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200, got {status} with {body:?}",
            case.case
        );
        assert_documents(case.case, &body, &case.expected);
        // A page that reported everything there was reports no continuation.
        assert!(
            body.get("nextToken").and_then(Value::as_str).is_none(),
            "case {:?}: a complete page must report no nextToken, got {body:?}",
            case.case
        );

        // The read reports and does not mutate: repeating it answers the same thing.
        let (again_status, again) = get_policies(&router, &read).await;
        assert_eq!(
            (again_status, &again),
            (status, &body),
            "case {:?}: the second read answered differently, got {again:?}",
            case.case
        );
    }
}

#[tokio::test]
async fn a_resource_several_shares_share_reports_one_document_for_each() {
    let router = emulator();
    // Two shares of the SAME resource, each with a different external identity: the
    // resource carries one instantiated policy per share.
    let first = create_ok(
        &router,
        "two shares",
        &json!({
            "name": "first",
            "resourceArns": [LOCAL_RESOURCE],
            "principals": [FOREIGN_ACCOUNT],
        }),
    )
    .await;
    let second = create_ok(
        &router,
        "two shares",
        &json!({
            "name": "second",
            "resourceArns": [LOCAL_RESOURCE],
            "principals": [OTHER_FOREIGN_ACCOUNT],
        }),
    )
    .await;
    let (first_arn, second_arn) = (
        arn_of("two shares", &first),
        arn_of("two shares", &second),
    );

    let (status, body) = get_policies(&router, &json!({"resourceArns": [LOCAL_RESOURCE]})).await;
    assert_eq!(
        status, 200,
        "case \"two shares\": expected 200, got {status} with {body:?}"
    );
    // The documents come in share ARN order, so the row is built in that order too.
    let mut expected = [
        (
            first_arn.clone(),
            LOCAL_RESOURCE.to_string(),
            vec![FOREIGN_ACCOUNT.to_string()],
        ),
        (
            second_arn.clone(),
            LOCAL_RESOURCE.to_string(),
            vec![OTHER_FOREIGN_ACCOUNT.to_string()],
        ),
    ];
    expected.sort_by(|left, right| left.0.cmp(&right.0));
    let in_share_order: Vec<(String, Vec<String>)> = expected
        .iter()
        .map(|(_, resource, principals)| (resource.clone(), principals.clone()))
        .collect();
    assert_documents("two shares", &body, &in_share_order);

    // The `principal` filter narrows the documents to the share that shares the
    // resource with that identity.
    for (case, principal) in [
        ("filter on the first share", FOREIGN_ACCOUNT),
        ("filter on the second share", OTHER_FOREIGN_ACCOUNT),
    ] {
        let (status, body) = get_policies(
            &router,
            &json!({"resourceArns": [LOCAL_RESOURCE], "principal": principal}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {body:?}"
        );
        assert_documents(
            case,
            &body,
            &[(LOCAL_RESOURCE.to_string(), vec![principal.to_string()])],
        );
    }
}

#[tokio::test]
async fn following_next_token_walks_every_document_exactly_once() {
    let router = emulator();
    create_ok(
        &router,
        "paging",
        &json!({
            "name": "paging",
            "resourceArns": [LOCAL_RESOURCE, LOCAL_RESOURCE_TWO],
            "principals": [FOREIGN_ACCOUNT],
        }),
    )
    .await;

    // Two resources, one document each, read a page at a time: every page holds at
    // most `maxResults` documents, a `nextToken` is reported exactly while documents
    // remain, and following it reports each document exactly once.
    for page_size in [1usize, 2, 3] {
        let case = format!("maxResults={page_size} over two documents");
        let mut seen: Vec<Value> = Vec::new();
        let mut next_token: Option<String> = None;
        loop {
            let mut body = json!({
                "resourceArns": [LOCAL_RESOURCE, LOCAL_RESOURCE_TWO],
                "maxResults": page_size,
            });
            if let Some(token) = &next_token {
                body["nextToken"] = json!(token);
            }
            let (status, page) = get_policies(&router, &body).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200 while paging, got {status} with {page:?}"
            );
            let documents = policies_of(&case, &page);
            assert!(
                documents.len() <= page_size,
                "case {case:?}: a page held {} documents, more than the {page_size} asked for",
                documents.len()
            );
            seen.extend(documents);
            next_token = page
                .get("nextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            assert_eq!(
                next_token.is_some(),
                seen.len() < 2,
                "case {case:?}: a nextToken is reported exactly while documents remain, \
                 got {page:?}"
            );
            if next_token.is_none() {
                break;
            }
        }
        let resources: Vec<String> = seen
            .iter()
            .map(|document| resource_of(&case, document))
            .collect();
        assert_eq!(
            resources,
            vec![LOCAL_RESOURCE.to_string(), LOCAL_RESOURCE_TWO.to_string()],
            "case {case:?}: paging must report each resource's document exactly once, \
             in the order the request named them"
        );
    }
}

#[tokio::test]
async fn a_resource_is_read_exactly_while_a_live_share_shares_it() {
    let router = emulator();

    // Nothing shares the resource yet.
    expect_not_found(&router, "never shared", &[LOCAL_RESOURCE]).await;

    let share = create_ok(
        &router,
        "shared",
        &json!({
            "name": "lifecycle",
            "resourceArns": [LOCAL_RESOURCE],
            "principals": [FOREIGN_ACCOUNT],
        }),
    )
    .await;
    let share_arn = arn_of("shared", &share);
    expect_one_document(&router, "shared", LOCAL_RESOURCE).await;

    // A resource the share no longer shares is no longer read, from the call that
    // starts the teardown on.
    let (status, body) = disassociate(
        &router,
        &json!({"resourceShareArn": share_arn, "resourceArns": [LOCAL_RESOURCE]}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"disassociated\": expected 200, got {status} with {body:?}"
    );
    expect_not_found(&router, "disassociated", &[LOCAL_RESOURCE]).await;

    // Associating it again makes it readable again: the association is what decides.
    let (status, body) = associate(
        &router,
        &json!({"resourceShareArn": share_arn, "resourceArns": [LOCAL_RESOURCE]}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"re-associated\": expected 200, got {status} with {body:?}"
    );
    expect_one_document(&router, "re-associated", LOCAL_RESOURCE).await;

    // Disassociating the external identity leaves the resource shared with nobody
    // outside the organization, so it carries no policy while still being read.
    let (status, body) = disassociate(
        &router,
        &json!({"resourceShareArn": share_arn, "principals": [FOREIGN_ACCOUNT]}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"principal disassociated\": expected 200, got {status} with {body:?}"
    );
    let (status, body) = get_policies(&router, &json!({"resourceArns": [LOCAL_RESOURCE]})).await;
    assert_eq!(
        status, 200,
        "case \"principal disassociated\": expected 200, got {status} with {body:?}"
    );
    assert_documents("principal disassociated", &body, &[]);

    // A second share of the same resource: deleting one of them leaves the resource
    // shared by the other.
    let second = create_ok(
        &router,
        "shared twice",
        &json!({"name": "second", "resourceArns": [LOCAL_RESOURCE]}),
    )
    .await;
    let second_arn = arn_of("shared twice", &second);
    let (status, body) = delete(&router, &second_arn, None).await;
    assert_eq!(
        status, 200,
        "case \"one of two shares deleted\": expected 200, got {status} with {body:?}"
    );
    let (status, body) = get_policies(&router, &json!({"resourceArns": [LOCAL_RESOURCE]})).await;
    assert_eq!(
        status, 200,
        "case \"one of two shares deleted\": expected 200, got {status} with {body:?}"
    );
    assert_documents("one of two shares deleted", &body, &[]);

    // Deleting every share that shares it leaves nothing to read.
    let (status, body) = delete(&router, &share_arn, None).await;
    assert_eq!(
        status, 200,
        "case \"every share deleted\": expected 200, got {status} with {body:?}"
    );
    expect_not_found(&router, "every share deleted", &[LOCAL_RESOURCE]).await;

    // One resource the request cannot place refuses the whole request, whichever
    // position it holds in the list.
    create_ok(
        &router,
        "one unknown among two",
        &json!({
            "name": "third",
            "resourceArns": [LOCAL_RESOURCE_TWO],
            "principals": [FOREIGN_ACCOUNT],
        }),
    )
    .await;
    expect_one_document(&router, "one shared", LOCAL_RESOURCE_TWO).await;
    expect_not_found(
        &router,
        "the unknown one second",
        &[LOCAL_RESOURCE_TWO, LOCAL_RESOURCE],
    )
    .await;
    expect_not_found(
        &router,
        "the unknown one first",
        &[LOCAL_RESOURCE, LOCAL_RESOURCE_TWO],
    )
    .await;
}

#[tokio::test]
async fn a_request_this_read_cannot_serve_is_refused() {
    let router = emulator();
    create_ok(
        &router,
        "refusals",
        &json!({"name": "refusals", "resourceArns": [LOCAL_RESOURCE]}),
    )
    .await;

    // A `nextToken` this read issues names one of the resources the SAME request
    // asks about, hex-encoded, so a token naming another resource belongs to another
    // request and is not one for this one.
    let foreign_token = hex::encode(LOCAL_RESOURCE_TWO);

    // The cap on the resources one call names, and one past it.
    let past_the_cap: Vec<String> = (0..=MAX_ENTITIES_PER_CALL)
        .map(|index| format!("arn:aws:ec2:{REGION}:{ACCOUNT}:subnet/subnet-{index}"))
        .collect();
    let at_the_cap: Vec<String> = past_the_cap[..MAX_ENTITIES_PER_CALL].to_vec();

    let cases: Vec<(&str, Value, u16, &str)> = vec![
        (
            "resourceArns names no resource",
            json!({"resourceArns": []}),
            400,
            "InvalidParameterException",
        ),
        (
            "a resourceArns entry is not an ARN",
            json!({"resourceArns": ["subnet-1111"]}),
            400,
            "MalformedArnException",
        ),
        (
            "a resourceArns entry is an ARN missing its resource",
            json!({"resourceArns": [format!("arn:aws:ec2:{REGION}:{ACCOUNT}:")]}),
            400,
            "MalformedArnException",
        ),
        (
            "principal is empty",
            json!({"resourceArns": [LOCAL_RESOURCE], "principal": ""}),
            400,
            "InvalidParameterException",
        ),
        (
            "a nextToken this read never issued",
            json!({"resourceArns": [LOCAL_RESOURCE], "nextToken": "not-a-token"}),
            400,
            "InvalidNextTokenException",
        ),
        (
            "a nextToken naming a resource this request does not",
            json!({"resourceArns": [LOCAL_RESOURCE], "nextToken": foreign_token}),
            400,
            "InvalidNextTokenException",
        ),
        (
            "a nextToken naming a resource this request does name",
            json!({"resourceArns": [LOCAL_RESOURCE], "nextToken": hex::encode(LOCAL_RESOURCE)}),
            200,
            "",
        ),
        (
            "maxResults one below the published minimum",
            json!({"resourceArns": [LOCAL_RESOURCE], "maxResults": 0}),
            400,
            "",
        ),
        (
            "maxResults at the published minimum",
            json!({"resourceArns": [LOCAL_RESOURCE], "maxResults": 1}),
            200,
            "",
        ),
        (
            "maxResults at the published maximum",
            json!({"resourceArns": [LOCAL_RESOURCE], "maxResults": MAX_PAGE_SIZE}),
            200,
            "",
        ),
        (
            "maxResults one above the published maximum",
            json!({"resourceArns": [LOCAL_RESOURCE], "maxResults": MAX_PAGE_SIZE + 1}),
            400,
            "",
        ),
        (
            "resourceArns one past the cap on the resources one call names",
            json!({"resourceArns": past_the_cap}),
            400,
            "InvalidParameterException",
        ),
        (
            // The count is accepted at the cap, so the read gets as far as placing
            // the resources — none of which this account shares.
            "resourceArns at the cap on the resources one call names",
            json!({"resourceArns": at_the_cap}),
            400,
            "ResourceArnNotFoundException",
        ),
    ];

    for (case, body, expected_status, expected_error) in cases {
        let (status, reported) = get_policies(&router, &body).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status}, got {status} with {reported:?}"
        );
        if !expected_error.is_empty() {
            assert_eq!(
                error_code(&reported),
                expected_error,
                "case {case:?}: wrong error, got {reported:?}"
            );
        }
        if status == 200 {
            // The share this suite created adds no identity outside the owning
            // account's organization, so its resource carries no policy.
            assert!(
                policies_of(case, &reported).is_empty(),
                "case {case:?}: expected no policy document, got {reported:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// `GetResourcePolicies` over the wire.
async fn get_policies(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "POST", "/getresourcepolicies", Some(body)).await
}

/// The documents a successful read reported, each parsed from the JSON string the
/// `policies` list carries. A resource with no document reports an empty list, never
/// an absent member, so the member has to be there.
fn policies_of(case: &str, body: &Value) -> Vec<Value> {
    body.get("policies")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: response carried no policies, got {body:?}"))
        .iter()
        .map(|document| {
            let text = document.as_str().unwrap_or_else(|| {
                panic!("case {case:?}: a policy must be a JSON string, got {document:?}")
            });
            serde_json::from_str(text).unwrap_or_else(|error| {
                panic!("case {case:?}: a policy must be a JSON document ({error}), got {text:?}")
            })
        })
        .collect()
}

/// The one statement a reported document carries.
fn statement_of(case: &str, document: &Value) -> Value {
    assert_eq!(
        document.get("Version").and_then(Value::as_str),
        Some(POLICY_LANGUAGE_VERSION),
        "case {case:?}: wrong policy language version, got {document:?}"
    );
    let statements = document
        .get("Statement")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: document carried no Statement, got {document:?}"));
    assert_eq!(
        statements.len(),
        1,
        "case {case:?}: expected one statement, got {document:?}"
    );
    statements[0].clone()
}

/// The resource a reported document was instantiated on.
fn resource_of(case: &str, document: &Value) -> String {
    statement_of(case, document)
        .get("Resource")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: statement named no Resource, got {document:?}"))
        .to_string()
}

/// Assert the read reported exactly `expected`: one `(resource ARN, principals)` pair
/// per document, in order.
fn assert_documents(case: &str, body: &Value, expected: &[(String, Vec<String>)]) {
    let documents = policies_of(case, body);
    assert_eq!(
        documents.len(),
        expected.len(),
        "case {case:?}: expected {} document(s), got {documents:?}",
        expected.len()
    );
    for (document, (resource, principals)) in documents.iter().zip(expected) {
        let statement = statement_of(case, document);
        assert_eq!(
            statement.get("Effect").and_then(Value::as_str),
            Some(POLICY_EFFECT_ALLOW),
            "case {case:?}: a share's statement must allow, got {statement:?}"
        );
        assert_eq!(
            statement.get("Resource").and_then(Value::as_str),
            Some(resource.as_str()),
            "case {case:?}: the statement must name the shared resource, got {statement:?}"
        );
        // The actions are the namespace of the shared resource's own service, read
        // off the resource ARN's service field.
        let service = resource.split(':').nth(2).unwrap_or_default();
        assert_eq!(
            statement.get("Action").and_then(Value::as_str),
            Some(format!("{service}:*").as_str()),
            "case {case:?}: the statement must name the resource's service namespace, \
             got {statement:?}"
        );
        let named: Vec<String> = statement
            .get("Principal")
            .and_then(|principal| principal.get("AWS"))
            .and_then(Value::as_array)
            .unwrap_or_else(|| {
                panic!("case {case:?}: statement named no AWS principal, got {statement:?}")
            })
            .iter()
            .map(|value| value.as_str().unwrap_or_default().to_string())
            .collect();
        let mut sorted = named.clone();
        sorted.sort();
        let mut wanted = principals.clone();
        wanted.sort();
        assert_eq!(
            sorted, wanted,
            "case {case:?}: the statement must name the identities added to the share, \
             got {named:?}"
        );
    }
}

/// Assert the read answers 200 with exactly one document, on `resource_arn`.
async fn expect_one_document(router: &MultiServiceRouter, case: &str, resource_arn: &str) {
    let (status, body) = get_policies(router, &json!({"resourceArns": [resource_arn]})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200, got {status} with {body:?}"
    );
    let documents = policies_of(case, &body);
    assert_eq!(
        documents.len(),
        1,
        "case {case:?}: expected one document, got {documents:?}"
    );
    assert_eq!(
        resource_of(case, &documents[0]),
        resource_arn,
        "case {case:?}: the document must be the one instantiated on this resource"
    );
}

/// Assert the read refuses `resource_arns`, naming the unplaceable resource in the
/// message it answers with.
async fn expect_not_found(router: &MultiServiceRouter, case: &str, resource_arns: &[&str]) {
    let (status, body) = get_policies(router, &json!({"resourceArns": resource_arns})).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400, got {status} with {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceArnNotFoundException",
        "case {case:?}: wrong error, got {body:?}"
    );
    let message = error_message(&body);
    assert!(
        resource_arns.iter().any(|arn| message.contains(arn)),
        "case {case:?}: the message must name the resource it refused, got {message:?}"
    );
}
