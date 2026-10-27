// SPDX-License-Identifier: CC0-1.0

//! Deleting a SES sending-authorization policy, end to end over the wire.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, so each one is a real
//! awsQuery request against the service: form-urlencoded body in, XML out, one
//! shared engine behind all of them. What is asserted is the RULE the operation
//! follows over a SET of inputs, not one recorded request:
//!
//! * a delete removes EXACTLY the named policy from the identity record the
//!   account holds, whichever spelling (name or ARN) the call names the identity
//!   by, and leaves every other policy — and every other identity — as it was,
//!   which `ListIdentityPolicies` and `GetIdentityPolicies` then report;
//! * the delete is keyed on the name and idempotent: a name the identity does not
//!   carry is a 200 that changes nothing, including a repeat of a delete that
//!   already removed the policy and a name no put could have stored;
//! * an identity this account has not verified is refused ("this operation is for
//!   the identity owner only"), and that refusal stores nothing;
//! * the freed name is available again: a put under it records the new document.

use crate::harness::{call, element, elements, emulator, field, map_entries, xml_text, Field};
use smithy_server_runtime::MultiServiceRouter;

/// `VerifyEmailIdentity`, so the account holds the identity a policy is put on.
async fn verify_email(router: &MultiServiceRouter, address: &str) {
    let (status, body) = call(
        router,
        "VerifyEmailIdentity",
        &[field("EmailAddress", address)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case VerifyEmailIdentity({address}): expected 200, got {status} with {body}"
    );
}

/// `VerifyDomainIdentity`, the other class of identity a policy may be put on.
async fn verify_domain(router: &MultiServiceRouter, domain: &str) {
    let (status, body) = call(router, "VerifyDomainIdentity", &[field("Domain", domain)]).await;
    assert_eq!(
        status, 200,
        "case VerifyDomainIdentity({domain}): expected 200, got {status} with {body}"
    );
}

/// `PutIdentityPolicy` that must be accepted — the precondition of a delete, not
/// what this suite is about.
async fn put_accepted(router: &MultiServiceRouter, identity: &str, name: &str, document: &str) {
    let (status, body) = call(
        router,
        "PutIdentityPolicy",
        &[
            field("Identity", identity),
            field("PolicyName", name),
            field("Policy", document),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case put {name:?} on {identity}: expected 200, got {status} with {body}"
    );
}

/// `DeleteIdentityPolicy`, as `(status, body)` — the refusal rows assert on both.
async fn delete_policy(router: &MultiServiceRouter, identity: &str, name: &str) -> (u16, String) {
    call(
        router,
        "DeleteIdentityPolicy",
        &[field("Identity", identity), field("PolicyName", name)],
    )
    .await
}

/// `DeleteIdentityPolicy` that must be accepted, with the case it belongs to.
async fn delete_accepted(router: &MultiServiceRouter, case: &str, identity: &str, name: &str) {
    let (status, body) = delete_policy(router, identity, name).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from DeleteIdentityPolicy({identity}, {name:?}), got \
         {status} with {body}"
    );
}

/// The policy names `ListIdentityPolicies` reports, in the order it reports them.
async fn policy_names(router: &MultiServiceRouter, identity: &str) -> Vec<String> {
    let (status, body) = call(
        router,
        "ListIdentityPolicies",
        &[field("Identity", identity)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case ListIdentityPolicies({identity}): expected 200, got {status} with {body}"
    );
    element(&body, "PolicyNames")
        .map(|list| elements(&list, "member"))
        .unwrap_or_default()
}

/// The `(name, document)` pairs `GetIdentityPolicies` reports for `names`.
async fn fetched_policies(
    router: &MultiServiceRouter,
    identity: &str,
    names: &[&str],
) -> Vec<(String, String)> {
    let mut params: Vec<Field> = vec![field("Identity", identity)];
    for (index, name) in names.iter().enumerate() {
        params.push(field(format!("PolicyNames.member.{}", index + 1), *name));
    }
    let (status, body) = call(router, "GetIdentityPolicies", &params).await;
    assert_eq!(
        status, 200,
        "case GetIdentityPolicies({identity}, {names:?}): expected 200, got {status} with {body}"
    );
    map_entries(&body)
        .into_iter()
        .map(|(name, document)| (name, xml_text(&document)))
        .collect()
}

/// One sending-authorization policy, written the way a client sends one: a
/// statement naming the `Principal` it authorizes to send through the identity.
/// `sid` is the statement's optional id, which is what tells two documents apart.
fn policy_with_sid(sid: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Sid\":\"{sid}\",\"Effect\":\"Allow\",\
         \"Principal\":{{\"AWS\":\"arn:aws:iam::000000000000:root\"}},\
         \"Action\":\"ses:SendEmail\",\"Resource\":\"arn:aws:ses:us-east-1:\
         000000000000:identity/example.com\"}}]}}"
    )
}

/// The policy every row that is not about the document itself starts from.
fn policy() -> String {
    policy_with_sid("AuthorizeOneSender")
}

#[tokio::test]
async fn a_delete_removes_exactly_the_named_policy_and_leaves_the_rest() {
    // The rows differ along what the rule generalises over: how many policies the
    // identity carries, WHICH of them is deleted (the first, a middle one, the
    // last), the class of identity, the spelling the delete names the identity by
    // (its name, or its ARN), and the character classes `PolicyName` admits —
    // "alphanumeric characters, dashes, and underscores", up to its modeled
    // length of 64.
    struct Row<'a> {
        case: &'a str,
        identity: &'a str,
        domain: bool,
        put: &'a [&'a str],
        deleted_as: &'a str,
        delete: &'a str,
        expected: &'a [&'a str],
    }
    let long_name = "n".repeat(64);
    let rows = [
        Row {
            case: "the only policy on an email identity",
            identity: "sender@example.com",
            domain: false,
            put: &["Authorize"],
            deleted_as: "sender@example.com",
            delete: "Authorize",
            expected: &[],
        },
        Row {
            case: "the first of three, by lexicographic order",
            identity: "many@example.com",
            domain: false,
            put: &["Zeta", "Alpha", "Mu"],
            deleted_as: "many@example.com",
            delete: "Alpha",
            expected: &["Mu", "Zeta"],
        },
        Row {
            case: "the middle of three",
            identity: "many@example.com",
            domain: false,
            put: &["Zeta", "Alpha", "Mu"],
            deleted_as: "many@example.com",
            delete: "Mu",
            expected: &["Alpha", "Zeta"],
        },
        Row {
            case: "the last of three",
            identity: "many@example.com",
            domain: false,
            put: &["Zeta", "Alpha", "Mu"],
            deleted_as: "many@example.com",
            delete: "Zeta",
            expected: &["Alpha", "Mu"],
        },
        Row {
            case: "a name carrying a dash, on a domain identity",
            identity: "example.com",
            domain: true,
            put: &["Delegate-Two", "Delegate_One"],
            deleted_as: "example.com",
            delete: "Delegate-Two",
            expected: &["Delegate_One"],
        },
        Row {
            case: "a name carrying digits, deleted through the identity's ARN",
            identity: "arn.example.com",
            domain: true,
            put: &["Policy2024", "Policy2025"],
            deleted_as: "arn:aws:ses:us-east-1:000000000000:identity/arn.example.com",
            delete: "Policy2024",
            expected: &["Policy2025"],
        },
        Row {
            case: "an email identity named by its ARN",
            identity: "arn-sender@example.com",
            domain: false,
            put: &["ByArn", "Kept"],
            deleted_as: "arn:aws:ses:us-west-2:000000000000:identity/arn-sender@example.com",
            delete: "ByArn",
            expected: &["Kept"],
        },
        Row {
            case: "a name at the modeled 64-character length",
            identity: "sender@example.com",
            domain: false,
            put: &[&long_name, "Kept"],
            deleted_as: "sender@example.com",
            delete: &long_name,
            expected: &["Kept"],
        },
    ];

    for row in rows {
        let router = emulator();
        if row.domain {
            verify_domain(&router, row.identity).await;
        } else {
            verify_email(&router, row.identity).await;
        }
        // A second identity, so a delete that reached across identities would
        // show up as a missing policy here.
        verify_email(&router, "bystander@example.com").await;
        put_accepted(&router, "bystander@example.com", "Untouched", &policy()).await;

        for name in row.put {
            put_accepted(&router, row.identity, name, &policy()).await;
        }
        delete_accepted(&router, row.case, row.deleted_as, row.delete).await;

        let listed = policy_names(&router, row.identity).await;
        assert_eq!(
            listed, row.expected,
            "case {:?}: expected the remaining names {:?}, got {listed:?}",
            row.case, row.expected
        );
        // The deleted name is ABSENT from a read by name, not reported empty.
        let fetched = fetched_policies(&router, row.identity, &[row.delete]).await;
        assert!(
            fetched.is_empty(),
            "case {:?}: a deleted policy must not be readable, got {fetched:?}",
            row.case
        );
        let bystander = policy_names(&router, "bystander@example.com").await;
        assert_eq!(
            bystander,
            vec!["Untouched".to_string()],
            "case {:?}: another identity's policies must be untouched, got {bystander:?}",
            row.case
        );
    }
}

