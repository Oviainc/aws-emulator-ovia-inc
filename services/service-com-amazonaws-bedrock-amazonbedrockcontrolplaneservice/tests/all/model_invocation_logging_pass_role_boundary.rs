// SPDX-License-Identifier: CC0-1.0

//! The ACCOUNT's pass-role boundary over the delivery role a model-invocation-logging
//! destination names, driven over the real wire path (a restJson1 request through the
//! multi-service router into this service mounted beside IAM and STS on one engine), so
//! what is asserted is what a client sees.
//!
//! # The rule
//!
//! Handing Amazon Bedrock the CloudWatch Logs delivery role a configuration names is a
//! permission of the CALLING PRINCIPAL rather than a property of the role: the account's
//! IAM has to allow that principal the `iam:PassRole` of that very role, so the account
//! bounds which of its roles each of its principals may hand out and two accounts answer
//! the same well-formed, same-account, correctly-trusted destination differently. A pass
//! the boundary withholds — no readable statement allows `iam:PassRole` on the role, a
//! statement allows it only on another role, or an explicit `Deny` covers it — is refused
//! with the modelled `AccessDeniedException` (`@httpError(403)`) carrying the one sentence
//! that denial is stated with, and installs NOTHING, so the configuration in force is left
//! exactly as it was. A pass the boundary allows is still decided over the role itself —
//! it has to be a role of this account whose trust policy admits the Amazon Bedrock service
//! principal — and a configuration that passes no role at all has no such decision to take.
//!
//! A refusal has to be GROUNDED in a document that was read, so a caller this emulator
//! holds no policies for (the account's own published credential, which is the position AWS
//! account root credentials are in) installs the destination: that is the recorded success
//! path, and it is what [`the_account_credential_passes_a_role_no_policy_bounds`] and the
//! role-free rows pin.
//!
//! # What the table generalises over
//!
//! [`an_invocation_logging_destination_needs_the_callers_pass_role_for_its_role`] holds one
//! row per shape the decision spans: the DOCUMENT in force on the caller (none bearing the
//! IAM action at all, `iam:PassRole` on every resource, on exactly the role being passed, on
//! this account's roles by a wildcard, on another role only, allowed everywhere beside an
//! explicit deny of that role, and the whole `iam` namespace) and the ROLE being passed (one
//! of this account trusted to the service, one of this account carrying no trust policy at
//! all, and one of another account) — the two halves crossed, so a boundary that allows the
//! pass does not override what the role itself withholds.
//!
//! The rows are driven in sequence against ONE emulator, and every row reads the
//! configuration back through `GetModelInvocationLoggingConfiguration` with the account's
//! own credential, so each get also witnesses that a refused put replaced nothing.

use serde_json::{json, Value};

use crate::control_plane_authorization::given_role;
use crate::custom_model_authorization::{
    emulator, given_role_policy, given_session, ACCOUNT_KEY_ID, SESSION_KEY_ID,
};
use crate::support::{Reply, ACCOUNT};

/// The URI `PutModelInvocationLoggingConfiguration` and
/// `GetModelInvocationLoggingConfiguration` are both bound to
/// (`smithy_query(query="operation",operation="PutModelInvocationLoggingConfiguration,GetModelInvocationLoggingConfiguration")`).
const LOGGING_URI: &str = "/logging/modelinvocations";

/// The sentence Amazon Bedrock states a denied pass-role with, spelled here so this suite
/// pins the wording a client sees rather than reading it off the service.
const PASS_ROLE_REFUSAL: &str = "Cross-account pass role is not allowed.";

/// The delivery role of THIS account, trusted to Amazon Bedrock — the role the recorded
/// setup creates alongside the log group.
const DELIVERY_ROLE: &str = "BedrockInvocationLogging";

/// A role of this account the IAM store holds with no trust policy at all, so the service
/// cannot be given it however the boundary decides the pass.
const UNTRUSTED_ROLE: &str = "BedrockInvocationLoggingUntrusted";

/// A role of this account the caller is allowed to pass instead of the delivery one.
const OTHER_ROLE: &str = "BedrockSomethingElse";

/// A 12-digit account id that is NOT the calling account.
const OTHER_ACCOUNT: &str = "123456789012";

/// The ARN this account holds `name` under.
fn role_arn(name: &str) -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/{name}")
}

/// One statement of an identity-based policy.
fn statement(effect: &str, action: &str, resource: &str) -> Value {
    json!({ "Effect": effect, "Action": action, "Resource": resource })
}

/// The document in force on the caller: every action of THIS service — so the operation's
/// own authorization is never what a row turns on — plus whatever the row says about the
/// IAM action that decides the pass.
fn caller_document(pass_role: &[Value]) -> Value {
    let mut statements = vec![statement("Allow", "bedrock:*", "*")];
    statements.extend(pass_role.iter().cloned());
    json!({ "Version": "2012-10-17", "Statement": statements })
}

