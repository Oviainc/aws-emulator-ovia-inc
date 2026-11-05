// SPDX-License-Identifier: CC0-1.0

//! The rules the resource-policy surface of a private CA follows: what `DeletePolicy`
//! removes and what it refuses, and which documents `PutPolicy` will attach.
//!
//! Three rules are under test, each over a table whose rows differ along what the rule
//! generalises over:
//!
//! 1. `DeletePolicy` is delete-if-exists — "If there is no policy attached to the private
//!    CA, this action will return successful" (its docs) — so it resolves only the
//!    AUTHORITY its `ResourceArn` names, in whatever lifecycle status that authority is in,
//!    removes whatever policy is attached and succeeds; `ResourceNotFoundException` is
//!    reserved for an ARN no authority of this service answers to. The rows span every
//!    `CertificateAuthorityStatus` the model declares, both a CA that carries a policy and
//!    one that carries none, a repeated call and two racing ones.
//! 2. A resource policy of a private CA grants only the actions a CA can be SHARED for, so
//!    a statement naming any other action is refused with `InvalidPolicyException` and
//!    nothing is attached. The accepted rows are the published catalog itself
//!    ([`POLICY_ALLOWED_ACTIONS`]), one row per entry; the refused rows span this service's
//!    management plane, a wildcard, another service's namespace, no namespace and a
//!    non-string.
//! 3. A resource policy states WHO it grants to, so each statement's `Principal` is
//!    resolved and a document naming an identity this emulator cannot resolve is refused
//!    with nothing attached. The rows span the wildcard, an account id, identity ARNs, a
//!    service principal and lists of them, against account ids and ARNs of accounts that
//!    are not here.
//!
//! Every call goes over the wire through the harness in [`super::private_ca_lifecycle`].

use serde_json::{json, Value};
use service_com_amazonaws_acmpca_acmprivateca::ops::types::CERTIFICATE_AUTHORITY_STATUS_VALUES;
use service_com_amazonaws_acmpca_acmprivateca::state::{
    CA_NOT_FOUND_MESSAGE, DISALLOWED_POLICY_ACTION_MESSAGE, POLICY_ALLOWED_ACTIONS,
    POLICY_NOT_FOUND_MESSAGE, POLICY_PRINCIPAL_NOT_FOUND_MESSAGE,
};

use crate::delete_certificate_authority::force_status;
use crate::private_ca_lifecycle::{root_ca_request, string, Emulator, ACCOUNT, REGION};
use crate::resource_policy::error_message;

/// A private CA created over the wire, named after `label`.
///
/// No wait for the clock: every row below either forces the status it is about or is
/// indifferent to it, and the policy operations are not gated on a CA's status.
async fn certificate_authority(emulator: &Emulator, label: &str) -> String {
    let created = emulator
        .ok(label, "CreateCertificateAuthority", &root_ca_request(label))
        .await;
    string(&created, "CertificateAuthorityArn", label)
}

/// A one-statement resource policy of `ca_arn` granting `actions` to `principal`.
fn policy(ca_arn: &str, principal: Value, actions: Value) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "share",
            "Effect": "Allow",
            "Principal": principal,
            "Action": actions,
            "Resource": ca_arn
        }]
    })
    .to_string()
}

/// The smallest document this service attaches to `ca_arn`: one allow-listed action granted
/// to the one account this emulator presents.
fn attachable_policy(ca_arn: &str) -> String {
    policy(
        ca_arn,
        json!({"AWS": ACCOUNT}),
        json!(format!("acm-pca:{}", POLICY_ALLOWED_ACTIONS[0])),
    )
}

/// Whether a response body carries no members — which is what an operation modelled with a
/// `Unit` output answers with.
fn carries_no_members(answer: &Value) -> bool {
    answer.is_null() || answer.as_object().is_some_and(serde_json::Map::is_empty)
}

/// Call `DeletePolicy` for `ca_arn`, requiring the success the rule states.
async fn delete_policy(emulator: &Emulator, case: &str, ca_arn: &str) {
    let answer = emulator
        .ok(case, "DeletePolicy", &json!({"ResourceArn": ca_arn}))
        .await;
    assert!(
        carries_no_members(&answer),
        "case {case:?}: expected DeletePolicy to answer an empty body, got {answer:?}"
    );
}

