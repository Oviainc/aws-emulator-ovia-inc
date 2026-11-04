// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Support App Slack channel configurations and the account
//! alias.
//!
//! Every assertion goes over the wire: a request is built, dispatched through the same
//! [`MultiServiceRouter`] the production emulator mounts, and only the HTTP status, the
//! `x-amzn-errortype` header and the JSON body are inspected. Nothing reaches into the
//! service's state struct, so a response that is not computed from persisted state
//! cannot pass.
//!
//! The tests are tables: each row is one input along the axis the rule under test
//! generalises over (team/channel id form and length, channel name, role ARN, which
//! notification flags are sent, severity, alias charset and length, how many
//! configurations are stored). The `T012ABCDEFG` / `C01234A5BCD` case from the
//! bootstrap issue is only one row.
//!
//! A channel configuration lives inside a Slack workspace registered to the account,
//! so a create that is meant to succeed goes through
//! [`Emulator::create_in_registered_workspace`], which registers the request's
//! workspace first; the precondition itself is covered in
//! [`crate::resource_preconditions`].

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use service_com_amazonaws_supportapp_supportapp::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request is signed for; this API's resources are not regional, so
/// no assertion depends on it.
const REGION: &str = "us-east-1";

/// The `X-Amz-Target` prefix the Organizations emulator mounted beside this service
/// routes on (that crate's `OPERATIONS.md`, "Wire protocol"). Registering a Slack
/// workspace is an Organizations-scoped call, so the harness drives that service's own
/// `CreateOrganization` to arrange the precondition.
const ORGANIZATIONS_TARGET_PREFIX: &str = "AWSOrganizationsV20161128";

/// The account's documented maximum number of Slack channel configurations
/// (`CreateSlackChannelConfiguration` doc: "You can add up to 20 Slack channels for
/// your account"). It bounds the whole listing, which is why the listing is reported
/// in one response.
const MAX_SLACK_CHANNEL_CONFIGURATIONS: usize = 20;

/// `accountAlias` is `len 1..30, pattern ^[\w\- ]+$`
/// (`smithy_query(query="shape", shape="awsAccountAlias")`).
const ACCOUNT_ALIAS_MIN_LEN: usize = 1;
const ACCOUNT_ALIAS_MAX_LEN: usize = 30;

/// The values `NotificationSeverityLevel` accepts
/// (`smithy_query(query="shape", shape="NotificationSeverityLevel")`).
const SEVERITY_LEVELS: [&str; 3] = ["none", "all", "high"];

/// One wire answer, reduced to what a client branches on.
///
/// `pub(crate)` so the sibling suites in this test target (see `tests/all.rs`) drive
/// the same wire harness instead of each building its own.
#[derive(Debug)]
pub(crate) struct Reply {
    pub(crate) status: u16,
    pub(crate) error_type: String,
    pub(crate) body: Value,
}

impl Reply {
    fn field(&self, name: &str) -> Option<&Value> {
        self.body.get(name).filter(|value| !value.is_null())
    }