/// A CloudWatch Logs destination naming `role`, under a log group named for `stem` so each
/// row's configuration is distinguishable from the row before it.
fn cloud_watch_config(stem: &str, role: &str) -> Value {
    json!({
        "cloudWatchConfig": { "logGroupName": format!("aws/bedrock/{stem}"), "roleArn": role },
        "textDataDeliveryEnabled": true,
        "imageDataDeliveryEnabled": false,
        "embeddingDataDeliveryEnabled": false,
    })
}

/// One row: what the caller's policies say about `iam:PassRole`, the role the destination
/// names, and whether the account lets this caller make that pass.
struct Row {
    case: &'static str,
    pass_role: Vec<Value>,
    role_arn: String,
    installed: bool,
}

fn rows() -> Vec<Row> {
    let delivery = role_arn(DELIVERY_ROLE);
    vec![
        Row {
            // The shape of a least-privilege policy for this service alone: it allows the
            // operation and says nothing about handing out a role.
            case: "a caller allowed this service's actions and no IAM action",
            pass_role: Vec::new(),
            role_arn: delivery.clone(),
            installed: false,
        },
        Row {
            case: "a caller allowed iam:PassRole on every resource",
            pass_role: vec![statement("Allow", "iam:PassRole", "*")],
            role_arn: delivery.clone(),
            installed: true,
        },
        Row {
            case: "a caller allowed iam:PassRole on exactly the role it passes",
            pass_role: vec![statement("Allow", "iam:PassRole", &delivery)],
            role_arn: delivery.clone(),
            installed: true,
        },
        Row {
            case: "a caller allowed iam:PassRole on this account's roles by a wildcard",
            pass_role: vec![statement(
                "Allow",
                "iam:PassRole",
                &format!("arn:aws:iam::{ACCOUNT}:role/*"),
            )],
            role_arn: delivery.clone(),
            installed: true,
        },
        Row {
            case: "a caller allowed iam:PassRole on another role only",
            pass_role: vec![statement("Allow", "iam:PassRole", &role_arn(OTHER_ROLE))],
            role_arn: delivery.clone(),
            installed: false,
        },
        Row {
            case: "a caller allowed iam:PassRole everywhere and denied it on that role",
            pass_role: vec![
                statement("Allow", "iam:PassRole", "*"),
                statement("Deny", "iam:PassRole", &delivery),
            ],
            role_arn: delivery.clone(),
            installed: false,
        },
        Row {
            case: "a caller allowed the whole iam namespace",
            pass_role: vec![statement("Allow", "iam:*", "*")],
            role_arn: delivery.clone(),
            installed: true,
        },
        Row {
            // The boundary allows the pass; the role itself cannot be given to the service,
            // because the trust document IAM reported for it grants nobody the assume.
            case: "a passable role this account holds no trust policy for",
            pass_role: vec![statement("Allow", "iam:PassRole", "*")],
            role_arn: role_arn(UNTRUSTED_ROLE),
            installed: false,
        },
        Row {
            // `iam:PassRole` is granted by the account that HOLDS the role, so no policy of
            // this account makes another account's role passable.
            case: "a role of another account the caller is allowed to pass",
            pass_role: vec![statement("Allow", "iam:PassRole", "*")],
            role_arn: format!("arn:aws:iam::{OTHER_ACCOUNT}:role/{DELIVERY_ROLE}"),
            installed: false,
        },
    ]
}

/// What `GetModelInvocationLoggingConfiguration` reports, read with the account's own
/// credential so the read is never the thing a row turns on.
async fn installed(emu: &crate::custom_model_authorization::Emulator, case: &str) -> Value {
    let reply: Reply = emu.call_as(ACCOUNT_KEY_ID, "GET", LOGGING_URI, None).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: GetModelInvocationLoggingConfiguration must answer 200, got {} {:?}",
        reply.status, reply.body
    );
    reply.get("loggingConfig")
}

