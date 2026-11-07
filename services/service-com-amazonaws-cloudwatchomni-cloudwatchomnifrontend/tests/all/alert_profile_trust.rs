// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the trust an alert needs over the access profile it evaluates
//! under, and for the refusal a missing trust grant earns.
//!
//! Every call goes through a real operation handler holding the SAME shared engine the
//! router hands every service, so a grant created by one handler is observable by the
//! next. Failures are the operations' own modelled errors, so each assertion checks the
//! wire facts that are requirements: the HTTP status, the error code and the message.
//!
//! The suite asserts the RULES rather than one recorded exchange:
//!
//! * an alert may assume an access profile only while a grant whose principal type is
//!   ALERT names the assume action and scopes THAT action group to THAT profile —
//!   asserted over the whole published `AccessGrantPrincipalType` catalog and over the
//!   actions and resource scopes a grant can carry;
//! * the refusal states the condition that failed and the step that clears it: HTTP 400,
//!   `ValidationException`, and a fixed sentence whose only variable part is the
//!   request's own `profileId`, interpolated unquoted — not the `AssumeStatus` DENIED
//!   sentence about the calling principal, so the two conditions stay distinguishable
//!   (a profile may report `assumeStatus` ALLOWED and still carry no alert trust);
//! * the decision is read from the store, so the SAME request that was refused succeeds
//!   once the trust grant exists, and the refused request persisted nothing;
//! * deleting the trust grant revokes what it granted, so the alert's profile stops
//!   backing an alert, and an access profile a grant still references cannot be deleted.
//!
//! Each assertion names its case and the actual value, so a failure identifies the row
//! of the table that broke.

use std::sync::Arc;

use smithy_server_runtime::{
    EmbeddedEmulator, MultiServiceRouter, Operation, OperationContext, OperationError, Service,
};

use service_com_amazonaws_cloudwatchomni_cloudwatchomnifrontend as omni;

use omni::ops::create_access_grant::CreateAccessGrant;
use omni::ops::create_access_profile::CreateAccessProfile;
use omni::ops::create_alert::{CreateAlert, CreateAlertError, CreateAlertErrorModeled};
use omni::ops::create_domain::CreateDomain;
use omni::ops::create_space::CreateSpace;
use omni::ops::delete_access_grant::DeleteAccessGrant;
use omni::ops::delete_access_profile::{
    DeleteAccessProfile, DeleteAccessProfileError, DeleteAccessProfileErrorModeled,
};
use omni::ops::get_access_profile::{GetAccessProfile, GetAccessProfileError};
use omni::ops::get_alert::GetAlert;
use omni::ops::types::{
    AccessGrantPrincipal, AlertCondition, AlertEvaluation, AlertRuleQuery, CreateAccessGrantInput,
    CreateAccessProfileInput, CreateAlertInput, CreateAlertOutput, CreateDomainInput,
    CreateSpaceInput, DeleteAccessGrantInput, DeleteAccessProfileInput, DeleteAccessProfileOutput,
    GetAccessProfileInput, GetAccessProfileOutput, GetAlertInput, ResourceScope, Rule,
    ScopedActions, TelemetryRule, ACCESS_GRANT_PRINCIPAL_TYPE_VALUES,
};
use omni::scaffolded::Scaffolded;
use omni::state::{
    ACCESS_GRANT_PERMISSION_CUSTOM, ACCESS_GRANT_PRINCIPAL_TYPE_ACCESS_PROFILE,
    ACCESS_GRANT_PRINCIPAL_TYPE_ALERT, ACCESS_PROFILE_RESOURCE_TYPE,
    ASSUME_ACCESS_PROFILE_ACTION, ASSUME_STATUS_ALLOWED, ASSUME_STATUS_DENIED,
};

/// The region the suite signs for.
const REGION: &str = "us-west-2";

/// The access key id the suite signs with.
const ACCESS_KEY_ID: &str = "AKIDEMULATOR";

/// A role ARN satisfying `Arn`'s `^arn:.+$`, which `CreateSpace` requires.
const DATA_ACCESS_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/OmniData";

