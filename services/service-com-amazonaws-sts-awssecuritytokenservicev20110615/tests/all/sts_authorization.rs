// SPDX-License-Identifier: CC0-1.0

//! What STS REFUSES, and who a credential speaks for.
//!
//! Four rules, each asserted over a table that spans the class it generalises over
//! rather than one recorded request:
//!
//! * a role session is minted only for a role the account HOLDS whose trust policy
//!   admits the calling principal for the action the request performs
//!   (`sts:AssumeRole`, plus `sts:TagSession` when it carries session tags and
//!   `sts:SetSourceIdentity` when it sets one) — and the refusal is the same
//!   `AccessDenied`/403 whether the role is absent or the policy withholds the grant,
//!   because STS does not disclose non-existence;
//! * the session's length is bounded by the role's own `MaxSessionDuration` and, when
//!   the call is itself made from a role session, by the one-hour role-chaining cap;
//! * each class of temporary credential may call only the STS operations its own
//!   documentation leaves it;
//! * `GetCallerIdentity` reports the identity of the credential that SIGNED the
//!   request — for an IAM user's own access key, that user's `UserId` and ARN as IAM
//!   holds them.
//!
//! Every call goes over the awsQuery wire through the same `MultiServiceRouter` the
//! emulator serves with; IAM's records are seeded in the one shared engine STS reads
//! them from.

use std::time::{Duration, SystemTime};

use service_com_amazonaws_iam_awsidentitymanagementv20100508 as iam;
use service_com_amazonaws_sts_awssecuritytokenservicev20110615 as sts;

use super::sts_sessions::{
    call, element, emulator, emulator_account, member, now_secs, seed_role, LONG_TERM_KEY,
};

/// A trust policy naming `principal` (whatever spelling) for `actions`.
fn trust_policy_for(principal: &str, actions: &[&str], condition: Option<&str>) -> String {
    let actions = actions
        .iter()
        .map(|action| format!("\"{action}\""))
        .collect::<Vec<_>>()
        .join(",");
    let condition = condition
        .map(|clause| format!(",\"Condition\":{clause}"))
        .unwrap_or_default();
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
         \"Principal\":{{\"AWS\":\"{principal}\"}},\"Action\":[{actions}]{condition}}}]}}"
    )
}

/// Record an IAM user and one of its access keys in the IAM store of the shared engine,
/// the way `iam:CreateUser` / `iam:CreateAccessKey` record them, and answer the user's
/// `UserId` and ARN.
fn seed_user(
    engine: &engine::InMemoryEngine,
    user_id: &str,
    user_name: &str,
    path: &str,
    access_key_id: &str,
) -> (String, String) {
    let arn = format!("arn:aws:iam::{}:user{path}{user_name}", emulator_account());
    let user = iam::ops::types::User {
        path: path.to_string(),
        user_name: user_name.to_string(),
        user_id: user_id.to_string(),
        arn: arn.clone(),
        create_date: aws_smithy_types::DateTime::from_secs(now_secs()),
        password_last_used: None,
        permissions_boundary: None,
        tags: None,
    };
    let record = iam::state::AccessKeyRecord {
        user_key: iam::state::name_key(user_name),
        status: iam::state::ACCESS_KEY_ACTIVE_STATUS.to_string(),
        create_date: aws_smithy_types::DateTime::from_secs(now_secs()),
    };
    let state = engine.state::<iam::ComAmazonawsIamAwsidentitymanagementv20100508State>();
    let mut iam_store = state.lock();
    iam_store
        .users
        .insert(iam::state::name_key(user_name), user);
    iam_store
        .access_keys
        .insert(access_key_id.to_string(), record);
    (user_id.to_string(), arn)
}

// ---------------------------------------------------------------------------
// Who may assume a role
// ---------------------------------------------------------------------------

