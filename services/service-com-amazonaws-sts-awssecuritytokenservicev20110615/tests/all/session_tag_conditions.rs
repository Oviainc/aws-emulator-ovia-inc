// SPDX-License-Identifier: CC0-1.0

//! A role's trust policy can gate an assume on the session tags the CALLER carries.
//!
//! RULE: a session tag set on a role session becomes that session's
//! `aws:PrincipalTag/<key>` context value byte-exactly, and a transitive tag key survives
//! into the next `AssumeRole` hop — so a trust-policy `Condition` over
//! `aws:PrincipalTag/<key>` decides the chained assume: it admits a session carrying a
//! value the clause holds for and refuses one that does not, with `AccessDenied` / HTTP
//! 403. Because the registry holds a session's tags IN FULL, the request answers for a
//! tag key the session does NOT carry as well — so an existence clause (`Null`), a
//! negated comparison (`StringNotEquals`) and a set-qualified one are decided rather than
//! left undecided.
//!
//! The rows differ along what the rule generalises over: the operator the clause names,
//! whether it asserts presence or absence, whether the caller's value matches, and the
//! character set the value is spelled in.

use super::sts_sessions::{
    call, element, emulator, member, seed_role, trust_policy, LONG_TERM_KEY,
};

/// A trust policy that lets this account's own principals assume the role and carry
/// session tags, but only when `condition` holds.
///
/// The condition is why this is written here: the crate's shared helper
/// (`sts_sessions::trust_policy`) builds the same document without one, and is used
/// directly for the role the first hop assumes.
fn tag_gated_trust_policy(condition: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
         \"Principal\":{{\"AWS\":\"arn:aws:iam::{}:root\"}},\
         \"Action\":[\"sts:AssumeRole\",\"sts:TagSession\"],\"Condition\":{condition}}}]}}",
        engine::emulator_account_id()
    )
}

/// The tag key every row below gates on.
const TENANT_KEY: &str = "tenant";

/// A tag value outside ASCII, so the rule is asserted on a value whose bytes a naive
/// comparison would mangle.
const UNICODE_VALUE: &str = "Café-Zürich-北京";