/// Require that `ca_arn` carries no policy, as the READ reports it.
async fn no_policy_attached(emulator: &Emulator, case: &str, ca_arn: &str) {
    let body = emulator
        .refused(
            case,
            "GetPolicy",
            &json!({"ResourceArn": ca_arn}),
            "ResourceNotFoundException",
        )
        .await;
    let message = error_message(&body, case);
    assert_eq!(
        message, POLICY_NOT_FOUND_MESSAGE,
        "case {case:?}: expected GetPolicy to report the policy as the absent resource, \
         got {message:?}"
    );
}

/// The document `ca_arn` carries, as `GetPolicy` displays it.
async fn attached_policy(emulator: &Emulator, case: &str, ca_arn: &str) -> String {
    let read = emulator
        .ok(case, "GetPolicy", &json!({"ResourceArn": ca_arn}))
        .await;
    string(&read, "Policy", case)
}

/// Rule 1: whatever status a resolvable CA is in, and whether or not it carries a policy,
/// `DeletePolicy` leaves it carrying none and succeeds — the first call and every repeat.
#[tokio::test]
async fn delete_policy_succeeds_for_a_resolvable_ca_in_every_status() {
    let emulator = Emulator::new();

    // Every `CertificateAuthorityStatus` the model declares, crossed with the two things a
    // CA can hold: a policy, or none.
    for status in CERTIFICATE_AUTHORITY_STATUS_VALUES {
        for carries_policy in [true, false] {
            let case = format!("a {status} CA that carries {}", if carries_policy { "a policy" } else { "none" });
            let arn = certificate_authority(&emulator, &format!("delete-{status}-{carries_policy}"))
                .await;
            if carries_policy {
                emulator
                    .ok(
                        &case,
                        "PutPolicy",
                        &json!({"ResourceArn": arn, "Policy": attachable_policy(&arn)}),
                    )
                    .await;
            }
            // Three of the declared statuses are reachable over no implemented operation,
            // so the status is put on the stored record the handlers read.
            force_status(&emulator, &arn, status, &case);

            // The authority EXISTS in that status: whatever the delete answers, it answers
            // about a CA this service reports.
            let described = emulator
                .ok(
                    &case,
                    "DescribeCertificateAuthority",
                    &json!({"CertificateAuthorityArn": arn}),
                )
                .await;
            let reported = string(
                described.get("CertificateAuthority").unwrap_or(&Value::Null),
                "Status",
                &case,
            );
            assert_eq!(
                &reported, status,
                "case {case:?}: expected the CA to be readable as {status}, got {reported:?}"
            );

            // Delete-if-exists, so the repeat is a success too.
            for attempt in 1..=2 {
                let case = format!("{case}, delete number {attempt}");
                delete_policy(&emulator, &case, &arn).await;
                no_policy_attached(&emulator, &case, &arn).await;
            }
        }
    }
}

/// Rule 1, concurrently: two `DeletePolicy` calls racing on one CA both succeed, and the
/// policy is gone once.
#[tokio::test]
async fn two_racing_delete_policy_calls_both_succeed() {
    let emulator = Emulator::new();
    let case = "two racing deletes";
    let arn = certificate_authority(&emulator, "racing-deletes").await;
    emulator
        .ok(
            case,
            "PutPolicy",
            &json!({"ResourceArn": arn, "Policy": attachable_policy(&arn)}),
        )
        .await;

    let request = json!({"ResourceArn": arn});
    let (first, second) = tokio::join!(
        emulator.call("DeletePolicy", &request),
        emulator.call("DeletePolicy", &request)
    );
    for (which, (status, body)) in [("first", &first), ("second", &second)] {
        assert_eq!(
            *status, 200,
            "case {case:?}: expected the {which} racing delete to answer 200, got \
             {status} {body:?}"
        );
        assert!(
            carries_no_members(body),
            "case {case:?}: expected the {which} racing delete to answer an empty body, \
             got {body:?}"
        );
    }
    no_policy_attached(&emulator, case, &arn).await;
}