/// `AccessGrantPermission`'s plain read level, from the model's own value list
/// (`smithy_query(query="shape",shape="AccessGrantPermission")`: SPACE_ADMIN, READ,
/// READ_WRITE_DELETE, CUSTOM). Unlike CUSTOM it carries no `scopedActions`, so a grant
/// at this level names no resource at all.
const READ_PERMISSION: &str = "READ";

/// The refusal AWS words for an alert that names an access profile it may not assume.
///
/// Fixed text, with the request's own `profileId` as its ONLY variable part,
/// interpolated unquoted — spelled out here rather than read from the handler, so the
/// test pins the wording instead of echoing it.
fn expected_refusal(profile_id: &str) -> String {
    format!(
        "The alert cannot assume access profile {profile_id}. Grant the profile a trust \
         grant with principalType=ALERT (customAction cloudwatch:AssumeAccessProfile) \
         before creating the alert."
    )
}

/// A SigV4 `Authorization` header whose credential scope names this service's signing
/// name (the router's routing key), the region the handlers record and [`ACCESS_KEY_ID`].
fn signed_request_parts() -> http::request::Parts {
    let (parts, ()) = http::Request::builder()
        .method("POST")
        .uri("/service/CloudWatchOmniFrontend/operation/Unused")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={ACCESS_KEY_ID}/20260101/{REGION}/cloudwatch/aws4_request, \
                 SignedHeaders=host, Signature=emulator"
            ),
        )
        .body(())
        .expect("a well-formed request")
        .into_parts();
    parts
}

/// An action name that is not the one assuming an access profile is. `ScopedActionName`
/// enumerates no catalog (`smithy_query(query="shape",shape="ScopedActions")`: `actions`
/// is a plain string list), so any other name stands for "a grant for other work".
const OTHER_ACTION: &str = "cloudwatch:GetTelemetry";

/// Which access profile a grant's resource scope names.
#[derive(Clone, Copy, Debug)]
enum Scope {
    /// No `scopedActions` at all, at the READ permission: the grant names no resource,
    /// so it is recorded against no access profile.
    Unscoped,
    /// A CUSTOM grant whose resource scope names the ARN of the profile the alert names.
    TheProfile,
    /// A CUSTOM grant scoped to the profile the alert names, but for [`OTHER_ACTION`]
    /// rather than for assuming it.
    TheProfileForAnotherAction,
    /// A CUSTOM grant whose resource scope names the ARN of ANOTHER profile of the same
    /// space.
    AnotherProfile,
    /// A CUSTOM grant that scopes the whole `AccessProfile` type:
    /// `ResourceScope.resourceArns` "when absent, defaults to \"*\"".
    EveryProfile,
}

/// A space holding two access profiles: the one an alert names below, and another, so a
/// grant can be scoped to a profile that is NOT the one under test.
#[derive(Clone, Debug)]
struct World {
    domain_id: String,
    space_id: String,
    profile_id: String,
    profile_arn: String,
    other_profile_arn: String,
}

/// One emulator: one shared engine, the service mounted on a router, and the request
/// metadata a handler reads.
struct Omni {
    engine: Arc<engine::InMemoryEngine>,
    service: omni::EmulatedService<engine::InMemoryEngine>,
    router: MultiServiceRouter,
    parts: http::request::Parts,
}

