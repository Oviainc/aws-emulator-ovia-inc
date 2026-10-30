// SPDX-License-Identifier: CC0-1.0

//! The size `ListProfilePermissions` reports for a signing profile's
//! cross-account permission policy, end to end over HTTP.
//!
//! The rule under test: `policySizeBytes` is the byte length of the profile's
//! resource policy document as the service would serve it — one statement per
//! stored grant, in AWS's policy language, with no whitespace and nothing in it
//! that is not part of that document ("Total size of the policy associated with
//! the Signing Profile in bytes", `smithy_query(query="shape",
//! shape="ListProfilePermissionsResponse")`, member `policySizeBytes`, target
//! `PolicySizeBytes`, default 0).
//!
//! That document is fixed in form and varies only in the values the grants carry,
//! so the rule is pinned as arithmetic rather than by re-serializing it here: for
//! a policy of `n` statements the reported size is
//!
//! ```text
//!   ENVELOPE + n * STATEMENT + (n - 1) + sum over statements of
//!       (len(statementId) + len(principal) + len(action) + len(resourceArn))
//! ```
//!
//! where `ENVELOPE` is the policy document with an empty statement list,
//! `STATEMENT` is one statement with empty values, the `n - 1` is the separator
//! between statements, and `resourceArn` is the ARN of what the grant applies to
//! — the profile, or the profile VERSION when the grant names one. Every value a
//! grant carries therefore appears EXACTLY ONCE in what is measured, and no value
//! is decorated: a document that quoted, escaped, indented or repeated any of them
//! would miss this arithmetic by the amount of that decoration.
//!
//! The rows differ along what the rule generalises over: how many statements the
//! policy holds, the length of each statement id (including both of
//! `ProfileName`-sized extremes for the profile the ARN names), which grantable
//! action the grant carries, the FORM of the principal (an account id and an IAM
//! role ARN), and whether the grant names a profile version — which is what makes
//! the measured resource the version ARN rather than the profile's.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_signer_wallabyservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service: what routes a request to signer.
const SIGNED_FOR_US_WEST_2: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-west-2/signer/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The region every request below is signed for.
const REGION: &str = "us-west-2";

/// The account id every generated emulator presents (`EMULATOR_ACCOUNT_ID`),
/// spelled out rather than imported so a wrong account in the handlers fails here.
const ACCOUNT_ID: &str = "000000000000";

/// A platform whose profiles need no `signingMaterial`, so a seeded profile is one
/// `PutSigningProfile` call.
const PLATFORM_ID: &str = "AWSLambda-SHA384-ECDSA";

/// The byte length of the policy document with NO statement in it.
///
/// `{"Version":"2012-10-17","Statement":[]}` — the policy-language version AWS's
/// grammar publishes, and the statement list. This is the whole document a policy
/// holding nothing would be, so it is the fixed part of every measurement.
const ENVELOPE_BYTES: i64 = 39;

/// The byte length of ONE statement whose every value is empty.
///
/// `{"Sid":"","Effect":"Allow","Principal":{"AWS":""},"Action":"","Resource":""}`
/// — the five members a Signer cross-account grant is expressed as: the statement
/// id, the effect (a grant is always an Allow), the principal the grant names, the
/// action it permits and the resource it applies to.
const STATEMENT_BYTES: i64 = 76;

/// The byte length of the separator between two statements of one policy.
const SEPARATOR_BYTES: i64 = 1;

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
    fn str_member(&self, member: &str) -> String {
        self.body[member].as_str().unwrap_or_default().to_string()
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
}

async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("signer.{REGION}.amazonaws.com"))
        .header("authorization", SIGNED_FOR_US_WEST_2);
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body().to_vec();
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

/// One grant this suite writes.
struct Grant {
    statement_id: String,
    action: &'static str,
    principal: String,
    /// `true` when the grant names the profile's current version, which makes the
    /// resource it applies to that version rather than the profile.
    names_the_version: bool,
}

/// The resource ARN a grant on `profile` applies to, as this service forms it:
/// the profile's ARN, with the version after it when the grant names one.
fn resource_arn(profile: &str, version: &str, names_the_version: bool) -> String {
    let profile_arn = format!("arn:aws:signer:{REGION}:{ACCOUNT_ID}:/signing-profiles/{profile}");
    if names_the_version {
        format!("{profile_arn}/{version}")
    } else {
        profile_arn
    }
}

