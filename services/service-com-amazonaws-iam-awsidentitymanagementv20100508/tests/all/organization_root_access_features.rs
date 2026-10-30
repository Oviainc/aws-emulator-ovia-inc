// SPDX-License-Identifier: CC0-1.0

//! What `ListOrganizationsFeatures` reports, end to end over the awsQuery wire.
//!
//! The rule under test: the centralized root access features are reported from the
//! organization the account belongs to — its id, and the `FeatureType` values that
//! organization holds enabled — and only to an account that meets every standing
//! requirement of that access; each requirement the account does not meet is the modelled
//! error AWS declares for it, at HTTP 400.
//!
//! Each test is a TABLE whose rows differ along what the rule generalises over: which
//! requirement the organization fails, how the organization is spelled (both published
//! bounds of `OrganizationIdType`), which features it holds enabled (every subset of the
//! modelled catalog, recorded in both orders), and whether the caller manages the
//! organization, administers its root access, or neither.
//!
//! The organization itself is state AWS Organizations — not IAM — puts an account in, so a
//! suite reaches the world it is about by recording it in the service's own store
//! (`tests/all/organizations_access_report_generation.rs` owns that helper). The awsQuery
//! harness is the one `tests/all/user_lifecycle.rs` owns and the other suites in this crate
//! share.

use service_com_amazonaws_iam_awsidentitymanagementv20100508::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::identity_policy_enforcement::{principal, statement};
use crate::organizations_access_report_generation::record_organization;
use crate::user_lifecycle::{
    account_id, assert_access_denied_message, call_signed_by, element, elements, error_code,
    router_with_engine,
};

/// The IAM action this operation is decided on, as a policy statement names it.
const ACTION: &str = "iam:ListOrganizationsFeatures";

/// One of the two centralized root access features the model declares.
///
/// Source: `FeatureType` (`smithy_query(query="shape", shape="FeatureType")`:
/// `RootCredentialsManagement`, `RootSessions`), which is the whole catalog
/// `ListOrganizationsFeatures$EnabledFeatures` reports out of.
const ROOT_CREDENTIALS_MANAGEMENT: &str = "RootCredentialsManagement";

/// The other value `FeatureType` declares. See [`ROOT_CREDENTIALS_MANAGEMENT`].
const ROOT_SESSIONS: &str = "RootSessions";

/// A 12-digit account that is not the one this emulator's callers speak for, for the rows
/// where the organization is managed or administered by somebody else.
const ANOTHER_ACCOUNT: &str = "111122223333";

/// A third account, for the row where the delegated administrators are accounts other than
/// the caller.
const A_THIRD_ACCOUNT: &str = "444455556666";

/// The organization id the AWS documentation itself uses, spelled as `OrganizationIdType`
/// requires (len 0..34, pattern `^o-[a-z0-9]{10,32}$`).
const ORGANIZATION: &str = "o-a1b2c3d4e5";

/// The shortest organization id the pattern admits: `o-` plus its minimum of 10 characters.
const SHORTEST_ORGANIZATION: &str = "o-0123456789";

/// The longest organization id the pattern admits: `o-` plus its maximum of 32 characters,
/// which is also the member's maximum length of 34.
fn longest_organization() -> String {
    format!("o-{}", "a9".repeat(16))
}

/// The organization recorded for a row: the organization an account is in, plus everything
/// centralized root access is decided from and reported out of.
#[derive(Debug, Clone)]
struct OrganizationFixture {
    organization_id: String,
    management_account_id: String,
    all_features_enabled: bool,
    iam_trusted_access_enabled: bool,
    delegated_administrators: Vec<String>,
    enabled_features: Vec<&'static str>,
}

impl OrganizationFixture {
    /// An organization that meets every standing requirement for the account this
    /// emulator's callers speak for: all features enabled, IAM trusted access enabled, and
    /// the caller managing it. A row changes the one fact it is about.
    fn serving(organization_id: &str) -> Self {
        Self {
            organization_id: organization_id.to_string(),
            management_account_id: account_id().to_string(),
            all_features_enabled: true,
            iam_trusted_access_enabled: true,
            delegated_administrators: Vec::new(),
            enabled_features: Vec::new(),
        }
    }

