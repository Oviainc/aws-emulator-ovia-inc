// SPDX-License-Identifier: CC0-1.0

//! A request failing BOTH a stored-resource precondition and the role check is refused on
//! the stored resource.
//!
//! RULE under test: when one request both fails a precondition this service decides against
//! the STORED resource the request acts on (the reference does not resolve, or the resolved
//! record's state does not admit the action) and names a role whose trust policy withholds
//! `sts:AssumeRole` from this service, the `UnprocessableEntityException` (422,
//! `smithy_query(query="operation",operation="CreateHarvestJob,CreateOriginEndpoint")`) is
//! keyed by the STORED resource's member and states only its reasons — the role is not what
//! such a caller has to fix, and the body carries no entry for it. The role refusal is the
//! answer only once the stored-resource precondition holds.
//!
//! The rows span what the rule generalises over: which operation carries the request
//! (`CreateHarvestJob`, `CreateOriginEndpoint`), which kind of stored-resource precondition
//! fails (an unresolvable reference, and a resolved record whose state forbids the action),
//! the form the trust policy takes (another service's principal, an explicit deny, no
//! statement at all, a different action), and which member the role travels under
//! (`s3Destination.roleArn`, a SPEKE provider's `roleArn` nested four deep). The control
//! rows — a stored resource that holds up its end, and a role the service may act as — pin
//! that nothing else moved: the role refusal still answers, and a request failing neither is
//! accepted.
//!
//! Every case drives real HTTP through the `MultiServiceRouter` — the same dispatch the
//! served emulator uses — over ONE engine carrying both MediaPackage's store and IAM's, so
//! the roles judged are the records `CreateRole` really wrote. The harness is the one
//! `tests/all/channel_lifecycle.rs` and `tests/all/harvest_job_rejections.rs` own.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::{call, error_code};
use crate::harvest_job_rejections::{create_role, emulator, policy, PRINCIPAL};

/// The channel every origin endpoint below packages.
const CHANNEL_ID: &str = "order-channel";

/// A channel id no create ever used, for the rows whose reference must not resolve.
const ABSENT_CHANNEL_ID: &str = "order-channel-never-created";

/// An origin-endpoint id no create ever used, for the rows whose reference must not resolve.
const ABSENT_ENDPOINT_ID: &str = "order-endpoint-never-created";

/// A startover window that retains a day of content, so a harvest of the endpoint can be
/// carried out.
const HARVESTABLE_WINDOW_SECONDS: i32 = 86_400;

/// The member a harvest job's destination role travels under.
const HARVEST_ROLE_MEMBER: &str = "s3Destination.roleArn";

/// The member a SPEKE key provider's role travels under on an HLS-packaged endpoint.
const SPEKE_ROLE_MEMBER: &str = "hlsPackage.encryption.spekeKeyProvider.roleArn";

/// The forms a trust policy takes around ONE question — whether it grants this service
/// `sts:AssumeRole`. How a document is READ is tested where that reader lives
/// (`services/common/src/iam_trust_policy.rs`); these rows only need each answer.
#[derive(Clone, Copy, Debug)]
enum Trust {
    /// Grants the principal the action, so the service may act as the role.
    Granted,
    /// Grants the action to another service's principal only.
    AnotherPrincipal,
    /// Names the principal and the action, but denies them.
    Denied,
    /// Carries no statement at all, so nothing is granted.
    NoStatement,
    /// Grants the principal an action that is not assuming the role.
    AnotherAction,
}

impl Trust {
    /// The trust-policy document as a client would send it to `CreateRole`.
    fn document(self) -> String {
        match self {
            Self::Granted => policy("Allow", "\"sts:AssumeRole\"", &format!("\"{PRINCIPAL}\"")),
            Self::AnotherPrincipal => {
                policy("Allow", "\"sts:AssumeRole\"", "\"mediatailor.amazonaws.com\"")
            }
            Self::Denied => policy("Deny", "\"sts:AssumeRole\"", &format!("\"{PRINCIPAL}\"")),
            Self::NoStatement => "{\"Version\":\"2012-10-17\",\"Statement\":[]}".to_string(),
            Self::AnotherAction => {
                policy("Allow", "\"s3:PutObject\"", &format!("\"{PRINCIPAL}\""))
            }
        }
    }
}

