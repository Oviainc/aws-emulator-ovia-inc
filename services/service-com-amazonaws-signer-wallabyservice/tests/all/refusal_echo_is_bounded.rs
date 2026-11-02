// SPDX-License-Identifier: CC0-1.0

//! How a refusal quotes a request value whose member the model puts NO length on,
//! end to end over HTTP.
//!
//! The rule under test: a clause that quotes a caller-supplied value whose member
//! carries no modelled size bound states that value up to ONE ceiling this
//! service applies to every such clause — past it, the prefix that fits plus the
//! whole value's byte count — so no request parameter can make the error body as
//! large as itself, whichever operation, member and binding (query, label, body)
//! carried it.
//!
//! The rows span what the rule generalises over: which operation refuses, which
//! unbounded member carried the value (`platformId`, a tagging `resourceArn`, a
//! `jobId`, a permission's `action`, `statementId` and `revisionId`), where on the
//! wire it arrived, how long it is (well inside the ceiling, exactly on it, one
//! byte past it, far past it) and whether its characters are one byte each or
//! several — a multi-byte value being the case where the ceiling falls inside a
//! character.
//!
//! A value the model DOES bound is not part of this rule: the boundary validator
//! refuses it before a handler sees it, so it is quoted as it arrived. Neither is
//! a refusal that quotes NO part of the request (a tagging `resourceArn` that is
//! no ARN at all, a `nextToken`): there is nothing for a ceiling to bound.
//!
//! One refusal of this service cannot be driven from the table above, because
//! reaching it takes state the oversized value itself had to be stored in: the 409
//! a grant takes when it names a `statementId` the profile's policy ALREADY holds.
//! It is driven on its own below, over the same size classes, together with the
//! size range past which such an id never reaches that clause at all.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_signer_wallabyservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for.
const REGION: &str = "us-west-2";

/// A SigV4 credential scope naming this service for [`REGION`]: what routes a
/// request to signer.
const SIGNED_FOR: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-west-2/signer/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The account id every generated emulator presents, spelled out rather than
/// imported so a wrong account in the handlers fails this suite.
const ACCOUNT_ID: &str = "000000000000";

/// A published `platformId` a seeded profile is created on.
const PLATFORM_ID: &str = "AWSLambda-SHA384-ECDSA";

/// A seeded profile that holds a permission policy.
const PROFILE_WITH_POLICY: &str = "Echo_Policy_Profile";

/// A seeded profile that holds no permission policy at all.
const PROFILE_WITHOUT_POLICY: &str = "Echo_Plain_Profile";

/// The one statement [`PROFILE_WITH_POLICY`]'s policy holds.
const SEEDED_STATEMENT_ID: &str = "Echo_Statement";

/// The ceiling this service states for a request value a clause quotes, written
/// out rather than imported: the bytes a client reads are pinned here
/// independently of the service's own constant.
const BOUND: usize = 16 * 1024;

/// The actions a cross-account permission may grant on a profile of
/// [`PLATFORM_ID`], in the order a refusal lists them — the signing action of
/// that platform's workflow, then the two every platform admits. Source:
/// `AddProfilePermissionRequest$action` (`smithy_query(query="doc",
/// shape="AddProfilePermissionRequest", member="action")`), which documents
/// `signer:SignPayload` as unsupported for AWS Lambda workflows.
const GRANTABLE_ACTIONS: &str = "signer:StartSigningJob, signer:GetSigningProfile, \
                                 signer:RevokeSignature";

/// An ACM certificate ARN a create names as its signing material, so a create
/// reaches the platform catalog instead of being refused for the absent member.
const CERTIFICATE_ARN: &str =
    "arn:aws:acm:us-west-2:000000000000:certificate/12345678-1234-1234-1234-123456789012";

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the HTTP status and the body as JSON.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The `message` member, or the empty string when the body carries none.
    fn message(&self) -> &str {
        self.body["message"].as_str().unwrap_or_default()
    }
}

/// `value` percent-encoded for ONE URI segment or query value: the unbounded
/// members under test are `@httpLabel`s and `@httpQuery`s that admit characters a
/// URI cannot hold literally, so everything outside the unreserved set is encoded,
/// the way an SDK sends it.
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

