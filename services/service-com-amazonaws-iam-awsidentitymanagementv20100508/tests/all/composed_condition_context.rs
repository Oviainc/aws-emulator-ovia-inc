// SPDX-License-Identifier: CC0-1.0

//! A `Condition` block whose clauses read DIFFERENT sources of the request context, end
//! to end over the awsQuery wire.
//!
//! RULE under test: IAM decides a statement's `Condition` against the context of the
//! request being made, which it populates itself — `aws:ResourceTag/<key>` from the tags
//! stored on the target the request addresses, `aws:RequestTag/<key>` from the tags the
//! request writes, `aws:TagKeys` from the set of keys it writes — and a block holds only
//! when EVERY clause in it holds, whichever source each clause reads. So one stored grant
//! over several targets permits exactly the (target, written tags) combinations all of its
//! clauses admit, refuses every other one `AccessDenied` (HTTP 403), and because the
//! decision is made before the write, a refused call leaves the target's stored tags
//! exactly as they were. The same question is asked of every call site in the
//! `Tag*`/`Untag*` family, each handing the gate its own tag input, so an untag call is
//! decided against the keys it removes and a caller no statement covers is refused rather
//! than served.
//!
//! The suites are TABLES whose rows differ along what the rule generalises over: which
//! clause of the block fails and which source it reads (the target's tags, the value the
//! request writes, the set of keys it writes, several at once), how many tags the request
//! carries, which of the targets the grant names is addressed, which operation the
//! decision is made for, and whether the caller holds a statement for the action at all.
//!
//! The awsQuery harness is the one `tests/all/user_lifecycle.rs` owns; the user, key,
//! policy and tag fixtures are `tests/all/request_condition_context.rs`'s and the instance
//! profile fixtures are `tests/all/instance_profile_tags.rs`'s, so a request is built and
//! read exactly one way across this crate.

use std::collections::BTreeMap;

use smithy_server_runtime::MultiServiceRouter;

use crate::instance_profile_tags::{create_instance_profile, stored_tags, tags};
use crate::request_condition_context::{access_key, create_user, held_tags, put_policy, user_arn};
use crate::role_lifecycle::{borrowed, param};
use crate::user_lifecycle::{account_id, call_signed_by, error_code, router};

/// The `owner` tag value the grant's resource-tag clause names, which one of the two
/// targets carries and the other does not.
const ADMITTED_OWNER: &str = "alpha";

/// The `owner` tag value the OTHER target carries, so a clause over the target's tags
/// fails on it while every other clause of the block still holds.
const REFUSED_OWNER: &str = "beta";

/// A tag every target carries and no request under test writes, so a refusal that left
/// the stored set alone is told apart from one that emptied it.
const RETAINED: (&str, &str) = ("retained", "yes");

/// A document allowing `iam:TagUser` on BOTH `targets` under one block whose clauses read
/// the three sources a tagging request publishes: the target's own tags, the value the
/// request writes and the set of keys it writes.
fn composed_tag_user_grant(targets: &[String]) -> String {
    let resources = targets
        .iter()
        .map(|arn| format!("\"{arn}\""))
        .collect::<Vec<_>>()
        .join(",");
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Action":"iam:TagUser","Resource":[{resources}],"Condition":{{"StringEquals":{{"aws:ResourceTag/owner":"{ADMITTED_OWNER}","aws:RequestTag/env":"dev"}},"ForAllValues:StringEquals":{{"aws:TagKeys":["env"]}}}}}}]}}"#
    )
}

/// One signed `TagUser` call writing `written` onto `target`.
async fn tag_user_signed(
    router: &MultiServiceRouter,
    signing_key: &str,
    target: &str,
    written: &[(&str, &str)],
) -> (u16, String) {
    let mut params = vec![param("UserName", target)];
    for (index, (key, value)) in written.iter().enumerate() {
        let position = index + 1;
        params.push(param(&format!("Tags.member.{position}.Key"), key));
        params.push(param(&format!("Tags.member.{position}.Value"), value));
    }
    call_signed_by(router, Some(signing_key), "TagUser", &borrowed(&params)).await
}

/// The tag set `target` holds, as a map, read with the fixture's own credentials so the
/// read is never the thing under test.
async fn held_map(
    router: &MultiServiceRouter,
    case: &str,
    target: &str,
) -> BTreeMap<String, String> {
    held_tags(router, case, target).await.into_iter().collect()
}