    pub(crate) fn string(&self, name: &str) -> Option<String> {
        self.field(name)
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    pub(crate) fn configurations(&self) -> Vec<Value> {
        self.field("slackChannelConfigurations")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }
}

/// An emulator with this service and the Organizations control plane mounted on one
/// fresh engine, driven in-process through the production router.
///
/// Organizations is mounted because registering a Slack workspace is an
/// Organizations-scoped call: the account has to be in an organization first, and
/// [`Emulator::create_organization`] puts it in one through that service's OWN
/// `CreateOrganization` over the wire, so no test reaches into a state struct to
/// arrange the precondition.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
    /// The one engine both mounted services share, kept so a suite can arrange a state
    /// no mounted operation can reach (see [`Emulator::hand_organization_to`]).
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signatures are not what these tests are about; the router still routes by the
        // SigV4 credential scope below.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine.clone()));
        router.register_service(
            service_com_amazonaws_organizations_awsorganizationsv20161128::EmulatedService::new(
                engine.clone(),
            ),
        );
        // Mounting the Organizations control plane establishes the organization the
        // emulated account belongs to, so that store is cleared again right after: what
        // these cases arrange — and refuse — is the membership itself, which
        // [`Emulator::create_organization`] then establishes over the wire.
        *engine
            .state::<service_com_amazonaws_organizations_awsorganizationsv20161128::state::ComAmazonawsOrganizationsAwsorganizationsv20161128State>()
            .lock() = Default::default();
        Self { router, engine }
    }

    /// The engine the mounted services share.
    ///
    /// `pub(crate)` so a sibling suite can call an operation of this service DIRECTLY,
    /// with a typed input, against the very store this harness's wire calls arranged —
    /// which is how [`crate::handler_validation_refusal_shape`] reads the error VALUE a
    /// handler returns for a request the wire can only show the status and body of.
    pub(crate) fn engine(&self) -> Arc<engine::InMemoryEngine> {
        Arc::clone(&self.engine)
    }

    /// Hand the organization this account is in over to ANOTHER management account, so
    /// this account is a MEMBER of its organization rather than the account that manages
    /// it — the other role an Organizations-scoped call can read for the caller.
    ///
    /// Organizations only reaches that state by invitation
    /// (`InviteAccountToOrganization` then `AcceptHandshake`), and both of those
    /// operations are still stubs in the Organizations emulator mounted here, so there is
    /// no wire path to it: the organization's own stored record is edited through that
    /// service's state module, which is the very state its handlers answer from — the
    /// management account the arrangement installs is what `DescribeOrganization` reports
    /// afterwards, so a test confirms the arrangement over the wire rather than assuming
    /// it. The calling account stays a member account of the organization, as
    /// `CreateOrganization` recorded it.
    pub(crate) fn hand_organization_to(&self, master_account_id: &str) {
        let state = service_com_amazonaws_organizations_awsorganizationsv20161128::state::service_state(
            &*self.engine,
        )
        .expect("the Organizations store must be reachable on the shared engine");
        let mut store = state.lock();
        let organization = store
            .organization
            .as_mut()
            .expect("the account must already be in an organization");
        let previous = std::mem::replace(
            &mut organization.master_account_id,
            master_account_id.to_string(),
        );
        // The management account's other stored fields are derived from its id, so they
        // follow it rather than being invented here.
        organization.master_account_arn = organization.master_account_arn.replace(&previous, master_account_id);
        organization.master_account_email = organization
            .master_account_email
            .replace(&previous, master_account_id);
        // The new management account is a member of the organization too, so it has a
        // record of its own, derived from the one the account it replaces carries.
        let mut management = store
            .accounts
            .get(&previous)
            .expect("the organization must hold its management account")
            .clone();
        management.arn = management.arn.replace(&previous, master_account_id);
        management.email = management.email.replace(&previous, master_account_id);
        management.id = master_account_id.to_string();
        store
            .accounts
            .insert(master_account_id.to_string(), management);
    }

    pub(crate) async fn call(&self, path: &str, body: Value) -> Reply {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/supportapp/aws4_request, \
             SignedHeaders=host, Signature=deadbeef"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("/control/{path}"))
            .header("host", format!("supportapp.{REGION}.amazonaws.com"))
            .header("authorization", authorization)
            .header("content-type", "application/json")
            .body(Bytes::from(
                serde_json::to_vec(&body).expect("serialize body"),
            ))
            .expect("build request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_type,
            body,
        }
    }

    pub(crate) async fn create(&self, body: Value) -> Reply {
        self.call("create-slack-channel-configuration", body).await
    }

    /// Call one operation of the Organizations control plane mounted beside this
    /// service, over THAT service's wire protocol (`awsJson1_1`, routed by
    /// `X-Amz-Target`).
    ///
    /// The answer is returned as-is so a caller can assert on it; `error_type` carries
    /// the awsJson `__type` of a failure, which is where that protocol puts the error
    /// code.
    pub(crate) async fn organizations(&self, operation: &str, body: Value) -> Reply {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/organizations/aws4_request, \
             SignedHeaders=host;x-amz-target, Signature=deadbeef"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("organizations.{REGION}.amazonaws.com"))
            .header("authorization", authorization)
            .header("content-type", "application/x-amz-json-1.1")
            .header(
                "x-amz-target",
                format!("{ORGANIZATIONS_TARGET_PREFIX}.{operation}"),
            )
            .body(Bytes::from(
                serde_json::to_vec(&body).expect("serialize body"),
            ))
            .expect("build request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let header_error = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        let error_type = if header_error.is_empty() {
            body.get("__type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        } else {
            header_error
        };
        Reply {
            status,
            error_type,
            body,
        }
    }

    /// Put this account in an organization through the Organizations emulator's own
    /// `CreateOrganization`, with `feature_set` when the caller names one.
    pub(crate) async fn create_organization(&self, feature_set: Option<&str>) -> Reply {
        let body = match feature_set {
            Some(value) => json!({ "FeatureSet": value }),
            None => json!({}),
        };
        self.organizations("CreateOrganization", body).await
    }

    /// Make sure this account is in an organization, which is what
    /// `RegisterSlackWorkspaceForOrganization` requires of the caller.
    ///
    /// Idempotent: an account that is already in one answers
    /// `AlreadyInOrganizationException`, which leaves the same end state, so a table may
    /// call this once per row.
    pub(crate) async fn ensure_organization(&self) {
        let reply = self.create_organization(None).await;
        assert!(
            reply.status == 200 || reply.error_type.ends_with("AlreadyInOrganizationException"),
            "the account must be in an organization: CreateOrganization answered {} / {} / {}",
            reply.status,
            reply.error_type,
            reply.body
        );
    }

    /// Record the Amazon Web Services Support Center console's authorization of the Slack
    /// workspace `team_id`, which is what makes that workspace exist for this API.
    ///
    /// A Slack workspace is authorized only through the interactive flow in that console,
    /// which is not an API call, so there is no wire path to this state: the authorization
    /// set this service's OWN handlers answer from is written through its state module,
    /// exactly as [`Emulator::hand_organization_to`] writes the organization's own stored
    /// record. Whether the arrangement took is then read back over the wire, by the
    /// registration it lets through; without it a registration answers the modelled
    /// `ResourceNotFoundException`, which is what [`crate::console_authorized_workspace`]
    /// covers.
    pub(crate) fn authorize_workspace_in_console(&self, team_id: &str) {
        let state = service_com_amazonaws_supportapp_supportapp::state::service_state(&*self.engine)
            .expect("the Support App store must be reachable on the shared engine");
        state
            .lock()
            .console_authorized_slack_workspaces
            .insert(team_id.to_string());
    }

    /// Register `team_id` as a Slack workspace of this account — the precondition
    /// `CreateSlackChannelConfiguration` enforces.
    ///
    /// The workspace is authorized in the console first, because a registration acts on an
    /// already authorized workspace, and registering is itself an Organizations-scoped
    /// call, so the account is put in an organization too; the two preconditions are
    /// covered in [`crate::console_authorized_workspace`] and
    /// [`crate::organization_membership`].
    pub(crate) async fn register_workspace(&self, team_id: &str) -> Reply {
        self.authorize_workspace_in_console(team_id);
        self.ensure_organization().await;
        self.call(
            "register-slack-workspace-for-organization",
            json!({ "teamId": team_id }),
        )
        .await
    }

    /// Remove `team_id` from this account's Slack workspace registry.
    pub(crate) async fn delete_workspace(&self, team_id: &str) -> Reply {
        self.call(
            "delete-slack-workspace-configuration",
            json!({ "teamId": team_id }),
        )
        .await
    }

    /// A create whose workspace is registered first, so whatever the create answers is
    /// about the request and the stored configurations rather than about the missing
    /// workspace. Registering a workspace the account already holds changes nothing, so
    /// a table may call this once per row with the same `teamId`.
    pub(crate) async fn create_in_registered_workspace(&self, body: Value) -> Reply {
        if let Some(team_id) = body.get("teamId").and_then(Value::as_str) {
            let registered = self.register_workspace(team_id).await;
            assert_eq!(
                registered.status, 200,
                "case {team_id:?}: expected the workspace registration to answer HTTP 200, \
                 got {} / {}",
                registered.status, registered.body
            );
        }
        self.create(body).await
    }

    pub(crate) async fn update(&self, body: Value) -> Reply {
        self.call("update-slack-channel-configuration", body).await
    }

    pub(crate) async fn delete(&self, team_id: &str, channel_id: &str) -> Reply {
        self.call(
            "delete-slack-channel-configuration",
            json!({ "teamId": team_id, "channelId": channel_id }),
        )
        .await
    }

    pub(crate) async fn list(&self, next_token: Option<&str>) -> Reply {
        let body = match next_token {
            Some(token) => json!({ "nextToken": token }),
            None => json!({}),
        };
        self.call("list-slack-channel-configurations", body).await
    }

    pub(crate) async fn put_alias(&self, alias: &str) -> Reply {
        self.call("put-account-alias", json!({ "accountAlias": alias }))
            .await
    }

    pub(crate) async fn get_alias(&self) -> Reply {
        self.call("get-account-alias", json!({})).await
    }

    pub(crate) async fn delete_alias(&self) -> Reply {
        self.call("delete-account-alias", json!({})).await
    }

    /// The single stored configuration for `(team_id, channel_id)` as the listing
    /// reports it, or `None` when the listing does not hold it. One call is enough: the
    /// listing reports every stored configuration.
    pub(crate) async fn listed(&self, team_id: &str, channel_id: &str) -> Option<Value> {
        let reply = self.list(None).await;
        assert_eq!(
            reply.status, 200,
            "listing must answer HTTP 200, got {} / {}",
            reply.status, reply.body
        );
        reply.configurations().into_iter().find(|item| {
            item.get("teamId").and_then(Value::as_str) == Some(team_id)
                && item.get("channelId").and_then(Value::as_str) == Some(channel_id)
        })
    }
}