/// One request: the method, the URI and the JSON body, if any.
struct Request {
    method: &'static str,
    uri: String,
    body: Option<Value>,
}

async fn call(router: &MultiServiceRouter, request: &Request) -> Answer {
    let payload = request
        .body
        .as_ref()
        .map(|value| value.to_string())
        .unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(request.method)
        .uri(&request.uri)
        .header("host", format!("signer.{REGION}.amazonaws.com"))
        .header("authorization", SIGNED_FOR);
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let http_request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(http_request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{} answered a body that is not JSON ({error}): {:?}",
                request.method,
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

/// `value` as a clause of a refusal quotes it: itself up to the ceiling, past it
/// the prefix that fits — cut on a character boundary — followed by the whole
/// value's byte count.
///
/// Written out here rather than taken from the service, so the two cannot agree
/// by construction.
fn quoted(value: &str) -> String {
    if value.len() <= BOUND {
        return value.to_string();
    }
    let mut end = BOUND;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}... [{} bytes]", &value[..end], value.len())
}

/// Which values a case can carry: every member under test admits any text except
/// `platformId`, which is held to a published character class first (a value
/// outside it is refused for the class, not by the catalog) and the tagging
/// `resourceArn`, whose ARN fields are read by position.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Filler {
    /// Single-byte characters only.
    Ascii,
    /// Any text, including characters of several bytes.
    AnyText,
}

/// The oversized values a case is driven with: one row per size class the rule
/// generalises over, and for a member that admits any text a row whose characters
/// are three bytes each, so the ceiling falls INSIDE a character.
fn fillers(filler: Filler) -> Vec<(&'static str, String)> {
    let mut rows = vec![
        ("a value well inside the ceiling", "a".repeat(1_000)),
        // The ceiling is inclusive: the longest value stated whole.
        ("a value exactly on the ceiling", "a".repeat(BOUND)),
        ("a value one byte past the ceiling", "a".repeat(BOUND + 1)),
        ("a value far past the ceiling", "a".repeat(2 * BOUND)),
    ];
    if filler == Filler::AnyText {
        // 16 384 is not a multiple of three, so the ceiling lands inside one of
        // these characters and the cut has to walk back off it.
        rows.push(("a multi-byte value past the ceiling", "€".repeat(6_000)));
    }
    rows
}

/// One refusal path: how the oversized value reaches the service, and the clause
/// the refusal states about it.
struct Case {
    /// The operation and member this row drives, for the assertion message.
    name: &'static str,
    /// Which values this member can carry.
    filler: Filler,
    /// The value the clause quotes, built from the filler — the filler itself for
    /// every member that carries it directly.
    carried: fn(&str) -> String,
    /// The request that carries `carried`, given the revision the seeded policy
    /// is at (which a row naming a permission statement has to send to reach the
    /// refusal under test).
    request: fn(&str, &str) -> Request,
    /// The HTTP status the refusal answers with.
    status: u16,
    /// The whole message the refusal states, given the value as a clause quotes
    /// it and the revision the seeded policy is at.
    clause: fn(&str, &str) -> String,
}

/// The filler itself: what every member that carries the value directly quotes.
fn itself(filler: &str) -> String {
    filler.to_string()
}

/// A well-formed signing-profile ARN of EXACTLY `filler.len()` bytes, whose
/// REGION field is the oversized text.
///
/// `arn:<partition>:signer:<region>:<account>:/signing-profiles/<name>` is the
/// form the service parses, and the region is a field it reads but no modelled
/// bound limits — so such an ARN is well formed and names a profile of no region
/// this endpoint serves, which is the missing-resource half of the tagging
/// refusal rather than the malformed half.
fn region_filled_profile_arn(filler: &str) -> String {
    let fixed = format!("arn:aws:signer::{ACCOUNT_ID}:/signing-profiles/{PROFILE_WITH_POLICY}");
    let region_length = filler
        .len()
        .checked_sub(fixed.len())
        .expect("every row is longer than an ARN's fixed fields");
    format!(
        "arn:aws:signer:{}:{ACCOUNT_ID}:/signing-profiles/{PROFILE_WITH_POLICY}",
        &filler[..region_length]
    )
}