impl Omni {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        // The router's service and the operations constructed below all hold the SAME
        // engine, so both front doors read and write one store.
        router.register_service(omni::EmulatedService::new(Arc::clone(&engine)));
        Self {
            service: omni::EmulatedService::new(Arc::clone(&engine)),
            engine,
            router,
            parts: signed_request_parts(),
        }
    }

    fn ctx(&self) -> OperationContext<'_> {
        OperationContext::new(
            &self.parts,
            self.service.definition().identity(),
            EmbeddedEmulator::from_router(self.router.clone()),
        )
    }

    /// A domain, a space of it and two access profiles of that space.
    async fn world(&self, case: &str, name: &str) -> World {
        let domain_id = CreateDomain::new(Arc::clone(&self.engine))
            .handle(
                CreateDomainInput {
                    name: format!("omni-{name}"),
                    identity_providers: vec!["IAM".to_string()],
                    ..Default::default()
                },
                &self.ctx(),
            )
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: CreateDomain failed: {error:?}"))
            .domain
            .domain_id;
        let space_id = CreateSpace::new(Arc::clone(&self.engine))
            .handle(
                CreateSpaceInput {
                    name: name.to_string(),
                    domain_id: domain_id.clone(),
                    data_access_role_arn: DATA_ACCESS_ROLE_ARN.to_string(),
                    ..Default::default()
                },
                &self.ctx(),
            )
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: CreateSpace failed: {error:?}"))
            .space
            .space_id;
        let (profile_id, profile_arn) = self.access_profile(case, &space_id, "subject").await;
        let (_, other_profile_arn) = self.access_profile(case, &space_id, "bystander").await;
        World {
            domain_id,
            space_id,
            profile_id,
            profile_arn,
            other_profile_arn,
        }
    }

    /// An access profile of `space_id`, and its ARN. No grant names it yet.
    async fn access_profile(&self, case: &str, space_id: &str, name: &str) -> (String, String) {
        let profile = CreateAccessProfile::new(Arc::clone(&self.engine))
            .handle(
                CreateAccessProfileInput {
                    space_id: space_id.to_string(),
                    name: name.to_string(),
                    ..Default::default()
                },
                &self.ctx(),
            )
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: CreateAccessProfile failed: {error:?}"))
            .access_profile;
        (profile.profile_id, profile.arn)
    }

    /// An access grant of `world`'s space, held by `principal_type` and scoped as `scope`
    /// says. Answers the id the service minted for it.
    async fn grant(
        &self,
        case: &str,
        world: &World,
        name: &str,
        principal_type: &str,
        scope: Scope,
    ) -> String {
        let (permission, scoped_actions) = match scope {
            Scope::Unscoped => (READ_PERMISSION, None),
            Scope::TheProfile => (
                ACCESS_GRANT_PERMISSION_CUSTOM,
                Some(scoped_to(
                    ASSUME_ACCESS_PROFILE_ACTION,
                    Some(vec![world.profile_arn.clone()]),
                )),
            ),
            Scope::TheProfileForAnotherAction => (
                ACCESS_GRANT_PERMISSION_CUSTOM,
                Some(scoped_to(
                    OTHER_ACTION,
                    Some(vec![world.profile_arn.clone()]),
                )),
            ),
            Scope::AnotherProfile => (
                ACCESS_GRANT_PERMISSION_CUSTOM,
                Some(scoped_to(
                    ASSUME_ACCESS_PROFILE_ACTION,
                    Some(vec![world.other_profile_arn.clone()]),
                )),
            ),
            Scope::EveryProfile => (
                ACCESS_GRANT_PERMISSION_CUSTOM,
                Some(scoped_to(ASSUME_ACCESS_PROFILE_ACTION, None)),
            ),
        };
        // An ACCESS_PROFILE principal is identified by a profile id, so a grant of that
        // principal type names the profile under test — the grant an operator reaches for
        // when the profile is the thing being authorized.
        let principal_id = if principal_type == ACCESS_GRANT_PRINCIPAL_TYPE_ACCESS_PROFILE {
            world.profile_id.clone()
        } else {
            name.to_string()
        };
        CreateAccessGrant::new(Arc::clone(&self.engine))
            .handle(
                CreateAccessGrantInput {
                    domain_id: world.domain_id.clone(),
                    space_id: world.space_id.clone(),
                    name: name.to_string(),
                    permission: permission.to_string(),
                    principal: AccessGrantPrincipal {
                        principal_type: principal_type.to_string(),
                        principal_id: Some(principal_id),
                        principal_attributes: None,
                    },
                    scoped_actions,
                    ..Default::default()
                },
                &self.ctx(),
            )
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: CreateAccessGrant failed: {error:?}"))
            .access_grant
            .grant_id
    }

    /// Remove an access grant, through the handler that owns it.
    async fn delete_grant(&self, case: &str, grant_id: &str) {
        DeleteAccessGrant::new(Arc::clone(&self.engine))
            .handle(
                DeleteAccessGrantInput {
                    grant_id: grant_id.to_string(),
                },
                &self.ctx(),
            )
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: DeleteAccessGrant failed: {error:?}"));
    }

    /// Ask for an access profile to be removed, and answer what the handler answered.
    async fn delete_profile(
        &self,
        space_id: &str,
        profile_id: &str,
    ) -> Result<DeleteAccessProfileOutput, DeleteAccessProfileError> {
        DeleteAccessProfile::new(Arc::clone(&self.engine))
            .handle(
                DeleteAccessProfileInput {
                    space_id: space_id.to_string(),
                    profile_id: profile_id.to_string(),
                },
                &self.ctx(),
            )
            .await
    }

    /// Read an access profile back, and answer what the reader answered.
    async fn read_profile(
        &self,
        space_id: &str,
        profile_id: &str,
    ) -> Result<GetAccessProfileOutput, GetAccessProfileError> {
        GetAccessProfile::new(Arc::clone(&self.engine))
            .handle(
                GetAccessProfileInput {
                    space_id: space_id.to_string(),
                    profile_id: profile_id.to_string(),
                },
                &self.ctx(),
            )
            .await
    }

    /// The `assumeStatus` the reader reports for a profile — the CALLING principal's
    /// authorization, which is a different question from an alert's trust.
    async fn assume_status(&self, case: &str, space_id: &str, profile_id: &str) -> String {
        self.read_profile(space_id, profile_id)
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: GetAccessProfile failed: {error:?}"))
            .access_profile
            .assume_status
            .unwrap_or_else(|| panic!("case {case:?}: the profile reported no assumeStatus"))
    }

    async fn create_alert(
        &self,
        input: &CreateAlertInput,
    ) -> Result<CreateAlertOutput, CreateAlertError> {
        CreateAlert::new(Arc::clone(&self.engine))
            .handle(input.clone(), &self.ctx())
            .await
    }

    /// The `profileId` the stored alert reports, read back through `GetAlert`.
    async fn stored_profile_id(&self, case: &str, space_id: &str, alert_id: &str) -> Option<String> {
        GetAlert::new(Arc::clone(&self.engine))
            .handle(
                GetAlertInput {
                    space_id: space_id.to_string(),
                    alert_id: alert_id.to_string(),
                },
                &self.ctx(),
            )
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: GetAlert failed: {error:?}"))
            .alert
            .profile_id
    }
}