/// One row of the round-trip table: what a create is given, and therefore what every
/// later read must report.
#[derive(Debug, Clone)]
struct Case {
    /// What makes this row different from the others, for assertion messages.
    label: &'static str,
    team_id: String,
    channel_id: String,
    channel_name: Option<String>,
    notify_on_create_or_reopen_case: Option<bool>,
    notify_on_add_correspondence_to_case: Option<bool>,
    notify_on_resolve_case: Option<bool>,
    severity: &'static str,
    channel_role_arn: String,
}

impl Case {
    fn create_body(&self) -> Value {
        let mut body = json!({
            "teamId": self.team_id,
            "channelId": self.channel_id,
            "notifyOnCaseSeverity": self.severity,
            "channelRoleArn": self.channel_role_arn,
        });
        if let Some(channel_name) = &self.channel_name {
            body["channelName"] = json!(channel_name);
        }
        for (member, value) in [
            (
                "notifyOnCreateOrReopenCase",
                self.notify_on_create_or_reopen_case,
            ),
            (
                "notifyOnAddCorrespondenceToCase",
                self.notify_on_add_correspondence_to_case,
            ),
            ("notifyOnResolveCase", self.notify_on_resolve_case),
        ] {
            if let Some(flag) = value {
                body[member] = json!(flag);
            }
        }
        body
    }

    /// The record the service must hold after this create: the submitted values, with
    /// the documented default of `false` applied to each absent notification flag
    /// ("If you don't specify these parameters in your request, they default to
    /// false." — `CreateSlackChannelConfigurationRequest$notifyOnCaseSeverity`).
    fn expected_record(&self) -> Map<String, Value> {
        let mut expected = Map::new();
        expected.insert("teamId".to_string(), json!(self.team_id));
        expected.insert("channelId".to_string(), json!(self.channel_id));
        if let Some(channel_name) = &self.channel_name {
            expected.insert("channelName".to_string(), json!(channel_name));
        }
        expected.insert(
            "notifyOnCreateOrReopenCase".to_string(),
            json!(self.notify_on_create_or_reopen_case.unwrap_or(false)),
        );
        expected.insert(
            "notifyOnAddCorrespondenceToCase".to_string(),
            json!(self.notify_on_add_correspondence_to_case.unwrap_or(false)),
        );
        expected.insert(
            "notifyOnResolveCase".to_string(),
            json!(self.notify_on_resolve_case.unwrap_or(false)),
        );
        expected.insert("notifyOnCaseSeverity".to_string(), json!(self.severity));
        expected.insert("channelRoleArn".to_string(), json!(self.channel_role_arn));
        expected
    }
}

/// A role ARN of `role_name`, matching `channelRoleArn`'s
/// `pattern ^arn:aws:iam::[0-9]{12}:role/(.+)$`.
pub(crate) fn role_arn(role_name: &str) -> String {
    format!(
        "arn:aws:iam::{account}:role/{role_name}",
        account = engine::EMULATOR_ACCOUNT_ID
    )
}

/// The table every round-trip assertion runs over. Rows differ in team and channel id
/// form and length, whether a channel name is present, which notification flags are
/// sent (and so which take the documented `false` default), the severity level and the
/// role ARN. `teamId`/`channelId` are `len 1..256 pattern ^\S+$`, `channelName` is
/// `len 1..256 pattern ^.+$` and `channelRoleArn` is `len 31..2048`, so the rows
/// include both published id bounds.
fn round_trip_cases() -> Vec<Case> {
    vec![
        // The case the bootstrap issue recorded: one row of many.
        Case {
            label: "the issue's recorded case",
            team_id: "T012ABCDEFG".to_string(),
            channel_id: "C01234A5BCD".to_string(),
            channel_name: Some("aws-support".to_string()),
            notify_on_create_or_reopen_case: Some(true),
            notify_on_add_correspondence_to_case: None,
            notify_on_resolve_case: None,
            severity: "high",
            channel_role_arn: role_arn("SupportAppRole"),
        },
        Case {
            label: "shortest legal ids, no channel name, every flag absent",
            team_id: "T".to_string(),
            channel_id: "C".to_string(),
            channel_name: None,
            notify_on_create_or_reopen_case: None,
            notify_on_add_correspondence_to_case: None,
            notify_on_resolve_case: None,
            severity: "none",
            channel_role_arn: role_arn("r"),
        },
        Case {
            label: "every flag sent true, severity all",
            team_id: "T-mixed.Case_9".to_string(),
            channel_id: "C-mixed.Case_9".to_string(),
            channel_name: Some("a channel name with spaces".to_string()),
            notify_on_create_or_reopen_case: Some(true),
            notify_on_add_correspondence_to_case: Some(true),
            notify_on_resolve_case: Some(true),
            severity: "all",
            channel_role_arn: role_arn("path/with/slashes/Role"),
        },
        Case {
            label: "longest legal ids and channel name, flags sent false",
            team_id: "T".repeat(256),
            channel_id: "C".repeat(256),
            channel_name: Some("n".repeat(256)),
            notify_on_create_or_reopen_case: Some(false),
            notify_on_add_correspondence_to_case: Some(false),
            notify_on_resolve_case: Some(false),
            severity: "none",
            channel_role_arn: role_arn(&"R".repeat(512)),
        },
    ]
}