/// Every required `GetRevocationStatus` query parameter, with `platformId` the one
/// under test and every other value one this endpoint serves.
fn get_revocation_status(platform_id: &str, _revision_id: &str) -> Request {
    let query = [
        ("signatureTimestamp", "2023-11-14T22%3A13%3A20Z".to_string()),
        ("platformId", encode(platform_id)),
        (
            "profileVersionArn",
            encode(&format!(
                "arn:aws:signer:{REGION}:{ACCOUNT_ID}:/signing-profiles/\
                 {PROFILE_WITH_POLICY}/abcde12345"
            )),
        ),
        (
            "jobArn",
            encode(&format!(
                "arn:aws:signer:{REGION}:{ACCOUNT_ID}:/signing-jobs/\
                 1a2b3c4d-5e6f-7890-abcd-ef1234567890"
            )),
        ),
        ("certificateHashes", "abcd".to_string()),
    ]
    .iter()
    .map(|(name, value)| format!("{name}={value}"))
    .collect::<Vec<_>>()
    .join("&");
    Request {
        method: "GET",
        uri: format!("/revocations?{query}"),
        body: None,
    }
}

fn get_signing_platform(platform_id: &str, _revision_id: &str) -> Request {
    Request {
        method: "GET",
        uri: format!("/signing-platforms/{}", encode(platform_id)),
        body: None,
    }
}

/// A create naming the platform under test, and the signing certificate a
/// platform the catalog does not publish is assumed to need: the member is
/// judged before the platform is resolved, so a create that leaves it out is
/// refused for the member rather than for the platform.
fn put_signing_profile(platform_id: &str, _revision_id: &str) -> Request {
    Request {
        method: "PUT",
        uri: "/signing-profiles/Echo_Created_Profile".to_string(),
        body: Some(json!({
            "platformId": platform_id,
            "signingMaterial": { "certificateArn": CERTIFICATE_ARN },
        })),
    }
}

fn list_tags_for_resource(resource_arn: &str, _revision_id: &str) -> Request {
    Request {
        method: "GET",
        uri: format!("/tags/{}", encode(resource_arn)),
        body: None,
    }
}

fn describe_signing_job(job_id: &str, _revision_id: &str) -> Request {
    Request {
        method: "GET",
        uri: format!("/signing-jobs/{}", encode(job_id)),
        body: None,
    }
}

fn revoke_signature(job_id: &str, _revision_id: &str) -> Request {
    Request {
        method: "PUT",
        uri: format!("/signing-jobs/{}/revoke", encode(job_id)),
        body: Some(json!({ "reason": "an echo-bound case" })),
    }
}

fn add_permission_with_action(action: &str, _revision_id: &str) -> Request {
    Request {
        method: "POST",
        uri: format!("/signing-profiles/{PROFILE_WITHOUT_POLICY}/permissions"),
        body: Some(json!({
            "statementId": "echo-action",
            "action": action,
            "principal": ACCOUNT_ID,
        })),
    }
}

/// A grant on the profile that HOLDS a policy, so the oversized `revisionId` is
/// judged against the revision that policy is at.
fn add_permission_with_revision(revision_id: &str, _current_revision: &str) -> Request {
    Request {
        method: "POST",
        uri: format!("/signing-profiles/{PROFILE_WITH_POLICY}/permissions"),
        body: Some(json!({
            "statementId": "echo-revision",
            "action": "signer:StartSigningJob",
            "principal": ACCOUNT_ID,
            "revisionId": revision_id,
        })),
    }
}

/// A removal from the profile that HOLDS a policy, naming the revision that
/// policy is at — so the oversized `statementId` is what the refusal is about.
fn remove_permission_by_statement(statement_id: &str, current_revision: &str) -> Request {
    Request {
        method: "DELETE",
        uri: format!(
            "/signing-profiles/{PROFILE_WITH_POLICY}/permissions/{}?revisionId={}",
            encode(statement_id),
            encode(current_revision)
        ),
        body: None,
    }
}