/// One group of scoped actions carrying `action`, limited to `resource_arns` — or, when
/// they are absent, to the whole `AccessProfile` resource type.
fn scoped_to(action: &str, resource_arns: Option<Vec<String>>) -> Vec<ScopedActions> {
    vec![ScopedActions {
        actions: vec![action.to_string()],
        resources: Some(vec![ResourceScope {
            resource_type: ACCESS_PROFILE_RESOURCE_TYPE.to_string(),
            resource_arns,
            tags: None,
            signal_types: None,
            row_scope_groups: None,
        }]),
        context_conditions: None,
    }]
}

/// The smallest rule `CreateAlert` admits: a query to run, one threshold tier with the
/// mode and comparator to compare it, and an evaluation cadence.
fn complete_rule() -> Rule {
    Rule::telemetryRule(TelemetryRule {
        query: Some(AlertRuleQuery {
            language: "SQL".to_string(),
            expression: "SELECT 1".to_string(),
        }),
        condition: Some(AlertCondition {
            threshold_mode: Some("COUNT_OF_RESULTS".to_string()),
            threshold_field: None,
            comparator: Some("GT".to_string()),
            warning_threshold: None,
            critical_threshold: Some(90.0),
        }),
        evaluation: Some(AlertEvaluation {
            interval_seconds: 300,
            pending_duration_seconds: None,
            recovery_duration_seconds: None,
        }),
        no_data: None,
    })
}