    fn managed_by(mut self, management_account_id: &str) -> Self {
        self.management_account_id = management_account_id.to_string();
        self
    }

    fn administered_by(mut self, accounts: &[&str]) -> Self {
        self.delegated_administrators = accounts.iter().map(ToString::to_string).collect();
        self
    }

    fn with_features(mut self, features: &[&'static str]) -> Self {
        self.enabled_features = features.to_vec();
        self
    }
}

/// Record `fixture` in the service's OWN store, as the world the call is then answered
/// from: the organization through the helper every suite records one with, and the
/// centralized root access facts onto that same record.
fn record(engine: &engine::InMemoryEngine, fixture: &OrganizationFixture) {
    record_organization(
        engine,
        &fixture.organization_id,
        &fixture.management_account_id,
    );
    let store = state::service_state(engine).expect("the IAM store");
    let mut held = store.lock();
    let organization = held
        .organization
        .as_mut()
        .expect("the organization just recorded");
    organization.all_features_enabled = fixture.all_features_enabled;
    organization.iam_trusted_access_enabled = fixture.iam_trusted_access_enabled;
    organization.root_access_delegated_administrator_ids = fixture
        .delegated_administrators
        .iter()
        .cloned()
        .collect();
    organization.enabled_root_access_features = fixture
        .enabled_features
        .iter()
        .map(ToString::to_string)
        .collect();
}

/// Everything the store holds about the organization — what a read must leave exactly as
/// it found it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct HeldOrganization {
    organization_id: String,
    all_features_enabled: bool,
    iam_trusted_access_enabled: bool,
    delegated_administrators: Vec<String>,
    enabled_features: Vec<String>,
}

/// The organization the store holds now, read straight out of it.
fn held_organization(engine: &engine::InMemoryEngine) -> Option<HeldOrganization> {
    let store = state::service_state(engine).expect("the IAM store");
    let held = store.lock();
    held.organization
        .as_ref()
        .map(|organization| HeldOrganization {
            organization_id: organization.organization_id.clone(),
            all_features_enabled: organization.all_features_enabled,
            iam_trusted_access_enabled: organization.iam_trusted_access_enabled,
            delegated_administrators: organization
                .root_access_delegated_administrator_ids
                .iter()
                .cloned()
                .collect(),
            enabled_features: organization
                .enabled_root_access_features
                .iter()
                .cloned()
                .collect(),
        })
}

/// One `ListOrganizationsFeatures` call, presented as signed by `akid` when a row names
/// one. The request carries no members of its own.
async fn list(router: &MultiServiceRouter, akid: Option<&str>) -> (u16, String) {
    call_signed_by(router, akid, "ListOrganizationsFeatures", &[]).await
}

/// One row of [`served_rows`]: the world recorded, and the report the rule requires of it.
struct ServedCase {
    case: &'static str,
    organization: OrganizationFixture,
    /// The `EnabledFeatures` list the call must report, in the order it must report it.
    reported: Vec<&'static str>,
}