fn remove_permission_with_revision(revision_id: &str, _current_revision: &str) -> Request {
    Request {
        method: "DELETE",
        uri: format!(
            "/signing-profiles/{PROFILE_WITH_POLICY}/permissions/{SEEDED_STATEMENT_ID}\
             ?revisionId={}",
            encode(revision_id)
        ),
        body: None,
    }
}

fn platform_not_found(value: &str, _revision_id: &str) -> String {
    format!("Signing platform not found for ID {value}")
}

fn no_such_tagged_profile(value: &str, _revision_id: &str) -> String {
    format!("profile with arn {value} does not exists")
}

/// The sentence the job READER words for a `jobId` nothing is held under — which
/// is not the one the revoke words (see [`revoke_job_not_found`]).
fn describe_job_not_found(value: &str, _revision_id: &str) -> String {
    format!("Signing job not found for id {value}")
}

/// The sentence `RevokeSignature` words for the same condition.
fn revoke_job_not_found(value: &str, _revision_id: &str) -> String {
    format!("Signing Job with id {value} does not exist.")
}

fn ungrantable_action(value: &str, _revision_id: &str) -> String {
    format!(
        "Action {value} not supported for platform {PLATFORM_ID}. \
         Action must be one of [{GRANTABLE_ACTIONS}]"
    )
}

/// The refusal a `revisionId` the policy is not at takes: the value the request
/// carried, and the revision the policy IS at.
fn stale_revision(value: &str, current_revision: &str) -> String {
    format!(
        "Specified revisionId ({value}) does not match the revisionId of the existing policy \
         ({current_revision})"
    )
}

fn statement_not_found(value: &str, _revision_id: &str) -> String {
    format!(
        "statement id {value} does not exist in the permissions of profile \
         {PROFILE_WITH_POLICY}"
    )
}

/// Every refusal of this service that quotes a value its member puts no size on.
fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "GetRevocationStatus platformId (query)",
            filler: Filler::Ascii,
            carried: itself,
            request: get_revocation_status,
            status: 400,
            clause: platform_not_found,
        },
        Case {
            name: "GetSigningPlatform platformId (label)",
            filler: Filler::Ascii,
            carried: itself,
            request: get_signing_platform,
            status: 404,
            clause: platform_not_found,
        },
        Case {
            name: "PutSigningProfile platformId (body)",
            filler: Filler::Ascii,
            carried: itself,
            request: put_signing_profile,
            status: 404,
            clause: platform_not_found,
        },
        Case {
            name: "ListTagsForResource resourceArn naming no profile (label)",
            filler: Filler::Ascii,
            carried: region_filled_profile_arn,
            request: list_tags_for_resource,
            status: 404,
            clause: no_such_tagged_profile,
        },
        Case {
            name: "DescribeSigningJob jobId (label)",
            filler: Filler::Ascii,
            carried: itself,
            request: describe_signing_job,
            status: 404,
            clause: describe_job_not_found,
        },
        Case {
            name: "RevokeSignature jobId (label)",
            filler: Filler::Ascii,
            carried: itself,
            request: revoke_signature,
            status: 404,
            clause: revoke_job_not_found,
        },
        Case {
            name: "AddProfilePermission action (body)",
            filler: Filler::AnyText,
            carried: itself,
            request: add_permission_with_action,
            status: 400,
            clause: ungrantable_action,
        },
        Case {
            name: "AddProfilePermission revisionId (body)",
            filler: Filler::AnyText,
            carried: itself,
            request: add_permission_with_revision,
            status: 409,
            clause: stale_revision,
        },
        Case {
            name: "RemoveProfilePermission statementId (label)",
            filler: Filler::Ascii,
            carried: itself,
            request: remove_permission_by_statement,
            status: 404,
            clause: statement_not_found,
        },
        Case {
            name: "RemoveProfilePermission revisionId (query)",
            filler: Filler::Ascii,
            carried: itself,
            request: remove_permission_with_revision,
            status: 409,
            clause: stale_revision,
        },
    ]
}

/// A profile of `name`, created on a published platform.
async fn seed_profile(router: &MultiServiceRouter, name: &str) {
    let answer = call(
        router,
        &Request {
            method: "PUT",
            uri: format!("/signing-profiles/{name}"),
            body: Some(json!({ "platformId": PLATFORM_ID })),
        },
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "seeding profile {name}: expected HTTP 200, got {} {:?}",
        answer.status, answer.body
    );
}

