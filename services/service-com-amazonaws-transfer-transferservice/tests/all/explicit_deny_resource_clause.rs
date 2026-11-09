// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHAT an explicit-deny refusal says, and WHICH resource the
//! denied action was decided against.
//!
//! RULE under test: IAM states an explicit-deny decision as "User: `<principal arn>` is
//! not authorized to perform: `<service:Action>` on resource: `<resource arn>` with an
//! explicit deny in an identity-based policy" — so every refusal this service's
//! authorization gate answers names the ARN the action was EVALUATED against, which is:
//!
//! * the ARN of the resource the request addresses, where it addresses one (a server, a
//!   user); and
//! * the resource TYPE's wildcard ARN in the request's Region, where the action is on a
//!   resource type whose instance the request cannot name — `CreateServer`, whose
//!   identifier is the service's to mint, is evaluated against
//!   `arn:aws:transfer:<region>:<account>:server/*`.
//!
//! The second half of the rule is observable twice: in the sentence, and in WHICH
//! statement denies — a deny scoped to the server type's wildcard refuses a create that
//! names no server, while one scoped to a single server does not.
//!
//! Every request goes over the real wire path (`POST /` with
//! `X-Amz-Target: TransferService.<Op>`) through the shared [`crate::harness`].

use serde_json::{json, Value};

use crate::harness::{member_of, Emulator, ACCOUNT, REGION, ROLE, SSH_KEY};

/// The IAM user every row signs as, and the ARN IAM gives it — the principal the
/// sentence names.
const PRINCIPAL_NAME: &str = "denied-user";
const PRINCIPAL_ARN: &str = "arn:aws:iam::000000000000:user/denied-user";

/// The user every row's user-addressed operation is about.
const USER_NAME: &str = "u-denied";

/// A policy allowing every action of this service and denying exactly `action` on
/// exactly `resource`.
fn allow_all_but(action: &str, resource: &str) -> String {
    format!(
        r#"{{"Version":"2012-10-17","Statement":[
            {{"Effect":"Allow","Action":"transfer:*","Resource":"*"}},
            {{"Effect":"Deny","Action":"transfer:{action}","Resource":"{resource}"}}]}}"#
    )
}

/// The denial sentence for a request that was decided against `resource`.
fn denial(action: &str, resource: Option<&str>) -> String {
    match resource {
        Some(resource) => format!(
            "User: {PRINCIPAL_ARN} is not authorized to perform: transfer:{action} \
             on resource: {resource} with an explicit deny in an identity-based policy"
        ),
        None => format!(
            "User: {PRINCIPAL_ARN} is not authorized to perform: transfer:{action} \
             with an explicit deny in an identity-based policy"
        ),
    }
}

/// `arn:aws:transfer:<region>:<account>:<resource>`.
fn transfer_arn(resource: &str) -> String {
    format!("arn:aws:transfer:{REGION}:{ACCOUNT}:{resource}")
}