/// Assert `reported` carries exactly the members `expected` describes.
fn assert_reported(label: &str, reported: &Value, expected: &Map<String, Value>) {
    for (member, value) in expected {
        let actual = reported.get(member.as_str()).filter(|v| !v.is_null());
        assert_eq!(
            actual,
            Some(value),
            "case {label:?}: expected {member} {value}, got {actual:?} in {reported}"
        );
    }
    // An absent channel name is reported as absent, not as an empty string: the model
    // constrains `channelName` to `len 1..256`.
    if !expected.contains_key("channelName") {
        assert!(
            reported.get("channelName").is_none_or(Value::is_null),
            "case {label:?}: expected no channelName, got {reported}"
        );
    }
}

#[tokio::test]
async fn a_created_configuration_is_reported_by_list_exactly_as_stored() {
    let emulator = Emulator::new();

    for case in round_trip_cases() {
        let label = case.label;
        let created = emulator
            .create_in_registered_workspace(case.create_body())
            .await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: expected create to answer HTTP 200, got {} / {}",
            created.status, created.body
        );

        let listed = emulator.listed(&case.team_id, &case.channel_id).await;
        let listed = listed.unwrap_or_else(|| {
            panic!("case {label:?}: expected the listing to report the created configuration")
        });
        assert_reported(label, &listed, &case.expected_record());

        // Listing twice reports the same thing: a read changes nothing.
        let again = emulator.listed(&case.team_id, &case.channel_id).await;
        assert_eq!(
            again.as_ref(),
            Some(&listed),
            "case {label:?}: a second listing reported {again:?} after the first reported {listed}"
        );
    }

    // Every row is in the listing at once, so no create overwrote another.
    let reported = emulator.list(None).await.configurations().len();
    assert_eq!(
        reported,
        round_trip_cases().len(),
        "expected every created configuration to be listed, got {reported}"
    );
}

#[tokio::test]
async fn configuring_an_already_configured_channel_is_a_conflict() {
    for case in round_trip_cases() {
        let label = case.label;
        let emulator = Emulator::new();
        let first = emulator
            .create_in_registered_workspace(case.create_body())
            .await;
        assert_eq!(
            first.status, 200,
            "case {label:?}: expected the first create to succeed, got {} / {}",
            first.status, first.body
        );

        // Same `(teamId, channelId)` pair, different everything else: the pair is the
        // configuration's identity.
        let mut body = case.create_body();
        body["channelName"] = json!("a-different-name");
        body["notifyOnCaseSeverity"] = json!("all");
        body["notifyOnCreateOrReopenCase"] = json!(true);
        body["channelRoleArn"] = json!(role_arn("OtherRole"));
        let second = emulator.create(body).await;
        assert_eq!(
            second.status, 409,
            "case {label:?}: expected HTTP 409 for an already configured channel, got {} / {}",
            second.status, second.body
        );
        assert_eq!(
            second.error_type, "ConflictException",
            "case {label:?}: expected ConflictException, got {:?}",
            second.error_type
        );

        // And the refused create changed nothing.
        let listed = emulator
            .listed(&case.team_id, &case.channel_id)
            .await
            .unwrap_or_else(|| panic!("case {label:?}: expected the configuration to survive"));
        assert_reported(label, &listed, &case.expected_record());
    }
}

#[tokio::test]
async fn update_applies_only_the_members_the_request_carries() {
    // Rows differ in which members the patch names: one member, several, all of them,
    // and none at all. Whatever a patch omits must keep its stored value, which is what
    // both the update's own answer and a later listing have to show.
    let patches: Vec<(&str, Value)> = vec![
        // The issue's recorded case: severity alone, leaving name, ARN and flags.
        ("severity alone", json!({ "notifyOnCaseSeverity": "all" })),
        ("channel name alone", json!({ "channelName": "renamed-channel" })),
        (
            "role ARN alone",
            json!({ "channelRoleArn": role_arn("UpdatedRole") }),
        ),
        (
            "one flag alone",
            json!({ "notifyOnAddCorrespondenceToCase": true }),
        ),
        (
            "a flag turned back off",
            json!({ "notifyOnCreateOrReopenCase": false, "notifyOnCaseSeverity": "none" }),
        ),
        (
            "every member at once",
            json!({
                "channelName": "fully-updated",
                "notifyOnCreateOrReopenCase": false,
                "notifyOnAddCorrespondenceToCase": true,
                "notifyOnResolveCase": true,
                "notifyOnCaseSeverity": "all",
                "channelRoleArn": role_arn("FullyUpdatedRole"),
            }),
        ),
        ("no member at all", json!({})),
    ];

    for (label, patch) in patches {
        let emulator = Emulator::new();
        let stored = Case {
            label: "update fixture",
            team_id: "T012ABCDEFG".to_string(),
            channel_id: "C01234A5BCD".to_string(),
            channel_name: Some("aws-support".to_string()),
            notify_on_create_or_reopen_case: Some(true),
            notify_on_add_correspondence_to_case: None,
            notify_on_resolve_case: None,
            severity: "high",
            channel_role_arn: role_arn("SupportAppRole"),
        };
        let created = emulator
            .create_in_registered_workspace(stored.create_body())
            .await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: expected create to succeed, got {} / {}",
            created.status, created.body
        );

        // The merged record: the stored one, with every member the patch names
        // replaced.
        let mut expected = stored.expected_record();
        for (member, value) in patch.as_object().expect("the patch is an object") {
            expected.insert(member.clone(), value.clone());
        }

        let mut body = patch.clone();
        body["teamId"] = json!(stored.team_id);
        body["channelId"] = json!(stored.channel_id);
        let updated = emulator.update(body).await;
        assert_eq!(
            updated.status, 200,
            "case {label:?}: expected update to answer HTTP 200, got {} / {}",
            updated.status, updated.body
        );
        assert_reported(label, &updated.body, &expected);

        // The merge persisted: the listing reports what the update answered.
        let listed = emulator
            .listed(&stored.team_id, &stored.channel_id)
            .await
            .unwrap_or_else(|| panic!("case {label:?}: expected the configuration to be listed"));
        assert_reported(label, &listed, &expected);
    }
}