/// The two profiles the permission cases need: one holding a policy with a
/// statement, one holding no policy at all — and the revision that policy is at,
/// which the rows naming a statement or a revision are judged against.
async fn seed_profiles(router: &MultiServiceRouter) -> String {
    seed_profile(router, PROFILE_WITH_POLICY).await;
    seed_profile(router, PROFILE_WITHOUT_POLICY).await;
    let granted = call(
        router,
        &Request {
            method: "POST",
            uri: format!("/signing-profiles/{PROFILE_WITH_POLICY}/permissions"),
            body: Some(json!({
                "statementId": SEEDED_STATEMENT_ID,
                "action": "signer:StartSigningJob",
                "principal": ACCOUNT_ID,
            })),
        },
    )
    .await;
    assert_eq!(
        granted.status, 200,
        "seeding a statement on {PROFILE_WITH_POLICY}: expected HTTP 200, got {} {:?}",
        granted.status, granted.body
    );
    granted.body["revisionId"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "seeding a statement on {PROFILE_WITH_POLICY}: expected a revisionId, got {:?}",
                granted.body
            )
        })
        .to_string()
}

/// Every refusal that quotes an unbounded request value states it up to the one
/// ceiling, whichever operation, member and binding carried it — and states a
/// value inside that ceiling whole.
#[tokio::test]
async fn every_unbounded_request_value_is_quoted_up_to_one_ceiling() {
    let router = emulator();
    let revision = seed_profiles(&router).await;

    for case in cases() {
        for (size, filler) in fillers(case.filler) {
            let carried = (case.carried)(&filler);
            let request = (case.request)(&carried, &revision);
            let label = format!("{} / {size}", case.name);
            let answer = call(&router, &request).await;

            assert_eq!(
                answer.status, case.status,
                "case {label:?}: expected HTTP {}, got {} {:?}",
                case.status, answer.status, answer.body
            );
            let expected = (case.clause)(&quoted(&carried), &revision);
            assert_eq!(
                answer.message(),
                expected,
                "case {label:?}: expected a message of {} bytes, got one of {} bytes: {:?}",
                expected.len(),
                answer.message().len(),
                answer.message()
            );
        }
    }
}

/// The error body is never sized by the request: whatever a caller sends for an
/// unbounded member, the message it is answered with stays within the ceiling plus
/// the fixed prose around it.
#[tokio::test]
async fn an_oversized_request_value_never_sizes_the_error_body() {
    let router = emulator();
    let revision = seed_profiles(&router).await;

    // The prose of every clause under test, plus the elision marker, is far
    // shorter than this: the point is that the message does not GROW with the
    // request.
    const PROSE: usize = 1_024;

    for case in cases() {
        for (size, filler) in fillers(case.filler) {
            if filler.len() <= BOUND {
                continue;
            }
            let carried = (case.carried)(&filler);
            let request = (case.request)(&carried, &revision);
            let label = format!("{} / {size}", case.name);
            let answer = call(&router, &request).await;
            assert!(
                answer.message().len() <= BOUND + PROSE,
                "case {label:?}: expected a message within {} bytes for a value of {} bytes, \
                 got one of {} bytes",
                BOUND + PROSE,
                carried.len(),
                answer.message().len()
            );
            assert!(
                answer.message().contains(&format!("[{} bytes]", carried.len())),
                "case {label:?}: expected the elided value to state its whole length, got a \
                 message of {} bytes starting {:?}",
                answer.message().len(),
                answer.message().chars().take(60).collect::<String>()
            );
        }
    }

    // A body-bound member takes a value far larger than any URI can carry, which
    // is the size class the rule exists for: a megabyte of request still answers
    // one ceiling's worth of message.
    let megabyte = "a".repeat(1024 * 1024);
    let answer = call(&router, &put_signing_profile(&megabyte, &revision)).await;
    assert_eq!(
        answer.message(),
        platform_not_found(&quoted(&megabyte), &revision),
        "case \"a megabyte in a body member\": expected the value stated up to the ceiling, got a \
         message of {} bytes",
        answer.message().len()
    );
    assert!(
        answer.message().len() <= BOUND + PROSE,
        "case \"a megabyte in a body member\": expected a message within {} bytes, got one of {} \
         bytes",
        BOUND + PROSE,
        answer.message().len()
    );
}

