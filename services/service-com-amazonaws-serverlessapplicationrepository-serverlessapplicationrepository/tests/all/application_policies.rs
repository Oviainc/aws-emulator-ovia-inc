// SPDX-License-Identifier: CC0-1.0

//! The permission policy of a Serverless Application Repository application, end to end
//! over HTTP: `PutApplicationPolicy` and `GetApplicationPolicy`.
//!
//! The rules under test, each exercised over a SET of policies rather than one recorded
//! request:
//!
//! 1. a put stores the statements it carries and `GetApplicationPolicy` answers with
//!    exactly them, member for member and in the order they were listed;
//! 2. a put REPLACES the whole policy — it never merges into the stored one — so after a
//!    put the policy is the statements of that call and nothing else, and a put carrying
//!    no statements leaves the application private again;
//! 3. a statement always carries a `statementId`: the caller's own, or a unique one the
//!    service mints for a statement that named none;
//! 4. the policy belongs to its application — an application never put is the empty
//!    policy, one application's policy is not another's, and a deleted application's
//!    policy is gone with it;
//! 5. a statement outside the documented contract (an action that is not an application
//!    permission, a principal that is neither an account id nor `*`, an organization id
//!    that is not one, a repeated `statementId`) is refused with the modelled
//!    `BadRequestException` and the stored policy is exactly as it was;
//! 6. a policy read or written on an application the catalogue does not hold is the
//!    modelled `NotFoundException`.
//!
//! Every case drives the real wire path — the router resolves this service from the
//! request's SigV4 credential scope, restJson1 routes on method + URI and deserializes
//! the body, and the handlers answer out of the one shared engine — so what is asserted
//! here is what a `serverlessrepo` SDK client sees.

use serde_json::{json, Value};

use super::applications::{created, minimal, path_label, Answer, Repo};

/// `/applications/{ApplicationId}/policy`, the URI the model binds both policy
/// operations to, with the ARN percent-encoded into its single path label.
fn policy_path(application_id: &str) -> String {
    format!("/applications/{}/policy", path_label(application_id))
}

/// `PutApplicationPolicy` with `statements`.
async fn put_policy(repo: &Repo, application_id: &str, statements: Value) -> Answer {
    repo.call(
        "PUT",
        &policy_path(application_id),
        Some(json!({ "statements": statements })),
    )
    .await
}

/// `GetApplicationPolicy`.
async fn get_policy(repo: &Repo, application_id: &str) -> Answer {
    repo.call("GET", &policy_path(application_id), None).await
}