/// `pairs` as the map a target must hold.
fn expected(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// One row of the composed-block table: which of the two targets the call addresses, the
/// tags it writes, and whether the block holds for that combination.
struct ComposedCase {
    case: &'static str,
    /// The `owner` tag of the target the call addresses — [`ADMITTED_OWNER`] for the one
    /// the resource-tag clause admits, [`REFUSED_OWNER`] for the one it does not.
    owner: &'static str,
    /// The tags the `TagUser` call writes.
    written: &'static [(&'static str, &'static str)],
    /// Whether every clause of the block holds, so the call is served.
    permitted: bool,
}

/// Rows spanning which clause of the one block fails, and therefore which source of the
/// request context decides the call: the target's stored tags, the value the request
/// writes, the set of keys it writes, and two of them at once.
const COMPOSED_CASES: &[ComposedCase] = &[
    ComposedCase {
        case: "every clause of the block holds",
        owner: ADMITTED_OWNER,
        written: &[("env", "dev")],
        permitted: true,
    },
    ComposedCase {
        case: "the value the request writes fails its clause",
        owner: ADMITTED_OWNER,
        written: &[("env", "prod")],
        permitted: false,
    },
    ComposedCase {
        case: "the target's stored tag fails its clause",
        owner: REFUSED_OWNER,
        written: &[("env", "dev")],
        permitted: false,
    },
    ComposedCase {
        case: "the target's tag and the written value both fail",
        owner: REFUSED_OWNER,
        written: &[("env", "prod")],
        permitted: false,
    },
    ComposedCase {
        case: "the set of keys written steps outside its clause",
        owner: ADMITTED_OWNER,
        written: &[("env", "dev"), ("tier", "gold")],
        permitted: false,
    },
    ComposedCase {
        case: "the request carries no value for the conditioned key",
        owner: ADMITTED_OWNER,
        written: &[("tier", "gold")],
        permitted: false,
    },
];

/// RULE: a block whose clauses read the target's tags, the tags the request writes and the
/// set of keys it writes holds only when every one of them holds — so one grant over two
/// targets serves exactly the combinations it admits, and a refusal leaves the addressed
/// target's stored tags, and the target it did not address, untouched.
#[tokio::test]
async fn a_block_over_three_context_sources_holds_only_when_every_clause_does() {
    let router = router();
    let caller = "composed-ctx-caller";
    create_user(&router, "the caller", caller, &[]).await;
    let signing_key = access_key(&router, "the caller", caller).await;

    for (row, case) in COMPOSED_CASES.iter().enumerate() {
        // A fresh PAIR of targets per row — one carrying the admitted owner tag and one
        // carrying the other — so the one grant names several resources, as a grant over a
        // tag-bearing fleet does, and the tags a row finds are the ones it wrote.
        let admitted = format!("composed-ctx-admitted{row}");
        let refused = format!("composed-ctx-refused{row}");
        for (target, owner) in [(&admitted, ADMITTED_OWNER), (&refused, REFUSED_OWNER)] {
            create_user(
                &router,
                case.case,
                target,
                &[("owner", owner), RETAINED],
            )
            .await;
        }
        let arns = vec![user_arn(&admitted), user_arn(&refused)];
        put_policy(
            &router,
            case.case,
            caller,
            "composed",
            &composed_tag_user_grant(&arns),
        )
        .await;

        let (addressed, bystander) = if case.owner == ADMITTED_OWNER {
            (&admitted, &refused)
        } else {
            (&refused, &admitted)
        };
        let (status, body) = tag_user_signed(&router, &signing_key, addressed, case.written).await;

        let held = held_map(&router, case.case, addressed).await;
        let mut want: Vec<(&str, &str)> = vec![("owner", case.owner), RETAINED];
        if case.permitted {
            assert_eq!(
                status, 200,
                "case {:?}: expected the grant to permit the call, got {status}: {body}",
                case.case
            );
            want.extend_from_slice(case.written);
            assert_eq!(
                held,
                expected(&want),
                "case {:?}: expected the permitted call to store its tags beside the held \
                 ones, got {held:?}",
                case.case
            );
        } else {
            assert_eq!(
                status, 403,
                "case {:?}: expected the call to be refused, got {status}: {body}",
                case.case
            );
            assert_eq!(
                error_code(&body),
                "AccessDenied",
                "case {:?}: expected AccessDenied, got {:?}",
                case.case,
                error_code(&body)
            );
            assert_eq!(
                held,
                expected(&want),
                "case {:?}: a refused write must leave the target's tags untouched, got \
                 {held:?}",
                case.case
            );
        }
        // Whatever the decision, the target the call did not address is unchanged.
        let untouched = held_map(&router, case.case, bystander).await;
        let bystander_owner = if case.owner == ADMITTED_OWNER {
            REFUSED_OWNER
        } else {
            ADMITTED_OWNER
        };
        assert_eq!(
            untouched,
            expected(&[("owner", bystander_owner), RETAINED]),
            "case {:?}: the target the call did not address must be unchanged, got \
             {untouched:?}",
            case.case
        );
    }
}

/// RULE: the resource-tag clause is decided from the tags the target holds AT THE INSTANT
/// of the call, so retagging the target flips the answer the SAME unchanged request gets,
/// in both directions, with nothing about the stored policy changing.
#[tokio::test]
async fn retagging_the_target_flips_the_same_composed_requests_answer() {
    let router = router();
    let caller = "composed-flip-caller";
    create_user(&router, "the caller", caller, &[]).await;
    let signing_key = access_key(&router, "the caller", caller).await;

    let target = "composed-flip-target";
    create_user(
        &router,
        "the target",
        target,
        &[("owner", REFUSED_OWNER), RETAINED],
    )
    .await;
    let arns = vec![user_arn(target)];
    put_policy(
        &router,
        "the grant",
        caller,
        "composed",
        &composed_tag_user_grant(&arns),
    )
    .await;

    // The target's owner tag is the one the clause does not admit.
    let (status, body) = tag_user_signed(&router, &signing_key, target, &[("env", "dev")]).await;
    assert_eq!(
        status, 403,
        "case \"before the retag\": expected the call to be refused, got {status}: {body}"
    );
    let held = held_map(&router, "before the retag", target).await;
    assert_eq!(
        held,
        expected(&[("owner", REFUSED_OWNER), RETAINED]),
        "case \"before the retag\": a refused write must store nothing, got {held:?}"
    );

    // Retag the target, with the fixture's own credentials, to the value the clause admits.
    let retag = vec![
        param("UserName", target),
        param("Tags.member.1.Key", "owner"),
        param("Tags.member.1.Value", ADMITTED_OWNER),
    ];
    let (status, body) = call_signed_by(&router, None, "TagUser", &borrowed(&retag)).await;
    assert_eq!(
        status, 200,
        "case \"the retag\": the fixture TagUser must succeed, got {status}: {body}"
    );

    // The same unchanged request is now served.
    let (status, body) = tag_user_signed(&router, &signing_key, target, &[("env", "dev")]).await;
    assert_eq!(
        status, 200,
        "case \"after the retag\": expected the same call to be permitted, got {status}: \
         {body}"
    );
    let held = held_map(&router, "after the retag", target).await;
    assert_eq!(
        held,
        expected(&[("owner", ADMITTED_OWNER), RETAINED, ("env", "dev")]),
        "case \"after the retag\": expected the permitted call to store its tag, got {held:?}"
    );

    // And retagging it back takes the grant away again, for the same request.
    let retag_back = vec![
        param("UserName", target),
        param("Tags.member.1.Key", "owner"),
        param("Tags.member.1.Value", REFUSED_OWNER),
    ];
    let (status, body) = call_signed_by(&router, None, "TagUser", &borrowed(&retag_back)).await;
    assert_eq!(
        status, 200,
        "case \"the retag back\": the fixture TagUser must succeed, got {status}: {body}"
    );
    let (status, body) = tag_user_signed(&router, &signing_key, target, &[("env", "staging")]).await;
    assert_eq!(
        status, 403,
        "case \"after the retag back\": expected the call to be refused again, got {status}: \
         {body}"
    );
    assert_eq!(
        error_code(&body),
        "AccessDenied",
        "case \"after the retag back\": expected AccessDenied, got {:?}",
        error_code(&body)
    );
    let held = held_map(&router, "after the retag back", target).await;
    assert_eq!(
        held,
        expected(&[("owner", REFUSED_OWNER), RETAINED, ("env", "dev")]),
        "case \"after the retag back\": the refused write must leave the stored tag as it \
         was, got {held:?}"
    );
}

/// One row of the untag table: whether the caller holds a statement for the action, the
/// keys the call removes, and whether it is served.
struct UntagCase {
    case: &'static str,
    /// Whether the caller's document carries a statement about `iam:UntagInstanceProfile`
    /// at all — a caller none covers is refused the implicit deny.
    granted: bool,
    /// The tag keys the `UntagInstanceProfile` call names.
    removed: &'static [&'static str],
    /// Whether the policies in force permit the call.
    permitted: bool,
}