/// What the service must answer a row with.
#[derive(Clone, Copy, Debug)]
enum Expect {
    /// The request is carried out.
    Accepted,
    /// Refused 422, keyed by this member, stating these reasons as a LIST — the shape
    /// everything this service decides against STATE reports.
    State(&'static str, &'static str),
    /// Refused 422, keyed by this member, stating as that member's single requirement the
    /// one sentence a role this service may not act as is refused with.
    Role(&'static str),
}

/// The requirement a role the service may not act as fails, worded as this service words it
/// for every member that carries such a role (`state::assumable_role_requirement`), with
/// the principal the policy has to admit substituted in.
fn role_requirement() -> String {
    format!(
        "must be assumable by MediaPackage. Please ensure that its trust policy includes \
         {PRINCIPAL}"
    )
}

/// Every key the refusal's `errors` map carries, which is what says WHICH failure the
/// service decided.
fn error_keys(refusal: &Value) -> Vec<String> {
    match refusal.get("errors").and_then(Value::as_object) {
        None => Vec::new(),
        Some(errors) => errors.keys().cloned().collect(),
    }
}

/// Assert the answer `(status, body)` is exactly what `expect` requires — including that the
/// body names NO member other than the one the rule keys the failure by.
fn assert_answer(case: &str, status: u16, body: &Value, expect: Expect) {
    let member = match expect {
        Expect::Accepted => {
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} {body:?}"
            );
            return;
        }
        Expect::State(member, reason) => {
            assert_eq!(
                body.pointer(&format!("/errors/{member}")),
                Some(&json!([reason])),
                "case {case:?}: expected errors.{member} == [{reason:?}], got {body:?}"
            );
            assert_eq!(
                body.get("message").and_then(Value::as_str),
                Some(format!("{member} {reason}").as_str()),
                "case {case:?}: expected message {:?}, got {body:?}",
                format!("{member} {reason}")
            );
            member
        }
        Expect::Role(member) => {
            let requirement = role_requirement();
            assert_eq!(
                body.pointer(&format!("/errors/{member}")),
                Some(&json!(requirement)),
                "case {case:?}: expected errors.{member} == {requirement:?}, got {body:?}"
            );
            let sentence = format!("{member} {requirement}");
            assert_eq!(
                body.get("message").and_then(Value::as_str),
                Some(sentence.as_str()),
                "case {case:?}: expected message {sentence:?}, got {body:?}"
            );
            member
        }
    };
    assert_eq!(
        status, 422,
        "case {case:?}: expected 422, got {status} {body:?}"
    );
    assert_eq!(
        error_code(body),
        "UnprocessableEntityException",
        "case {case:?}: expected UnprocessableEntityException, got {body:?}"
    );
    assert_eq!(
        error_keys(body),
        vec![member.to_string()],
        "case {case:?}: the refusal must name only {member:?}, got {body:?}"
    );
}

/// Assert the service stored nothing under `uri`.
async fn assert_not_stored(router: &MultiServiceRouter, case: &str, uri: &str) {
    let (status, described) = call(router, "GET", uri, None).await;
    assert_eq!(
        status, 404,
        "case {case:?}: a refused request must persist nothing, got {status} {described:?}"
    );
    assert_eq!(
        error_code(&described),
        "NotFoundException",
        "case {case:?}: expected NotFoundException, got {described:?}"
    );
}

/// Create the channel every endpoint below packages.
async fn create_channel(router: &MultiServiceRouter) {
    let (status, body) = call(
        router,
        "POST",
        "/channels",
        Some(&json!({ "id": CHANNEL_ID })),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateChannel expected 200, got {status} {body:?}"
    );
}

/// How the origin endpoint a harvest row submits against is left.
#[derive(Clone, Copy, Debug)]
enum Endpoint {
    /// No endpoint of that id exists, so `originEndpointId` resolves to nothing.
    Absent,
    /// Stored with startover playback disabled, so the record retains nothing to harvest.
    NoStartover,
    /// Stored retaining a startover window, so a harvest of it can be carried out.
    Harvestable,
}

/// One `CreateHarvestJob` row: the state of the endpoint it harvests, the trust policy of
/// the destination role, and the answer the rule requires.
#[derive(Clone, Copy, Debug)]
struct HarvestRow {
    case: &'static str,
    endpoint: Endpoint,
    trust: Trust,
    expect: Expect,
}

