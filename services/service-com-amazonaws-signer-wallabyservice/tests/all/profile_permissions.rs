// SPDX-License-Identifier: CC0-1.0

//! The cross-account permission policy of an AWS Signer signing profile, end to
//! end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, the
//! `@httpLabel` / `@httpQuery` bindings, deserialization, the handler, the
//! persisted policy and serialization — the same path a client takes. The rules
//! under test are the ones Signer states for the three operations bound to
//! `/signing-profiles/{profileName}/permissions`:
//!
//! * a profile's permission policy is a revision-versioned set of statements keyed
//!   by `statementId`: `AddProfilePermission` adds one and
//!   `RemoveProfilePermission` takes one out, each answering with the revision the
//!   policy moved to, and `ListProfilePermissions` reports exactly the statements
//!   the policy holds, that same revision and the byte size of the document they
//!   form;
//! * a write naming a `revisionId` that is not the policy's current one — a
//!   superseded revision, or one the service never issued — is refused 409
//!   `ConflictException` and changes nothing, which is what makes two writers
//!   holding one revision unable to both succeed;
//! * an `action` outside the four the model documents as grantable is refused 400
//!   `ValidationException`, and a statement id, profile or profile version that
//!   does not exist is refused 404.
//!
//! The cases are tables: a row per input class the rules generalise over (which
//! grantable action, the form of the principal, whether the grant names a profile
//! version, the form and length of the statement id, how stale the named revision
//! is, and which statement of a many-statement policy a call names).

use std::collections::BTreeMap;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_signer_wallabyservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service: what routes a request to signer.
const SIGNED_FOR_US_WEST_2: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-west-2/signer/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The error `code` a refusal of a request whose profile does not exist carries.
/// Spelled out here rather than imported so a wrong code in the handlers fails.
const ERROR_CODE_PROFILE_NOT_FOUND: &str = "ProfileNotFound";

/// The error `code` a 404 for a sub-resource of the profile carries — the
/// modelled shape's own name.
const ERROR_CODE_NOT_FOUND: &str = "ResourceNotFoundException";

/// The error `code` a lost optimistic-concurrency check carries.
const ERROR_CODE_CONFLICT: &str = "ConflictException";

/// The error `code` a refusal on the request's own values carries.
const ERROR_CODE_VALIDATION: &str = "ValidationException";

/// Every action a cross-account permission may grant, written out here rather
/// than imported so a wrong catalog in the handlers fails this suite.
///
/// Source: `AddProfilePermissionRequest$action` — "You can designate the
/// following actions to an account" followed by exactly these four
/// (`smithy_query(query="doc", shape="AddProfilePermissionRequest",
/// member="action")`).
const GRANTABLE_ACTIONS: [&str; 4] = [
    "signer:StartSigningJob",
    "signer:SignPayload",
    "signer:GetSigningProfile",
    "signer:RevokeSignature",
];

/// A platform whose profiles need no `signingMaterial`, so a seeded profile is
/// one `PutSigningProfile` call.
const PLATFORM_ID: &str = "AWSLambda-SHA384-ECDSA";

/// The most statements one `ListProfilePermissions` answer may report, written
/// out here rather than imported so a wrong page size in the handler fails this
/// suite (`crate::state::permissions::PERMISSION_PAGE_LIMIT`).
const PAGE_LIMIT: usize = 25;

/// The most bytes one profile's permission policy may hold, written out here for
/// the same reason (`crate::state::permissions::MAX_POLICY_SIZE_BYTES`).
const MAX_POLICY_SIZE_BYTES: i64 = 20_480;

/// The error `code` a grant past [`MAX_POLICY_SIZE_BYTES`] carries.
const ERROR_CODE_SERVICE_LIMIT_EXCEEDED: &str = "ServiceLimitExceededException";