#[tokio::test]
async fn the_documented_severity_and_flag_combination_is_enforced() {
    // The rule documented on `notifyOnCaseSeverity`: `high` or `all` needs at least one
    // of the three flags true, and `none` needs every flag the request is worded over
    // to be false — on create, the whole configuration. Rows span the severity levels
    // and the flag combinations on each side of that rule.
    let cases: [(&str, &str, [Option<bool>; 3], u16); 10] = [
        ("high with one flag true", "high", [Some(true), None, None], 200),
        (
            "all with the last flag true",
            "all",
            [None, None, Some(true)],
            200,
        ),
        (
            "all with every flag true",
            "all",
            [Some(true), Some(true), Some(true)],
            200,
        ),
        ("high with no flag sent", "high", [None, None, None], 400),
        ("all with no flag sent", "all", [None, None, None], 400),
        (
            "high with every flag sent false",
            "high",
            [Some(false), Some(false), Some(false)],
            400,
        ),
        ("none with no flag sent", "none", [None, None, None], 200),
        (
            "none with every flag sent false",
            "none",
            [Some(false), Some(false), Some(false)],
            200,
        ),
        ("none with one flag true", "none", [None, Some(true), None], 400),
        (
            "none with every flag true",
            "none",
            [Some(true), Some(true), Some(true)],
            400,
        ),
    ];

    for (label, severity, flags, expected_status) in cases {
        let emulator = Emulator::new();
        let case = Case {
            label: "severity rule fixture",
            team_id: "T012ABCDEFG".to_string(),
            channel_id: "C01234A5BCD".to_string(),
            channel_name: Some("aws-support".to_string()),
            notify_on_create_or_reopen_case: flags[0],
            notify_on_add_correspondence_to_case: flags[1],
            notify_on_resolve_case: flags[2],
            severity,
            channel_role_arn: role_arn("SupportAppRole"),
        };
        let created = emulator
            .create_in_registered_workspace(case.create_body())
            .await;
        assert_eq!(
            created.status, expected_status,
            "case {label:?}: expected HTTP {expected_status}, got {} / {}",
            created.status, created.body
        );
        if expected_status == 400 {
            assert_eq!(
                created.error_type, "ValidationException",
                "case {label:?}: expected ValidationException, got {:?}",
                created.error_type
            );
            // A refused create stores nothing.
            assert!(
                emulator
                    .listed(&case.team_id, &case.channel_id)
                    .await
                    .is_none(),
                "case {label:?}: a refused create must store nothing"
            );
        }
    }

    // The same rule on update, each clause over the scope the UPDATE request words it
    // with: `all`/`high` over the record the update would leave behind, and `none`
    // over only the notification flags the request itself specifies. The fixture
    // notifies on create-or-reopen at severity `high`. The last two columns are the
    // severity and the create-or-reopen flag the configuration must HOLD afterwards —
    // for a refused row, the stored ones, since a refused update changes nothing. The
    // scope of the `none` clause is covered exhaustively in
    // [`crate::update_severity_rule_scope`].
    let fixture = json!({
        "teamId": "T012ABCDEFG",
        "channelId": "C01234A5BCD",
        "notifyOnCaseSeverity": "high",
        "notifyOnCreateOrReopenCase": true,
        "channelRoleArn": role_arn("SupportAppRole"),
    });
    for (label, patch, expected_status, expected_severity, expected_flag) in [
        (
            "severity to none with no notification flag in the request",
            json!({ "notifyOnCaseSeverity": "none" }),
            200,
            "none",
            true,
        ),
        (
            "severity to none with the stored flag turned off",
            json!({ "notifyOnCaseSeverity": "none", "notifyOnCreateOrReopenCase": false }),
            200,
            "none",
            false,
        ),
        (
            "severity to none with a flag the request itself sets true",
            json!({ "notifyOnCaseSeverity": "none", "notifyOnResolveCase": true }),
            400,
            "high",
            true,
        ),
        (
            "severity to none with the stored flag restated true",
            json!({ "notifyOnCaseSeverity": "none", "notifyOnCreateOrReopenCase": true }),
            400,
            "high",
            true,
        ),
        (
            "the only true flag turned off while severity stays high",
            json!({ "notifyOnCreateOrReopenCase": false }),
            400,
            "high",
            true,
        ),
    ] {
        let emulator = Emulator::new();
        let created = emulator
            .create_in_registered_workspace(fixture.clone())
            .await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: expected create to succeed, got {} / {}",
            created.status, created.body
        );
        let mut body = patch;
        body["teamId"] = json!("T012ABCDEFG");
        body["channelId"] = json!("C01234A5BCD");
        let updated = emulator.update(body).await;
        assert_eq!(
            updated.status, expected_status,
            "case {label:?}: expected HTTP {expected_status}, got {} / {}",
            updated.status, updated.body
        );
        if expected_status == 400 {
            assert_eq!(
                updated.error_type, "ValidationException",
                "case {label:?}: expected ValidationException, got {:?}",
                updated.error_type
            );
        }
        let listed = emulator
            .listed("T012ABCDEFG", "C01234A5BCD")
            .await
            .unwrap_or_else(|| panic!("case {label:?}: expected the configuration to survive"));
        assert_eq!(
            listed.get("notifyOnCaseSeverity").and_then(Value::as_str),
            Some(expected_severity),
            "case {label:?}: expected severity {expected_severity:?} to be stored, got {listed}"
        );
        assert_eq!(
            listed
                .get("notifyOnCreateOrReopenCase")
                .and_then(Value::as_bool),
            Some(expected_flag),
            "case {label:?}: expected NotifyOnCreateOrReopenCase {expected_flag} to be stored, \
             got {listed}"
        );
    }
}