/// The smallest alert request the model admits, for the parents `space_id`/`profile_id`.
fn create_input(space_id: &str, profile_id: &str, name: &str) -> CreateAlertInput {
    CreateAlertInput {
        space_id: space_id.to_string(),
        profile_id: profile_id.to_string(),
        name: name.to_string(),
        description: None,
        rule: complete_rule(),
        notifications_enabled: None,
        tags: None,
        notification_rules: None,
        client_token: None,
    }
}

/// The message of the modelled `ValidationException` a refused `CreateAlert` carries,
/// after checking the error code and HTTP status the model declares for it.
fn validation_message(case: &str, result: Result<CreateAlertOutput, CreateAlertError>) -> String {
    let error = match result {
        Ok(output) => panic!(
            "case {case:?}: expected ValidationException (400), got Ok({:?})",
            output.alert.alert_id
        ),
        Err(error) => error,
    };
    assert_eq!(
        error.shape_name(),
        "ValidationException",
        "case {case:?}: expected error code ValidationException, got {error:?}"
    );
    assert_eq!(
        error.http_status(),
        400,
        "case {case:?}: expected HTTP 400, got {}",
        error.http_status()
    );
    match error {
        Scaffolded::Modeled(CreateAlertErrorModeled::ValidationException(refusal)) => {
            refusal.message
        }
        other => panic!("case {case:?}: expected the modelled ValidationException, got {other:?}"),
    }
}

/// Check a refused `CreateAlert`: the message is the fixed sentence with this request's
/// `profileId` interpolated unquoted, and nothing else.
fn assert_refusal(case: &str, message: &str, profile_id: &str, space_id: &str) {
    assert_eq!(
        message,
        expected_refusal(profile_id),
        "case {case:?}: unexpected message {message:?}"
    );
    assert!(
        message.contains(&format!("access profile {profile_id}.")),
        "case {case:?}: expected the profile id interpolated unquoted, got {message:?}"
    );
    assert!(
        !message.contains(space_id),
        "case {case:?}: expected no space id in the message, got {message:?}"
    );
    assert!(
        !message.contains("calling principal"),
        "case {case:?}: expected the alert's trust condition rather than the calling \
         principal's, got {message:?}"
    );
}

#[tokio::test]
async fn only_an_alert_principal_grant_on_the_profile_lets_an_alert_assume_it() {
    let omni = Omni::new();
    // The rows are the whole published `AccessGrantPrincipalType` catalog
    // (`smithy_query(query="shape",shape="AccessGrantPrincipalType")`), each holding a
    // grant scoped to the very profile the alert names: ALERT ("An alert.") is the only
    // principal type whose grant authorizes an alert to assume it, so every other row —
    // ACCESS_PROFILE ("An access profile.") included — must be refused.
    for (index, principal_type) in ACCESS_GRANT_PRINCIPAL_TYPE_VALUES.iter().enumerate() {
        let case = format!("a {principal_type}-principal grant scoped to the profile");
        let world = omni.world(&case, &format!("principals-{index}")).await;
        omni.grant(&case, &world, "trust", principal_type, Scope::TheProfile)
            .await;

        let result = omni
            .create_alert(&create_input(&world.space_id, &world.profile_id, "alert"))
            .await;
        if *principal_type == ACCESS_GRANT_PRINCIPAL_TYPE_ALERT {
            let created = result
                .unwrap_or_else(|error| panic!("case {case:?}: CreateAlert failed: {error:?}"))
                .alert;
            let alert_id = created
                .alert_id
                .unwrap_or_else(|| panic!("case {case:?}: the created alert carries no alertId"));
            assert_eq!(
                omni.stored_profile_id(&case, &world.space_id, &alert_id)
                    .await
                    .as_deref(),
                Some(world.profile_id.as_str()),
                "case {case:?}: expected the stored alert to evaluate under {:?}",
                world.profile_id
            );
        } else {
            let message = validation_message(&case, result);
            assert_refusal(&case, &message, &world.profile_id, &world.space_id);
        }
    }
}