#[tokio::test]
async fn deleting_a_name_the_identity_does_not_carry_is_a_success_that_changes_nothing() {
    // "This operation returns successfully even if a policy with the specified
    // name does not exist." The rows span the ways a name can be absent: never
    // put, differing from a stored name only by case, a name a put would refuse
    // outright (`PolicyName` admits only alphanumerics, dashes and underscores),
    // the longest name the model admits, and a name already deleted.
    let stored = ["Authorize", "Delegate_One"];
    let rows = [
        ("a name never put", "NeverPut"),
        ("a stored name in another case", "authorize"),
        ("a name a put refuses outright", "authorize.marketer"),
        ("a name of digits", "20241026"),
        ("the longest name the model admits", &"n".repeat(64) as &str),
        ("a single-character name", "x"),
    ];

    for (case, absent) in rows {
        let router = emulator();
        verify_email(&router, "sender@example.com").await;
        for name in stored {
            put_accepted(&router, "sender@example.com", name, &policy()).await;
        }

        // Deleting it once, and again: the answer and the store are the same both
        // times, so the call is idempotent on the name.
        for attempt in 1..=2 {
            delete_accepted(&router, case, "sender@example.com", absent).await;
            let listed = policy_names(&router, "sender@example.com").await;
            assert_eq!(
                listed,
                stored.map(str::to_string).to_vec(),
                "case {case:?} (attempt {attempt}): deleting an absent name must change nothing, \
                 got {listed:?}"
            );
        }
    }

    // A repeat of a delete that DID remove a policy is the same success, for the
    // same reason: after the first call the name is simply not there.
    let router = emulator();
    verify_email(&router, "sender@example.com").await;
    for name in stored {
        put_accepted(&router, "sender@example.com", name, &policy()).await;
    }
    for attempt in 1..=3 {
        delete_accepted(
            &router,
            "a repeated delete of a stored policy",
            "sender@example.com",
            "Authorize",
        )
        .await;
        let listed = policy_names(&router, "sender@example.com").await;
        assert_eq!(
            listed,
            vec!["Delegate_One".to_string()],
            "case a repeated delete (attempt {attempt}): expected only the other policy, got \
             {listed:?}"
        );
    }
}