/// The worlds in which the report IS served: every subset of the modelled feature catalog,
/// both published bounds of the organization id, and each account the access is served to.
fn served_rows() -> Vec<ServedCase> {
    vec![
        ServedCase {
            case: "the management account, with no feature enabled",
            organization: OrganizationFixture::serving(ORGANIZATION),
            reported: Vec::new(),
        },
        ServedCase {
            case: "the management account, with root credentials management enabled",
            organization: OrganizationFixture::serving(ORGANIZATION)
                .with_features(&[ROOT_CREDENTIALS_MANAGEMENT]),
            reported: vec![ROOT_CREDENTIALS_MANAGEMENT],
        },
        ServedCase {
            case: "the management account, with root sessions enabled",
            organization: OrganizationFixture::serving(SHORTEST_ORGANIZATION)
                .with_features(&[ROOT_SESSIONS]),
            reported: vec![ROOT_SESSIONS],
        },
        ServedCase {
            case: "the management account, with both features enabled",
            organization: OrganizationFixture::serving(ORGANIZATION)
                .with_features(&[ROOT_CREDENTIALS_MANAGEMENT, ROOT_SESSIONS]),
            reported: vec![ROOT_CREDENTIALS_MANAGEMENT, ROOT_SESSIONS],
        },
        ServedCase {
            // The same two features, recorded the other way round: what is reported is the
            // modelled catalog narrowed to what is enabled, so the order does not follow
            // whichever write came last.
            case: "the management account, with both features enabled in the other order",
            organization: OrganizationFixture::serving(ORGANIZATION)
                .with_features(&[ROOT_SESSIONS, ROOT_CREDENTIALS_MANAGEMENT]),
            reported: vec![ROOT_CREDENTIALS_MANAGEMENT, ROOT_SESSIONS],
        },
        ServedCase {
            case: "a delegated administrator of an organization another account manages",
            organization: OrganizationFixture::serving(SHORTEST_ORGANIZATION)
                .managed_by(ANOTHER_ACCOUNT)
                .administered_by(&[account_id()])
                .with_features(&[ROOT_SESSIONS]),
            reported: vec![ROOT_SESSIONS],
        },
        ServedCase {
            case: "one of several delegated administrators",
            organization: OrganizationFixture::serving(ORGANIZATION)
                .managed_by(ANOTHER_ACCOUNT)
                .administered_by(&[A_THIRD_ACCOUNT, account_id()])
                .with_features(&[ROOT_CREDENTIALS_MANAGEMENT]),
            reported: vec![ROOT_CREDENTIALS_MANAGEMENT],
        },
    ]
}

/// RULE: for an account the centralized root access operations are served to, the report is
/// the organization it belongs to — that organization's id, and the modelled features it
/// holds enabled.
#[tokio::test]
async fn the_report_is_the_organization_the_account_belongs_to() {
    // The longest organization id the member admits, as its own row beside the table's.
    let longest = longest_organization();
    let rows = served_rows().into_iter().chain(::std::iter::once(ServedCase {
        case: "the longest organization id the member admits",
        organization: OrganizationFixture::serving(&longest)
            .with_features(&[ROOT_CREDENTIALS_MANAGEMENT, ROOT_SESSIONS]),
        reported: vec![ROOT_CREDENTIALS_MANAGEMENT, ROOT_SESSIONS],
    }));

    for ServedCase {
        case,
        organization,
        reported,
    } in rows
    {
        let (router, engine) = router_with_engine();
        record(&engine, &organization);

        let (status, body) = list(&router, None).await;

        assert_eq!(
            status, 200,
            "case {case:?}: expected the features to be reported, got {status}: {body}"
        );
        assert_eq!(
            element(&body, "OrganizationId"),
            organization.organization_id,
            "case {case:?}: expected the organization the account belongs to, got {:?}: {body}",
            element(&body, "OrganizationId")
        );
        assert_eq!(
            elements(&body, "member"),
            reported,
            "case {case:?}: expected the enabled features {reported:?}, got {:?}: {body}",
            elements(&body, "member")
        );
    }
}

/// One row of [`refused_rows`]: the world recorded — `None` for an account in no
/// organization at all — and the refusal the rule requires of it.
struct RefusedCase {
    case: &'static str,
    organization: Option<OrganizationFixture>,
    /// The wire error code the refusal carries.
    code: &'static str,
    /// The whole message it carries: the reason AWS states for that error.
    message: &'static str,
}