/// The size the rule says a policy of `grants` on `profile` has.
fn expected_size(profile: &str, version: &str, grants: &[Grant]) -> i64 {
    let count = i64::try_from(grants.len()).expect("a test policy holds few statements");
    let values: i64 = grants
        .iter()
        .map(|grant| {
            let resource = resource_arn(profile, version, grant.names_the_version);
            i64::try_from(
                grant.statement_id.len()
                    + grant.principal.len()
                    + grant.action.len()
                    + resource.len(),
            )
            .expect("a test grant carries few bytes")
        })
        .sum();
    ENVELOPE_BYTES + count * STATEMENT_BYTES + (count - 1) * SEPARATOR_BYTES + values
}

/// A profile of `name`, and the version this service minted for it.
async fn seed_profile(router: &MultiServiceRouter, name: &str) -> String {
    let answer = call(
        router,
        "PUT",
        &format!("/signing-profiles/{name}"),
        Some(json!({ "platformId": PLATFORM_ID })),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "seeding profile {name}: expected 200, got {} {:?}",
        answer.status, answer.body
    );
    answer.str_member("profileVersion")
}

/// One case: a profile name and the grants written onto it.
struct SizeCase {
    case: &'static str,
    profile: &'static str,
    grants: Vec<Grant>,
}

/// The rows: one statement and several, statement ids at both ends of the range a
/// policy can carry, every action grantable on this platform, both forms of
/// principal the model documents, and a grant that names a profile version beside
/// one that does not.
///
/// The actions are the ones `AddProfilePermissionRequest$action` documents for an
/// AWS Lambda workflow profile (`smithy_query(query="doc",
/// shape="AddProfilePermissionRequest", member="action")`), and the principal
/// forms are the two that member names: "an IAM role or another AWS account ID".
fn size_cases() -> Vec<SizeCase> {
    vec![
        SizeCase {
            case: "one grant, a short statement id and an account-id principal",
            profile: "Size_One",
            grants: vec![Grant {
                statement_id: "s1".to_string(),
                action: "signer:StartSigningJob",
                principal: "210987654321".to_string(),
                names_the_version: false,
            }],
        },
        SizeCase {
            case: "one grant whose statement id is long",
            profile: "Size_Long_Statement_Id",
            grants: vec![Grant {
                statement_id: "A".repeat(200),
                action: "signer:GetSigningProfile",
                principal: "210987654321".to_string(),
                names_the_version: false,
            }],
        },
        SizeCase {
            case: "one grant to an IAM role principal",
            profile: "Size_Role_Principal",
            grants: vec![Grant {
                statement_id: "AllowRoleRead".to_string(),
                action: "signer:GetSigningProfile",
                principal: format!("arn:aws:iam::{ACCOUNT_ID}:role/CodeSigner"),
                names_the_version: false,
            }],
        },
        SizeCase {
            case: "one grant that names the profile version",
            profile: "Size_Versioned",
            grants: vec![Grant {
                statement_id: "AllowVersion".to_string(),
                action: "signer:RevokeSignature",
                principal: "210987654321".to_string(),
                names_the_version: true,
            }],
        },
        SizeCase {
            case: "three grants, mixing the actions, the principals and the versions",
            profile: "Size_Three",
            grants: vec![
                Grant {
                    statement_id: "aaa".to_string(),
                    action: "signer:StartSigningJob",
                    principal: "210987654321".to_string(),
                    names_the_version: false,
                },
                Grant {
                    statement_id: "bbbbbbbbbbbbbbbbbbbb".to_string(),
                    action: "signer:GetSigningProfile",
                    principal: format!("arn:aws:iam::{ACCOUNT_ID}:role/CodeSigner"),
                    names_the_version: true,
                },
                Grant {
                    statement_id: "ccc".to_string(),
                    action: "signer:RevokeSignature",
                    principal: ACCOUNT_ID.to_string(),
                    names_the_version: false,
                },
            ],
        },
        SizeCase {
            // `ProfileName` is `@length(2..64)`, and the profile's name travels
            // into the document through the resource ARN of every statement, so
            // both ends of that range are measured.
            case: "one grant on a profile whose name is at the modelled minimum",
            profile: "Pz",
            grants: vec![Grant {
                statement_id: "AllowShortName".to_string(),
                action: "signer:StartSigningJob",
                principal: "210987654321".to_string(),
                names_the_version: false,
            }],
        },
        SizeCase {
            case: "one grant on a profile whose name is at the modelled maximum",
            profile: "Size_Maximum_Length_Profile_Name_Padded_To_Sixty_Four_Characters",
            grants: vec![Grant {
                statement_id: "AllowLongName".to_string(),
                action: "signer:StartSigningJob",
                principal: "210987654321".to_string(),
                names_the_version: false,
            }],
        },
    ]
}