/// Rule 1's one refusal: the ARN names no authority at all. A CA that exists — in any
/// status, carrying nothing — is never reported as the absent resource.
#[tokio::test]
async fn delete_policy_reports_not_found_only_for_an_arn_naming_no_authority() {
    let emulator = Emulator::new();
    // A CA exists, so each refusal is about the ARN in the request rather than an empty
    // store, and the contrast below is between two ARNs of the same shape.
    let existing = certificate_authority(&emulator, "resolvable-neighbour").await;

    // Each row satisfies the modelled `Arn` pattern and names a certificate authority this
    // service never minted: a fresh id, an id of another shape, and the same resource in
    // another account, Region and partition.
    let rows: Vec<(&str, String)> = vec![
        (
            "an id no CA was minted with",
            format!(
                "arn:aws:acm-pca:{REGION}:{ACCOUNT}:certificate-authority/\
                 00000000-0000-0000-0000-000000000000"
            ),
        ),
        (
            "an id of another shape entirely",
            format!("arn:aws:acm-pca:{REGION}:{ACCOUNT}:certificate-authority/not-a-uuid"),
        ),
        (
            "another account's CA",
            format!("arn:aws:acm-pca:{REGION}:999999999999:certificate-authority/abcdef"),
        ),
        (
            "another Region's CA",
            format!("arn:aws:acm-pca:us-gov-west-1:{ACCOUNT}:certificate-authority/abcdef"),
        ),
        (
            "another partition's CA",
            format!("arn:aws-cn:acm-pca:cn-north-1:{ACCOUNT}:certificate-authority/abcdef"),
        ),
    ];

    for (case, arn) in &rows {
        let body = emulator
            .refused(
                case,
                "DeletePolicy",
                &json!({"ResourceArn": arn}),
                "ResourceNotFoundException",
            )
            .await;
        let message = error_message(&body, case);
        assert_eq!(
            message, CA_NOT_FOUND_MESSAGE,
            "case {case:?}: expected DeletePolicy to answer the service's CA not-found \
             sentence, got {message:?}"
        );
    }

    // The same call against the CA that IS there succeeds, policy or no policy.
    delete_policy(&emulator, "the CA that exists", &existing).await;
}

/// Rule 2: the actions a private CA's resource policy can grant are the ones a CA is
/// shared for, each spelled in this service's namespace.
#[tokio::test]
async fn put_policy_grants_only_the_actions_a_private_ca_is_shared_for() {
    let emulator = Emulator::new();
    let arn = certificate_authority(&emulator, "policy-actions").await;
    let namespaced: Vec<String> = POLICY_ALLOWED_ACTIONS
        .iter()
        .map(|action| format!("acm-pca:{action}"))
        .collect();

    // One accepted row per entry of the published catalog — so an entry no report cited
    // (`ListTags`, `GetCertificateAuthorityCsr`) is exercised too — and one carrying the
    // whole catalog at once.
    let mut accepted: Vec<(String, Value)> = namespaced
        .iter()
        .map(|action| (format!("the single action {action}"), json!(action)))
        .collect();
    accepted.push(("every allowed action at once".to_string(), json!(namespaced)));

    for (case, actions) in &accepted {
        let document = policy(&arn, json!({"AWS": ACCOUNT}), actions.clone());
        emulator
            .ok(
                case,
                "PutPolicy",
                &json!({"ResourceArn": arn, "Policy": document}),
            )
            .await;
        let displayed = attached_policy(&emulator, case, &arn).await;
        assert_eq!(
            displayed, document,
            "case {case:?}: expected the attached document back, got {displayed:?}"
        );
        // Cleared again, so the next row's refusal is observably an attach that did not
        // happen rather than the row before it.
        delete_policy(&emulator, case, &arn).await;
    }

    // Each refused row names at least one action a certificate authority cannot be shared
    // for: this service's management plane (create, update, delete, tag, permissions,
    // policy writes, revocation), a wildcard that would cover them, another service's
    // namespace, no namespace at all, and a value that is not an action.
    let refused: Vec<(&str, Value)> = vec![
        ("deleting the authority", json!("acm-pca:DeleteCertificateAuthority")),
        ("creating an authority", json!("acm-pca:CreateCertificateAuthority")),
        ("updating the authority", json!("acm-pca:UpdateCertificateAuthority")),
        ("restoring the authority", json!("acm-pca:RestoreCertificateAuthority")),
        ("writing the authority's tags", json!("acm-pca:TagCertificateAuthority")),
        ("granting a permission on it", json!("acm-pca:CreatePermission")),
        ("writing its policy", json!("acm-pca:PutPolicy")),
        ("deleting its policy", json!("acm-pca:DeletePolicy")),
        ("revoking a certificate", json!("acm-pca:RevokeCertificate")),
        ("every action of this service", json!("acm-pca:*")),
        ("every action of every service", json!("*")),
        ("an action of another service", json!("s3:GetObject")),
        ("an action with no namespace", json!("IssueCertificate")),
        ("an empty action", json!("")),
        (
            "one allowed action beside one that is not",
            json!(["acm-pca:GetCertificate", "acm-pca:DeleteCertificateAuthority"]),
        ),
        ("an action that is not a string", json!(17)),
    ];

    for (case, actions) in &refused {
        let body = emulator
            .refused(
                case,
                "PutPolicy",
                &json!({"ResourceArn": arn,
                        "Policy": policy(&arn, json!({"AWS": ACCOUNT}), actions.clone())}),
                "InvalidPolicyException",
            )
            .await;
        let message = error_message(&body, case);
        assert_eq!(
            message, DISALLOWED_POLICY_ACTION_MESSAGE,
            "case {case:?}: expected the refusal to name the resource type whose policy \
             cannot grant the action, got {message:?}"
        );
        no_policy_attached(&emulator, case, &arn).await;
    }
}

