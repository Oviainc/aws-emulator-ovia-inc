// SPDX-License-Identifier: CC0-1.0

//! WHO a request signed with a vended credential speaks for, as the ACCOUNT answers it.
//!
//! RULE: a session credential minted for an IAM role is recorded in the account's identity
//! store as the session OF that role, so every service that authorizes a signed request
//! resolves the same principal from the request's access key id and judges the call against
//! the policies in force on that role. There are two readings of that question in this
//! emulator — the token service's own session registry, which `sts:GetCallerIdentity` answers
//! from, and the account's identity record, which a service's authorization gate reads — and a
//! mint has to leave BOTH, or one credential speaks for a role at one front door and for
//! nobody at another, which is a call nothing can refuse.
//!
//! This suite pins both readings of the credentials `GetCredentialsForIdentity` vends: the ARN
//! the token service reports for the key, and the role the account's identity record places
//! behind it. The rows differ along which of the pool's roles the request resolves — the
//! unauthenticated role, the authenticated role, and a role a mapping rule resolves — because
//! the record has to name the role THAT vend was of.
//!
//! The harness is `vended_role_session`'s: this service and the token service on ONE shared
//! engine, with the IAM records seeded into that same engine as `iam:CreateRole` writes them.

use std::time::SystemTime;

use serde_json::json;
use service_com_amazonaws_iam_awsidentitymanagementv20100508 as iam;

use crate::vended_role_session::{
    element, expected_session_arn, trust_policy, vended, Emulator,
};

/// RULE: the vended credential is resolved to the role it is a session of by the account's
/// identity store as well as by the token service, and the two name the same role.
#[tokio::test]
async fn a_vended_credential_is_recorded_as_a_session_of_its_role() {
    let emulator = Emulator::new();
    let pool_id = emulator.pool("vended-identity-record").await;
    let authenticated = emulator.seed_role("record-auth", Some(trust_policy(&pool_id)));
    let unauthenticated = emulator.seed_role("record-unauth", Some(trust_policy(&pool_id)));
    let mapped = emulator.seed_role("record-mapped", Some(trust_policy(&pool_id)));
    emulator
        .ok(
            "SetIdentityPoolRoles",
            &json!({
                "IdentityPoolId": &pool_id,
                "Roles": {"authenticated": &authenticated, "unauthenticated": &unauthenticated},
                "RoleMappings": {
                    "accounts.google.com": {
                        "Type": "Rules",
                        "AmbiguousRoleResolution": "Deny",
                        "RulesConfiguration": {"Rules": [{
                            "Claim": "email",
                            "MatchType": "Equals",
                            "Value": "someone@example.com",
                            "RoleARN": &mapped,
                        }]},
                    },
                },
            }),
        )
        .await;

    // (case, the Logins the request presents, the CustomRoleArn it names, the role it
    // resolves, that role's ARN)
    let rows = [
        (
            "no Logins: the unauthenticated role",
            None,
            None,
            "record-unauth",
            &unauthenticated,
        ),
        (
            "one login: the authenticated role",
            Some(json!({"graph.facebook.com": "a.token.value"})),
            None,
            "record-auth",
            &authenticated,
        ),
        (
            "a CustomRoleArn naming a role a mapping rule resolves to",
            Some(json!({"accounts.google.com": "t1"})),
            Some(&mapped),
            "record-mapped",
            &mapped,
        ),
    ];

    for (case, logins, custom_role_arn, role_name, role_arn) in rows {
        let identity_id = emulator.identity(&pool_id, logins.clone()).await;
        let mut request = json!({"IdentityId": &identity_id});
        if let Some(logins) = &logins {
            request["Logins"] = logins.clone();
        }
        if let Some(custom_role_arn) = custom_role_arn {
            request["CustomRoleArn"] = json!(custom_role_arn);
        }
        let issued = emulator.ok("GetCredentialsForIdentity", &request).await;
        let (access_key_id, _) = vended(&issued);
        let expected_session = expected_session_arn(role_name);

        // Reading one: the token service's own registry, which is what reports an identity.
        let (status, body) = emulator.caller_identity(access_key_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetCallerIdentity under the vended credential expected 200, got \
             {status} with {body}"
        );
        let reported = element(&body, "Arn");
        assert_eq!(
            reported, expected_session,
            "case {case:?}: expected the token service to report {expected_session}, got \
             {reported}"
        );

        // Reading two: the account's identity record, which is what a service's authorization
        // gate resolves an access key id through — so what it names is the role a call made
        // with this credential is judged against.
        let now = aws_smithy_types::DateTime::from(SystemTime::now());
        let held = emulator
            .engine
            .state::<iam::ComAmazonawsIamAwsidentitymanagementv20100508State>();
        let record = held.lock().role_session(access_key_id, now).cloned();
        let record = record.unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected the account to hold a role session for the vended \
                 credential, so a call made with it is judged against {role_arn}"
            )
        });
        assert_eq!(
            record.session_arn, expected_session,
            "case {case:?}: expected the record to name the session {expected_session}, got \
             {:?}",
            record.session_arn
        );
        assert_eq!(
            &record.role_arn, role_arn,
            "case {case:?}: expected the record to place {role_arn} behind the credential, got \
             {:?}",
            record.role_arn
        );
        // A credential the account holds no record for speaks for the account itself, which no
        // policy of a role bounds: the two readings naming the same role is what makes a call
        // made with this credential refusable.
        assert!(
            reported == record.session_arn,
            "case {case:?}: expected both readings to name one session, got {reported} and {:?}",
            record.session_arn
        );
    }
}