/// A role session is minted only for a role the account HOLDS whose trust policy admits
/// the calling principal — and every refusal is the same `AccessDenied`/403, because STS
/// does not disclose whether the role exists.
///
/// The rows differ along what the rule generalises over: whether the account holds the
/// role at all, the path it holds it under, which principal spelling the policy names
/// (the account root, the bare account id, the caller's own ARN, the `"*"` wildcard,
/// another account's root), the effect, which actions it lists, whether it carries no
/// policy at all, and whether an `sts:ExternalId` condition is satisfied, violated or
/// unanswered.
#[tokio::test]
async fn a_role_is_assumed_only_when_the_account_holds_it_and_its_trust_policy_admits_the_caller() {
    let account = emulator_account();
    let caller_arn = format!("arn:aws:iam::{account}:user/emulator");
    let root = format!("arn:aws:iam::{account}:root");
    let external_id_clause = "{\"StringEquals\":{\"sts:ExternalId\":\"shared-secret-1\"}}";

    // (case, the trust policy to seed — `None` seeds no role at all, the path the
    // account holds the role under, the path the request's ARN names, the ExternalId the
    // request carries, expected status)
    type Row = (
        &'static str,
        Option<String>,
        &'static str,
        &'static str,
        Option<&'static str>,
        u16,
    );
    let rows: Vec<Row> = vec![
        (
            "the account root, which delegates to the whole account",
            Some(trust_policy_for(&root, &["sts:AssumeRole"], None)),
            "/",
            "/",
            None,
            200,
        ),
        (
            "the bare account id AWS accepts in the root's place",
            Some(trust_policy_for(&account, &["sts:AssumeRole"], None)),
            "/",
            "/",
            None,
            200,
        ),
        (
            "the caller's own ARN",
            Some(trust_policy_for(&caller_arn, &["sts:AssumeRole"], None)),
            "/",
            "/",
            None,
            200,
        ),
        (
            "every principal",
            Some(trust_policy_for("*", &["sts:AssumeRole"], None)),
            "/",
            "/",
            None,
            200,
        ),
        (
            "a role under a path, named by that path",
            Some(trust_policy_for(&root, &["sts:AssumeRole"], None)),
            "/service-role/",
            "/service-role/",
            None,
            200,
        ),
        (
            "an action wildcard covering the assume",
            Some(trust_policy_for(&root, &["sts:Assume*"], None)),
            "/",
            "/",
            None,
            200,
        ),
        (
            "a role the account does not hold",
            None,
            "/",
            "/",
            None,
            403,
        ),
        (
            "a role held under another path than the ARN names",
            Some(trust_policy_for(&root, &["sts:AssumeRole"], None)),
            "/service-role/",
            "/",
            None,
            403,
        ),
        (
            "a trust policy naming another account's root",
            Some(trust_policy_for(
                "arn:aws:iam::111122223333:root",
                &["sts:AssumeRole"],
                None,
            )),
            "/",
            "/",
            None,
            403,
        ),
        (
            "a trust policy listing only another action",
            Some(trust_policy_for(&root, &["sts:TagSession"], None)),
            "/",
            "/",
            None,
            403,
        ),
        (
            "an explicit deny of the assume",
            Some(
                trust_policy_for(&root, &["sts:AssumeRole"], None).replace("\"Allow\"", "\"Deny\""),
            ),
            "/",
            "/",
            None,
            403,
        ),
        ("a role carrying no trust policy at all", Some(String::new()), "/", "/", None, 403),
        (
            "an ExternalId condition the request satisfies",
            Some(trust_policy_for(
                &root,
                &["sts:AssumeRole"],
                Some(external_id_clause),
            )),
            "/",
            "/",
            Some("shared-secret-1"),
            200,
        ),
        (
            "an ExternalId condition the request contradicts",
            Some(trust_policy_for(
                &root,
                &["sts:AssumeRole"],
                Some(external_id_clause),
            )),
            "/",
            "/",
            Some("shared-secret-2"),
            403,
        ),
        (
            "an ExternalId condition the request does not answer",
            Some(trust_policy_for(
                &root,
                &["sts:AssumeRole"],
                Some(external_id_clause),
            )),
            "/",
            "/",
            None,
            403,
        ),
    ];

    for (index, (case, seeded, held_path, named_path, external_id, expected_status)) in
        rows.into_iter().enumerate()
    {
        let (router, engine) = emulator();
        let role_name = "probe";
        if let Some(document) = &seeded {
            seed_role(
                &engine,
                &format!("AROASEEDEDTRUST{index:06}"),
                role_name,
                held_path,
                (!document.is_empty()).then_some(document.as_str()),
                None,
            );
        }
        let requested = format!("arn:aws:iam::{account}:role{named_path}{role_name}");
        let mut params = vec![
            ("RoleArn", requested.clone()),
            ("RoleSessionName", "probe-session".to_string()),
        ];
        if let Some(external_id) = external_id {
            params.push(("ExternalId", external_id.to_string()));
        }
        let (status, body) = call(&router, LONG_TERM_KEY, "AssumeRole", &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} {body}"
        );
        if expected_status == 403 {
            assert_eq!(
                element(&body, "Code").as_deref(),
                Some("AccessDenied"),
                "case {case:?}: expected AccessDenied, got {body}"
            );
            assert_eq!(
                element(&body, "Message").as_deref(),
                Some(
                    format!(
                        "User: {caller_arn} is not authorized to perform: sts:AssumeRole on \
                         resource: {requested}"
                    )
                    .as_str()
                ),
                "case {case:?}: the refusal must name the principal, the action and the role, \
                 got {body}"
            );
            assert!(
                element(&body, "AccessKeyId").is_none(),
                "case {case:?}: a refused assume must mint nothing, got {body}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Session tags and source identity are their own grants
// ---------------------------------------------------------------------------

/// Carrying session tags, and setting a source identity, are each their OWN action in
/// the role's trust policy: a policy that allows only `sts:AssumeRole` admits the plain
/// assume and refuses both.
///
/// The rows differ in which actions the policy lists and which of the two the request
/// performs, and span a tag value outside ASCII (tag values are text, so the grant does
/// not depend on the bytes) and several tags at once.
#[tokio::test]
async fn session_tags_and_a_source_identity_each_need_their_own_grant() {
    let account = emulator_account();
    let root = format!("arn:aws:iam::{account}:root");

    // (case, actions the trust policy lists, the tags the request carries, the source
    // identity it sets, expected status, the action the refusal must name)
    type Row = (
        &'static str,
        &'static [&'static str],
        &'static [(&'static str, &'static str)],
        Option<&'static str>,
        u16,
        Option<&'static str>,
    );
    let rows: Vec<Row> = vec![
        (
            "a plain assume under an assume-only policy",
            &["sts:AssumeRole"],
            &[],
            None,
            200,
            None,
        ),
        (
            "one session tag under an assume-only policy",
            &["sts:AssumeRole"],
            &[("team", "platform")],
            None,
            403,
            Some("sts:TagSession"),
        ),
        (
            "one session tag with the tagging grant",
            &["sts:AssumeRole", "sts:TagSession"],
            &[("team", "platform")],
            None,
            200,
            None,
        ),
        (
            "several session tags with the tagging grant",
            &["sts:AssumeRole", "sts:TagSession"],
            &[("team", "platform"), ("tier", "gold"), ("env", "prod")],
            None,
            200,
            None,
        ),
        (
            "a tag value outside ASCII with the tagging grant",
            &["sts:AssumeRole", "sts:TagSession"],
            &[("owner", "Ünïcödé-ヴァリュー")],
            None,
            200,
            None,
        ),
        (
            "a tag value outside ASCII without it",
            &["sts:AssumeRole"],
            &[("owner", "Ünïcödé-ヴァリュー")],
            None,
            403,
            Some("sts:TagSession"),
        ),
        (
            "a source identity under an assume-only policy",
            &["sts:AssumeRole"],
            &[],
            Some("alice"),
            403,
            Some("sts:SetSourceIdentity"),
        ),
        (
            "a source identity with the source-identity grant",
            &["sts:AssumeRole", "sts:SetSourceIdentity"],
            &[],
            Some("alice"),
            200,
            None,
        ),
        (
            "both, with only the tagging grant",
            &["sts:AssumeRole", "sts:TagSession"],
            &[("team", "platform")],
            Some("alice"),
            403,
            Some("sts:SetSourceIdentity"),
        ),
        (
            "both, with both grants",
            &["sts:AssumeRole", "sts:TagSession", "sts:SetSourceIdentity"],
            &[("team", "platform")],
            Some("bob@example.com"),
            200,
            None,
        ),
    ];

    for (index, (case, actions, tags, source_identity, expected_status, refused_action)) in
        rows.into_iter().enumerate()
    {
        let (router, engine) = emulator();
        let (role_arn, _role_id) = seed_role(
            &engine,
            &format!("AROASEEDEDGRANT{index:06}"),
            "tagged",
            "/",
            Some(&trust_policy_for(&root, actions, None)),
            None,
        );
        let mut params = vec![
            ("RoleArn", role_arn.clone()),
            ("RoleSessionName", "probe-session".to_string()),
        ];
        for (position, (key, value)) in tags.iter().enumerate() {
            let at = position + 1;
            params.push((
                Box::leak(format!("Tags.member.{at}.Key").into_boxed_str()),
                (*key).to_string(),
            ));
            params.push((
                Box::leak(format!("Tags.member.{at}.Value").into_boxed_str()),
                (*value).to_string(),
            ));
        }
        if let Some(value) = source_identity {
            params.push(("SourceIdentity", value.to_string()));
        }

        let (status, body) = call(&router, LONG_TERM_KEY, "AssumeRole", &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} {body}"
        );
        match refused_action {
            Some(action) => {
                assert_eq!(
                    element(&body, "Code").as_deref(),
                    Some("AccessDenied"),
                    "case {case:?}: expected AccessDenied, got {body}"
                );
                let message = element(&body, "Message").unwrap_or_default();
                assert!(
                    message.contains(action),
                    "case {case:?}: the refusal must name {action}, got {message:?}"
                );
            }
            None => {
                // The session was minted, and what the request set is what it reports.
                assert_eq!(
                    element(&body, "SourceIdentity").as_deref(),
                    source_identity,
                    "case {case:?}: expected SourceIdentity {source_identity:?} in {body}"
                );
                let access_key_id = member(case, &body, "AccessKeyId");
                let held = engine
                    .state::<sts::state::ComAmazonawsStsAwssecuritytokenservicev20110615State>();
                let recorded: Vec<(String, String)> = held
                    .lock()
                    .session(&access_key_id)
                    .map(|session| session.tags.clone())
                    .unwrap_or_default();
                let expected: Vec<(String, String)> = tags
                    .iter()
                    .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                    .collect();
                assert_eq!(
                    recorded, expected,
                    "case {case:?}: the session must carry exactly the tags the request named"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Role chaining
// ---------------------------------------------------------------------------

/// A session's length is bounded by the role's own `MaxSessionDuration`, and a
/// role-CHAINED assume by the documented one-hour cap; a request past either bound is
/// refused with `ValidationError`/400 and mints nothing. The `PolicyArns` count is
/// bounded the same way.
///
/// The rows put the request on each side of each bound — the role's maximum (stored, and
/// absent so the default applies), the chaining cap, and the managed-session-policy
/// count — including the bound itself.
#[tokio::test]
async fn a_session_is_refused_past_the_roles_maximum_the_chaining_cap_and_the_policy_count() {
    let account = emulator_account();
    let root = format!("arn:aws:iam::{account}:root");
    let trust = trust_policy_for(&root, &["sts:AssumeRole"], None);

    // (case, the role's stored MaxSessionDuration, the requested duration, whether the
    // assume is itself made from a role session, how many PolicyArns it names, expected
    // status)
    type Row = (&'static str, Option<i32>, Option<i32>, bool, usize, u16);
    let rows: Vec<Row> = vec![
        ("a role maximum of one hour, asking for it", Some(3600), Some(3600), false, 0, 200),
        ("a role maximum of one hour, asking for a second more", Some(3600), Some(3601), false, 0, 400),
        ("a role maximum of twelve hours, asking for it", Some(43_200), Some(43_200), false, 0, 200),
        ("a role maximum of two hours, asking for one second more", Some(7200), Some(7201), false, 0, 400),
        ("no stored maximum, asking for the default hour", None, Some(3600), false, 0, 200),
        ("no stored maximum, asking past the default hour", None, Some(3601), false, 0, 400),
        ("no stored maximum, asking for nothing", None, None, false, 0, 200),
        ("a chained assume asking for the one-hour cap", Some(43_200), Some(3600), true, 0, 200),
        ("a chained assume asking for a second past it", Some(43_200), Some(3601), true, 0, 400),
        ("a chained assume asking for twelve hours", Some(43_200), Some(43_200), true, 0, 400),
        ("an unchained assume asking for twelve hours", Some(43_200), Some(43_200), false, 0, 200),
        ("the ten managed session policies the operation accepts", Some(43_200), None, false, 10, 200),
        ("one managed session policy too many", Some(43_200), None, false, 11, 400),
    ];

    for (index, (case, role_max, requested, chained, policy_count, expected_status)) in
        rows.into_iter().enumerate()
    {
        let (router, engine) = emulator();
        let (role_arn, _role_id) = seed_role(
            &engine,
            &format!("AROASEEDEDBOUND{index:06}"),
            "bounded",
            "/",
            Some(&trust),
            role_max,
        );
        // A chained assume is one made with the credential of a role session, so the
        // chain starts from a session this very role minted.
        let caller_key = if chained {
            let (status, body) = call(
                &router,
                LONG_TERM_KEY,
                "AssumeRole",
                &[
                    ("RoleArn", role_arn.clone()),
                    ("RoleSessionName", "first".to_string()),
                ],
            )
            .await;
            assert_eq!(status, 200, "case {case:?}: the first assume: {body}");
            member(case, &body, "AccessKeyId")
        } else {
            LONG_TERM_KEY.to_string()
        };

        let mut params = vec![
            ("RoleArn", role_arn.clone()),
            ("RoleSessionName", "second".to_string()),
        ];
        if let Some(duration) = requested {
            params.push(("DurationSeconds", duration.to_string()));
        }
        for position in 1..=policy_count {
            params.push((
                Box::leak(format!("PolicyArns.member.{position}.arn").into_boxed_str()),
                format!("arn:aws:iam::{account}:policy/session-{position}"),
            ));
        }

        let (status, body) = call(&router, &caller_key, "AssumeRole", &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} {body}"
        );
        if expected_status == 400 {
            assert_eq!(
                element(&body, "Code").as_deref(),
                Some("ValidationError"),
                "case {case:?}: expected a ValidationError, got {body}"
            );
            assert!(
                element(&body, "AccessKeyId").is_none(),
                "case {case:?}: a refused assume must mint nothing, got {body}"
            );
        }
    }
}

/// A transitive session tag passes to the next session in a role chain, and so does the
/// source identity — and each needs, on the TARGET role's trust policy, the grant for the
/// value this mint SETS on the new session: `sts:TagSession` for the tags it sets, carried
/// forward or not, and `sts:SetSourceIdentity` for the source identity it sets, named by
/// the chained request or carried by the calling session.
///
/// The rows differ in which actions the target's trust policy lists and whether the
/// chained request names a source identity of its own.
#[tokio::test]
async fn transitive_tags_and_a_source_identity_travel_the_chain_under_the_targets_grants() {
    let account = emulator_account();
    let root = format!("arn:aws:iam::{account}:root");

    // (case, the actions the TARGET role's trust policy lists, the source identity the
    // CHAINED request names, expected status of the chained assume, the action its
    // refusal must name)
    type Row = (
        &'static str,
        &'static [&'static str],
        Option<&'static str>,
        u16,
        Option<&'static str>,
    );
    let rows: [Row; 6] = [
        (
            "a target role that allows every grant the chained mint could need",
            &["sts:AssumeRole", "sts:TagSession", "sts:SetSourceIdentity"],
            None,
            200,
            None,
        ),
        (
            "a target role that allows only the assume",
            &["sts:AssumeRole"],
            None,
            403,
            Some("sts:TagSession"),
        ),
        (
            "a target role that allows the assume and the source identity only",
            &["sts:AssumeRole", "sts:SetSourceIdentity"],
            None,
            403,
            Some("sts:TagSession"),
        ),
        (
            "a carried source identity against a target that allows the assume and tagging",
            &["sts:AssumeRole", "sts:TagSession"],
            None,
            403,
            Some("sts:SetSourceIdentity"),
        ),
        (
            "a chained request that names a source identity without that grant",
            &["sts:AssumeRole", "sts:TagSession"],
            Some("alice"),
            403,
            Some("sts:SetSourceIdentity"),
        ),
        (
            "a chained request that names a source identity with that grant",
            &["sts:AssumeRole", "sts:TagSession", "sts:SetSourceIdentity"],
            Some("alice"),
            200,
            None,
        ),
    ];

    for (index, (case, target_actions, chained_source_identity, expected_status, refused_action)) in
        rows.into_iter().enumerate()
    {
        let (router, engine) = emulator();
        let (first_arn, _) = seed_role(
            &engine,
            &format!("AROASEEDEDCHAIN1{index:05}"),
            "first",
            "/",
            Some(&trust_policy_for(
                &root,
                &["sts:AssumeRole", "sts:TagSession", "sts:SetSourceIdentity"],
                None,
            )),
            Some(43_200),
        );
        let (second_arn, second_id) = seed_role(
            &engine,
            &format!("AROASEEDEDCHAIN2{index:05}"),
            "second",
            "/",
            Some(&trust_policy_for(&root, target_actions, None)),
            Some(43_200),
        );

        let (status, body) = call(
            &router,
            LONG_TERM_KEY,
            "AssumeRole",
            &[
                ("RoleArn", first_arn),
                ("RoleSessionName", "hop-one".to_string()),
                ("SourceIdentity", "alice".to_string()),
                ("Tags.member.1.Key", "project".to_string()),
                ("Tags.member.1.Value", "atlas".to_string()),
                ("Tags.member.2.Key", "scratch".to_string()),
                ("Tags.member.2.Value", "ignored".to_string()),
                ("TransitiveTagKeys.member.1", "project".to_string()),
            ],
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: the first assume: {body}");
        assert_eq!(
            element(&body, "SourceIdentity").as_deref(),
            Some("alice"),
            "case {case:?}: the first session must echo its source identity: {body}"
        );
        let first_key = member(case, &body, "AccessKeyId");

        let mut chained = vec![
            ("RoleArn", second_arn),
            ("RoleSessionName", "hop-two".to_string()),
        ];
        if let Some(named) = chained_source_identity {
            chained.push(("SourceIdentity", named.to_string()));
        }
        let (status, body) = call(&router, &first_key, "AssumeRole", &chained).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status} for the chained assume, got {status} \
             {body}"
        );
        if let Some(action) = refused_action {
            let message = element(&body, "Message").unwrap_or_default();
            assert!(
                element(&body, "Code").as_deref() == Some("AccessDenied")
                    && message.contains(action),
                "case {case:?}: the chained assume must be refused for {action}, got {body}"
            );
            continue;
        }

        // The source identity persisted without being asked for again, and the
        // transitive tag came along while the non-transitive one did not.
        assert_eq!(
            element(&body, "SourceIdentity").as_deref(),
            Some("alice"),
            "case {case:?}: the source identity must persist across the chain: {body}"
        );
        assert_eq!(
            member(case, &body, "AssumedRoleId"),
            format!("{second_id}:hop-two"),
            "case {case:?}: the chained session must carry the target role's own id: {body}"
        );
        let second_key = member(case, &body, "AccessKeyId");
        let held =
            engine.state::<sts::state::ComAmazonawsStsAwssecuritytokenservicev20110615State>();
        let session = held
            .lock()
            .session(&second_key)
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: the chained session was not recorded"));
        assert_eq!(
            session.tags,
            vec![("project".to_string(), "atlas".to_string())],
            "case {case:?}: only the transitive tag passes to the next session"
        );
        assert_eq!(
            session.transitive_tag_keys,
            vec!["project".to_string()],
            "case {case:?}: a carried tag stays transitive for the next hop"
        );
    }
}

/// A session tag key the calling session carries FORWARD is already set on the session a
/// chained mint continues, so a request that passes that key again is refused — and the
/// refusal is the one a repeated tag key gets, `InvalidParameterValue` with HTTP 400,
/// naming the key it collided on.
///
/// The rows differ in which key the chained request passes: the carried one, the same key
/// in another case (tag keys are compared without case), a key the first session holds
/// WITHOUT carrying it forward, and a key neither session holds — only the first two of
/// which collide with an inherited tag.
#[tokio::test]
async fn a_tag_key_the_calling_session_carries_forward_cannot_be_passed_again() {
    let account = emulator_account();
    let root = format!("arn:aws:iam::{account}:root");

    // (case, the tag key the chained request passes, expected status, the key the refusal
    // must name)
    let rows: [(&str, &str, u16, Option<&str>); 4] = [
        (
            "the carried key itself",
            "project",
            400,
            Some("project"),
        ),
        (
            "the carried key in another case",
            "PROJECT",
            400,
            Some("project"),
        ),
        (
            "a key the first session holds but does not carry forward",
            "scratch",
            200,
            None,
        ),
        ("a key neither session holds", "region", 200, None),
    ];

    for (index, (case, passed_key, expected_status, collided)) in rows.into_iter().enumerate() {
        let (router, engine) = emulator();
        let grants = &["sts:AssumeRole", "sts:TagSession"];
        let (first_arn, _) = seed_role(
            &engine,
            &format!("AROASEEDEDINHER1{index:05}"),
            "first",
            "/",
            Some(&trust_policy_for(&root, grants, None)),
            Some(43_200),
        );
        let (second_arn, _) = seed_role(
            &engine,
            &format!("AROASEEDEDINHER2{index:05}"),
            "second",
            "/",
            Some(&trust_policy_for(&root, grants, None)),
            Some(43_200),
        );

        let (status, body) = call(
            &router,
            LONG_TERM_KEY,
            "AssumeRole",
            &[
                ("RoleArn", first_arn),
                ("RoleSessionName", "hop-one".to_string()),
                ("Tags.member.1.Key", "project".to_string()),
                ("Tags.member.1.Value", "atlas".to_string()),
                ("Tags.member.2.Key", "scratch".to_string()),
                ("Tags.member.2.Value", "local".to_string()),
                ("TransitiveTagKeys.member.1", "project".to_string()),
            ],
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: the first assume: {body}");
        let first_key = member(case, &body, "AccessKeyId");

        let (status, body) = call(
            &router,
            &first_key,
            "AssumeRole",
            &[
                ("RoleArn", second_arn),
                ("RoleSessionName", "hop-two".to_string()),
                ("Tags.member.1.Key", passed_key.to_string()),
                ("Tags.member.1.Value", "rewritten".to_string()),
            ],
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status} for the chained assume, got {status} \
             {body}"
        );
        match collided {
            Some(key) => {
                let message = element(&body, "Message").unwrap_or_default();
                assert!(
                    element(&body, "Code").as_deref() == Some("InvalidParameterValue")
                        && message.contains(key),
                    "case {case:?}: the refusal must be InvalidParameterValue naming {key:?}, \
                     got {body}"
                );
                assert!(
                    element(&body, "AccessKeyId").is_none(),
                    "case {case:?}: a refused assume must mint nothing, got {body}"
                );
            }
            None => {
                let second_key = member(case, &body, "AccessKeyId");
                let held = engine
                    .state::<sts::state::ComAmazonawsStsAwssecuritytokenservicev20110615State>();
                let session = held.lock().session(&second_key).cloned().unwrap_or_else(|| {
                    panic!("case {case:?}: the chained session was not recorded")
                });
                assert!(
                    session
                        .tags
                        .iter()
                        .any(|(key, value)| key == passed_key && value == "rewritten"),
                    "case {case:?}: the request's own tag must be set, got {:?}",
                    session.tags
                );
                assert!(
                    session
                        .tags
                        .iter()
                        .any(|(key, value)| key == "project" && value == "atlas"),
                    "case {case:?}: the carried tag must still pass forward, got {:?}",
                    session.tags
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The condition keys a mint publishes
// ---------------------------------------------------------------------------

/// A trust policy's `Condition` is decided against the keys the MINT determines, so a
/// role may be scoped to a session name, a source identity or a set of session tags.
///
/// The rows differ in which key the clause names and whether the request satisfies it,
/// and span a multi-valued key under IAM's `ForAllValues:` set qualifier beside the
/// single-valued keys.
#[tokio::test]
async fn a_trust_policy_condition_is_decided_against_the_keys_the_mint_determines() {
    let account = emulator_account();
    let root = format!("arn:aws:iam::{account}:root");

    // (case, the condition clause, the session name, the source identity, the session
    // tags, the transitive tag keys, expected status)
    type Row = (
        &'static str,
        &'static str,
        &'static str,
        Option<&'static str>,
        &'static [(&'static str, &'static str)],
        &'static [&'static str],
        u16,
    );
    let rows: Vec<Row> = vec![
        (
            "a session name the request matches",
            "{\"StringEquals\":{\"sts:RoleSessionName\":\"audited\"}}",
            "audited",
            None,
            &[],
            &[],
            200,
        ),
        (
            "a session name the request does not match",
            "{\"StringEquals\":{\"sts:RoleSessionName\":\"audited\"}}",
            "other",
            None,
            &[],
            &[],
            403,
        ),
        (
            "a session-name pattern the request matches",
            "{\"StringLike\":{\"sts:RoleSessionName\":\"audit-*\"}}",
            "audit-42",
            None,
            &[],
            &[],
            200,
        ),
        (
            "a source identity the request matches",
            "{\"StringEquals\":{\"sts:SourceIdentity\":\"alice\"}}",
            "s1",
            Some("alice"),
            &[],
            &[],
            200,
        ),
        (
            "a source identity the request contradicts",
            "{\"StringEquals\":{\"sts:SourceIdentity\":\"alice\"}}",
            "s1",
            Some("bob"),
            &[],
            &[],
            403,
        ),
        (
            "a request tag value the request matches",
            "{\"StringEquals\":{\"aws:RequestTag/project\":\"atlas\"}}",
            "s1",
            None,
            &[("project", "atlas")],
            &[],
            200,
        ),
        (
            "a request tag value the request contradicts",
            "{\"StringEquals\":{\"aws:RequestTag/project\":\"atlas\"}}",
            "s1",
            None,
            &[("project", "borealis")],
            &[],
            403,
        ),
        (
            "every transitive tag key the clause admits",
            "{\"ForAllValues:StringEquals\":{\"sts:TransitiveTagKeys\":[\"project\",\"tier\"]}}",
            "s1",
            None,
            &[("project", "atlas"), ("tier", "gold")],
            &["project", "tier"],
            200,
        ),
        (
            "a transitive tag key the clause does not admit",
            "{\"ForAllValues:StringEquals\":{\"sts:TransitiveTagKeys\":[\"project\"]}}",
            "s1",
            None,
            &[("project", "atlas"), ("tier", "gold")],
            &["project", "tier"],
            403,
        ),
        (
            "an operator this emulator cannot decide, which grants nothing",
            "{\"NumericLessThan\":{\"aws:MultiFactorAuthAge\":\"300\"}}",
            "s1",
            None,
            &[],
            &[],
            403,
        ),
    ];

    for (index, (case, condition, session_name, source_identity, tags, transitive, expected_status)) in
        rows.into_iter().enumerate()
    {
        let (router, engine) = emulator();
        let (role_arn, _role_id) = seed_role(
            &engine,
            &format!("AROASEEDEDCOND{index:07}"),
            "conditioned",
            "/",
            Some(&trust_policy_for(
                &root,
                &["sts:AssumeRole", "sts:TagSession", "sts:SetSourceIdentity"],
                Some(condition),
            )),
            Some(43_200),
        );
        let mut params = vec![
            ("RoleArn", role_arn),
            ("RoleSessionName", session_name.to_string()),
        ];
        if let Some(value) = source_identity {
            params.push(("SourceIdentity", value.to_string()));
        }
        for (position, (key, value)) in tags.iter().enumerate() {
            let at = position + 1;
            params.push((
                Box::leak(format!("Tags.member.{at}.Key").into_boxed_str()),
                (*key).to_string(),
            ));
            params.push((
                Box::leak(format!("Tags.member.{at}.Value").into_boxed_str()),
                (*value).to_string(),
            ));
        }
        for (position, key) in transitive.iter().enumerate() {
            let at = position + 1;
            params.push((
                Box::leak(format!("TransitiveTagKeys.member.{at}").into_boxed_str()),
                (*key).to_string(),
            ));
        }

        let (status, body) = call(&router, LONG_TERM_KEY, "AssumeRole", &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} {body}"
        );
        if expected_status == 403 {
            assert_eq!(
                element(&body, "Code").as_deref(),
                Some("AccessDenied"),
                "case {case:?}: expected AccessDenied, got {body}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The constraints a session tag must satisfy
// ---------------------------------------------------------------------------

/// Naming a transitive tag KEY is part of the same act as passing a tag, so it needs the
/// `sts:TagSession` grant too — even when the request names no tag of its own.
#[tokio::test]
async fn naming_a_transitive_tag_key_alone_still_needs_the_tagging_grant() {
    let account = emulator_account();
    let root = format!("arn:aws:iam::{account}:root");

    // (case, the actions the trust policy lists, expected status)
    for (index, (case, actions, expected_status)) in [
        ("an assume-only policy", &["sts:AssumeRole"][..], 403),
        (
            "a policy that allows the tagging",
            &["sts:AssumeRole", "sts:TagSession"][..],
            200,
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let (router, engine) = emulator();
        let (role_arn, _role_id) = seed_role(
            &engine,
            &format!("AROASEEDEDTRANSONLY{index:02}"),
            "transitive-only",
            "/",
            Some(&trust_policy_for(&root, actions, None)),
            Some(43_200),
        );
        let (status, body) = call(
            &router,
            LONG_TERM_KEY,
            "AssumeRole",
            &[
                ("RoleArn", role_arn),
                ("RoleSessionName", "probe".to_string()),
                ("TransitiveTagKeys.member.1", "project".to_string()),
            ],
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} {body}"
        );
        if expected_status == 403 {
            let message = element(&body, "Message").unwrap_or_default();
            assert!(
                element(&body, "Code").as_deref() == Some("AccessDenied")
                    && message.contains("sts:TagSession"),
                "case {case:?}: the refusal must name sts:TagSession, got {body}"
            );
        }
    }
}

/// A session tag's key and value are refused outside what the model admits — the
/// documented 128-character key and 256-character value, both over one character class —
/// with `ValidationError`/400, and nothing is minted.
///
/// The rows sit on each side of each published bound, counted in CHARACTERS (so a value
/// of multi-byte characters is admitted at the same count as an ASCII one) and span the
/// character class on both sides.
#[tokio::test]
async fn a_session_tag_outside_the_modelled_key_and_value_constraints_is_refused() {
    let account = emulator_account();
    let root = format!("arn:aws:iam::{account}:root");

    // The published bounds: `tagKeyType` (length 1..128) and `tagValueType`
    // (length 0..256), this service's model.
    let key_max = sts::state::MAX_TAG_KEY_CHARS;
    let value_max = sts::state::MAX_TAG_VALUE_CHARS;
    let rows: Vec<(String, String, String, u16)> = vec![
        ("a key at the maximum".to_string(), "k".repeat(key_max), "v".to_string(), 200),
        ("a key one past the maximum".to_string(), "k".repeat(key_max + 1), "v".to_string(), 400),
        ("an empty key, under the minimum".to_string(), String::new(), "v".to_string(), 400),
        ("a value at the maximum".to_string(), "k".to_string(), "v".repeat(value_max), 200),
        ("a value one past the maximum".to_string(), "k".to_string(), "v".repeat(value_max + 1), 400),
        ("an empty value, which the model admits".to_string(), "k".to_string(), String::new(), 200),
        (
            "a value of multi-byte characters at the maximum, counted in characters".to_string(),
            "k".to_string(),
            "é".repeat(value_max),
            200,
        ),
        (
            "a value of multi-byte characters one past it".to_string(),
            "k".to_string(),
            "é".repeat(value_max + 1),
            400,
        ),
        (
            "letters, separators, numbers and the punctuation the class admits".to_string(),
            "Ünïcödé_.:/=+-@".to_string(),
            "ヴァリュー 42".to_string(),
            200,
        ),
        (
            "a value outside the class".to_string(),
            "k".to_string(),
            "no\u{1F389}party".to_string(),
            400,
        ),
        (
            "a key outside the class".to_string(),
            "no\u{1F389}party".to_string(),
            "v".to_string(),
            400,
        ),
    ];

    for (index, (case, key, value, expected_status)) in rows.into_iter().enumerate() {
        let (router, engine) = emulator();
        let (role_arn, _role_id) = seed_role(
            &engine,
            &format!("AROASEEDEDTAGLIM{index:05}"),
            "tag-limits",
            "/",
            Some(&trust_policy_for(
                &root,
                &["sts:AssumeRole", "sts:TagSession"],
                None,
            )),
            Some(43_200),
        );
        let (status, body) = call(
            &router,
            LONG_TERM_KEY,
            "AssumeRole",
            &[
                ("RoleArn", role_arn),
                ("RoleSessionName", "tagged".to_string()),
                ("Tags.member.1.Key", key),
                ("Tags.member.1.Value", value),
            ],
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} {body}"
        );
        if expected_status == 400 {
            assert_eq!(
                element(&body, "Code").as_deref(),
                Some("ValidationError"),
                "case {case:?}: expected a ValidationError, got {body}"
            );
            assert!(
                element(&body, "AccessKeyId").is_none(),
                "case {case:?}: a refused mint must leave no credential, got {body}"
            );
        }
    }
}

/// One session holds at most the documented number of session tags, and a role chain
/// cannot grow a stored session past it: the bound is checked over the EFFECTIVE set —
/// the tags the request names PLUS the transitive tags the calling session carries
/// forward — so the record the chain writes is bounded however the chain is extended.
///
/// The rows sit on each side of the published maximum, both for one request's own tags
/// and for a chained request whose own tags would push the carried set past it.
#[tokio::test]
async fn one_session_holds_at_most_the_documented_number_of_tags_however_the_chain_grows() {
    let account = emulator_account();
    let root = format!("arn:aws:iam::{account}:root");
    let max = sts::state::MAX_SESSION_TAGS;

    // (case, how many tags the first assume passes and marks transitive, how many the
    // chained assume passes of its own, expected status of the chained assume)
    let rows: [(&str, usize, usize, u16); 4] = [
        ("the maximum, carried forward, nothing added", max, 0, 200),
        ("half carried forward, half added", max / 2, max - max / 2, 200),
        ("the maximum carried forward, one added", max, 1, 400),
        ("half carried forward, one too many added", max / 2, max - max / 2 + 1, 400),
    ];

    for (index, (case, carried, added, expected_status)) in rows.into_iter().enumerate() {
        let (router, engine) = emulator();
        let (role_arn, _role_id) = seed_role(
            &engine,
            &format!("AROASEEDEDTAGCAP{index:05}"),
            "tag-cap",
            "/",
            Some(&trust_policy_for(
                &root,
                &["sts:AssumeRole", "sts:TagSession"],
                None,
            )),
            Some(43_200),
        );

        let mut first = vec![
            ("RoleArn", role_arn.clone()),
            ("RoleSessionName", "hop-one".to_string()),
        ];
        for position in 1..=carried {
            first.push((
                Box::leak(format!("Tags.member.{position}.Key").into_boxed_str()),
                format!("carried{position}"),
            ));
            first.push((
                Box::leak(format!("Tags.member.{position}.Value").into_boxed_str()),
                "v".to_string(),
            ));
            first.push((
                Box::leak(format!("TransitiveTagKeys.member.{position}").into_boxed_str()),
                format!("carried{position}"),
            ));
        }
        let (status, body) = call(&router, LONG_TERM_KEY, "AssumeRole", &first).await;
        assert_eq!(status, 200, "case {case:?}: the first assume: {body}");
        let first_key = member(case, &body, "AccessKeyId");

        let mut second = vec![
            ("RoleArn", role_arn),
            ("RoleSessionName", "hop-two".to_string()),
        ];
        for position in 1..=added {
            second.push((
                Box::leak(format!("Tags.member.{position}.Key").into_boxed_str()),
                format!("added{position}"),
            ));
            second.push((
                Box::leak(format!("Tags.member.{position}.Value").into_boxed_str()),
                "v".to_string(),
            ));
        }
        let (status, body) = call(&router, &first_key, "AssumeRole", &second).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status} for the chained assume, got {status} \
             {body}"
        );
        if expected_status == 400 {
            assert_eq!(
                element(&body, "Code").as_deref(),
                Some("ValidationError"),
                "case {case:?}: expected a ValidationError, got {body}"
            );
            assert!(
                element(&body, "AccessKeyId").is_none(),
                "case {case:?}: a refused mint must leave no credential, got {body}"
            );
        } else {
            let held = engine
                .state::<sts::state::ComAmazonawsStsAwssecuritytokenservicev20110615State>();
            let recorded = held
                .lock()
                .session(&member(case, &body, "AccessKeyId"))
                .map(|session| (session.tags.len(), session.transitive_tag_keys.len()))
                .unwrap_or_else(|| panic!("case {case:?}: the chained session was not recorded"));
            assert!(
                recorded.0 <= max && recorded.1 <= max,
                "case {case:?}: the recorded session holds {recorded:?} tags/transitive keys, \
                 past the {max} a session may hold"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Which operations a credential's class may call
// ---------------------------------------------------------------------------
/// Each class of credential may call only the STS operations its own documentation
/// leaves it: a long-term key may call them all, an `AssumeRole` session may not call
/// `GetSessionToken` or `GetFederationToken`, a `GetSessionToken` session may call only
/// `AssumeRole` and `GetCallerIdentity`, and a `GetFederationToken` session only
/// `GetCallerIdentity`. Everything else is `AccessDenied`/403.
#[tokio::test]
async fn a_credentials_class_bounds_which_sts_operations_it_may_call() {
    let (router, engine) = emulator();
    let account = emulator_account();
    let root = format!("arn:aws:iam::{account}:root");
    let (role_arn, _role_id) = seed_role(
        &engine,
        "AROASEEDEDCLASS000001",
        "callable",
        "/",
        Some(&trust_policy_for(&root, &["sts:AssumeRole"], None)),
        Some(43_200),
    );

    // One credential of each class, minted the way a client mints it.
    let assume = vec![
        ("RoleArn", role_arn.clone()),
        ("RoleSessionName", "class-probe".to_string()),
    ];
    let mint = |action: &'static str, params: Vec<(&'static str, String)>| {
        let router = &router;
        async move {
            let (status, body) = call(router, LONG_TERM_KEY, action, &params).await;
            assert_eq!(status, 200, "minting with {action}: {body}");
            member(action, &body, "AccessKeyId")
        }
    };
    let role_key = mint("AssumeRole", assume.clone()).await;
    let session_key = mint("GetSessionToken", vec![]).await;
    let federation_key = mint(
        "GetFederationToken",
        vec![("Name", "broker".to_string())],
    )
    .await;

    // (case, the credential, the operation, its parameters, expected status)
    type Probe = (&'static str, String, &'static str, Vec<(&'static str, String)>, u16);
    let probes: Vec<Probe> = vec![
        ("a long-term key: AssumeRole", LONG_TERM_KEY.to_string(), "AssumeRole", assume.clone(), 200),
        ("a long-term key: GetSessionToken", LONG_TERM_KEY.to_string(), "GetSessionToken", vec![], 200),
        ("a long-term key: GetFederationToken", LONG_TERM_KEY.to_string(), "GetFederationToken", vec![("Name", "broker".to_string())], 200),
        ("a long-term key: GetCallerIdentity", LONG_TERM_KEY.to_string(), "GetCallerIdentity", vec![], 200),
        ("a long-term key: GetAccessKeyInfo", LONG_TERM_KEY.to_string(), "GetAccessKeyInfo", vec![("AccessKeyId", LONG_TERM_KEY.to_string())], 200),
        ("a role session: AssumeRole", role_key.clone(), "AssumeRole", assume.clone(), 200),
        ("a role session: GetSessionToken", role_key.clone(), "GetSessionToken", vec![], 403),
        ("a role session: GetFederationToken", role_key.clone(), "GetFederationToken", vec![("Name", "broker".to_string())], 403),
        ("a role session: GetCallerIdentity", role_key.clone(), "GetCallerIdentity", vec![], 200),
        ("a role session: GetAccessKeyInfo", role_key, "GetAccessKeyInfo", vec![("AccessKeyId", LONG_TERM_KEY.to_string())], 200),
        ("a session token: AssumeRole", session_key.clone(), "AssumeRole", assume.clone(), 200),
        ("a session token: GetSessionToken", session_key.clone(), "GetSessionToken", vec![], 403),
        ("a session token: GetFederationToken", session_key.clone(), "GetFederationToken", vec![("Name", "broker".to_string())], 403),
        ("a session token: GetCallerIdentity", session_key.clone(), "GetCallerIdentity", vec![], 200),
        ("a session token: GetAccessKeyInfo", session_key, "GetAccessKeyInfo", vec![("AccessKeyId", LONG_TERM_KEY.to_string())], 403),
        ("a federation token: AssumeRole", federation_key.clone(), "AssumeRole", assume, 403),
        ("a federation token: GetSessionToken", federation_key.clone(), "GetSessionToken", vec![], 403),
        ("a federation token: GetFederationToken", federation_key.clone(), "GetFederationToken", vec![("Name", "broker".to_string())], 403),
        ("a federation token: GetCallerIdentity", federation_key.clone(), "GetCallerIdentity", vec![], 200),
        ("a federation token: GetAccessKeyInfo", federation_key, "GetAccessKeyInfo", vec![("AccessKeyId", LONG_TERM_KEY.to_string())], 403),
    ];

    for (case, key, action, params, expected_status) in probes {
        let (status, body) = call(&router, &key, action, &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} {body}"
        );
        if expected_status == 403 {
            assert_eq!(
                element(&body, "Code").as_deref(),
                Some("AccessDenied"),
                "case {case:?}: expected AccessDenied, got {body}"
            );
            assert!(
                element(&body, "AccessKeyId").is_none(),
                "case {case:?}: a refused call must mint nothing, got {body}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// GetCallerIdentity resolves the signing credential
// ---------------------------------------------------------------------------

/// `GetCallerIdentity` reports the identity of the credential that SIGNED the request:
/// for an IAM user's own long-term access key, that user's `UserId` and ARN exactly as
/// IAM holds them — so two users are never indistinguishable — and for a key IAM holds
/// no record of, the ambient identity.
///
/// The rows differ in the user name's character set, the path IAM holds the user under,
/// and the case the name was created with (IAM names "are not distinguished by case").
#[tokio::test]
async fn get_caller_identity_reports_the_iam_user_whose_key_signed_the_request() {
    let (router, engine) = emulator();
    // (case, the UserId IAM issued, the user name, the path, the access key id)
    let rows: [(&str, &str, &str, &str, &str); 4] = [
        ("a user at the default path", "AIDASEEDEDUSER000001", "alice", "/", "EMULATORAKALICE00001"),
        ("a user under a path", "AIDASEEDEDUSER000002", "bob", "/engineering/", "EMULATORAKBOB0000001"),
        ("a mixed-case name", "AIDASEEDEDUSER000003", "Carol.Smith", "/", "EMULATORAKCAROL00001"),
        ("a name holding the characters IAM admits", "AIDASEEDEDUSER000004", "d_e-f+g=h,i.j@k", "/", "EMULATORAKDELTA00001"),
    ];

    let mut seen: Vec<String> = Vec::new();
    for (case, user_id, user_name, path, access_key_id) in rows {
        let (user_id, user_arn) = seed_user(&engine, user_id, user_name, path, access_key_id);
        let (status, body) = call(&router, access_key_id, "GetCallerIdentity", &[]).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} {body}");
        assert_eq!(
            (
                member(case, &body, "UserId"),
                member(case, &body, "Account"),
                member(case, &body, "Arn"),
            ),
            (user_id.clone(), emulator_account(), user_arn.clone()),
            "case {case:?}: expected the signing user's own identity, got {body}"
        );
        assert!(
            !seen.contains(&user_arn),
            "case {case:?}: two users must not resolve to one ARN ({user_arn})"
        );
        seen.push(user_arn);
    }

    // A key IAM holds no record of is the ambient identity, not one of the users above.
    let (status, body) = call(&router, "EMULATORAKUNKNOWN001", "GetCallerIdentity", &[]).await;
    assert_eq!(status, 200, "case 'an unheld key': {body}");
    assert_eq!(
        member("an unheld key", &body, "Arn"),
        format!("arn:aws:iam::{}:user/emulator", emulator_account()),
        "case 'an unheld key': expected the ambient identity, got {body}"
    );

    // And a session minted BY one of those users still speaks for that user.
    let (status, body) = call(&router, "EMULATORAKALICE00001", "GetSessionToken", &[]).await;
    assert_eq!(status, 200, "case 'a session of alice': {body}");
    let session_key = member("a session of alice", &body, "AccessKeyId");
    let (status, body) = call(&router, &session_key, "GetCallerIdentity", &[]).await;
    assert_eq!(status, 200, "case 'a session of alice': {body}");
    assert_eq!(
        member("a session of alice", &body, "Arn"),
        format!("arn:aws:iam::{}:user/alice", emulator_account()),
        "case 'a session of alice': a session token keeps the minting user's identity, got {body}"
    );
}

/// A role deleted from IAM can no longer be assumed, while the sessions it already
/// minted keep working until they expire: resolution reads IAM as it stands NOW.
#[tokio::test]
async fn a_role_removed_from_iam_can_no_longer_be_assumed() {
    let (router, engine) = emulator();
    let account = emulator_account();
    let root = format!("arn:aws:iam::{account}:root");
    let (role_arn, _role_id) = seed_role(
        &engine,
        "AROASEEDEDLIFECYCLE01",
        "ephemeral",
        "/",
        Some(&trust_policy_for(&root, &["sts:AssumeRole"], None)),
        Some(43_200),
    );
    let params = vec![
        ("RoleArn", role_arn.clone()),
        ("RoleSessionName", "before".to_string()),
    ];

    let (status, body) = call(&router, LONG_TERM_KEY, "AssumeRole", &params).await;
    assert_eq!(status, 200, "case 'before the delete': {body}");
    let minted = member("before the delete", &body, "AccessKeyId");

    engine
        .state::<iam::ComAmazonawsIamAwsidentitymanagementv20100508State>()
        .lock()
        .roles
        .remove(&iam::state::name_key("ephemeral"));

    let (status, body) = call(&router, LONG_TERM_KEY, "AssumeRole", &params).await;
    assert_eq!(
        (status, element(&body, "Code").as_deref()),
        (403, Some("AccessDenied")),
        "case 'after the delete': a role the account no longer holds must not be assumable, got \
         {status} {body}"
    );

    // The session minted before the delete is a credential this service handed out; it
    // keeps working for as long as it was promised.
    let (status, body) = call(&router, &minted, "GetCallerIdentity", &[]).await;
    assert_eq!(
        status, 200,
        "case 'after the delete': the session already minted must keep resolving, got {body}"
    );
}

/// The registry records what each mint was promised, and the clock is the only thing
/// that retires it: a role session minted for the chaining cap expires an hour out.
#[tokio::test]
async fn a_chained_session_expires_at_the_cap_it_was_granted() {
    let (router, engine) = emulator();
    let account = emulator_account();
    let root = format!("arn:aws:iam::{account}:root");
    let (role_arn, _role_id) = seed_role(
        &engine,
        "AROASEEDEDCHAINCAP01",
        "chained",
        "/",
        Some(&trust_policy_for(&root, &["sts:AssumeRole"], None)),
        Some(43_200),
    );

    let (status, body) = call(
        &router,
        LONG_TERM_KEY,
        "AssumeRole",
        &[
            ("RoleArn", role_arn.clone()),
            ("RoleSessionName", "hop-one".to_string()),
            ("DurationSeconds", "43200".to_string()),
        ],
    )
    .await;
    assert_eq!(status, 200, "case 'the first hop': {body}");
    let first_key = member("the first hop", &body, "AccessKeyId");

    let at = SystemTime::now();
    let (status, body) = call(
        &router,
        &first_key,
        "AssumeRole",
        &[
            ("RoleArn", role_arn),
            ("RoleSessionName", "hop-two".to_string()),
        ],
    )
    .await;
    assert_eq!(status, 200, "case 'the second hop': {body}");
    let key = member("the second hop", &body, "AccessKeyId");
    let held = engine.state::<sts::state::ComAmazonawsStsAwssecuritytokenservicev20110615State>();
    let expires_at = held
        .lock()
        .session(&key)
        .map(|session| session.expires_at)
        .unwrap_or_else(|| panic!("case 'the second hop': the session was not recorded"));
    let cap = Duration::from_secs(u64::try_from(sts::state::ROLE_CHAINING_MAX_DURATION_SECONDS).expect("a positive cap"));
    assert!(
        expires_at >= at + cap && expires_at <= at + cap + Duration::from_secs(60),
        "case 'the second hop': a chained session must expire at the one-hour cap, got \
         {expires_at:?} for a call at {at:?}"
    );
}

// ---------------------------------------------------------------------------
// A credential the emulator never issued
// ---------------------------------------------------------------------------

/// A request whose signing access key id was never issued is refused with the code an
/// SDK's credential-refresh path keys on — `InvalidClientTokenId`, HTTP 403 — not with
/// an authorization denial: nothing was authorized, because no principal was found.
///
/// The rows differ in the SHAPE of the never-issued key: the long-term `AKIA` class, the
/// temporary `ASIA` class, and a key id of the emulator's own shape, so the answer is
/// about the key being unknown rather than about how it is spelled.
#[tokio::test]
async fn a_never_issued_access_key_is_refused_as_an_unresolvable_credential() {
    let engine: std::sync::Arc<engine::InMemoryEngine> =
        std::sync::Arc::new(engine::InMemoryEngine::new());
    let mut router = smithy_server_runtime::MultiServiceRouter::new();
    // Authentication ON: the refusal under test is the one the emulator's own
    // authentication layer raises before any handler runs.
    router.set_request_authentication(true);
    router.register_service(sts::EmulatedService::new(std::sync::Arc::clone(&engine)));

    for key in [
        "AKIAIOSFODNN7EXAMPLE",
        "ASIAIOSFODNN7EXAMPLE",
        "EMULATORAKNEVERISSUED",
    ] {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential={key}/20260101/us-east-1/sts/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=0123456789abcdef"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri("https://sts.amazonaws.com/")
            .header("host", "sts.amazonaws.com")
            .header("content-type", "application/x-www-form-urlencoded")
            .header("x-amz-date", "20260101T000000Z")
            .header("authorization", authorization)
            .body(bytes::Bytes::from(
                "Action=GetCallerIdentity&Version=2011-06-15".to_string(),
            ))
            .expect("a well-formed awsQuery request");
        let response = router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = String::from_utf8_lossy(response.body()).to_string();
        assert_eq!(
            status, 403,
            "case {key:?}: expected HTTP 403 for a key that was never issued, got {status} {body}"
        );
        assert_eq!(
            element(&body, "Code").as_deref(),
            Some("InvalidClientTokenId"),
            "case {key:?}: expected InvalidClientTokenId, got {body}"
        );
    }
}