/// RULE: a CloudWatch Logs destination is installed only where the account's IAM lets the
/// CALLING principal pass the role it names to Amazon Bedrock, and only where that role is
/// one of this account the service may be given; any other answer is 403
/// `AccessDeniedException` with the one sentence the denial is stated with, and installs
/// nothing.
#[tokio::test]
async fn an_invocation_logging_destination_needs_the_callers_pass_role_for_its_role() {
    let emu = emulator();
    given_session(&emu.engine);
    // The roles the rows name, as `iam:CreateRole` records them: the delivery role trusted
    // to this service, one carrying no trust policy, and one the caller may pass instead.
    let trust = crate::control_plane_authorization::trust(None);
    given_role(&emu.engine, DELIVERY_ROLE, Some(&trust));
    given_role(&emu.engine, OTHER_ROLE, Some(&trust));
    given_role(&emu.engine, UNTRUSTED_ROLE, None);

    // What an independent read must report: nothing until the first admitted put, and the
    // configuration that put installed from then on.
    let mut in_force = Value::Null;
    assert_eq!(
        installed(&emu, "before any put").await,
        in_force,
        "case \"before any put\": nothing is configured until a put installs something"
    );

    for (index, row) in rows().iter().enumerate() {
        given_role_policy(&emu.engine, Some(&caller_document(&row.pass_role)));
        let config = cloud_watch_config(&format!("pass-role-{index}"), &row.role_arn);
        let reply = emu
            .call_as(
                SESSION_KEY_ID,
                "PUT",
                LOGGING_URI,
                Some(json!({ "loggingConfig": config.clone() })),
            )
            .await;

        if row.installed {
            // `PutModelInvocationLoggingConfigurationOutput` declares no members, so an
            // admitted put answers the 200 and nothing else.
            assert_eq!(
                (reply.status, reply.error.as_deref()),
                (200, None),
                "case {:?}: a pass the account allows must answer 200, got {} {:?} {:?}",
                row.case,
                reply.status,
                reply.error,
                reply.body
            );
            in_force = config;
        } else {
            assert_eq!(
                (
                    reply.status,
                    reply.error.as_deref(),
                    reply.get("message")
                ),
                (403, Some("AccessDeniedException"), json!(PASS_ROLE_REFUSAL)),
                "case {:?}: a pass the account does not allow must be refused 403 \
                 AccessDeniedException with the recorded sentence, got {} {:?} {:?}",
                row.case,
                reply.status,
                reply.error,
                reply.body
            );
        }

        let reported = installed(&emu, row.case).await;
        assert_eq!(
            reported, in_force,
            "case {:?}: the configuration in force must be the last admitted one, got \
             {reported:?}",
            row.case
        );
    }
}

/// RULE (the grounding half): a refusal has to rest on a document that was READ, so a
/// caller this emulator holds no policies for installs the destination — and a
/// configuration that names no role-bearing destination passes no role at all, so the
/// decision does not arise for it however the caller's policies read.
#[tokio::test]
async fn the_account_credential_passes_a_role_no_policy_bounds() {
    let emu = emulator();
    given_session(&emu.engine);
    given_role(
        &emu.engine,
        DELIVERY_ROLE,
        Some(&crate::control_plane_authorization::trust(None)),
    );

    // The account's own published credential holds no IAM policies, which is the position
    // AWS account root credentials are in: the pass is admitted and the destination reads
    // back exactly as it was submitted.
    let config = cloud_watch_config("account-credential", &role_arn(DELIVERY_ROLE));
    let reply = emu
        .call_as(
            ACCOUNT_KEY_ID,
            "PUT",
            LOGGING_URI,
            Some(json!({ "loggingConfig": config.clone() })),
        )
        .await;
    assert_eq!(
        (reply.status, reply.error.as_deref()),
        (200, None),
        "case \"the account's own credential\": expected 200, got {} {:?} {:?}",
        reply.status,
        reply.error,
        reply.body
    );
    let reported = installed(&emu, "the account's own credential").await;
    assert_eq!(
        reported, config,
        "case \"the account's own credential\": the destination must read back as submitted, \
         got {reported:?}"
    );

    // The SAME caller whose policies bear no IAM action at all installs a configuration
    // that passes no role: `S3Config` declares no role member, so there is no pass to
    // authorize, and the per-modality switches round-trip whole.
    given_role_policy(&emu.engine, Some(&caller_document(&[])));
    let role_free = json!({
        "s3Config": { "bucketName": "bedrock-invocation-logs", "keyPrefix": "prod/" },
        "textDataDeliveryEnabled": false,
        "imageDataDeliveryEnabled": true,
        "embeddingDataDeliveryEnabled": true,
    });
    let reply = emu
        .call_as(
            SESSION_KEY_ID,
            "PUT",
            LOGGING_URI,
            Some(json!({ "loggingConfig": role_free.clone() })),
        )
        .await;
    assert_eq!(
        (reply.status, reply.error.as_deref()),
        (200, None),
        "case \"a configuration that passes no role\": expected 200, got {} {:?} {:?}",
        reply.status,
        reply.error,
        reply.body
    );
    let reported = installed(&emu, "a configuration that passes no role").await;
    assert_eq!(
        reported, role_free,
        "case \"a configuration that passes no role\": the configuration must read back as \
         submitted, got {reported:?}"
    );

    // And the same caller naming a role-bearing destination is refused, leaving the
    // role-free configuration in force: the difference is the pass, not the put.
    let reply = emu
        .call_as(
            SESSION_KEY_ID,
            "PUT",
            LOGGING_URI,
            Some(json!({
                "loggingConfig": cloud_watch_config("unbounded", &role_arn(DELIVERY_ROLE)),
            })),
        )
        .await;
    assert_eq!(
        (reply.status, reply.error.as_deref(), reply.get("message")),
        (403, Some("AccessDeniedException"), json!(PASS_ROLE_REFUSAL)),
        "case \"a destination under a caller allowed no IAM action\": expected the recorded \
         403, got {} {:?} {:?}",
        reply.status,
        reply.error,
        reply.body
    );
    let reported = installed(&emu, "a destination under a caller allowed no IAM action").await;
    assert_eq!(
        reported, role_free,
        "case \"a destination under a caller allowed no IAM action\": a refused put replaces \
         nothing, got {reported:?}"
    );
}