/// Rule 3: the identity a statement grants to is resolved, and a document naming one that
/// is not here attaches nothing.
#[tokio::test]
async fn put_policy_resolves_the_principal_each_statement_grants_to() {
    let emulator = Emulator::new();
    let arn = certificate_authority(&emulator, "policy-principals").await;
    let action = json!(format!("acm-pca:{}", POLICY_ALLOWED_ACTIONS[0]));

    // Every form that names an identity this emulator can resolve: every identity at all,
    // the one account it presents, that account's root, a user and a role of it, a service
    // principal, and lists of them.
    let accepted: Vec<(&str, Value)> = vec![
        ("every identity", json!("*")),
        ("every identity, under AWS", json!({"AWS": "*"})),
        ("this emulator's account", json!({"AWS": ACCOUNT})),
        (
            "that account's root",
            json!({"AWS": format!("arn:aws:iam::{ACCOUNT}:root")}),
        ),
        (
            "a role of that account",
            json!({"AWS": format!("arn:aws:iam::{ACCOUNT}:role/certificate-issuer")}),
        ),
        (
            "a user of that account",
            json!({"AWS": format!("arn:aws:iam::{ACCOUNT}:user/ca-admin")}),
        ),
        (
            "an identity ARN written as the shorthand string",
            json!(format!("arn:aws:iam::{ACCOUNT}:root")),
        ),
        (
            "two resolvable identities",
            json!({"AWS": [ACCOUNT, format!("arn:aws:iam::{ACCOUNT}:root")]}),
        ),
        (
            "the service principal that renews certificates",
            json!({"Service": "acm.amazonaws.com"}),
        ),
    ];

    for (case, principal) in &accepted {
        let document = policy(&arn, principal.clone(), action.clone());
        emulator
            .ok(
                case,
                "PutPolicy",
                &json!({"ResourceArn": arn, "Policy": document}),
            )
            .await;
        let displayed = attached_policy(&emulator, case, &arn).await;
        assert_eq!(
            displayed, document,
            "case {case:?}: expected the attached document back, got {displayed:?}"
        );
        delete_policy(&emulator, case, &arn).await;
    }

    // Each refused row names an identity that is not here: an account id this emulator
    // does not present, an identity of such an account, text that is no identity at all,
    // and a `Principal` element that names nobody.
    let refused: Vec<(&str, Value)> = vec![
        ("an account that does not exist", json!({"AWS": "999999999999"})),
        ("another absent account", json!({"AWS": "111122223333"})),
        (
            "the root of an account that does not exist",
            json!({"AWS": "arn:aws:iam::999999999999:root"}),
        ),
        (
            "a role of an account that does not exist",
            json!({"AWS": "arn:aws:iam::111122223333:role/issuer"}),
        ),
        (
            "one resolvable identity beside one that is not",
            json!({"AWS": [ACCOUNT, "999999999999"]}),
        ),
        ("text that is no identity", json!({"AWS": "nobody"})),
        ("an empty identity", json!({"AWS": ""})),
        ("an account id of the wrong length", json!({"AWS": "99999999999"})),
        ("an identity outside this emulator", json!({"Federated": "example.com"})),
        ("a principal naming nobody", json!({})),
        ("a principal that is not a string or an object", json!(17)),
    ];

    for (case, principal) in &refused {
        let body = emulator
            .refused(
                case,
                "PutPolicy",
                &json!({"ResourceArn": arn,
                        "Policy": policy(&arn, principal.clone(), action.clone())}),
                "InvalidPolicyException",
            )
            .await;
        let message = error_message(&body, case);
        assert_eq!(
            message, POLICY_PRINCIPAL_NOT_FOUND_MESSAGE,
            "case {case:?}: expected the refusal to report the principal as not found, \
             got {message:?}"
        );
        no_policy_attached(&emulator, case, &arn).await;
    }
}
