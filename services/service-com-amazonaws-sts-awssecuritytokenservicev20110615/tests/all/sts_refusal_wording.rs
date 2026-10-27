// SPDX-License-Identifier: CC0-1.0

//! How STS WORDS three refusals, each over the class of requests that provoke it.
//!
//! Three rules, each asserted over a table rather than one recorded request:
//!
//! * a request naming more managed session policies than a mint accepts is refused with
//!   the SERVICE's own sentence about that cap — the parameter and the limit, fixed text
//!   with nothing of the request in it — rather than with the framework's
//!   member-constraint rendering, which states a member path and echoes the value. The
//!   cap is the service's, not the model's: `policyDescriptorListType` carries no
//!   `@length` (`smithy_query(query="shape", shape="policyDescriptorListType")`), so no
//!   modelled constraint is being reported.
//! * a role-session mint the target role's trust policy withholds a grant for is refused
//!   naming the action the CALLER asked to perform: `sts:SetSourceIdentity` for a request
//!   that names `SourceIdentity`, and `sts:AssumeRole` for a chained request that names
//!   none, where the value is one STS carries forward rather than one the caller set.
//! * "the source identity value persists across chained role sessions"
//!   (`AssumeRoleRequest$SourceIdentity`), so a chained mint cannot replace it: naming a
//!   DIFFERENT value is refused as a validation error, naming the same one again is not a
//!   change and is served.
//!
//! Every call goes over the awsQuery wire through the same `MultiServiceRouter` the
//! emulator serves with.

use service_com_amazonaws_sts_awssecuritytokenservicev20110615 as sts;

use super::sts_sessions::{
    call, element, emulator, emulator_account, member, seed_role, trust_policy, LONG_TERM_KEY,
};

/// A managed policy ARN of the AWS-owned namespace, distinct per `index`.
///
/// AWS managed policies are named in the `arn:aws:iam::aws:policy/` namespace, which is
/// the namespace the recorded over-limit request used; the names are this test's own, so
/// no row depends on a particular published policy existing.
fn managed_policy_arn(index: usize) -> String {
    format!("arn:aws:iam::aws:policy/EmulatorProbePolicy{index:02}")
}

/// The `PolicyArns.member.<n>.arn` form fields naming `count` distinct managed policies,
/// each field name OWNED (the wire helper borrows them, so nothing has to be leaked to
/// hand it a `&'static str`).
fn policy_arn_fields(count: usize) -> Vec<(String, String)> {
    (0..count)
        .map(|index| {
            (
                format!("PolicyArns.member.{}.arn", index + 1),
                managed_policy_arn(index),
            )
        })
        .collect()
}

/// `fields` as the wire helper takes them.
fn borrowed(fields: &[(String, String)]) -> Vec<(&str, String)> {
    fields
        .iter()
        .map(|(name, value)| (name.as_str(), value.clone()))
        .collect()
}

/// Over the cap, every mint that carries `PolicyArns` states the service's own sentence —
/// and at the cap the request is not refused for the count at all.
#[tokio::test]
async fn naming_too_many_managed_session_policies_is_refused_in_sts_s_own_words() {
    let expected = format!(
        "The limit of ARNs for the PolicyArns parameter was exceeded. {} ARNs can be requested \
         at most in an API request.",
        sts::state::MAX_MANAGED_SESSION_POLICIES
    );

    // One row per (operation, count): the operations whose input carries `PolicyArns` and
    // whose over-limit leg is reachable over the wire. `AssumeRoleWithWebIdentity` carries
    // `PolicyArns` too and emits the same sentence, but it decides the WEB IDENTITY TOKEN
    // first (see its handler), so a request of it never reaches the cap unless the token is
    // one this emulator issued; `web_identity_federation.rs` owns that operation's wire
    // behaviour.
    let over_limit = [
        sts::state::MAX_MANAGED_SESSION_POLICIES + 1,
        sts::state::MAX_MANAGED_SESSION_POLICIES + 2,
        sts::state::MAX_MANAGED_SESSION_POLICIES * 3,
    ];
    for action in ["AssumeRole", "GetFederationToken"] {
        for count in over_limit {
            let case = format!("{action} naming {count} managed policies");
            let (router, engine) = emulator();
            let (role_arn, _) = seed_role(
                &engine,
                "AROASEEDEDOVERLIMIT01",
                "over-limit",
                "/",
                Some(&trust_policy(&["sts:AssumeRole"])),
                Some(43_200),
            );
            let fields = policy_arn_fields(count);
            let mut params = borrowed(&fields);
            if action == "AssumeRole" {
                params.push(("RoleArn", role_arn));
                params.push(("RoleSessionName", "over-limit".to_string()));
            } else {
                params.push(("Name", "over-limit".to_string()));
            }

            let (status, body) = call(&router, LONG_TERM_KEY, action, &params).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} {body}"
            );
            assert_eq!(
                element(&body, "Code").as_deref(),
                Some("ValidationError"),
                "case {case:?}: expected the ValidationError code, got {body}"
            );
            let message = member(&case, &body, "Message");
            assert_eq!(
                message, expected,
                "case {case:?}: expected the service's own over-limit sentence, got {message:?}"
            );
            // The refusal states the service's limit and nothing of the request: not the
            // ARNs it carried, and not the member path a constraint rendering would name.
            assert!(
                !message.contains("policyArns") && !message.contains(&managed_policy_arn(0)),
                "case {case:?}: the refusal must carry nothing of the request, got {message:?}"
            );
        }
    }

    // At the cap the count is accepted: the boundary is the limit itself, so the published
    // maximum is served and only what passes it is refused.
    let (router, engine) = emulator();
    let (role_arn, _) = seed_role(
        &engine,
        "AROASEEDEDATLIMIT001",
        "at-limit",
        "/",
        Some(&trust_policy(&["sts:AssumeRole"])),
        Some(43_200),
    );
    let fields = policy_arn_fields(sts::state::MAX_MANAGED_SESSION_POLICIES);
    let mut params = borrowed(&fields);
    params.push(("RoleArn", role_arn));
    params.push(("RoleSessionName", "at-limit".to_string()));
    let (status, body) = call(&router, LONG_TERM_KEY, "AssumeRole", &params).await;
    assert_eq!(
        status, 200,
        "expected the published maximum number of managed session policies to be served, got \
         {status} {body}"
    );
}