#[tokio::test]
async fn an_alert_trust_grant_counts_only_where_it_is_scoped_to_that_profile() {
    let omni = Omni::new();
    // The rows vary what an ALERT-principal grant is scoped to and which action it names,
    // plus the row with no grant at all: a grant authorizes an alert over the profiles the
    // resource scopes of the SAME action group name, and `ResourceScope.resourceArns`
    // "when absent, defaults to \"*\"" — every profile of that type.
    for (index, (case, scope, allowed)) in [
        ("no grant at all", None, false),
        (
            "an ALERT-principal grant that names no resource",
            Some(Scope::Unscoped),
            false,
        ),
        (
            "an ALERT-principal grant scoped to the profile for another action",
            Some(Scope::TheProfileForAnotherAction),
            false,
        ),
        (
            "an ALERT-principal grant scoped to another profile of the space",
            Some(Scope::AnotherProfile),
            false,
        ),
        (
            "an ALERT-principal grant scoped to the profile by ARN",
            Some(Scope::TheProfile),
            true,
        ),
        (
            "an ALERT-principal grant scoping every profile of the type",
            Some(Scope::EveryProfile),
            true,
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let world = omni.world(case, &format!("scopes-{index}")).await;
        if let Some(scope) = scope {
            omni.grant(
                case,
                &world,
                "trust",
                ACCESS_GRANT_PRINCIPAL_TYPE_ALERT,
                scope,
            )
            .await;
        }

        let result = omni
            .create_alert(&create_input(&world.space_id, &world.profile_id, "alert"))
            .await;
        if allowed {
            let alert_id = result
                .unwrap_or_else(|error| panic!("case {case:?}: CreateAlert failed: {error:?}"))
                .alert
                .alert_id
                .unwrap_or_else(|| panic!("case {case:?}: the created alert carries no alertId"));
            assert_eq!(
                omni.stored_profile_id(case, &world.space_id, &alert_id)
                    .await
                    .as_deref(),
                Some(world.profile_id.as_str()),
                "case {case:?}: expected the stored alert to evaluate under {:?}",
                world.profile_id
            );
        } else {
            let message = validation_message(case, result);
            assert_refusal(case, &message, &world.profile_id, &world.space_id);
        }
    }
}

#[tokio::test]
async fn the_refusal_is_the_alerts_trust_condition_and_not_the_callers_assume_status() {
    let omni = Omni::new();
    // Two profiles that differ in the CALLER's authorization to assume them — the first
    // carries a grant of its own, so `assumeStatus` reports ALLOWED; the second carries
    // none, so it reports DENIED — and agree in carrying no ALERT trust. The alert
    // refusal is the same sentence for both, so the message reports the alert's missing
    // trust rather than the caller's authorization, and the two conditions stay apart.
    let mut rendered = Vec::new();
    for (index, (case, scope, assume_status)) in [
        (
            "a profile the caller may assume, with no alert trust",
            Some(Scope::TheProfile),
            ASSUME_STATUS_ALLOWED,
        ),
        ("a profile no grant names at all", None, ASSUME_STATUS_DENIED),
    ]
    .into_iter()
    .enumerate()
    {
        let world = omni.world(case, &format!("wording-{index}")).await;
        if let Some(scope) = scope {
            // ACCESS_PROFILE is the principal type the profile itself holds — the grant
            // an operator reaches for first, and not the one an alert assumes it through.
            omni.grant(case, &world, "trust", "ACCESS_PROFILE", scope)
                .await;
        }
        assert_eq!(
            omni.assume_status(case, &world.space_id, &world.profile_id)
                .await,
            assume_status,
            "case {case:?}: expected the profile to report assumeStatus {assume_status}"
        );

        let message = validation_message(
            case,
            omni.create_alert(&create_input(&world.space_id, &world.profile_id, "alert"))
                .await,
        );
        assert_refusal(case, &message, &world.profile_id, &world.space_id);
        rendered.push((case, message.replace(&world.profile_id, "<profileId>")));
    }

    // The template is fixed: with each request's own profile id taken out, the two
    // refusals are the same sentence, so nothing else in it varies with the request.
    let (first_case, first) = &rendered[0];
    for (case, message) in &rendered[1..] {
        assert_eq!(
            message, first,
            "case {case:?}: expected the same template as case {first_case:?}, got {message:?}"
        );
    }
}

#[tokio::test]
async fn a_refused_alert_persists_nothing_and_the_same_request_succeeds_once_trust_exists() {
    let omni = Omni::new();
    let case = "the same request before and after the trust grant";
    let world = omni.world(case, "after-trust").await;
    let input = CreateAlertInput {
        client_token: Some("one-token".to_string()),
        ..create_input(&world.space_id, &world.profile_id, "alert")
    };

    let message = validation_message(case, omni.create_alert(&input).await);
    assert_refusal(case, &message, &world.profile_id, &world.space_id);

    // The refusal is read from the store, not from the shape of the request: the SAME
    // request, idempotency token included, goes through once the trust grant exists —
    // which also shows the refused attempt persisted neither an alert nor its token.
    omni.grant(
        case,
        &world,
        "trust",
        ACCESS_GRANT_PRINCIPAL_TYPE_ALERT,
        Scope::TheProfile,
    )
    .await;
    let created = omni
        .create_alert(&input)
        .await
        .unwrap_or_else(|error| panic!("case {case:?}: CreateAlert failed: {error:?}"))
        .alert;
    let alert_id = created
        .alert_id
        .clone()
        .unwrap_or_else(|| panic!("case {case:?}: the created alert carries no alertId"));
    assert_eq!(
        omni.stored_profile_id(case, &world.space_id, &alert_id)
            .await
            .as_deref(),
        Some(world.profile_id.as_str()),
        "case {case:?}: expected the stored alert to evaluate under {:?}, got {:?}",
        world.profile_id,
        created.profile_id
    );
}

#[tokio::test]
async fn deleting_the_trust_grant_revokes_what_it_granted() {
    let omni = Omni::new();
    let case = "the trust grant is deleted";
    let world = omni.world(case, "revoke").await;
    let grant_id = omni
        .grant(
            case,
            &world,
            "trust",
            ACCESS_GRANT_PRINCIPAL_TYPE_ALERT,
            Scope::TheProfile,
        )
        .await;

    // While the grant exists the profile is assumable and backs an alert.
    assert_eq!(
        omni.assume_status(case, &world.space_id, &world.profile_id)
            .await,
        ASSUME_STATUS_ALLOWED,
        "case {case:?}: expected the granted profile to report assumeStatus \
         {ASSUME_STATUS_ALLOWED}"
    );
    let alert_id = omni
        .create_alert(&create_input(&world.space_id, &world.profile_id, "before"))
        .await
        .unwrap_or_else(|error| panic!("case {case:?}: CreateAlert failed: {error:?}"))
        .alert
        .alert_id
        .unwrap_or_else(|| panic!("case {case:?}: the created alert carries no alertId"));

    // `DeleteAccessGrant` revokes the access the grant granted, so the trust it recorded
    // against the profile goes with it.
    omni.delete_grant(case, &grant_id).await;
    assert_eq!(
        omni.assume_status(case, &world.space_id, &world.profile_id)
            .await,
        ASSUME_STATUS_DENIED,
        "case {case:?}: expected the profile to report assumeStatus {ASSUME_STATUS_DENIED} \
         once its only grant is gone"
    );
    let message = validation_message(
        case,
        omni.create_alert(&create_input(&world.space_id, &world.profile_id, "after"))
            .await,
    );
    assert_refusal(case, &message, &world.profile_id, &world.space_id);

    // @idempotent, and the model declares no `ResourceNotFoundException`: deleting the
    // same id again succeeds, and the alert created while the trust held is untouched.
    omni.delete_grant(case, &grant_id).await;
    assert_eq!(
        omni.stored_profile_id(case, &world.space_id, &alert_id)
            .await
            .as_deref(),
        Some(world.profile_id.as_str()),
        "case {case:?}: expected the alert created earlier to survive the revocation"
    );
}

#[tokio::test]
async fn an_access_profile_cannot_be_deleted_while_access_grants_reference_it() {
    let omni = Omni::new();
    // The rows are the two ways a grant references a profile: its resource scopes name
    // the profile, and the profile IS the principal the grant is held by.
    for (index, (case, principal_type, scope)) in [
        (
            "a grant whose resource scope names the profile",
            ACCESS_GRANT_PRINCIPAL_TYPE_ALERT,
            Scope::TheProfile,
        ),
        (
            "a grant held by the profile itself",
            ACCESS_GRANT_PRINCIPAL_TYPE_ACCESS_PROFILE,
            Scope::Unscoped,
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let world = omni.world(case, &format!("in-use-{index}")).await;
        let grant_id = omni.grant(case, &world, "trust", principal_type, scope).await;

        // While the grant references it the profile stays, and the refusal names the
        // grant to remove first.
        let refused = omni
            .delete_profile(&world.space_id, &world.profile_id)
            .await;
        let conflict = match refused {
            Ok(output) => panic!("case {case:?}: expected ConflictException (409), got {output:?}"),
            Err(error) => error,
        };
        assert_eq!(
            conflict.shape_name(),
            "ConflictException",
            "case {case:?}: expected error code ConflictException, got {conflict:?}"
        );
        assert_eq!(
            conflict.http_status(),
            409,
            "case {case:?}: expected HTTP 409, got {}",
            conflict.http_status()
        );
        match conflict {
            Scaffolded::Modeled(DeleteAccessProfileErrorModeled::ConflictException(refusal)) => {
                assert!(
                    refusal.message.contains(&grant_id)
                        && refusal.message.contains(&world.profile_id),
                    "case {case:?}: expected the refusal to name the profile and the grant \
                     {grant_id:?}, got {:?}",
                    refusal.message
                );
            }
            other => panic!("case {case:?}: expected the modelled ConflictException, got {other:?}"),
        }
        omni.read_profile(&world.space_id, &world.profile_id)
            .await
            .unwrap_or_else(|error| {
                panic!("case {case:?}: the refused delete must leave the profile: {error:?}")
            });

        // Once nothing references it the profile goes, and the reader stops resolving it.
        omni.delete_grant(case, &grant_id).await;
        omni.delete_profile(&world.space_id, &world.profile_id)
            .await
            .unwrap_or_else(|error| {
                panic!("case {case:?}: DeleteAccessProfile failed: {error:?}")
            });
        let gone = omni
            .read_profile(&world.space_id, &world.profile_id)
            .await
            .err()
            .unwrap_or_else(|| panic!("case {case:?}: the deleted profile still resolves"));
        assert_eq!(
            (gone.shape_name(), gone.http_status()),
            ("ResourceNotFoundException", 404),
            "case {case:?}: expected the deleted profile to answer 404, got {gone:?}"
        );

        // @idempotent, and the model declares no `ResourceNotFoundException`: deleting an
        // id this space does not hold succeeds without removing anything, so a profile
        // named under the wrong space is left where it is.
        omni.delete_profile(&world.space_id, &world.profile_id)
            .await
            .unwrap_or_else(|error| {
                panic!("case {case:?}: deleting an id already gone must succeed: {error:?}")
            });
        let elsewhere = omni.world(case, &format!("in-use-elsewhere-{index}")).await;
        omni.delete_profile(&world.space_id, &elsewhere.profile_id)
            .await
            .unwrap_or_else(|error| {
                panic!("case {case:?}: deleting under the wrong space must succeed: {error:?}")
            });
        omni.read_profile(&elsewhere.space_id, &elsewhere.profile_id)
            .await
            .unwrap_or_else(|error| {
                panic!(
                    "case {case:?}: a profile named under another space must be left alone: \
                     {error:?}"
                )
            });
    }
}