/// `policySizeBytes` is the byte length of the canonical policy document the
/// stored grants form: the fixed envelope, one fixed-size statement per grant and
/// each grant's own values exactly once.
#[tokio::test]
async fn the_reported_policy_size_is_the_canonical_documents_length() {
    for case in size_cases() {
        let router = emulator();
        let label = case.case;
        let profile = case.profile;
        assert!(
            (2..=64).contains(&profile.len()),
            "case {label:?}: the row's profile name must satisfy ProfileName's bounds, got {} \
             characters",
            profile.len()
        );
        let version = seed_profile(&router, profile).await;

        for grant in &case.grants {
            let mut body = json!({
                "statementId": grant.statement_id,
                "action": grant.action,
                "principal": grant.principal,
            });
            if grant.names_the_version {
                body["profileVersion"] = json!(version);
            }
            let added = call(
                &router,
                "POST",
                &format!("/signing-profiles/{profile}/permissions"),
                Some(body),
            )
            .await;
            assert_eq!(
                added.status, 200,
                "case {label:?}: granting {:?} expected 200, got {} {:?}",
                grant.statement_id, added.status, added.body
            );
        }

        let listed = call(
            &router,
            "GET",
            &format!("/signing-profiles/{profile}/permissions"),
            None,
        )
        .await;
        assert_eq!(
            listed.status, 200,
            "case {label:?}: expected ListProfilePermissions 200, got {} {:?}",
            listed.status, listed.body
        );
        let expected = expected_size(profile, &version, &case.grants);
        assert_eq!(
            listed.policy_size_bytes(),
            expected,
            "case {label:?}: expected policySizeBytes {expected}, got {}",
            listed.policy_size_bytes()
        );
    }
}

/// The size moves with the document and with nothing else: one more statement
/// adds exactly that statement's own bytes, and a statement id one byte longer
/// moves the size by exactly one byte.
///
/// This is the differential half of the same rule: it holds whatever the fixed
/// envelope is, so it catches a measurement that decorates or repeats a value
/// even where the arithmetic above is read as a statement about the constants.
#[tokio::test]
async fn the_reported_policy_size_moves_only_with_the_document() {
    let router = emulator();
    let profile = "Size_Differential";
    let version = seed_profile(&router, profile).await;
    let resource = resource_arn(profile, &version, false);
    let action = "signer:GetSigningProfile";
    let principal = "210987654321";

    // One statement id per row, each one byte longer than the one before it, so
    // the step the size takes is the step the document takes.
    let mut previous: Option<i64> = None;
    for length in 1..=8usize {
        let statement_id = "k".repeat(length);
        let added = call(
            &router,
            "POST",
            &format!("/signing-profiles/{profile}/permissions"),
            Some(json!({
                "statementId": statement_id,
                "action": action,
                "principal": principal,
            })),
        )
        .await;
        assert_eq!(
            added.status, 200,
            "case {length} statements: granting {statement_id:?} expected 200, got {} {:?}",
            added.status, added.body
        );
        let listed = call(
            &router,
            "GET",
            &format!("/signing-profiles/{profile}/permissions"),
            None,
        )
        .await;
        let size = listed.policy_size_bytes();

        if let Some(before) = previous {
            // The new statement contributed its own bytes, a separator and the
            // fixed per-statement structure — nothing else changed in the
            // document, because no other statement's values changed.
            let contributed = STATEMENT_BYTES
                + SEPARATOR_BYTES
                + i64::try_from(
                    statement_id.len() + principal.len() + action.len() + resource.len(),
                )
                .expect("a test grant carries few bytes");
            assert_eq!(
                size - before,
                contributed,
                "case {length} statements: expected the size to grow by {contributed}, got {}",
                size - before
            );
        }
        previous = Some(size);
    }
}