/// The `statements` of a policy answer, in the order the service listed them.
fn statements_of(answer: &Answer) -> Vec<Value> {
    answer
        .at("statements")
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// One list member of a statement, as strings; the empty list when the member is absent.
fn list_of(statement: &Value, member: &str) -> Vec<String> {
    statement
        .get(member)
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .map(|entry| entry.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// A statement's `statementId`, or the empty string when it carries none.
fn statement_id(statement: &Value) -> String {
    statement
        .get("statementId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Whether `value` has the lowercase hyphenated uuid shape a minted `statementId`
/// carries: five hex groups of 8-4-4-4-12.
fn is_uuid_shaped(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == 5
        && groups
            .iter()
            .zip([8usize, 4, 4, 4, 12])
            .all(|(group, width)| {
                group.len() == width
                    && group
                        .bytes()
                        .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            })
}

/// A stored statement answers with what the request asked for: the same actions and
/// principals in the same order, the organization ids it named (the empty list when it
/// named none), and its own `statementId` — or, when it named none, a minted one.
fn assert_statement_matches(case: &str, index: usize, requested: &Value, stored: &Value) {
    assert_eq!(
        list_of(stored, "actions"),
        list_of(requested, "actions"),
        "case {case:?}: statement {index}: expected the requested actions, got {stored}"
    );
    assert_eq!(
        list_of(stored, "principals"),
        list_of(requested, "principals"),
        "case {case:?}: statement {index}: expected the requested principals, got {stored}"
    );
    assert_eq!(
        list_of(stored, "principalOrgIDs"),
        list_of(requested, "principalOrgIDs"),
        "case {case:?}: statement {index}: expected the requested principalOrgIDs, got {stored}"
    );
    let actual = statement_id(stored);
    match requested.get("statementId").and_then(Value::as_str) {
        Some(requested_id) => assert_eq!(
            actual, requested_id,
            "case {case:?}: statement {index}: expected the requested statementId, got {actual:?}"
        ),
        None => assert!(
            is_uuid_shaped(&actual),
            "case {case:?}: statement {index}: a statement that named no statementId must be \
             stored under a minted uuid-shaped one, got {actual:?}"
        ),
    }
}

/// Every application permission a statement may grant.
///
/// The catalogue's source is the `Actions` member's own documentation
/// (`smithy_query(query="doc", shape="ApplicationPolicyStatement", member="Actions")`),
/// which points at the Application Permissions list of the Serverless Application
/// Repository developer guide. Asserted here as a whole, so an entry no recording cites
/// is covered too.
const APPLICATION_PERMISSIONS: &[&str] = &[
    "CreateCloudFormationChangeSet",
    "CreateCloudFormationTemplate",
    "Deploy",
    "GetApplication",
    "ListApplicationDependencies",
    "ListApplicationVersions",
    "SearchApplications",
    "UnshareApplication",
];

/// RULE 1 and 3: a put stores its statements and the next get answers with exactly them,
/// member for member and in order; a statement that named no `statementId` is stored
/// under a minted one.
///
/// The rows differ along what the rule generalises over: how many statements the policy
/// has, how many actions and principals each carries, which permissions they are, whether
/// the principal is an account id or `*`, whether the statement is conditioned on
/// organization ids, whether it names its own `statementId`, and how the action is spelled.
#[tokio::test]
async fn a_put_policy_reads_back_statement_for_statement() {
    let repo = Repo::new();
    let cases: Vec<(&str, Value)> = vec![
        ("no statements at all: the empty policy", json!([])),
        (
            "one statement: one permission, one account principal, its own id",
            json!([{
                "actions": ["Deploy"],
                "principals": ["123456789012"],
                "statementId": "share-with-one-account"
            }]),
        ),
        (
            "two statements, each with its own id, listed in this order",
            json!([
                {
                    "actions": ["GetApplication", "ListApplicationVersions"],
                    "principals": ["123456789012", "210987654321"],
                    "statementId": "readers"
                },
                {
                    "actions": ["Deploy"],
                    "principals": ["111122223333"],
                    "statementId": "deployers"
                }
            ]),
        ),
        (
            "a statement that names no statementId",
            json!([{ "actions": ["Deploy"], "principals": ["123456789012"] }]),
        ),
        (
            "the public principal: * makes the application public",
            json!([{
                "actions": ["Deploy"],
                "principals": ["*"],
                "statementId": "public"
            }]),
        ),
        (
            "an organization-conditioned statement",
            json!([{
                "actions": ["Deploy"],
                "principals": ["*"],
                "principalOrgIDs": ["o-abcde12345", "o-0123456789abcdef"],
                "statementId": "org-wide"
            }]),
        ),
        (
            "every application permission in one statement",
            json!([{
                "actions": APPLICATION_PERMISSIONS,
                "principals": ["123456789012"],
                "statementId": "everything"
            }]),
        ),
        (
            "an action spelled as the IAM action it is",
            json!([{
                "actions": ["serverlessrepo:GetApplication"],
                "principals": ["123456789012"],
                "statementId": "iam-spelling"
            }]),
        ),
        (
            "three statements, one of each principal form, no ids at all",
            json!([
                { "actions": ["SearchApplications"], "principals": ["123456789012"] },
                { "actions": ["GetApplication"], "principals": ["*"] },
                {
                    "actions": ["ListApplicationDependencies"],
                    "principals": ["*"],
                    "principalOrgIDs": ["o-zyxwv98765"]
                }
            ]),
        ),
    ];

    for (index, (case, requested)) in cases.iter().enumerate() {
        let application_id = created(&repo, &format!("policy-roundtrip-{index}")).await;
        let put = put_policy(&repo, &application_id, requested.clone()).await;
        assert_eq!(
            put.status, 200,
            "case {case:?}: expected 200, got {} with {}",
            put.status, put.body
        );

        let requested_statements = requested.as_array().cloned().unwrap_or_default();
        let put_statements = statements_of(&put);
        assert_eq!(
            put_statements.len(),
            requested_statements.len(),
            "case {case:?}: expected {} statement(s) back, got {}",
            requested_statements.len(),
            put.body
        );
        for (position, (requested_statement, stored)) in
            requested_statements.iter().zip(&put_statements).enumerate()
        {
            assert_statement_matches(case, position, requested_statement, stored);
        }

        // ROUND TRIP: the reader answers out of the same stored record, so it reports the
        // put's own answer back verbatim.
        let got = get_policy(&repo, &application_id).await;
        assert_eq!(
            got.status, 200,
            "case {case:?}: expected 200 from GetApplicationPolicy, got {} with {}",
            got.status, got.body
        );
        assert_eq!(
            statements_of(&got),
            put_statements,
            "case {case:?}: the stored policy must be what the put answered, got {}",
            got.body
        );
        // A read changes nothing: a second read answers the same.
        let again = get_policy(&repo, &application_id).await;
        assert_eq!(
            statements_of(&again),
            put_statements,
            "case {case:?}: reading a policy must not change it, got {}",
            again.body
        );
    }
}

/// RULE 3's point: a minted `statementId` is UNIQUE — within one policy, and between two
/// puts of the very same statements.
#[tokio::test]
async fn a_minted_statement_id_is_unique() {
    let repo = Repo::new();
    let application_id = created(&repo, "policy-minted-ids").await;
    let policy = json!([
        { "actions": ["Deploy"], "principals": ["123456789012"] },
        { "actions": ["GetApplication"], "principals": ["210987654321"] },
        { "actions": ["SearchApplications"], "principals": ["*"] }
    ]);

    let first = statements_of(&put_policy(&repo, &application_id, policy.clone()).await);
    let second = statements_of(&put_policy(&repo, &application_id, policy.clone()).await);
    let first_ids: Vec<String> = first.iter().map(statement_id).collect();
    let second_ids: Vec<String> = second.iter().map(statement_id).collect();

    assert_eq!(first_ids.len(), 3, "expected three ids, got {first_ids:?}");
    for ids in [&first_ids, &second_ids] {
        let unique: std::collections::BTreeSet<&String> = ids.iter().collect();
        assert_eq!(
            unique.len(),
            ids.len(),
            "case {ids:?}: the minted ids of one policy must differ from each other"
        );
    }
    for (position, (before, after)) in first_ids.iter().zip(&second_ids).enumerate() {
        assert_ne!(
            before, after,
            "statement {position}: re-putting the same statement must mint a new id, both said \
             {before:?}"
        );
    }
}

/// RULE 2: a put REPLACES the policy. Asserted as a sequence of puts on one application —
/// each row's policy is the whole policy afterwards, and no statement of the row before it
/// survives.
#[tokio::test]
async fn a_put_replaces_the_whole_policy() {
    let repo = Repo::new();
    let application_id = created(&repo, "policy-replace").await;
    let sequence: Vec<(&str, Value)> = vec![
        (
            "one statement",
            json!([{
                "actions": ["Deploy"],
                "principals": ["123456789012"],
                "statementId": "first"
            }]),
        ),
        (
            "two different statements replace it",
            json!([
                {
                    "actions": ["GetApplication"],
                    "principals": ["210987654321"],
                    "statementId": "second"
                },
                {
                    "actions": ["SearchApplications"],
                    "principals": ["*"],
                    "statementId": "third"
                }
            ]),
        ),
        (
            "a policy that keeps one of them and adds one",
            json!([
                {
                    "actions": ["SearchApplications"],
                    "principals": ["*"],
                    "statementId": "third"
                },
                {
                    "actions": ["ListApplicationVersions"],
                    "principals": ["111122223333"],
                    "statementId": "fourth"
                }
            ]),
        ),
        ("no statements: the policy is cleared", json!([])),
        (
            "a policy put after a cleared one",
            json!([{
                "actions": ["Deploy"],
                "principals": ["*"],
                "principalOrgIDs": ["o-abcde12345"],
                "statementId": "fifth"
            }]),
        ),
    ];

    for (case, requested) in &sequence {
        let put = put_policy(&repo, &application_id, requested.clone()).await;
        assert_eq!(
            put.status, 200,
            "case {case:?}: expected 200, got {} with {}",
            put.status, put.body
        );

        let expected_ids: Vec<String> = requested
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(statement_id)
            .collect();
        let got = get_policy(&repo, &application_id).await;
        let stored_ids: Vec<String> = statements_of(&got).iter().map(statement_id).collect();
        assert_eq!(
            stored_ids, expected_ids,
            "case {case:?}: the policy must be exactly the statements of the last put, got {}",
            got.body
        );
    }
}

/// RULE 4: the policy belongs to its application. An application never put is the empty
/// policy, one application's policy is not another's, and the policy goes away with the
/// application — a re-created application of the same name starts private again.
#[tokio::test]
async fn a_policy_belongs_to_its_application() {
    let repo = Repo::new();
    let shared = created(&repo, "policy-owner").await;
    let untouched = created(&repo, "policy-bystander").await;

    for (case, application_id) in [("the owner", &shared), ("the bystander", &untouched)] {
        let got = get_policy(&repo, application_id).await;
        assert_eq!(
            got.status, 200,
            "case {case:?}: expected 200, got {} with {}",
            got.status, got.body
        );
        assert_eq!(
            statements_of(&got),
            Vec::<Value>::new(),
            "case {case:?}: an application whose policy was never set must report the empty \
             policy, got {}",
            got.body
        );
    }

    let put = put_policy(
        &repo,
        &shared,
        json!([{
            "actions": ["Deploy"],
            "principals": ["123456789012"],
            "statementId": "only-the-owner"
        }]),
    )
    .await;
    assert_eq!(
        put.status, 200,
        "expected 200, got {} with {}",
        put.status, put.body
    );
    let bystander = get_policy(&repo, &untouched).await;
    assert_eq!(
        statements_of(&bystander),
        Vec::<Value>::new(),
        "one application's policy must not reach another's, got {}",
        bystander.body
    );

    // Deleting the application takes its policy with it, and a new application published
    // under the same name — and so the same ARN — starts private.
    let deleted = repo.delete_application(&shared).await;
    assert_eq!(
        deleted.status, 204,
        "expected 204 from DeleteApplication, got {} with {}",
        deleted.status, deleted.body
    );
    let after_delete = get_policy(&repo, &shared).await;
    assert_eq!(
        after_delete.status, 404,
        "a deleted application has no policy to read, got {} with {}",
        after_delete.status, after_delete.body
    );
    let recreated = repo.create_application(minimal("policy-owner")).await;
    assert_eq!(
        recreated.status, 201,
        "expected 201 from CreateApplication, got {} with {}",
        recreated.status, recreated.body
    );
    let fresh = get_policy(&repo, &shared).await;
    assert_eq!(
        statements_of(&fresh),
        Vec::<Value>::new(),
        "a re-published application must start private, got {}",
        fresh.body
    );
}

/// RULE 5: a statement outside the contract is refused, and the stored policy is exactly
/// as it was.
///
/// The rows differ along which part of the contract the statement breaks: the permission
/// catalogue, the principal form, the organization-id form, the `statementId`'s
/// uniqueness and length, the required-ness of the two lists, and the emulator's own
/// ceiling on how large a policy it retains.
#[tokio::test]
async fn a_refused_policy_leaves_the_stored_policy_as_it_was() {
    let repo = Repo::new();
    let application_id = created(&repo, "policy-refusals").await;
    let kept = json!([{
        "actions": ["Deploy"],
        "principals": ["123456789012"],
        "statementId": "the-stored-statement"
    }]);
    let stored = statements_of(&put_policy(&repo, &application_id, kept.clone()).await);
    assert_eq!(stored.len(), 1, "the policy under test must be stored first");

    let many: Vec<Value> = (0..=100)
        .map(|index| {
            json!({
                "actions": ["Deploy"],
                "principals": ["123456789012"],
                "statementId": format!("statement-{index}")
            })
        })
        .collect();
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "an action that is not an application permission",
            json!([{ "actions": ["DeleteApplication"], "principals": ["123456789012"] }]),
            "Actions",
        ),
        (
            "an action of another service",
            json!([{ "actions": ["s3:GetObject"], "principals": ["123456789012"] }]),
            "Actions",
        ),
        (
            "a permission misspelled in case",
            json!([{ "actions": ["deploy"], "principals": ["123456789012"] }]),
            "Actions",
        ),
        (
            "no actions at all",
            json!([{ "actions": [], "principals": ["123456789012"] }]),
            "Actions",
        ),
        (
            "no principals at all",
            json!([{ "actions": ["Deploy"], "principals": [] }]),
            "Principals",
        ),
        (
            "a principal that is not an account id",
            json!([{ "actions": ["Deploy"], "principals": ["not-an-account"] }]),
            "Principals",
        ),
        (
            "an account id of the wrong length",
            json!([{ "actions": ["Deploy"], "principals": ["12345"] }]),
            "Principals",
        ),
        (
            "a principal with one bad entry among good ones",
            json!([{
                "actions": ["Deploy"],
                "principals": ["123456789012", "1234567890123"]
            }]),
            "Principals",
        ),
        (
            "an organization id that is not one",
            json!([{
                "actions": ["Deploy"],
                "principals": ["*"],
                "principalOrgIDs": ["org-12345"]
            }]),
            "PrincipalOrgIDs",
        ),
        (
            "an organization id too short for the condition key",
            json!([{
                "actions": ["Deploy"],
                "principals": ["*"],
                "principalOrgIDs": ["o-abc"]
            }]),
            "PrincipalOrgIDs",
        ),
        (
            "two statements under one statementId",
            json!([
                {
                    "actions": ["Deploy"],
                    "principals": ["123456789012"],
                    "statementId": "repeated"
                },
                {
                    "actions": ["GetApplication"],
                    "principals": ["210987654321"],
                    "statementId": "repeated"
                }
            ]),
            "StatementId",
        ),
        (
            "an empty statementId",
            json!([{
                "actions": ["Deploy"],
                "principals": ["123456789012"],
                "statementId": ""
            }]),
            "StatementId",
        ),
        (
            "one statement past the ceiling on a stored policy",
            Value::Array(many),
            "Statements",
        ),
    ];

    for (case, requested, member) in &cases {
        let refused = put_policy(&repo, &application_id, requested.clone()).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected 400, got {} with {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.at("__type").as_str().unwrap_or_default(),
            "BadRequestException",
            "case {case:?}: expected the modelled shape BadRequestException, got {}",
            refused.body
        );
        assert_eq!(
            refused.at("errorCode").as_str().unwrap_or_default(),
            "InvalidRequestException",
            "case {case:?}: errorCode must name the condition, got {}",
            refused.body
        );
        let message = refused.at("message").as_str().unwrap_or_default().to_string();
        assert!(
            message.contains(member),
            "case {case:?}: the refusal must name the member {member}, got {message:?}"
        );

        let after = get_policy(&repo, &application_id).await;
        assert_eq!(
            statements_of(&after),
            stored,
            "case {case:?}: a refused put must leave the stored policy as it was, got {}",
            after.body
        );
    }

    // The refusals consumed nothing: the application still takes a good policy.
    let accepted = put_policy(
        &repo,
        &application_id,
        json!([{
            "actions": ["GetApplication"],
            "principals": ["*"],
            "statementId": "after-the-refusals"
        }]),
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "expected 200 after the refusals, got {} with {}",
        accepted.status, accepted.body
    );
    assert_eq!(
        statement_id(&statements_of(&accepted)[0]),
        "after-the-refusals",
        "the accepted policy replaced the stored one, got {}",
        accepted.body
    );
}

/// RULE 6: a policy of an application the catalogue does not hold is a 404 naming the id
/// the caller asked for — on the reader and on the writer alike, and the writer stores
/// nothing on the way.
#[tokio::test]
async fn a_policy_of_an_absent_application_is_not_found() {
    let repo = Repo::new();
    let absent = [
        "arn:aws:serverlessrepo:us-west-2:000000000000:applications/absent-policy-app",
        "arn:aws:serverlessrepo:us-west-2:000000000000:applications/another-absent-app",
        "arn:aws:serverlessrepo:us-east-1:000000000000:applications/absent-in-another-region",
    ];

    for application_id in absent {
        let statements = json!([{ "actions": ["Deploy"], "principals": ["123456789012"] }]);
        for (operation, answer) in [
            ("GetApplicationPolicy", get_policy(&repo, application_id).await),
            (
                "PutApplicationPolicy",
                put_policy(&repo, application_id, statements).await,
            ),
        ] {
            assert_eq!(
                answer.status, 404,
                "case {operation} {application_id:?}: expected 404, got {} with {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.at("__type").as_str().unwrap_or_default(),
                "NotFoundException",
                "case {operation} {application_id:?}: got {}",
                answer.body
            );
            assert_eq!(
                answer.at("errorCode").as_str().unwrap_or_default(),
                "ApplicationNotFoundException",
                "case {operation} {application_id:?}: got {}",
                answer.body
            );
            assert!(
                answer
                    .at("message")
                    .as_str()
                    .unwrap_or_default()
                    .contains(application_id),
                "case {operation} {application_id:?}: the message must name the id asked for, \
                 got {}",
                answer.body
            );
        }
    }
}