/// The two roles of a chain: one the caller may assume with every grant, and a target
/// whose trust policy lists exactly `target_actions`.
fn chain_roles(
    engine: &engine::InMemoryEngine,
    index: usize,
    target_actions: &[&str],
) -> (String, String) {
    let (first, _) = seed_role(
        engine,
        &format!("AROASEEDEDWORDING1{index:04}"),
        &format!("wording-first-{index}"),
        "/",
        Some(&trust_policy(&[
            "sts:AssumeRole",
            "sts:TagSession",
            "sts:SetSourceIdentity",
        ])),
        Some(43_200),
    );
    let (second, _) = seed_role(
        engine,
        &format!("AROASEEDEDWORDING2{index:04}"),
        &format!("wording-second-{index}"),
        "/",
        Some(&trust_policy(target_actions)),
        Some(43_200),
    );
    (first, second)
}

/// A chained mint refused for the source identity names the action the CALLER asked to
/// perform: the grant's own action when the request named the value, and `sts:AssumeRole`
/// when STS carried it forward.
#[tokio::test]
async fn a_refused_source_identity_names_the_action_the_request_asked_for() {
    let account = emulator_account();

    // (case, the source identity the FIRST hop sets, the one the CHAINED request names,
    // the action the refusal must name)
    type Row = (
        &'static str,
        &'static str,
        Option<&'static str>,
        &'static str,
    );
    let rows: Vec<Row> = vec![
        (
            "a source identity the chained request names itself",
            "alice",
            Some("alice"),
            "sts:SetSourceIdentity",
        ),
        (
            "a source identity carried forward, the chained request naming none",
            "alice",
            None,
            "sts:AssumeRole",
        ),
        (
            "another value carried forward, the chained request naming none",
            "workload-7",
            None,
            "sts:AssumeRole",
        ),
        (
            "a source identity spelled with the characters the member admits",
            "svc+role=ops,team.one@example.com",
            Some("svc+role=ops,team.one@example.com"),
            "sts:SetSourceIdentity",
        ),
    ];

    for (index, (case, first_identity, chained_identity, expected_action)) in
        rows.into_iter().enumerate()
    {
        let (router, engine) = emulator();
        // The target allows the assume and tagging, so the ONLY grant it withholds is the
        // source identity one: whatever the refusal names, it is that leg being refused.
        let (first_arn, second_arn) =
            chain_roles(&engine, index, &["sts:AssumeRole", "sts:TagSession"]);

        let (status, body) = call(
            &router,
            LONG_TERM_KEY,
            "AssumeRole",
            &[
                ("RoleArn", first_arn),
                ("RoleSessionName", "hop-one".to_string()),
                ("SourceIdentity", first_identity.to_string()),
            ],
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: the first hop: {status} {body}");
        let session_key = member(case, &body, "AccessKeyId");
        let session_arn = member(case, &body, "Arn");

        let mut chained = vec![
            ("RoleArn", second_arn.clone()),
            ("RoleSessionName", "hop-two".to_string()),
        ];
        if let Some(named) = chained_identity {
            chained.push(("SourceIdentity", named.to_string()));
        }
        let (status, body) = call(&router, &session_key, "AssumeRole", &chained).await;
        assert_eq!(
            status, 403,
            "case {case:?}: expected HTTP 403 for the chained assume, got {status} {body}"
        );
        assert_eq!(
            element(&body, "Code").as_deref(),
            Some("AccessDenied"),
            "case {case:?}: expected the AccessDenied code, got {body}"
        );
        let message = member(case, &body, "Message");
        assert_eq!(
            message,
            format!(
                "User: {session_arn} is not authorized to perform: {expected_action} on \
                 resource: {second_arn}"
            ),
            "case {case:?}: expected the refusal to name {expected_action}, got {message:?}"
        );
        assert!(
            session_arn.starts_with(&format!("arn:aws:sts::{account}:assumed-role/")),
            "case {case:?}: the refusal must name the calling SESSION, got {session_arn}"
        );
    }
}

/// A chained mint cannot change the source identity the calling session carries: a
/// different value is a validation error, the same value is served, and a session that
/// carries none can still have one set.
#[tokio::test]
async fn a_chained_mint_cannot_replace_the_source_identity_already_set() {
    // (case, the source identity the first hop sets, the one the chained request names,
    // the status the chained assume answers, the value the new session reports)
    type Row = (
        &'static str,
        Option<&'static str>,
        Option<&'static str>,
        u16,
        Option<&'static str>,
    );
    let rows: Vec<Row> = vec![
        (
            "a different value named mid-chain",
            Some("alice"),
            Some("bob"),
            400,
            None,
        ),
        (
            "a value differing only in case",
            Some("alice"),
            Some("Alice"),
            400,
            None,
        ),
        (
            "the same value named again",
            Some("alice"),
            Some("alice"),
            200,
            Some("alice"),
        ),
        (
            "no value named, so the carried one persists",
            Some("alice"),
            None,
            200,
            Some("alice"),
        ),
        (
            "a first hop that set none, so the chain may set one",
            None,
            Some("bob"),
            200,
            Some("bob"),
        ),
        (
            "neither hop naming one",
            None,
            None,
            200,
            None,
        ),
    ];

    for (index, (case, first_identity, chained_identity, expected_status, expected_reported)) in
        rows.into_iter().enumerate()
    {
        let (router, engine) = emulator();
        // Both roles allow every grant a chained mint could need, so the only thing that
        // can refuse a row is the rule under test.
        let (first_arn, second_arn) = chain_roles(
            &engine,
            100 + index,
            &["sts:AssumeRole", "sts:TagSession", "sts:SetSourceIdentity"],
        );

        let mut first = vec![
            ("RoleArn", first_arn),
            ("RoleSessionName", "hop-one".to_string()),
        ];
        if let Some(named) = first_identity {
            first.push(("SourceIdentity", named.to_string()));
        }
        let (status, body) = call(&router, LONG_TERM_KEY, "AssumeRole", &first).await;
        assert_eq!(status, 200, "case {case:?}: the first hop: {status} {body}");
        let session_key = member(case, &body, "AccessKeyId");

        let mut chained = vec![
            ("RoleArn", second_arn),
            ("RoleSessionName", "hop-two".to_string()),
        ];
        if let Some(named) = chained_identity {
            chained.push(("SourceIdentity", named.to_string()));
        }
        let (status, body) = call(&router, &session_key, "AssumeRole", &chained).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status} for the chained assume, got {status} \
             {body}"
        );
        if expected_status == 400 {
            assert_eq!(
                element(&body, "Code").as_deref(),
                Some("ValidationError"),
                "case {case:?}: expected the ValidationError code, got {body}"
            );
            assert_eq!(
                member(case, &body, "Message"),
                // Spelled out here, not read off the implementation's constant: the
                // wording IS the requirement, so a test that re-asserted the constant
                // would pass whatever the constant said.
                "The source identity is already set for this assume role session",
                "case {case:?}: expected AWS's own sentence for a source identity already set, \
                 got {body}"
            );
            assert!(
                element(&body, "AccessKeyId").is_none(),
                "case {case:?}: a refused chained mint must mint nothing, got {body}"
            );
            continue;
        }
        assert_eq!(
            element(&body, "SourceIdentity").as_deref(),
            expected_reported,
            "case {case:?}: expected the new session to report {expected_reported:?}, got {body}"
        );
        // The value the response reports is the value the session was recorded with, read
        // back through the credential it was minted under.
        let minted = member(case, &body, "AccessKeyId");
        let recorded = engine
            .state::<sts::ComAmazonawsStsAwssecuritytokenservicev20110615State>()
            .lock()
            .session(&minted)
            .and_then(|session| session.source_identity.clone());
        assert_eq!(
            recorded.as_deref(),
            expected_reported,
            "case {case:?}: expected the stored session to carry {expected_reported:?}, got \
             {recorded:?}"
        );
    }
}