#[tokio::test]
async fn an_unconfigured_channel_cannot_be_updated_or_deleted() {
    let emulator = Emulator::new();
    // A stored neighbour, so the 404 is about the named pair and not an empty store.
    let present = Case {
        label: "neighbour",
        team_id: "T012ABCDEFG".to_string(),
        channel_id: "C01234A5BCD".to_string(),
        channel_name: None,
        notify_on_create_or_reopen_case: None,
        notify_on_add_correspondence_to_case: None,
        notify_on_resolve_case: None,
        severity: "none",
        channel_role_arn: role_arn("SupportAppRole"),
    };
    assert_eq!(
        emulator
            .create_in_registered_workspace(present.create_body())
            .await
            .status,
        200,
        "the neighbouring configuration must be created"
    );

    // Rows: another registered team with a known channel, the stored team with an
    // unknown channel, the shortest legal ids, and a channel id one character longer
    // than the stored one. Each row's workspace is registered below, because a channel
    // configuration can only be missing from a workspace the account HAS: an
    // unregistered `teamId` is the documented conflict instead, covered in
    // [`crate::channel_configuration_workspace_precondition`].
    let absent: [(&str, &str); 4] = [
        ("T99999ZZZZZ", "C01234A5BCD"),
        ("T012ABCDEFG", "C99999ZZZZZ"),
        ("T", "C"),
        ("T012ABCDEFG", "C01234A5BCDx"),
    ];
    for (team_id, _) in absent {
        assert_eq!(
            emulator.register_workspace(team_id).await.status,
            200,
            "case {team_id:?}: the workspace the row looks a channel up in must be registered"
        );
    }
    for (team_id, channel_id) in absent {
        for (operation, reply) in [
            (
                "UpdateSlackChannelConfiguration",
                emulator
                    .update(json!({
                        "teamId": team_id,
                        "channelId": channel_id,
                        "notifyOnCaseSeverity": "none",
                    }))
                    .await,
            ),
            (
                "DeleteSlackChannelConfiguration",
                emulator.delete(team_id, channel_id).await,
            ),
        ] {
            let label = format!("{operation} on ({team_id}, {channel_id})");
            assert_eq!(
                reply.status, 404,
                "case {label:?}: expected HTTP 404, got {} / {}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error_type, "ResourceNotFoundException",
                "case {label:?}: expected ResourceNotFoundException, got {:?}",
                reply.error_type
            );
        }
    }

    // None of those refusals touched the stored neighbour.
    let listed = emulator
        .listed(&present.team_id, &present.channel_id)
        .await
        .expect("the neighbouring configuration must survive");
    assert_reported("neighbour", &listed, &present.expected_record());
}

#[tokio::test]
async fn delete_removes_only_the_named_configuration() {
    let emulator = Emulator::new();
    let cases = round_trip_cases();
    for case in &cases {
        assert_eq!(
            emulator
                .create_in_registered_workspace(case.create_body())
                .await
                .status,
            200,
            "case {:?}: expected create to succeed",
            case.label
        );
    }

    let mut remaining: Vec<&Case> = cases.iter().collect();
    for case in &cases {
        let label = case.label;
        let deleted = emulator.delete(&case.team_id, &case.channel_id).await;
        assert_eq!(
            deleted.status, 200,
            "case {label:?}: expected delete to answer HTTP 200, got {} / {}",
            deleted.status, deleted.body
        );

        // Gone from the listing, and a second delete is a 404.
        assert!(
            emulator
                .listed(&case.team_id, &case.channel_id)
                .await
                .is_none(),
            "case {label:?}: expected the configuration to be gone from the listing"
        );
        let again = emulator.delete(&case.team_id, &case.channel_id).await;
        assert_eq!(
            again.status, 404,
            "case {label:?}: expected a second delete to answer HTTP 404, got {} / {}",
            again.status, again.body
        );
        // And updating it now fails the same way.
        let updated = emulator
            .update(json!({
                "teamId": case.team_id,
                "channelId": case.channel_id,
                "notifyOnCaseSeverity": "none",
            }))
            .await;
        assert_eq!(
            updated.status, 404,
            "case {label:?}: expected update after delete to answer HTTP 404, got {} / {}",
            updated.status, updated.body
        );

        // Every other configuration is untouched.
        remaining.retain(|other| other.label != label);
        for other in &remaining {
            let listed = emulator
                .listed(&other.team_id, &other.channel_id)
                .await
                .unwrap_or_else(|| {
                    panic!(
                        "case {label:?}: deleting it must leave {:?} listed",
                        other.label
                    )
                });
            assert_reported(other.label, &listed, &other.expected_record());
        }
    }

    let left = emulator.list(None).await;
    assert_eq!(
        left.configurations(),
        Vec::<Value>::new(),
        "expected an empty listing once every configuration was deleted, got {}",
        left.body
    );
    assert!(
        left.string("nextToken").is_none(),
        "expected no nextToken on an empty listing, got {}",
        left.body
    );
}