/// The harvest rows: each kind of stored-endpoint failure crossed with each form of a trust
/// policy that withholds the grant, plus the controls.
const HARVEST_ROWS: &[HarvestRow] = &[
    HarvestRow {
        case: "an endpoint retaining nothing and a role trusting another principal",
        endpoint: Endpoint::NoStartover,
        trust: Trust::AnotherPrincipal,
        expect: Expect::State("originEndpointId", "must have startover enabled"),
    },
    HarvestRow {
        case: "an endpoint retaining nothing and a role denying this principal",
        endpoint: Endpoint::NoStartover,
        trust: Trust::Denied,
        expect: Expect::State("originEndpointId", "must have startover enabled"),
    },
    HarvestRow {
        case: "an endpoint retaining nothing and a role granting nothing at all",
        endpoint: Endpoint::NoStartover,
        trust: Trust::NoStatement,
        expect: Expect::State("originEndpointId", "must have startover enabled"),
    },
    HarvestRow {
        case: "an endpoint retaining nothing and a role granting another action",
        endpoint: Endpoint::NoStartover,
        trust: Trust::AnotherAction,
        expect: Expect::State("originEndpointId", "must have startover enabled"),
    },
    HarvestRow {
        case: "no such endpoint and a role trusting another principal",
        endpoint: Endpoint::Absent,
        trust: Trust::AnotherPrincipal,
        expect: Expect::State("originEndpointId", "not found"),
    },
    HarvestRow {
        case: "no such endpoint and a role granting nothing at all",
        endpoint: Endpoint::Absent,
        trust: Trust::NoStatement,
        expect: Expect::State("originEndpointId", "not found"),
    },
    HarvestRow {
        case: "an endpoint retaining nothing and a role the service may act as",
        endpoint: Endpoint::NoStartover,
        trust: Trust::Granted,
        expect: Expect::State("originEndpointId", "must have startover enabled"),
    },
    HarvestRow {
        case: "a harvestable endpoint and a role trusting another principal",
        endpoint: Endpoint::Harvestable,
        trust: Trust::AnotherPrincipal,
        expect: Expect::Role(HARVEST_ROLE_MEMBER),
    },
    HarvestRow {
        case: "a harvestable endpoint and a role denying this principal",
        endpoint: Endpoint::Harvestable,
        trust: Trust::Denied,
        expect: Expect::Role(HARVEST_ROLE_MEMBER),
    },
    HarvestRow {
        case: "a harvestable endpoint and a role the service may act as",
        endpoint: Endpoint::Harvestable,
        trust: Trust::Granted,
        expect: Expect::Accepted,
    },
];