/// The error `code` a `nextToken` this service never handed out carries.
const ERROR_CODE_INVALID_NEXT_TOKEN: &str = "InvalidNextToken";

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: status and JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The modelled `code` member, or `None` when the body carries none.
    fn code(&self) -> Option<&str> {
        self.body["code"].as_str()
    }

    /// The modelled `message` member, or the empty string when there is none.
    fn message(&self) -> &str {
        self.body["message"].as_str().unwrap_or_default()
    }

    /// The `revisionId` this answer reports, or `None` when it reports none —
    /// a different claim from reporting an empty one.
    fn revision_id(&self) -> Option<&str> {
        self.body["revisionId"].as_str()
    }

    /// `policySizeBytes`, which `ListProfilePermissions` declares required.
    fn policy_size_bytes(&self) -> i64 {
        self.body["policySizeBytes"].as_i64().unwrap_or_else(|| {
            panic!(
                "ListProfilePermissions must report policySizeBytes: {:?}",
                self.body
            )
        })
    }

    /// The reported statements, keyed by `statementId`, each as
    /// `(action, principal, profileVersion)`.
    fn permissions(&self) -> BTreeMap<String, (String, String, Option<String>)> {
        self.body["permissions"]
            .as_array()
            .unwrap_or_else(|| {
                panic!(
                    "ListProfilePermissions must report a permissions list: {:?}",
                    self.body
                )
            })
            .iter()
            .map(|permission| {
                (
                    permission["statementId"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                    (
                        permission["action"].as_str().unwrap_or_default().to_string(),
                        permission["principal"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string(),
                        permission["profileVersion"].as_str().map(str::to_string),
                    ),
                )
            })
            .collect()
    }

    /// The statement ids, in the order the answer reported them.
    fn statement_ids(&self) -> Vec<String> {
        self.body["permissions"]
            .as_array()
            .unwrap_or_else(|| panic!("expected a permissions list: {:?}", self.body))
            .iter()
            .map(|permission| {
                permission["statementId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect()
    }
}

/// `value` percent-encoded for ONE URI segment or query value: a `statementId`
/// is an `@httpLabel` and a `revisionId` an `@httpQuery`
/// (`smithy_query(query="operation", operation="RemoveProfilePermission")`), and
/// both are unconstrained strings, so an SDK encodes everything outside the
/// unreserved set. Spelled out here, the way this crate's other suites spell it,
/// so a test suite takes no dependency of its own.
fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
                (byte as char).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "signer.us-west-2.amazonaws.com")
        .header("authorization", SIGNED_FOR_US_WEST_2);
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

async fn put_profile(router: &MultiServiceRouter, name: &str) -> Answer {
    call(
        router,
        "PUT",
        &format!("/signing-profiles/{name}"),
        Some(json!({ "platformId": PLATFORM_ID })),
    )
    .await
}

async fn add_permission(router: &MultiServiceRouter, name: &str, request: Value) -> Answer {
    call(
        router,
        "POST",
        &format!("/signing-profiles/{name}/permissions"),
        Some(request),
    )
    .await
}

async fn remove_permission(
    router: &MultiServiceRouter,
    name: &str,
    statement_id: &str,
    revision_id: &str,
) -> Answer {
    call(
        router,
        "DELETE",
        &format!(
            "/signing-profiles/{name}/permissions/{}?revisionId={}",
            encode(statement_id),
            encode(revision_id)
        ),
        None,
    )
    .await
}

async fn list_permissions(router: &MultiServiceRouter, name: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/signing-profiles/{name}/permissions"),
        None,
    )
    .await
}

async fn list_permissions_from(
    router: &MultiServiceRouter,
    name: &str,
    next_token: &str,
) -> Answer {
    call(
        router,
        "GET",
        &format!(
            "/signing-profiles/{name}/permissions?nextToken={}",
            encode(next_token)
        ),
        None,
    )
    .await
}

/// A profile of `name`, and the version `PutSigningProfile` minted for it.
async fn seed_profile(router: &MultiServiceRouter, name: &str) -> String {
    let answer = put_profile(router, name).await;
    assert_eq!(
        answer.status, 200,
        "seeding profile {name}: expected 200, got {} {:?}",
        answer.status, answer.body
    );
    answer.body["profileVersion"]
        .as_str()
        .unwrap_or_else(|| panic!("PutSigningProfile must report a profileVersion: {:?}", answer.body))
        .to_string()
}

/// A grant to write and read back. The rows differ along what the rule
/// generalises over: which grantable action, the form of the principal, whether
/// the grant pins a profile version, and the form and length of the statement id.
struct GrantCase {
    statement_id: &'static str,
    action: &'static str,
    principal: &'static str,
    /// Whether the request pins the grant to the profile's current version.
    pin_version: bool,
}

fn grant_cases() -> Vec<GrantCase> {
    vec![
        // The action the Signer cross-account tutorial grants, to a bare account
        // id, unpinned.
        GrantCase {
            statement_id: "prod-account",
            action: "signer:StartSigningJob",
            principal: "123456789012",
            pin_version: false,
        },
        // An action the issue did not cite, to an IAM ROLE principal, pinned to
        // one profile version.
        GrantCase {
            statement_id: "Dev_Account_2",
            action: "signer:GetSigningProfile",
            principal: "arn:aws:iam::123456789012:role/CodeSigner",
            pin_version: true,
        },
        // A one-character statement id, and the action that is not supported for
        // Lambda workflows (the model documents the restriction, not a refusal of
        // the grant).
        GrantCase {
            statement_id: "a",
            action: "signer:SignPayload",
            principal: "210987654321",
            pin_version: false,
        },
        // A long statement id carrying the punctuation an unconstrained string
        // admits, pinned.
        GrantCase {
            statement_id: "statement.id_for-the-revocation-grant.0123456789",
            action: "signer:RevokeSignature",
            principal: "arn:aws:iam::210987654321:root",
            pin_version: true,
        },
    ]
}

impl GrantCase {
    fn request(&self, profile_version: &str) -> Value {
        let mut request = json!({
            "statementId": self.statement_id,
            "action": self.action,
            "principal": self.principal,
        });
        if self.pin_version {
            request["profileVersion"] = json!(profile_version);
        }
        request
    }

    fn expected_version(&self, profile_version: &str) -> Option<String> {
        self.pin_version.then(|| profile_version.to_string())
    }
}

#[tokio::test]
async fn every_grant_round_trips_through_list_profile_permissions() {
    let router = emulator();
    let profile_version = seed_profile(&router, "RoundTrip_Profile").await;

    // A profile nobody has granted anything on holds no policy at all.
    let empty = list_permissions(&router, "RoundTrip_Profile").await;
    assert_eq!(
        empty.status, 200,
        "empty policy: expected 200, got {} {:?}",
        empty.status, empty.body
    );
    assert!(
        empty.permissions().is_empty(),
        "empty policy: expected no permissions, got {:?}",
        empty.permissions()
    );
    assert_eq!(
        empty.policy_size_bytes(),
        0,
        "empty policy: expected policySizeBytes 0, got {}",
        empty.policy_size_bytes()
    );
    assert_eq!(
        empty.revision_id(),
        None,
        "empty policy: expected no revisionId, got {:?}",
        empty.revision_id()
    );

    let mut expected: BTreeMap<String, (String, String, Option<String>)> = BTreeMap::new();
    let mut revisions: Vec<String> = Vec::new();
    let mut previous_size = 0;

    for case in grant_cases() {
        let added = add_permission(
            &router,
            "RoundTrip_Profile",
            case.request(&profile_version),
        )
        .await;
        assert_eq!(
            added.status, 200,
            "case {}: expected 200, got {} {:?}",
            case.statement_id, added.status, added.body
        );
        let revision = added
            .revision_id()
            .unwrap_or_else(|| {
                panic!(
                    "case {}: AddProfilePermission must report a revisionId, got {:?}",
                    case.statement_id, added.body
                )
            })
            .to_string();
        assert!(
            !revisions.contains(&revision),
            "case {}: expected a revisionId no earlier write returned, got {revision} after {revisions:?}",
            case.statement_id
        );
        revisions.push(revision.clone());

        expected.insert(
            case.statement_id.to_string(),
            (
                case.action.to_string(),
                case.principal.to_string(),
                case.expected_version(&profile_version),
            ),
        );

        let listed = list_permissions(&router, "RoundTrip_Profile").await;
        assert_eq!(
            listed.status, 200,
            "case {}: list expected 200, got {} {:?}",
            case.statement_id, listed.status, listed.body
        );
        assert_eq!(
            listed.permissions(),
            expected,
            "case {}: expected the policy to hold {expected:?}, got {:?}",
            case.statement_id,
            listed.permissions()
        );
        assert_eq!(
            listed.revision_id(),
            Some(revision.as_str()),
            "case {}: expected the list to report the revision the write returned ({revision}), got {:?}",
            case.statement_id,
            listed.revision_id()
        );

        // The policy document carries this statement's own bytes, so adding one
        // can only grow the size the policy reports.
        let own_bytes = (case.statement_id.len() + case.action.len() + case.principal.len()) as i64;
        let size = listed.policy_size_bytes();
        assert!(
            size >= previous_size + own_bytes,
            "case {}: expected policySizeBytes to grow by at least the statement's own \
             {own_bytes} bytes from {previous_size}, got {size}",
            case.statement_id
        );
        previous_size = size;
    }

    // `nextToken` is the statement id a page resumes at, so a token a client holds
    // reports the suffix of the same total order.
    let all = list_permissions(&router, "RoundTrip_Profile").await;
    let ids = all.statement_ids();
    assert_eq!(
        ids,
        expected.keys().cloned().collect::<Vec<_>>(),
        "expected the statements in statementId order, got {ids:?}"
    );
    for start in 0..ids.len() {
        let page = list_permissions_from(&router, "RoundTrip_Profile", &ids[start]).await;
        assert_eq!(
            page.statement_ids(),
            ids[start..].to_vec(),
            "nextToken {}: expected the suffix {:?}, got {:?}",
            ids[start],
            &ids[start..],
            page.statement_ids()
        );
        assert_eq!(
            page.policy_size_bytes(),
            all.policy_size_bytes(),
            "nextToken {}: expected policySizeBytes to report the WHOLE policy ({}), got {}",
            ids[start],
            all.policy_size_bytes(),
            page.policy_size_bytes()
        );
    }
}

#[tokio::test]
async fn removing_a_statement_takes_only_the_one_it_names() {
    // A row per statement of a four-statement policy: the first, two in the
    // middle and the last in `statementId` order.
    for removed in grant_cases() {
        let router = emulator();
        let profile_version = seed_profile(&router, "Remove_Profile").await;
        let mut revision = String::new();
        for case in grant_cases() {
            let added = add_permission(&router, "Remove_Profile", case.request(&profile_version))
                .await;
            assert_eq!(
                added.status, 200,
                "seeding {}: expected 200, got {} {:?}",
                case.statement_id, added.status, added.body
            );
            revision = added.revision_id().unwrap_or_default().to_string();
        }

        let before = list_permissions(&router, "Remove_Profile").await;
        let removal = remove_permission(&router, "Remove_Profile", removed.statement_id, &revision)
            .await;
        assert_eq!(
            removal.status, 200,
            "removing {}: expected 200, got {} {:?}",
            removed.statement_id, removal.status, removal.body
        );
        let new_revision = removal
            .revision_id()
            .unwrap_or_else(|| {
                panic!(
                    "removing {}: RemoveProfilePermission must report a revisionId, got {:?}",
                    removed.statement_id, removal.body
                )
            })
            .to_string();
        assert_ne!(
            new_revision, revision,
            "removing {}: expected a new revisionId, got the one the policy already had ({revision})",
            removed.statement_id
        );

        let after = list_permissions(&router, "Remove_Profile").await;
        let mut expected = before.permissions();
        expected.remove(removed.statement_id);
        assert_eq!(
            after.permissions(),
            expected,
            "removing {}: expected the policy to hold {expected:?}, got {:?}",
            removed.statement_id,
            after.permissions()
        );
        assert_eq!(
            after.revision_id(),
            Some(new_revision.as_str()),
            "removing {}: expected the list to report the revision the removal returned, got {:?}",
            removed.statement_id,
            after.revision_id()
        );
        assert!(
            after.policy_size_bytes() < before.policy_size_bytes(),
            "removing {}: expected policySizeBytes to shrink from {}, got {}",
            removed.statement_id,
            before.policy_size_bytes(),
            after.policy_size_bytes()
        );

        // The statement is gone, so removing it again names nothing.
        let again =
            remove_permission(&router, "Remove_Profile", removed.statement_id, &new_revision).await;
        assert_eq!(
            again.status, 404,
            "removing {} twice: expected 404, got {} {:?}",
            removed.statement_id, again.status, again.body
        );
        assert_eq!(
            again.code(),
            Some(ERROR_CODE_NOT_FOUND),
            "removing {} twice: expected code {ERROR_CODE_NOT_FOUND}, got {:?}",
            removed.statement_id,
            again.code()
        );
    }
}

/// How a request's `revisionId` relates to the policy's current one.
struct StaleRevisionCase {
    case: &'static str,
    /// How many further writes happen after the revision is captured, so the row
    /// spans "superseded once" and "superseded repeatedly".
    writes_after_capture: usize,
    /// A revision the service never issued, used instead of a captured one.
    never_issued: Option<&'static str>,
}

fn stale_revision_cases() -> Vec<StaleRevisionCase> {
    vec![
        StaleRevisionCase {
            case: "superseded by one later write",
            writes_after_capture: 1,
            never_issued: None,
        },
        StaleRevisionCase {
            case: "superseded by three later writes",
            writes_after_capture: 3,
            never_issued: None,
        },
        StaleRevisionCase {
            case: "a revision id this service never issued",
            writes_after_capture: 1,
            never_issued: Some("00000000-0000-0000-0000-000000000000"),
        },
        StaleRevisionCase {
            case: "a revision id that is not even an id",
            writes_after_capture: 0,
            never_issued: Some("not-a-revision"),
        },
    ]
}

#[tokio::test]
async fn a_write_naming_a_revision_that_is_not_current_is_refused_and_changes_nothing() {
    for row in stale_revision_cases() {
        let router = emulator();
        let profile_version = seed_profile(&router, "Revision_Profile").await;

        // The first grant needs no revision: the profile holds no policy yet.
        let first = add_permission(
            &router,
            "Revision_Profile",
            json!({
                "statementId": "first",
                "action": GRANTABLE_ACTIONS[0],
                "principal": "123456789012",
            }),
        )
        .await;
        assert_eq!(
            first.status, 200,
            "{}: first grant expected 200, got {} {:?}",
            row.case, first.status, first.body
        );
        let mut current = first.revision_id().unwrap_or_default().to_string();
        let captured = current.clone();

        for write in 0..row.writes_after_capture {
            let added = add_permission(
                &router,
                "Revision_Profile",
                json!({
                    "statementId": format!("later-{write}"),
                    "action": GRANTABLE_ACTIONS[write % GRANTABLE_ACTIONS.len()],
                    "principal": "210987654321",
                    "revisionId": current,
                }),
            )
            .await;
            assert_eq!(
                added.status, 200,
                "{}: write {write} with the current revision expected 200, got {} {:?}",
                row.case, added.status, added.body
            );
            current = added.revision_id().unwrap_or_default().to_string();
        }

        let stale = row.never_issued.map_or(captured, str::to_string);
        let before = list_permissions(&router, "Revision_Profile").await;

        // An ADD naming it is refused…
        let add = add_permission(
            &router,
            "Revision_Profile",
            json!({
                "statementId": "refused",
                "action": GRANTABLE_ACTIONS[1],
                "principal": "123456789012",
                "profileVersion": profile_version,
                "revisionId": stale,
            }),
        )
        .await;
        assert_eq!(
            add.status, 409,
            "{}: add with revision {stale} expected 409, got {} {:?}",
            row.case, add.status, add.body
        );
        assert_eq!(
            add.code(),
            Some(ERROR_CODE_CONFLICT),
            "{}: add with revision {stale} expected code {ERROR_CODE_CONFLICT}, got {:?}",
            row.case,
            add.code()
        );
        assert!(
            add.message().contains(&stale),
            "{}: expected the refusal to name the revision it refused ({stale}), got {:?}",
            row.case,
            add.message()
        );

        // …and so is a REMOVE naming it.
        let remove = remove_permission(&router, "Revision_Profile", "first", &stale).await;
        assert_eq!(
            remove.status, 409,
            "{}: remove with revision {stale} expected 409, got {} {:?}",
            row.case, remove.status, remove.body
        );
        assert_eq!(
            remove.code(),
            Some(ERROR_CODE_CONFLICT),
            "{}: remove with revision {stale} expected code {ERROR_CODE_CONFLICT}, got {:?}",
            row.case,
            remove.code()
        );

        let after = list_permissions(&router, "Revision_Profile").await;
        assert_eq!(
            after.permissions(),
            before.permissions(),
            "{}: expected the refused writes to change nothing, got {:?} after {:?}",
            row.case,
            after.permissions(),
            before.permissions()
        );
        assert_eq!(
            after.revision_id(),
            before.revision_id(),
            "{}: expected the refused writes to leave the revision at {:?}, got {:?}",
            row.case,
            before.revision_id(),
            after.revision_id()
        );
    }
}

#[tokio::test]
async fn two_writers_holding_one_revision_cannot_both_succeed() {
    let router = emulator();
    seed_profile(&router, "Concurrent_Profile").await;
    let seeded = add_permission(
        &router,
        "Concurrent_Profile",
        json!({
            "statementId": "seed",
            "action": GRANTABLE_ACTIONS[0],
            "principal": "123456789012",
        }),
    )
    .await;
    let shared = seeded.revision_id().unwrap_or_default().to_string();

    // Both writers read the same revision and then write against it: the first
    // moves the policy forward, the second is told it lost.
    let winner = add_permission(
        &router,
        "Concurrent_Profile",
        json!({
            "statementId": "writer-one",
            "action": GRANTABLE_ACTIONS[1],
            "principal": "210987654321",
            "revisionId": shared,
        }),
    )
    .await;
    assert_eq!(
        winner.status, 200,
        "first writer: expected 200, got {} {:?}",
        winner.status, winner.body
    );
    let loser = add_permission(
        &router,
        "Concurrent_Profile",
        json!({
            "statementId": "writer-two",
            "action": GRANTABLE_ACTIONS[2],
            "principal": "210987654321",
            "revisionId": shared,
        }),
    )
    .await;
    assert_eq!(
        loser.status, 409,
        "second writer: expected 409, got {} {:?}",
        loser.status, loser.body
    );
    assert_eq!(
        loser.code(),
        Some(ERROR_CODE_CONFLICT),
        "second writer: expected code {ERROR_CODE_CONFLICT}, got {:?}",
        loser.code()
    );

    let listed = list_permissions(&router, "Concurrent_Profile").await;
    assert_eq!(
        listed.statement_ids(),
        vec!["seed".to_string(), "writer-one".to_string()],
        "expected only the winning writer's statement beside the seed, got {:?}",
        listed.statement_ids()
    );
    assert_eq!(
        listed.revision_id(),
        winner.revision_id(),
        "expected the policy to stand at the winning writer's revision {:?}, got {:?}",
        winner.revision_id(),
        listed.revision_id()
    );
}

#[tokio::test]
async fn a_statement_id_the_policy_already_holds_is_refused() {
    let router = emulator();
    seed_profile(&router, "Duplicate_Profile").await;
    let first = add_permission(
        &router,
        "Duplicate_Profile",
        json!({
            "statementId": "only-one",
            "action": GRANTABLE_ACTIONS[0],
            "principal": "123456789012",
        }),
    )
    .await;
    let revision = first.revision_id().unwrap_or_default().to_string();

    // Every row names the SAME statement id, differing in what else it carries:
    // the same grant again, a different action, a different principal.
    for (case, request) in [
        (
            "the same grant again",
            json!({
                "statementId": "only-one",
                "action": GRANTABLE_ACTIONS[0],
                "principal": "123456789012",
                "revisionId": revision,
            }),
        ),
        (
            "a different action under the same id",
            json!({
                "statementId": "only-one",
                "action": GRANTABLE_ACTIONS[3],
                "principal": "123456789012",
                "revisionId": revision,
            }),
        ),
        (
            "a different principal under the same id, with no revision named",
            json!({
                "statementId": "only-one",
                "action": GRANTABLE_ACTIONS[0],
                "principal": "210987654321",
            }),
        ),
    ] {
        let answer = add_permission(&router, "Duplicate_Profile", request).await;
        assert_eq!(
            answer.status, 409,
            "case {case:?}: expected 409, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.code(),
            Some(ERROR_CODE_CONFLICT),
            "case {case:?}: expected code {ERROR_CODE_CONFLICT}, got {:?}",
            answer.code()
        );
    }

    let listed = list_permissions(&router, "Duplicate_Profile").await;
    assert_eq!(
        listed.permissions().len(),
        1,
        "expected the refused duplicates to leave one statement, got {:?}",
        listed.permissions()
    );
    assert_eq!(
        listed.revision_id(),
        Some(revision.as_str()),
        "expected the refused duplicates to leave the revision at {revision}, got {:?}",
        listed.revision_id()
    );
}

#[tokio::test]
async fn an_action_outside_the_documented_catalog_is_refused() {
    let router = emulator();
    seed_profile(&router, "Action_Profile").await;

    // Every action the model documents as grantable is accepted — including the
    // ones this issue's units never named.
    for (index, action) in GRANTABLE_ACTIONS.iter().enumerate() {
        let answer = add_permission(
            &router,
            "Action_Profile",
            json!({
                "statementId": format!("grant-{index}"),
                "action": action,
                "principal": "123456789012",
            }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "action {action}: expected 200, got {} {:?}",
            answer.status, answer.body
        );
    }

    // And one past the catalog is refused, whichever way it falls outside it: a
    // Signer operation that is not grantable, another service's action, the
    // action without its service prefix, a wildcard, and the empty string.
    for action in [
        "signer:ListSigningJobs",
        "signer:PutSigningProfile",
        "s3:GetObject",
        "StartSigningJob",
        "signer:*",
        "",
    ] {
        let answer = add_permission(
            &router,
            "Action_Profile",
            json!({
                "statementId": "refused",
                "action": action,
                "principal": "123456789012",
            }),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "action {action:?}: expected 400, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.code(),
            Some(ERROR_CODE_VALIDATION),
            "action {action:?}: expected code {ERROR_CODE_VALIDATION}, got {:?}",
            answer.code()
        );
        assert!(
            answer.message().contains("action"),
            "action {action:?}: expected the refusal to name the member, got {:?}",
            answer.message()
        );
    }

    let listed = list_permissions(&router, "Action_Profile").await;
    assert_eq!(
        listed.permissions().len(),
        GRANTABLE_ACTIONS.len(),
        "expected one statement per grantable action and nothing from the refusals, got {:?}",
        listed.permissions()
    );
}

#[tokio::test]
async fn a_resource_no_profile_or_policy_holds_is_refused_404() {
    let router = emulator();
    let profile_version = seed_profile(&router, "Known_Profile").await;

    // No profile of that name: every one of the three operations says so, with
    // the code a client branches on.
    for (case, answer) in [
        (
            "add",
            add_permission(
                &router,
                "Unknown_Profile",
                json!({
                    "statementId": "whatever",
                    "action": GRANTABLE_ACTIONS[0],
                    "principal": "123456789012",
                }),
            )
            .await,
        ),
        (
            "remove",
            remove_permission(&router, "Unknown_Profile", "whatever", "some-revision").await,
        ),
        ("list", list_permissions(&router, "Unknown_Profile").await),
    ] {
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected 404, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.code(),
            Some(ERROR_CODE_PROFILE_NOT_FOUND),
            "case {case:?}: expected code {ERROR_CODE_PROFILE_NOT_FOUND}, got {:?}",
            answer.code()
        );
    }

    // A grant pinned to a version the profile has never had names no resource.
    // The rows differ in how the version is wrong: never minted, and the version
    // of a different profile.
    let other_version = seed_profile(&router, "Other_Profile").await;
    for (case, version) in [
        ("a version never minted", "ZZZZZZZZZZ".to_string()),
        ("another profile's version", other_version.clone()),
    ] {
        assert_ne!(
            version, profile_version,
            "case {case:?}: the row's version must differ from the profile's own"
        );
        let answer = add_permission(
            &router,
            "Known_Profile",
            json!({
                "statementId": "pinned",
                "action": GRANTABLE_ACTIONS[0],
                "principal": "123456789012",
                "profileVersion": version,
            }),
        )
        .await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected 404, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.code(),
            Some(ERROR_CODE_NOT_FOUND),
            "case {case:?}: expected code {ERROR_CODE_NOT_FOUND}, got {:?}",
            answer.code()
        );
    }

    // A removal against a profile that holds no policy, and one naming a
    // statement the policy does not hold.
    let empty = remove_permission(&router, "Known_Profile", "absent", "some-revision").await;
    assert_eq!(
        empty.status, 404,
        "removal against a profile with no policy: expected 404, got {} {:?}",
        empty.status, empty.body
    );
    assert_eq!(
        empty.code(),
        Some(ERROR_CODE_NOT_FOUND),
        "removal against a profile with no policy: expected code {ERROR_CODE_NOT_FOUND}, got {:?}",
        empty.code()
    );

    let added = add_permission(
        &router,
        "Known_Profile",
        json!({
            "statementId": "present",
            "action": GRANTABLE_ACTIONS[0],
            "principal": "123456789012",
            "profileVersion": profile_version,
        }),
    )
    .await;
    let revision = added.revision_id().unwrap_or_default().to_string();
    let absent = remove_permission(&router, "Known_Profile", "absent", &revision).await;
    assert_eq!(
        absent.status, 404,
        "removal naming a statement the policy lacks: expected 404, got {} {:?}",
        absent.status, absent.body
    );
    assert_eq!(
        absent.code(),
        Some(ERROR_CODE_NOT_FOUND),
        "removal naming a statement the policy lacks: expected code {ERROR_CODE_NOT_FOUND}, got {:?}",
        absent.code()
    );
}

#[tokio::test]
async fn a_new_profile_version_leaves_the_policy_and_its_revision_alone() {
    let router = emulator();
    let first_version = seed_profile(&router, "Versioned_Profile").await;
    let added = add_permission(
        &router,
        "Versioned_Profile",
        json!({
            "statementId": "pinned-to-the-first-version",
            "action": GRANTABLE_ACTIONS[0],
            "principal": "123456789012",
            "profileVersion": first_version,
        }),
    )
    .await;
    let before = list_permissions(&router, "Versioned_Profile").await;

    // A second put mints a new version of the SAME profile; the policy is a
    // resource of the profile, not of one of its versions.
    let second_version = seed_profile(&router, "Versioned_Profile").await;
    assert_ne!(
        second_version, first_version,
        "a second PutSigningProfile must mint a new version, got {second_version} twice"
    );

    let after = list_permissions(&router, "Versioned_Profile").await;
    assert_eq!(
        after.permissions(),
        before.permissions(),
        "expected the policy to survive a new profile version, got {:?} after {:?}",
        after.permissions(),
        before.permissions()
    );
    assert_eq!(
        after.revision_id(),
        added.revision_id(),
        "expected the revision to stay at {:?}, got {:?}",
        added.revision_id(),
        after.revision_id()
    );
    assert_eq!(
        after.policy_size_bytes(),
        before.policy_size_bytes(),
        "expected policySizeBytes to stay at {}, got {}",
        before.policy_size_bytes(),
        after.policy_size_bytes()
    );
}

#[tokio::test]
async fn a_policy_larger_than_one_page_is_served_in_pages() {
    let router = emulator();
    seed_profile(&router, "Paged_Profile").await;

    // Rows differ in how far the policy reaches past the page size: one page
    // exactly, one statement past it, and a page and a half.
    let mut granted: Vec<String> = Vec::new();
    for count in [PAGE_LIMIT, PAGE_LIMIT + 1, PAGE_LIMIT + PAGE_LIMIT / 2] {
        while granted.len() < count {
            let statement_id = format!("statement-{:03}", granted.len());
            let answer = add_permission(
                &router,
                "Paged_Profile",
                json!({
                    "statementId": statement_id,
                    "action": GRANTABLE_ACTIONS[granted.len() % GRANTABLE_ACTIONS.len()],
                    "principal": "123456789012",
                }),
            )
            .await;
            assert_eq!(
                answer.status, 200,
                "granting {statement_id}: expected 200, got {} {:?}",
                answer.status, answer.body
            );
            granted.push(statement_id);
        }
        granted.sort();

        // A paged walk reports every statement exactly once, in one total order,
        // and no page is larger than the page size.
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        for _ in 0..64 {
            let page = match token.as_deref() {
                Some(token) => list_permissions_from(&router, "Paged_Profile", token).await,
                None => list_permissions(&router, "Paged_Profile").await,
            };
            assert_eq!(
                page.status, 200,
                "case {count} statements: expected 200, got {} {:?}",
                page.status, page.body
            );
            let ids = page.statement_ids();
            assert!(
                ids.len() <= PAGE_LIMIT,
                "case {count} statements: expected at most {PAGE_LIMIT} per page, got {}",
                ids.len()
            );
            assert!(
                page.policy_size_bytes() <= MAX_POLICY_SIZE_BYTES,
                "case {count} statements: expected policySizeBytes within the bound, got {}",
                page.policy_size_bytes()
            );
            walked.extend(ids);
            match page.body["nextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            walked, granted,
            "case {count} statements: expected the walk to report every statement in order, \
             got {walked:?}"
        );
    }
}

#[tokio::test]
async fn a_grant_that_would_carry_the_policy_past_its_size_limit_is_refused() {
    let router = emulator();
    seed_profile(&router, "Large_Policy_Profile").await;

    // Each grant carries a principal long enough that the policy reaches its byte
    // limit in a bounded number of calls; the row that trips it is the first one
    // whose document would not fit.
    let principal = format!("arn:aws:iam::123456789012:role/{}", "r".repeat(900));
    let mut accepted = 0;
    let mut refused = None;
    for index in 0..64 {
        let statement_id = format!("statement-{index:03}");
        let answer = add_permission(
            &router,
            "Large_Policy_Profile",
            json!({
                "statementId": statement_id,
                "action": GRANTABLE_ACTIONS[index % GRANTABLE_ACTIONS.len()],
                "principal": principal,
            }),
        )
        .await;
        if answer.status == 200 {
            accepted += 1;
            continue;
        }
        refused = Some((statement_id, answer));
        break;
    }

    let (statement_id, answer) = refused.unwrap_or_else(|| {
        panic!("expected a grant past {MAX_POLICY_SIZE_BYTES} bytes to be refused, {accepted} were accepted")
    });
    assert!(
        accepted > 0,
        "case {statement_id}: expected the policy to admit grants before the limit, got none"
    );
    assert_eq!(
        answer.status, 402,
        "case {statement_id}: expected 402, got {} {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.code(),
        Some(ERROR_CODE_SERVICE_LIMIT_EXCEEDED),
        "case {statement_id}: expected code {ERROR_CODE_SERVICE_LIMIT_EXCEEDED}, got {:?}",
        answer.code()
    );

    // The refused grant stored nothing, and the policy stands within its bound.
    let listed = list_permissions(&router, "Large_Policy_Profile").await;
    assert!(
        !listed.statement_ids().contains(&statement_id),
        "case {statement_id}: expected the refused grant to store nothing, got {:?}",
        listed.statement_ids()
    );
    assert!(
        listed.policy_size_bytes() <= MAX_POLICY_SIZE_BYTES,
        "expected the policy to stay within {MAX_POLICY_SIZE_BYTES} bytes, got {}",
        listed.policy_size_bytes()
    );
}

#[tokio::test]
async fn a_next_token_this_service_never_handed_out_is_refused() {
    let router = emulator();
    seed_profile(&router, "Token_Profile").await;

    // A token against a profile that holds no policy at all is one no page of it
    // could have produced.
    let no_policy = list_permissions_from(&router, "Token_Profile", "statement-000").await;
    assert_eq!(
        no_policy.status, 400,
        "token against an empty policy: expected 400, got {} {:?}",
        no_policy.status, no_policy.body
    );
    assert_eq!(
        no_policy.code(),
        Some(ERROR_CODE_INVALID_NEXT_TOKEN),
        "token against an empty policy: expected code {ERROR_CODE_INVALID_NEXT_TOKEN}, got {:?}",
        no_policy.code()
    );

    let granted = add_permission(
        &router,
        "Token_Profile",
        json!({
            "statementId": "held",
            "action": GRANTABLE_ACTIONS[0],
            "principal": "123456789012",
        }),
    )
    .await;
    assert_eq!(
        granted.status, 200,
        "seeding: expected 200, got {} {:?}",
        granted.status, granted.body
    );

    // Rows differ in how the token falls outside the ids the policy holds: before
    // them in the order, after them, a statement that was never granted, and a
    // revision id rather than a statement id.
    for (case, token) in [
        ("before every statement id", "a"),
        ("after every statement id", "zzz"),
        ("a statement the policy never held", "never-granted"),
        (
            "a revision id in place of a statement id",
            granted.revision_id().unwrap_or_default(),
        ),
    ] {
        let answer = list_permissions_from(&router, "Token_Profile", token).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.code(),
            Some(ERROR_CODE_INVALID_NEXT_TOKEN),
            "case {case:?}: expected code {ERROR_CODE_INVALID_NEXT_TOKEN}, got {:?}",
            answer.code()
        );
    }

    // The one token the policy does admit is a statement id it holds.
    let held = list_permissions_from(&router, "Token_Profile", "held").await;
    assert_eq!(
        held.statement_ids(),
        vec!["held".to_string()],
        "expected the page at the statement's own id to report it, got {:?}",
        held.statement_ids()
    );
}

#[tokio::test]
async fn a_grant_with_no_principal_or_no_statement_id_is_refused() {
    let router = emulator();
    seed_profile(&router, "Empty_Member_Profile").await;

    // Both members are required strings the model puts no length on, so the
    // boundary validator admits an empty one; neither is a grant AWS could hold.
    for (member, request) in [
        (
            "principal",
            json!({
                "statementId": "no-principal",
                "action": GRANTABLE_ACTIONS[0],
                "principal": "",
            }),
        ),
        (
            "statementId",
            json!({
                "statementId": "   ",
                "action": GRANTABLE_ACTIONS[0],
                "principal": "123456789012",
            }),
        ),
    ] {
        let answer = add_permission(&router, "Empty_Member_Profile", request).await;
        assert_eq!(
            answer.status, 400,
            "member {member}: expected 400, got {} {:?}",
            answer.status, answer.body
        );
        assert!(
            answer.message().contains(member),
            "member {member}: expected the refusal to name the member, got {:?}",
            answer.message()
        );
    }

    let listed = list_permissions(&router, "Empty_Member_Profile").await;
    assert!(
        listed.permissions().is_empty(),
        "expected the refused grants to store nothing, got {:?}",
        listed.permissions()
    );
}