/// The `(teamId, channelId)` pairs `ListSlackChannelConfigurations` reports, in the
/// order it reported them, asserting the listing came back WHOLE: one HTTP 200 with
/// every stored configuration and no `nextToken`.
async fn listed_pairs(emulator: &Emulator, label: &str) -> Vec<(String, String)> {
    let reply = emulator.list(None).await;
    assert_eq!(
        reply.status, 200,
        "case {label:?}: expected HTTP 200 for the listing, got {} / {}",
        reply.status, reply.body
    );
    assert!(
        reply.string("nextToken").is_none(),
        "case {label:?}: expected no nextToken on a listing the account quota bounds, got {}",
        reply.body
    );
    reply
        .configurations()
        .iter()
        .map(|item| {
            (
                item.get("teamId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                item.get("channelId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect()
}

#[tokio::test]
async fn the_listing_reports_every_stored_configuration_in_one_response() {
    // Rows differ in how many configurations are stored: none, one, a handful, the
    // twelve of the recorded case, one short of the documented account maximum, and
    // that maximum itself. Whatever the count, the account cannot hold more than the
    // quota, so the listing is never split.
    for stored in [
        0usize,
        1,
        3,
        12,
        MAX_SLACK_CHANNEL_CONFIGURATIONS - 1,
        MAX_SLACK_CHANNEL_CONFIGURATIONS,
    ] {
        let emulator = Emulator::new();
        let mut expected: Vec<(String, String)> = Vec::new();
        for index in 0..stored {
            let team_id = format!("T{:03}", index % 3);
            let channel_id = format!("C{index:03}");
            let severity = SEVERITY_LEVELS[index % SEVERITY_LEVELS.len()];
            let created = emulator
                .create_in_registered_workspace(json!({
                    "teamId": team_id,
                    "channelId": channel_id,
                    "notifyOnCaseSeverity": severity,
                    // `none` requires every notification flag to be false, `high` and
                    // `all` require at least one to be true.
                    "notifyOnResolveCase": severity != "none",
                    "channelRoleArn": role_arn("SupportAppRole"),
                }))
                .await;
            assert_eq!(
                created.status, 200,
                "case {stored} stored: expected create to succeed, got {} / {}",
                created.status, created.body
            );
            expected.push((team_id, channel_id));
        }
        // The listing is ordered by the `(teamId, channelId)` pair.
        expected.sort();

        let label = format!("{stored} stored");
        let reported = listed_pairs(&emulator, &label).await;
        assert_eq!(
            reported, expected,
            "case {label:?}: expected every stored pair once in key order, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn a_create_past_the_documented_channel_quota_is_refused() {
    let emulator = Emulator::new();
    // Fill the account to its documented maximum.
    for index in 0..MAX_SLACK_CHANNEL_CONFIGURATIONS {
        let created = emulator
            .create_in_registered_workspace(json!({
                "teamId": "T012ABCDEFG",
                "channelId": format!("C{index:03}"),
                "notifyOnCaseSeverity": "none",
                "channelRoleArn": role_arn("SupportAppRole"),
            }))
            .await;
        assert_eq!(
            created.status, 200,
            "case {index} of the quota: expected create to succeed, got {} / {}",
            created.status, created.body
        );
    }

    // Rows differ in the pair the over-quota create names: another channel in the same
    // workspace, a channel in a different workspace, and the shortest legal ids.
    let over_quota: [(&str, &str, &str); 3] = [
        ("another channel in the same workspace", "T012ABCDEFG", "C999"),
        ("a channel in another workspace", "T999ZZZZZZZ", "C999"),
        ("the shortest legal ids", "T", "C"),
    ];
    for (label, team_id, channel_id) in over_quota {
        let refused = emulator
            .create_in_registered_workspace(json!({
                "teamId": team_id,
                "channelId": channel_id,
                "notifyOnCaseSeverity": "none",
                "channelRoleArn": role_arn("SupportAppRole"),
            }))
            .await;
        assert_eq!(
            refused.status, 402,
            "case {label:?}: expected HTTP 402 past the quota, got {} / {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type, "ServiceQuotaExceededException",
            "case {label:?}: expected ServiceQuotaExceededException, got {:?}",
            refused.error_type
        );
        assert!(
            emulator.listed(team_id, channel_id).await.is_none(),
            "case {label:?}: a refused create must store nothing"
        );
    }

    // A full account still reports exactly its quota, and reconfiguring a channel it
    // already holds is the conflict, not the quota: that create adds nothing.
    let stored = listed_pairs(&emulator, "a full account").await;
    assert_eq!(
        stored.len(),
        MAX_SLACK_CHANNEL_CONFIGURATIONS,
        "expected the account to hold exactly its quota, got {}",
        stored.len()
    );
    let duplicate = emulator
        .create(json!({
            "teamId": "T012ABCDEFG",
            "channelId": "C000",
            "notifyOnCaseSeverity": "none",
            "channelRoleArn": role_arn("SupportAppRole"),
        }))
        .await;
    assert_eq!(
        duplicate.error_type, "ConflictException",
        "expected an already configured channel to stay a conflict at the quota, got {:?} ({})",
        duplicate.error_type, duplicate.status
    );

    // Deleting one frees a slot: the quota bounds what is stored, not what was ever
    // created.
    assert_eq!(
        emulator.delete("T012ABCDEFG", "C000").await.status,
        200,
        "expected the delete that frees a slot to succeed"
    );
    let after_delete = emulator
        .create_in_registered_workspace(json!({
            "teamId": "T999ZZZZZZZ",
            "channelId": "C999",
            "notifyOnCaseSeverity": "none",
            "channelRoleArn": role_arn("SupportAppRole"),
        }))
        .await;
    assert_eq!(
        after_delete.status, 200,
        "expected a create into the freed slot to succeed, got {} / {}",
        after_delete.status, after_delete.body
    );
}

#[tokio::test]
async fn a_next_token_no_listing_returned_is_refused() {
    let emulator = Emulator::new();
    for index in 0..3 {
        emulator
            .create_in_registered_workspace(json!({
                "teamId": "T012ABCDEFG",
                "channelId": format!("C{index:03}"),
                "notifyOnCaseSeverity": "none",
                "channelRoleArn": role_arn("SupportAppRole"),
            }))
            .await;
    }

    // Tokens differing in form: not hex at all, odd-length hex, hex that decodes to
    // bytes that are not text, and hex of a key. No listing of this service returns a
    // token, so none of them names a position.
    for token in ["not-a-cursor", "abc", "fffe", "5430313241424344454647"] {
        let reply = emulator.list(Some(token)).await;
        assert_eq!(
            reply.status, 400,
            "case {token:?}: expected HTTP 400 for a nextToken no listing returned, got {} / {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_type, "ValidationException",
            "case {token:?}: expected ValidationException, got {:?}",
            reply.error_type
        );
    }
}

#[tokio::test]
async fn the_account_alias_round_trips_through_put_and_get() {
    let emulator = Emulator::new();

    // No alias has been put: the operation models no not-found error, so the answer is
    // a 200 with `accountAlias` absent.
    let before = emulator.get_alias().await;
    assert_eq!(
        before.status, 200,
        "expected HTTP 200 before any alias was put, got {} / {}",
        before.status, before.body
    );
    assert!(
        before.string("accountAlias").is_none(),
        "expected no accountAlias before any was put, got {}",
        before.body
    );

    // Rows span `accountAlias`'s charset and both published length bounds: the word
    // characters the pattern allows, a hyphen, a space, digits, an underscore, the
    // shortest legal alias and the longest.
    let aliases = [
        "my-team",
        "Support Team 1",
        "under_score",
        "0123456789",
        &"a".repeat(ACCOUNT_ALIAS_MIN_LEN),
        &"z".repeat(ACCOUNT_ALIAS_MAX_LEN),
    ];
    for alias in aliases {
        let put = emulator.put_alias(alias).await;
        assert_eq!(
            put.status, 200,
            "case {alias:?}: expected put to answer HTTP 200, got {} / {}",
            put.status, put.body
        );

        // A put replaces the single stored alias, and the get reads it back.
        let got = emulator.get_alias().await;
        assert_eq!(
            got.status, 200,
            "case {alias:?}: expected get to answer HTTP 200, got {} / {}",
            got.status, got.body
        );
        assert_eq!(
            got.string("accountAlias").as_deref(),
            Some(alias),
            "case {alias:?}: expected the persisted alias, got {:?}",
            got.string("accountAlias")
        );

        // Reading twice reports the same thing: the get changes nothing.
        let again = emulator.get_alias().await;
        assert_eq!(
            again.string("accountAlias"),
            got.string("accountAlias"),
            "case {alias:?}: a second get reported {:?}",
            again.string("accountAlias")
        );
    }
}

#[tokio::test]
async fn an_alias_outside_the_modelled_constraints_is_refused() {
    let emulator = Emulator::new();
    assert_eq!(
        emulator.put_alias("keep-me").await.status,
        200,
        "the stored alias must be put"
    );

    // One past each published length bound, plus characters outside
    // `pattern ^[\w\- ]+$`.
    let rejected: Vec<String> = vec![
        String::new(),
        "z".repeat(ACCOUNT_ALIAS_MAX_LEN + 1),
        "no!".to_string(),
        "slash/alias".to_string(),
        "tab\talias".to_string(),
    ];
    for alias in &rejected {
        let reply = emulator.put_alias(alias).await;
        assert_eq!(
            reply.status, 400,
            "case {alias:?}: expected HTTP 400, got {} / {}",
            reply.status, reply.body
        );
        // A refused put leaves the stored alias alone.
        let got = emulator.get_alias().await;
        assert_eq!(
            got.string("accountAlias").as_deref(),
            Some("keep-me"),
            "case {alias:?}: expected the stored alias to survive, got {}",
            got.body
        );
    }
}

#[tokio::test]
async fn a_create_outside_the_modelled_constraints_is_refused() {
    let emulator = Emulator::new();
    let valid = json!({
        "teamId": "T012ABCDEFG",
        "channelId": "C01234A5BCD",
        "notifyOnCaseSeverity": "none",
        "channelRoleArn": role_arn("SupportAppRole"),
    });
    // The workspace the valid request names is registered, so every refusal below is
    // about the request's own values and not about the missing-workspace precondition.
    assert_eq!(
        emulator.register_workspace("T012ABCDEFG").await.status,
        200,
        "the workspace must be registered"
    );

    // Rows: each `@required` member absent, a value outside each published length
    // bound, a value outside `^\S+$`, a severity outside the modelled value set, and a
    // role ARN outside `^arn:aws:iam::[0-9]{12}:role/(.+)$`.
    let mut rejected: Vec<(&str, Value)> = Vec::new();
    for required in ["teamId", "channelId", "notifyOnCaseSeverity", "channelRoleArn"] {
        let mut body = valid.clone();
        body.as_object_mut().expect("object").remove(required);
        rejected.push((required, body));
    }
    for (label, member, value) in [
        ("empty teamId", "teamId", json!("")),
        ("teamId past its bound", "teamId", json!("T".repeat(257))),
        ("teamId with a space", "teamId", json!("T012 ABCDEFG")),
        ("empty channelId", "channelId", json!("")),
        (
            "channelId past its bound",
            "channelId",
            json!("C".repeat(257)),
        ),
        ("empty channelName", "channelName", json!("")),
        (
            "severity outside the value set",
            "notifyOnCaseSeverity",
            json!("critical"),
        ),
        (
            "a role ARN that is not an IAM role ARN",
            "channelRoleArn",
            json!("arn:aws:iam::123456789012:user/NotARole"),
        ),
        (
            "a role ARN below its length bound",
            "channelRoleArn",
            json!("arn:aws:iam::1:role/r"),
        ),
    ] {
        let mut body = valid.clone();
        body[member] = value;
        rejected.push((label, body));
    }

    for (label, body) in rejected {
        let reply = emulator.create(body).await;
        assert_eq!(
            reply.status, 400,
            "case {label:?}: expected HTTP 400, got {} / {}",
            reply.status, reply.body
        );
        // Nothing was stored for a refused create.
        let listed = emulator.list(None).await;
        assert_eq!(
            listed.configurations(),
            Vec::<Value>::new(),
            "case {label:?}: a refused create must store nothing, got {}",
            listed.body
        );
    }

    // The same request with every constraint satisfied is accepted, so the refusals
    // above are about the values and not about the route.
    assert_eq!(
        emulator.create(valid).await.status,
        200,
        "the valid create must be accepted"
    );
}