/// The two refusals one `platformId` can take — the published character class,
/// then the published catalog — are bounded ALIKE: the same oversized value,
/// refused one step apart, is quoted through the same ceiling by both.
#[tokio::test]
async fn both_refusals_of_one_platform_id_are_bounded_alike() {
    let router = emulator();

    // Inside the published character class, so the catalog is what refuses it.
    let in_class = "a".repeat(2 * BOUND);
    // The same value with one character the class excludes, so the class refuses
    // it instead.
    let out_of_class = format!("{in_class}!");

    let catalog = call(&router, &get_revocation_status(&in_class, "")).await;
    assert_eq!(
        catalog.status, 400,
        "case \"an in-class value past the ceiling\": expected HTTP 400, got {} {:?}",
        catalog.status, catalog.body
    );
    assert_eq!(
        catalog.message(),
        platform_not_found(&quoted(&in_class), ""),
        "case \"an in-class value past the ceiling\": expected the catalog refusal to state the \
         value up to the ceiling, got a message of {} bytes",
        catalog.message().len()
    );

    let class = call(&router, &get_revocation_status(&out_of_class, "")).await;
    assert_eq!(
        class.status, 400,
        "case \"a value the class excludes\": expected HTTP 400, got {} {:?}",
        class.status, class.body
    );
    assert!(
        class
            .message()
            .contains(&format!("[{} bytes]", out_of_class.len())),
        "case \"a value the class excludes\": expected the class refusal to elide the value, got \
         a message of {} bytes",
        class.message().len()
    );

    // Neither answer is sized by the request: both stay within one ceiling.
    for (label, message, sent) in [
        ("the catalog refusal", catalog.message(), in_class.len()),
        ("the class refusal", class.message(), out_of_class.len()),
    ] {
        assert!(
            message.len() <= BOUND + 1_024,
            "case {label:?}: expected a message within {} bytes for a value of {sent} bytes, got \
             one of {} bytes",
            BOUND + 1_024,
            message.len()
        );
    }
}

/// The most bytes a statement id can have and still be STORED in a policy: the
/// bound one profile's policy holds (20 480 bytes, spelled out rather than
/// imported — `crate::state::permissions::MAX_POLICY_SIZE_BYTES`) less a kilobyte
/// for the policy document around the one statement that carries the id.
const STORABLE_STATEMENT_ID: usize = 20_480 - 1_024;

/// The size classes of [`fillers`] a policy can hold a statement id in — derived
/// from that one table so the two cannot drift apart.
///
/// The duplicate refusal exists only once the id is STORED, so a class past what a
/// policy holds is driven by
/// [`a_statement_id_past_the_policy_bound_is_never_quoted`] instead.
fn statement_id_fillers() -> Vec<(&'static str, String)> {
    fillers(Filler::AnyText)
        .into_iter()
        .filter(|(_, value)| value.len() <= STORABLE_STATEMENT_ID)
        .collect()
}