/// A chained assume into a tag-gated role is decided by the tags the CALLING session
/// carries — including by their absence.
#[tokio::test]
async fn a_trust_policy_condition_on_a_principal_tag_decides_the_chained_assume() {
    // (case, the condition the TARGET role's trust policy carries, the tag the FIRST hop
    // sets transitively — `None` for a session carrying none — the status the chained
    // assume must answer)
    type Row = (&'static str, String, Option<(&'static str, &'static str)>, u16);
    let rows: Vec<Row> = vec![
        (
            "an equality clause the caller's tag satisfies",
            format!("{{\"StringEquals\":{{\"aws:PrincipalTag/{TENANT_KEY}\":\"acme\"}}}}"),
            Some((TENANT_KEY, "acme")),
            200,
        ),
        (
            "an equality clause the caller's tag does not satisfy",
            format!("{{\"StringEquals\":{{\"aws:PrincipalTag/{TENANT_KEY}\":\"acme\"}}}}"),
            Some((TENANT_KEY, "globex")),
            403,
        ),
        (
            "an equality clause under a session carrying no such tag",
            format!("{{\"StringEquals\":{{\"aws:PrincipalTag/{TENANT_KEY}\":\"acme\"}}}}"),
            None,
            403,
        ),
        (
            "a presence clause under a tagged session",
            format!("{{\"Null\":{{\"aws:PrincipalTag/{TENANT_KEY}\":\"false\"}}}}"),
            Some((TENANT_KEY, "acme")),
            200,
        ),
        (
            "a presence clause under an untagged session",
            format!("{{\"Null\":{{\"aws:PrincipalTag/{TENANT_KEY}\":\"false\"}}}}"),
            None,
            403,
        ),
        (
            "an absence clause under an untagged session",
            format!("{{\"Null\":{{\"aws:PrincipalTag/{TENANT_KEY}\":\"true\"}}}}"),
            None,
            200,
        ),
        (
            "an absence clause under a tagged session",
            format!("{{\"Null\":{{\"aws:PrincipalTag/{TENANT_KEY}\":\"true\"}}}}"),
            Some((TENANT_KEY, "acme")),
            403,
        ),
        (
            "a negated comparison the caller's tag satisfies",
            format!("{{\"StringNotEquals\":{{\"aws:PrincipalTag/{TENANT_KEY}\":\"globex\"}}}}"),
            Some((TENANT_KEY, "acme")),
            200,
        ),
        (
            "a negated comparison the caller's tag does not satisfy",
            format!("{{\"StringNotEquals\":{{\"aws:PrincipalTag/{TENANT_KEY}\":\"globex\"}}}}"),
            Some((TENANT_KEY, "globex")),
            403,
        ),
        (
            "a pattern clause the caller's tag satisfies",
            format!("{{\"StringLike\":{{\"aws:PrincipalTag/{TENANT_KEY}\":\"ac*\"}}}}"),
            Some((TENANT_KEY, "acme")),
            200,
        ),
        (
            "a clause on a value outside ASCII the caller carries byte-exactly",
            format!("{{\"StringEquals\":{{\"aws:PrincipalTag/city\":\"{UNICODE_VALUE}\"}}}}"),
            Some(("city", UNICODE_VALUE)),
            200,
        ),
        (
            "a clause on a value outside ASCII the caller does not carry",
            format!("{{\"StringEquals\":{{\"aws:PrincipalTag/city\":\"{UNICODE_VALUE}\"}}}}"),
            Some(("city", "Cafe-Zurich-Beijing")),
            403,
        ),
    ];

    for (index, (case, condition, carried, expected_status)) in rows.into_iter().enumerate() {
        let (router, engine) = emulator();
        let (first_arn, _) = seed_role(
            &engine,
            &format!("AROASEEDEDTAGGATE1{index:04}"),
            &format!("tag-gate-first-{index}"),
            "/",
            Some(&trust_policy(&["sts:AssumeRole", "sts:TagSession"])),
            Some(43_200),
        );
        let (second_arn, _) = seed_role(
            &engine,
            &format!("AROASEEDEDTAGGATE2{index:04}"),
            &format!("tag-gate-second-{index}"),
            "/",
            Some(&tag_gated_trust_policy(&condition)),
            Some(43_200),
        );

        let mut first = vec![
            ("RoleArn", first_arn),
            ("RoleSessionName", "hop-one".to_string()),
        ];
        if let Some((key, value)) = carried {
            first.push(("Tags.member.1.Key", key.to_string()));
            first.push(("Tags.member.1.Value", value.to_string()));
            first.push(("TransitiveTagKeys.member.1", key.to_string()));
        }
        let (status, body) = call(&router, LONG_TERM_KEY, "AssumeRole", &first).await;
        assert_eq!(status, 200, "case {case:?}: the first hop: {status} {body}");
        let session_key = member(case, &body, "AccessKeyId");

        // The tag really is on the session the chain is made from, read back out of the
        // registry under the credential it was minted with, so a row that passes cannot
        // pass on a tag that was never stored.
        let stored: Vec<(String, String)> = engine
            .state::<service_com_amazonaws_sts_awssecuritytokenservicev20110615::ComAmazonawsStsAwssecuritytokenservicev20110615State>()
            .lock()
            .session(&session_key)
            .map(|session| session.tags.clone())
            .unwrap_or_default();
        let expected_stored: Vec<(String, String)> = carried
            .map(|(key, value)| vec![(key.to_string(), value.to_string())])
            .unwrap_or_default();
        assert_eq!(
            stored, expected_stored,
            "case {case:?}: expected the calling session to carry {expected_stored:?}"
        );

        let (status, body) = call(
            &router,
            &session_key,
            "AssumeRole",
            &[
                ("RoleArn", second_arn),
                ("RoleSessionName", "hop-two".to_string()),
            ],
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status} for the chained assume, got {status} \
             {body}"
        );
        if expected_status == 403 {
            assert_eq!(
                element(&body, "Code").as_deref(),
                Some("AccessDenied"),
                "case {case:?}: expected the AccessDenied code, got {body}"
            );
            assert!(
                element(&body, "AccessKeyId").is_none(),
                "case {case:?}: a refused assume must mint nothing, got {body}"
            );
        } else {
            assert!(
                element(&body, "AccessKeyId").is_some(),
                "case {case:?}: expected a minted session, got {body}"
            );
        }
    }
}