/// The worlds in which the report is REFUSED: one row per standing requirement of
/// centralized root access the account does not meet.
fn refused_rows() -> Vec<RefusedCase> {
    vec![
        RefusedCase {
            case: "an account in no organization at all",
            organization: None,
            code: "OrganizationNotFoundException",
            message: "The request was rejected because no organization is associated with \
                      your account.",
        },
        RefusedCase {
            case: "an organization without All features",
            organization: Some(OrganizationFixture {
                all_features_enabled: false,
                ..OrganizationFixture::serving(ORGANIZATION)
                    .with_features(&[ROOT_CREDENTIALS_MANAGEMENT])
            }),
            code: "OrganizationNotInAllFeaturesModeException",
            message: "The request was rejected because your organization does not have All \
                      features enabled.",
        },
        RefusedCase {
            case: "an organization IAM has no trusted access to",
            organization: Some(OrganizationFixture {
                iam_trusted_access_enabled: false,
                ..OrganizationFixture::serving(ORGANIZATION).with_features(&[ROOT_SESSIONS])
            }),
            code: "ServiceAccessNotEnabledException",
            message: "The request was rejected because trusted access is not enabled for IAM \
                      in Organizations.",
        },
        RefusedCase {
            case: "a member account of an organization with no delegated administrator",
            organization: Some(
                OrganizationFixture::serving(ORGANIZATION).managed_by(ANOTHER_ACCOUNT),
            ),
            code: "AccountNotManagementOrDelegatedAdministratorException",
            message: "The request was rejected because the account making the request is not \
                      the management account or delegated administrator account for \
                      centralized root access.",
        },
        RefusedCase {
            case: "a member account of an organization another account administers",
            organization: Some(
                OrganizationFixture::serving(SHORTEST_ORGANIZATION)
                    .managed_by(ANOTHER_ACCOUNT)
                    .administered_by(&[A_THIRD_ACCOUNT])
                    .with_features(&[ROOT_CREDENTIALS_MANAGEMENT, ROOT_SESSIONS]),
            ),
            code: "AccountNotManagementOrDelegatedAdministratorException",
            message: "The request was rejected because the account making the request is not \
                      the management account or delegated administrator account for \
                      centralized root access.",
        },
    ]
}

/// RULE: each standing requirement of centralized root access the account does not meet is
/// refused with the modelled error AWS declares for it, at HTTP 400, carrying the reason
/// that error states and reporting no organization and no feature.
#[tokio::test]
async fn each_unmet_requirement_is_refused_with_its_own_modelled_error() {
    for RefusedCase {
        case,
        organization,
        code,
        message,
    } in refused_rows()
    {
        let (router, engine) = router_with_engine();
        if let Some(organization) = organization.as_ref() {
            record(&engine, organization);
        }

        let (status, body) = list(&router, None).await;

        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            code,
            "case {case:?}: expected the error code {code}, got {:?}: {body}",
            error_code(&body)
        );
        assert_eq!(
            element(&body, "Message"),
            message,
            "case {case:?}: expected the reason the error states, got {:?}",
            element(&body, "Message")
        );
        assert!(
            elements(&body, "member").is_empty(),
            "case {case:?}: a refused call must report no feature, got {:?}",
            elements(&body, "member")
        );
    }
}

/// RULE: the report is a READ — what it reports is what the store holds, and asking for it
/// leaves the organization exactly as it was, so a second call answers the same thing.
#[tokio::test]
async fn reporting_the_features_changes_nothing_a_later_caller_sees() {
    for ServedCase {
        case,
        organization,
        reported,
    } in served_rows()
    {
        let (router, engine) = router_with_engine();
        record(&engine, &organization);
        let before = held_organization(&engine);

        let (first_status, first) = list(&router, None).await;
        let (second_status, second) = list(&router, None).await;

        assert_eq!(
            (first_status, second_status),
            (200, 200),
            "case {case:?}: expected both reads to be served, got {first_status} then \
             {second_status}: {first} / {second}"
        );
        assert_eq!(
            elements(&second, "member"),
            reported,
            "case {case:?}: the second read must report the same features, got {:?}: {second}",
            elements(&second, "member")
        );
        assert_eq!(
            element(&second, "OrganizationId"),
            element(&first, "OrganizationId"),
            "case {case:?}: the second read must report the same organization, got {:?} after \
             {:?}",
            element(&second, "OrganizationId"),
            element(&first, "OrganizationId")
        );
        assert_eq!(
            held_organization(&engine),
            before,
            "case {case:?}: reading the features must leave the organization unchanged, got \
             {:?}",
            held_organization(&engine)
        );
    }
}

/// What a row of [`policy_rows`] expects of the call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    /// HTTP 200: the caller's policies allow the action, so the organization decides.
    Served,
    /// HTTP 403 `AccessDenied`: they do not, whatever the organization says.
    Refused,
}

/// One row of [`policy_rows`]: what the signing user's documents say about this action, and
/// whether the call reaches the organization at all.
struct PolicyCase {
    case: &'static str,
    policies: Vec<(String, String)>,
    expect: Expect,
}