#[tokio::test]
async fn an_identity_this_account_does_not_hold_is_refused_and_nothing_is_removed() {
    let router = emulator();
    // One verified identity carrying a policy, so the refusals below are about
    // the identity named and a leak would be visible as a removed policy.
    verify_email(&router, "sender@example.com").await;
    put_accepted(&router, "sender@example.com", "Authorize", &policy()).await;

    // The rows span the classes of identity name a request can carry: an address,
    // a domain, and an ARN — none of them verified by this account.
    for unknown in [
        "stranger@example.com",
        "not-verified.example",
        "arn:aws:ses:us-east-1:000000000000:identity/stranger.example",
    ] {
        // Both a name the verified identity carries and one nobody carries: the
        // refusal is about the identity, so the policy name cannot change it.
        for name in ["Authorize", "NeverPut"] {
            let (status, body) = delete_policy(&router, unknown, name).await;
            assert_eq!(
                status, 400,
                "case DeleteIdentityPolicy({unknown}, {name:?}): an identity this account has \
                 not verified is refused, got {status} with {body}"
            );
            assert_eq!(
                element(&body, "Code").as_deref(),
                Some("ValidationException"),
                "case DeleteIdentityPolicy({unknown}, {name:?}): expected the refusal code, got \
                 {body}"
            );
        }
    }

    let listed = policy_names(&router, "sender@example.com").await;
    assert_eq!(
        listed,
        vec!["Authorize".to_string()],
        "case after the refusals: the verified identity keeps its policy, got {listed:?}"
    );
}

#[tokio::test]
async fn a_deleted_name_is_free_again_and_a_put_under_it_records_the_new_document() {
    let router = emulator();
    verify_email(&router, "sender@example.com").await;

    // The rows differ in the name being recycled, spanning the admitted character
    // classes, and in the documents put under it before and after the delete.
    for (case, name) in [
        ("a plain name", "Authorize"),
        ("a name with a dash and an underscore", "Authorize-Marketer_2"),
        ("a name of digits", "2024"),
    ] {
        let first = policy_with_sid("First");
        let second = policy_with_sid("Second");

        put_accepted(&router, "sender@example.com", name, &first).await;
        let fetched = fetched_policies(&router, "sender@example.com", &[name]).await;
        assert_eq!(
            fetched,
            vec![(name.to_string(), first.clone())],
            "case {case:?}: the stored document must be readable before the delete, got \
             {fetched:?}"
        );

        delete_accepted(&router, case, "sender@example.com", name).await;
        let fetched = fetched_policies(&router, "sender@example.com", &[name]).await;
        assert!(
            fetched.is_empty(),
            "case {case:?}: nothing may be readable under a deleted name, got {fetched:?}"
        );

        // The freed name takes a NEW document, and nothing of the removed one
        // survives.
        put_accepted(&router, "sender@example.com", name, &second).await;
        let fetched = fetched_policies(&router, "sender@example.com", &[name]).await;
        assert_eq!(
            fetched,
            vec![(name.to_string(), second.clone())],
            "case {case:?}: a put after the delete must record the new document, got {fetched:?}"
        );

        // Leave the identity as it was found, so the next row starts from one
        // policy of its own.
        delete_accepted(&router, case, "sender@example.com", name).await;
        let listed = policy_names(&router, "sender@example.com").await;
        assert!(
            listed.is_empty(),
            "case {case:?}: the identity must carry nothing after the final delete, got \
             {listed:?}"
        );
    }
}