/// A grant that names a `statementId` the profile's policy already holds states
/// that id through the SAME ceiling as every row above.
///
/// This is the one refusal of the permissions slice the table cannot reach: the
/// conflict exists only once the oversized id is in the policy, so each row stores
/// it first and then names it again. The conflict is decided before the policy's
/// byte bound, so the clause is reached with the caller's own id in hand — and the
/// refusal is a refusal, so the policy it was judged against is left as it was.
///
/// Each row gets its own profile: one policy holds a bounded number of bytes and
/// these ids are most of that bound.
#[tokio::test]
async fn a_duplicate_statement_id_is_quoted_up_to_one_ceiling() {
    let router = emulator();

    for (index, (size, statement_id)) in statement_id_fillers().into_iter().enumerate() {
        let profile = format!("Echo_Duplicate_Profile_{index}");
        seed_profile(&router, &profile).await;
        let grant = Request {
            method: "POST",
            uri: format!("/signing-profiles/{profile}/permissions"),
            body: Some(json!({
                "statementId": statement_id,
                "action": "signer:StartSigningJob",
                "principal": ACCOUNT_ID,
            })),
        };

        let stored = call(&router, &grant).await;
        assert_eq!(
            stored.status, 200,
            "case {size:?}: expected the first grant to be stored (HTTP 200), got {} {:?}",
            stored.status, stored.body
        );
        let revision = stored.body["revisionId"].as_str().unwrap_or_default().to_string();

        let again = call(&router, &grant).await;
        assert_eq!(
            again.status, 409,
            "case {size:?}: expected HTTP 409, got {} {:?}",
            again.status, again.body
        );
        let expected = format!(
            "statement id {} already exists in the permissions of profile {profile}",
            quoted(&statement_id)
        );
        assert_eq!(
            again.message(),
            expected,
            "case {size:?}: expected a message of {} bytes for an id of {} bytes, got one of {} \
             bytes: {:?}",
            expected.len(),
            statement_id.len(),
            again.message().len(),
            again.message().chars().take(60).collect::<String>()
        );

        // The refused grant changed nothing: the policy still holds the one
        // statement it was seeded with, at the revision that seeding minted.
        let listed = call(
            &router,
            &Request {
                method: "GET",
                uri: format!("/signing-profiles/{profile}/permissions"),
                body: None,
            },
        )
        .await;
        assert_eq!(
            listed.body["permissions"].as_array().map(Vec::len),
            Some(1),
            "case {size:?}: expected the refused duplicate to leave one statement, got {:?}",
            listed.body["permissions"]
        );
        assert_eq!(
            listed.body["revisionId"].as_str(),
            Some(revision.as_str()),
            "case {size:?}: expected the refused duplicate to leave the revision at {revision:?}, \
             got {:?}",
            listed.body["revisionId"]
        );
    }
}

/// A `statementId` larger than a whole policy may hold never reaches the
/// duplicate-statement clause at all: the grant is refused for the policy's byte
/// bound, and that refusal states sizes instead of quoting the value — so the
/// error body is bounded over the WHOLE size range this member admits, not only up
/// to the ceiling.
#[tokio::test]
async fn a_statement_id_past_the_policy_bound_is_never_quoted() {
    /// The most bytes one profile's permission policy holds, spelled out rather
    /// than imported (`crate::state::permissions::MAX_POLICY_SIZE_BYTES`): an id
    /// longer than this cannot be in a policy, so no conflict can quote it.
    const MAX_POLICY_SIZE_BYTES: usize = 20_480;

    /// The fixed prose of the refusal, far longer than the sentence under test:
    /// the point is that the message does not GROW with the request.
    const PROSE: usize = 1_024;

    let router = emulator();
    let profile = "Echo_Oversized_Statement_Profile";
    seed_profile(&router, profile).await;

    for (size, statement_id) in [
        (
            "an id one byte past what a policy holds",
            "s".repeat(MAX_POLICY_SIZE_BYTES + 1),
        ),
        ("an id of 64 KiB", "s".repeat(64 * 1024)),
        ("a megabyte of id", "s".repeat(1024 * 1024)),
    ] {
        let answer = call(
            &router,
            &Request {
                method: "POST",
                uri: format!("/signing-profiles/{profile}/permissions"),
                body: Some(json!({
                    "statementId": statement_id,
                    "action": "signer:StartSigningJob",
                    "principal": ACCOUNT_ID,
                })),
            },
        )
        .await;
        assert_eq!(
            answer.status, 402,
            "case {size:?}: expected HTTP 402, got {} {:?}",
            answer.status,
            answer.message().chars().take(60).collect::<String>()
        );
        assert!(
            answer.message().len() <= PROSE,
            "case {size:?}: expected a message within {PROSE} bytes for an id of {} bytes, got \
             one of {} bytes",
            statement_id.len(),
            answer.message().len()
        );
        assert!(
            !answer.message().contains(&statement_id[..256]),
            "case {size:?}: expected the refusal to quote no part of the id, got {:?}",
            answer.message().chars().take(60).collect::<String>()
        );
    }
}