/// RULE: the refusal names the principal, the action AND the resource the action was
/// evaluated against — the addressed resource's own ARN, or the resource type's wildcard
/// for an action whose instance the request does not name.
///
/// The rows differ along the operation and so along the resource type the action is
/// decided on: a create whose identifier the service mints (the server wildcard), a
/// read, an update, a delete and a state change of one server (that server's ARN), the
/// writes and reads addressed at one user (that user's ARN), and a listing that
/// addresses no resource at all (no clause).
#[tokio::test]
async fn an_explicit_deny_names_the_resource_the_action_was_evaluated_against() {
    let any_server = transfer_arn("server/*");

    // (case, operation, the request's extra members, the resource the refusal must name,
    //  as a function of the server id)
    type ResourceOf = fn(&str) -> Option<String>;
    let rows: Vec<(&str, &str, Value, ResourceOf)> = vec![
        (
            "a create whose identifier the service mints",
            "CreateServer",
            json!({}),
            |_| Some(format!("arn:aws:transfer:{REGION}:{ACCOUNT}:server/*")),
        ),
        (
            "a read of one server",
            "DescribeServer",
            json!({}),
            |server_id| Some(format!("arn:aws:transfer:{REGION}:{ACCOUNT}:server/{server_id}")),
        ),
        (
            "an update of one server",
            "UpdateServer",
            json!({}),
            |server_id| Some(format!("arn:aws:transfer:{REGION}:{ACCOUNT}:server/{server_id}")),
        ),
        (
            "a delete of one server",
            "DeleteServer",
            json!({}),
            |server_id| Some(format!("arn:aws:transfer:{REGION}:{ACCOUNT}:server/{server_id}")),
        ),
        (
            "a state change of one server",
            "StartServer",
            json!({}),
            |server_id| Some(format!("arn:aws:transfer:{REGION}:{ACCOUNT}:server/{server_id}")),
        ),
        (
            "a listing of one server's users",
            "ListUsers",
            json!({}),
            |server_id| Some(format!("arn:aws:transfer:{REGION}:{ACCOUNT}:server/{server_id}")),
        ),
        (
            "a create of one user",
            "CreateUser",
            json!({"UserName": "u-fresh", "Role": ROLE}),
            |server_id| {
                Some(format!(
                    "arn:aws:transfer:{REGION}:{ACCOUNT}:user/{server_id}/u-fresh"
                ))
            },
        ),
        (
            "a read of one user",
            "DescribeUser",
            json!({"UserName": USER_NAME}),
            |server_id| {
                Some(format!(
                    "arn:aws:transfer:{REGION}:{ACCOUNT}:user/{server_id}/{USER_NAME}"
                ))
            },
        ),
        (
            "a key import onto one user",
            "ImportSshPublicKey",
            json!({"UserName": USER_NAME, "SshPublicKeyBody": SSH_KEY}),
            |server_id| {
                Some(format!(
                    "arn:aws:transfer:{REGION}:{ACCOUNT}:user/{server_id}/{USER_NAME}"
                ))
            },
        ),
        (
            "a listing that addresses no resource of its own",
            "ListServers",
            json!({}),
            |_| None,
        ),
    ];

    for (case, operation, members, resource_of) in rows {
        // The server and user the row addresses are created BEFORE the principal is
        // seeded, so the setup is never what the gate refuses.
        let emulator = Emulator::new();
        let server_id = emulator.create_server(json!({})).await;
        emulator.create_user(&server_id, USER_NAME).await;
        emulator.seed_calling_principal(PRINCIPAL_NAME, &allow_all_but(operation, "*"));

        let mut request = members.clone();
        if operation != "CreateServer" && operation != "ListServers" {
            request["ServerId"] = json!(&server_id);
        }
        let answer = emulator.call(operation, request).await;
        assert_eq!(
            answer.status, 403,
            "case {case:?}: expected HTTP 403, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "AccessDeniedException",
            "case {case:?}: expected AccessDeniedException, got body {}",
            answer.body
        );
        let message = member_of(&answer.body, "Message")
            .as_str()
            .unwrap_or_default()
            .to_string();
        let expected = resource_of(&server_id);
        assert_eq!(
            message,
            denial(operation, expected.as_deref()),
            "case {case:?}: unexpected denial sentence"
        );
    }

    // The resource a create is decided against is observable in WHICH statement denies
    // it, not only in the sentence: the server type's wildcard covers it, one server does
    // not, and a deny on another Region's servers does not either.
    let other_region_servers = format!("arn:aws:transfer:eu-west-1:{ACCOUNT}:server/*");
    let scoped: Vec<(&str, String, bool)> = vec![
        ("a deny on the server type's wildcard", any_server.clone(), true),
        ("a deny on every resource", "*".to_string(), true),
        (
            "a deny on one server",
            transfer_arn("server/s-0123456789abcdef0"),
            false,
        ),
        (
            "a deny on another Region's servers",
            other_region_servers,
            false,
        ),
        ("a deny on the user type's wildcard", transfer_arn("user/*"), false),
    ];
    for (case, resource, denied) in scoped {
        let emulator = Emulator::new();
        emulator.seed_calling_principal(PRINCIPAL_NAME, &allow_all_but("CreateServer", &resource));
        let answer = emulator.call("CreateServer", json!({})).await;
        if denied {
            assert_eq!(
                answer.status, 403,
                "case {case:?}: expected the create to be refused, got {} with body {}",
                answer.status, answer.body
            );
            let message = member_of(&answer.body, "Message")
                .as_str()
                .unwrap_or_default()
                .to_string();
            assert_eq!(
                message,
                denial("CreateServer", Some(&any_server)),
                "case {case:?}: unexpected denial sentence"
            );
        } else {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected the create to be served, got {} with body {}",
                answer.status, answer.body
            );
        }
    }
}