/// Create the endpoint a harvest row submits against, and answer the id it names.
async fn provision_endpoint(router: &MultiServiceRouter, row: &HarvestRow, index: usize) -> String {
    let startover = match row.endpoint {
        Endpoint::Absent => return ABSENT_ENDPOINT_ID.to_string(),
        Endpoint::NoStartover => 0,
        Endpoint::Harvestable => HARVESTABLE_WINDOW_SECONDS,
    };
    let id = format!("order-endpoint-{index}");
    let (status, created) = call(
        router,
        "POST",
        "/origin_endpoints",
        Some(&json!({
            "id": id,
            "channelId": CHANNEL_ID,
            "hlsPackage": {},
            "startoverWindowSeconds": startover
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: CreateOriginEndpoint expected 200, got {status} {created:?}",
        row.case
    );
    id
}

#[tokio::test]
async fn a_harvest_is_refused_on_its_endpoint_before_its_destination_role() {
    let router = emulator();
    create_channel(&router).await;

    for (index, row) in HARVEST_ROWS.iter().enumerate() {
        let endpoint = provision_endpoint(&router, row, index).await;
        let role_arn = create_role(
            &router,
            &format!("order-harvest-role-{index}"),
            &row.trust.document(),
        )
        .await;
        let id = format!("order-harvest-{index}");
        let (status, answered) = call(
            &router,
            "POST",
            "/harvest_jobs",
            Some(&json!({
                "id": id,
                "originEndpointId": endpoint,
                "startTime": "2026-01-01T00:00:00Z",
                "endTime": "2026-01-01T01:00:00Z",
                "s3Destination": {
                    "bucketName": "order-harvests",
                    "manifestKey": "harvests/index.m3u8",
                    "roleArn": role_arn
                }
            })),
        )
        .await;

        assert_answer(row.case, status, &answered, row.expect);
        match row.expect {
            Expect::Accepted => {
                // Accepted: the read renders the record the submission wrote.
                let (status, described) =
                    call(&router, "GET", &format!("/harvest_jobs/{id}"), None).await;
                assert_eq!(
                    status, 200,
                    "case {:?}: DescribeHarvestJob expected 200, got {status} {described:?}",
                    row.case
                );
                assert_eq!(
                    described, answered,
                    "case {:?}: the read must render the stored record, got {described:?}",
                    row.case
                );
            }
            _ => assert_not_stored(&router, row.case, &format!("/harvest_jobs/{id}")).await,
        }
    }
}

/// One `CreateOriginEndpoint` row: whether the channel it packages exists, the trust policy
/// of the SPEKE role its encryption names, and the answer the rule requires.
#[derive(Clone, Copy, Debug)]
struct EndpointRow {
    case: &'static str,
    /// `true` when the create names the stored channel, `false` when it names none.
    channel_exists: bool,
    trust: Trust,
    expect: Expect,
}

/// The origin-endpoint rows: the same rule where the stored-resource failure is an
/// unresolvable `channelId` and the role travels four levels deep inside the packaging
/// configuration.
const ENDPOINT_ROWS: &[EndpointRow] = &[
    EndpointRow {
        case: "no such channel and a SPEKE role trusting another principal",
        channel_exists: false,
        trust: Trust::AnotherPrincipal,
        expect: Expect::State("channelId", "not found"),
    },
    EndpointRow {
        case: "no such channel and a SPEKE role denying this principal",
        channel_exists: false,
        trust: Trust::Denied,
        expect: Expect::State("channelId", "not found"),
    },
    EndpointRow {
        case: "no such channel and a SPEKE role granting nothing at all",
        channel_exists: false,
        trust: Trust::NoStatement,
        expect: Expect::State("channelId", "not found"),
    },
    EndpointRow {
        case: "no such channel and a SPEKE role the service may act as",
        channel_exists: false,
        trust: Trust::Granted,
        expect: Expect::State("channelId", "not found"),
    },
    EndpointRow {
        case: "the stored channel and a SPEKE role granting another action",
        channel_exists: true,
        trust: Trust::AnotherAction,
        expect: Expect::Role(SPEKE_ROLE_MEMBER),
    },
    EndpointRow {
        case: "the stored channel and a SPEKE role trusting another principal",
        channel_exists: true,
        trust: Trust::AnotherPrincipal,
        expect: Expect::Role(SPEKE_ROLE_MEMBER),
    },
    EndpointRow {
        case: "the stored channel and a SPEKE role the service may act as",
        channel_exists: true,
        trust: Trust::Granted,
        expect: Expect::Accepted,
    },
];

/// A SPEKE key provider carrying `role_arn`. `ResourceId`, `RoleArn`, `SystemIds` and `Url`
/// are all required on the shape
/// (`smithy_query(query="shape",shape="com.amazonaws.mediapackage#SpekeKeyProvider")`), so
/// the request reaches the handler instead of being refused for a missing member.
fn speke(role_arn: &str) -> Value {
    json!({
        "resourceId": "order-endpoint",
        "roleArn": role_arn,
        "systemIds": ["81376844-f976-481e-a84e-cc25d39b0b33"],
        "url": "https://speke.example.com/v1/copyProtection"
    })
}

#[tokio::test]
async fn an_endpoint_is_refused_on_its_channel_before_its_speke_role() {
    let router = emulator();
    create_channel(&router).await;

    for (index, row) in ENDPOINT_ROWS.iter().enumerate() {
        let role_arn = create_role(
            &router,
            &format!("order-speke-role-{index}"),
            &row.trust.document(),
        )
        .await;
        let id = format!("order-encrypted-endpoint-{index}");
        let channel = if row.channel_exists {
            CHANNEL_ID
        } else {
            ABSENT_CHANNEL_ID
        };
        let (status, answered) = call(
            &router,
            "POST",
            "/origin_endpoints",
            Some(&json!({
                "id": id,
                "channelId": channel,
                "hlsPackage": {
                    "encryption": { "spekeKeyProvider": speke(&role_arn) }
                }
            })),
        )
        .await;

        assert_answer(row.case, status, &answered, row.expect);
        match row.expect {
            Expect::Accepted => {
                let (status, described) =
                    call(&router, "GET", &format!("/origin_endpoints/{id}"), None).await;
                assert_eq!(
                    status, 200,
                    "case {:?}: DescribeOriginEndpoint expected 200, got {status} {described:?}",
                    row.case
                );
                assert_eq!(
                    described.pointer("/hlsPackage/encryption/spekeKeyProvider/roleArn"),
                    Some(&json!(role_arn)),
                    "case {:?}: the stored endpoint must carry the SPEKE role, got \
                     {described:?}",
                    row.case
                );
            }
            _ => assert_not_stored(&router, row.case, &format!("/origin_endpoints/{id}")).await,
        }
    }
}