/// The identity-policy rows.
fn policy_rows() -> Vec<PolicyCase> {
    vec![
        PolicyCase {
            case: "an Allow naming the action",
            policies: vec![("allow".to_string(), statement("Allow", ACTION, "*"))],
            expect: Expect::Served,
        },
        PolicyCase {
            case: "an Allow whose action element is a wildcard run",
            policies: vec![("allow-list".to_string(), statement("Allow", "iam:List*", "*"))],
            expect: Expect::Served,
        },
        PolicyCase {
            case: "no policy at all",
            policies: Vec::new(),
            expect: Expect::Refused,
        },
        PolicyCase {
            case: "an Allow and an explicit Deny",
            policies: vec![
                ("allow".to_string(), statement("Allow", ACTION, "*")),
                ("deny".to_string(), statement("Deny", ACTION, "*")),
            ],
            expect: Expect::Refused,
        },
    ]
}

/// RULE: IAM authorizes its own API, so the signing principal's identity-based policies
/// decide this call — and they decide it with the refusal AWS words for an authorization
/// decision rather than with any of the organization's own.
#[tokio::test]
async fn identity_policies_decide_the_call_before_the_organization_is_consulted() {
    for (row, policy_case) in policy_rows().into_iter().enumerate() {
        let PolicyCase {
            case,
            policies,
            expect,
        } = policy_case;
        let (router, engine) = router_with_engine();
        record(
            &engine,
            &OrganizationFixture::serving(ORGANIZATION).with_features(&[ROOT_SESSIONS]),
        );
        let user_name = format!("reader{row}");
        let access_key_id = principal(&router, case, &user_name, &policies).await;

        let (status, body) = list(&router, Some(&access_key_id)).await;

        match expect {
            Expect::Served => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the report to be served, got {status}: {body}"
                );
                assert_eq!(
                    elements(&body, "member"),
                    vec![ROOT_SESSIONS.to_string()],
                    "case {case:?}: expected the enabled feature to be reported, got {:?}: \
                     {body}",
                    elements(&body, "member")
                );
            }
            Expect::Refused => {
                assert_eq!(
                    status, 403,
                    "case {case:?}: expected HTTP 403, got {status}: {body}"
                );
                assert_eq!(
                    error_code(&body),
                    "AccessDenied",
                    "case {case:?}: expected the error code AccessDenied, got {:?}: {body}",
                    error_code(&body)
                );
                let reason = if policies.is_empty() {
                    format!("because no identity-based policy allows the {ACTION} action")
                } else {
                    "with an explicit deny in an identity-based policy".to_string()
                };
                assert_access_denied_message(
                    case,
                    &element(&body, "Message"),
                    &format!("arn:aws:iam::{}:user/{user_name}", account_id()),
                    ACTION,
                    "*",
                    &reason,
                );
            }
        }
    }
}

/// RULE: the authorization decision comes FIRST — a caller whose policies do not allow the
/// action is refused the same way whether or not the account is in an organization at all,
/// so the organization's own refusals never leak to a principal that may not ask.
#[tokio::test]
async fn an_unauthorized_caller_is_refused_whatever_the_organization_is() {
    let worlds: Vec<(&str, Option<OrganizationFixture>)> = vec![
        ("no organization at all", None),
        (
            "an organization that would serve the report",
            Some(OrganizationFixture::serving(ORGANIZATION).with_features(&[ROOT_SESSIONS])),
        ),
        (
            "an organization that would refuse the report",
            Some(OrganizationFixture::serving(ORGANIZATION).managed_by(ANOTHER_ACCOUNT)),
        ),
    ];

    for (case, organization) in worlds {
        let (router, engine) = router_with_engine();
        if let Some(organization) = organization.as_ref() {
            record(&engine, organization);
        }
        let access_key_id = principal(&router, case, "unauthorized", &[]).await;

        let (status, body) = list(&router, Some(&access_key_id)).await;

        assert_eq!(
            status, 403,
            "case {case:?}: expected HTTP 403 for a caller no policy allows, got {status}: \
             {body}"
        );
        assert_eq!(
            error_code(&body),
            "AccessDenied",
            "case {case:?}: expected the error code AccessDenied, got {:?}: {body}",
            error_code(&body)
        );
    }
}