/// Rows spanning the two refusals an untag call meets — no statement about the action, and
/// a statement whose set clause the removed keys step outside — beside the key set it
/// admits.
const UNTAG_CASES: &[UntagCase] = &[
    UntagCase {
        case: "no statement names the action",
        granted: false,
        removed: &["env"],
        permitted: false,
    },
    UntagCase {
        case: "the key the clause admits",
        granted: true,
        removed: &["env"],
        permitted: true,
    },
    UntagCase {
        case: "a key outside the clause",
        granted: true,
        removed: &["owner"],
        permitted: false,
    },
    UntagCase {
        case: "a key set stepping outside the clause",
        granted: true,
        removed: &["env", "owner"],
        permitted: false,
    },
];

/// RULE: an untag call is decided against the caller's policies before anything is removed,
/// with the keys it removes handed to the decision as `aws:TagKeys` — so a conditioned
/// grant drops exactly the key sets it admits, every other call is refused `AccessDenied`,
/// and a refused call leaves the profile's stored tags as they were.
#[tokio::test]
async fn untagging_an_instance_profile_is_decided_against_the_keys_it_removes() {
    let router = router();
    let caller = "untag-ctx-caller";
    create_user(&router, "the caller", caller, &[]).await;
    let signing_key = access_key(&router, "the caller", caller).await;

    for (row, case) in UNTAG_CASES.iter().enumerate() {
        // A fresh profile per row, carrying the key the clause admits and one it does not.
        let profile = format!("untag-ctx-profile{row}");
        create_instance_profile(
            &router,
            &profile,
            &tags(&[("env", "dev"), ("owner", ADMITTED_OWNER)]),
        )
        .await;
        let profile_arn = format!("arn:aws:iam::{}:instance-profile/{profile}", account_id());
        let document = if case.granted {
            format!(
                r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Action":"iam:UntagInstanceProfile","Resource":"{profile_arn}","Condition":{{"ForAllValues:StringEquals":{{"aws:TagKeys":["env"]}}}}}}]}}"#
            )
        } else {
            format!(
                r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Action":"iam:GetUser","Resource":"{}"}}]}}"#,
                user_arn(caller)
            )
        };
        put_policy(&router, case.case, caller, "untagging", &document).await;

        let mut params = vec![param("InstanceProfileName", &profile)];
        for (index, key) in case.removed.iter().enumerate() {
            params.push(param(&format!("TagKeys.member.{}", index + 1), key));
        }
        let (status, body) = call_signed_by(
            &router,
            Some(&signing_key),
            "UntagInstanceProfile",
            &borrowed(&params),
        )
        .await;

        let (held, _) = stored_tags(&router, &profile).await;
        if case.permitted {
            assert_eq!(
                status, 200,
                "case {:?}: expected the grant to permit the call, got {status}: {body}",
                case.case
            );
            assert_eq!(
                held,
                expected(&[("owner", ADMITTED_OWNER)]),
                "case {:?}: expected the permitted call to drop the named key, got {held:?}",
                case.case
            );
        } else {
            assert_eq!(
                status, 403,
                "case {:?}: expected the call to be refused, got {status}: {body}",
                case.case
            );
            assert_eq!(
                error_code(&body),
                "AccessDenied",
                "case {:?}: expected AccessDenied, got {:?}",
                case.case,
                error_code(&body)
            );
            assert_eq!(
                held,
                expected(&[("env", "dev"), ("owner", ADMITTED_OWNER)]),
                "case {:?}: a refused removal must leave the profile's tags untouched, got \
                 {held:?}",
                case.case
            );
        }
    }
}
