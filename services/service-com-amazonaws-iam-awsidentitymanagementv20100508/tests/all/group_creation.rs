// SPDX-License-Identifier: CC0-1.0

//! `CreateGroup`'s bounds and refusals, end to end over the awsQuery wire.
//!
//! The happy path — the minted group, its identifiers and its read-back through
//! `ListGroups` — is pinned by `tests/all/group_listing.rs`. This suite pins the
//! BOUNDS of the same create: the account-wide group quota, and the published
//! constraints on `GroupName` and `Path`, each asserted with the store read back
//! afterwards so a refusal that persisted something anyway fails here.
//!
//! Every test drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does, so an assertion here is
//! an assertion about what a client on the socket sees. The awsQuery harness
//! (router, call, element, elements, error_code) is the one
//! `tests/all/user_lifecycle.rs` owns and every suite in this crate shares.
//!
//! Each test is a TABLE whose rows differ along what the rule being asserted
//! generalises over: the name alphabet, the name length at and past each published
//! bound, the path depth and length, and the number of groups the account holds.

use smithy_server_runtime::MultiServiceRouter;

use crate::user_lifecycle::{account_id, call, element, elements, error_code, router};

/// Attempt one `CreateGroup`, answering the HTTP status and body.
async fn create_group(router: &MultiServiceRouter, group_name: &str, path: &str) -> (u16, String) {
    call(
        router,
        "CreateGroup",
        &[("GroupName", group_name), ("Path", path)],
    )
    .await
}

/// The group names the account holds under `prefix`, as `ListGroups` reports them.
async fn names_under(router: &MultiServiceRouter, prefix: &str) -> Vec<String> {
    let (status, body) = call(router, "ListGroups", &[("PathPrefix", prefix)]).await;
    assert_eq!(
        status, 200,
        "case {prefix:?}: expected ListGroups 200, got {status}: {body}"
    );
    elements(&body, "GroupName")
}

/// RULE: an account holds a bounded number of groups, and a create past the quota is
/// refused with `LimitExceeded` (HTTP 409) rather than growing the account; the
/// refusal names the quota and its value.
///
/// This drives one router round trip per group up to the quota; keep that in mind if
/// more quota rows land here.
#[tokio::test]
async fn creating_past_the_group_quota_is_refused() {
    // The quota the handler enforces, read from the service's own constant so the
    // test cannot drift from it — and pinned to the number AWS publishes, so a wrong
    // constant fails here rather than silently moving the bound.
    const QUOTA: usize =
        service_com_amazonaws_iam_awsidentitymanagementv20100508::state::MAX_GROUPS_PER_ACCOUNT;
    assert_eq!(
        QUOTA, 300,
        "AWS's IAM and STS quotas page gives \"IAM groups in an account: 300\", got {QUOTA}"
    );

    let router = router();
    for n in 0..QUOTA {
        let name = format!("quota_group_{n}");
        let (status, body) = create_group(&router, &name, "/quota/").await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected 200 up to the quota, got {status}: {body}"
        );
    }

    // Rows: the first create past the quota, and a second one, each refused and each
    // leaving the account exactly at the quota. They differ in path too, so the
    // refusal is the account's bound rather than one path's.
    for (attempt, path) in [("over_the_line", "/over/"), ("over_the_line_again", "/")] {
        let (status, body) = create_group(&router, attempt, path).await;
        assert_eq!(
            status, 409,
            "case {attempt:?} at {path}: expected 409, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "LimitExceeded",
            "case {attempt:?} at {path}: expected LimitExceeded, got {:?}",
            error_code(&body)
        );
        let message = element(&body, "Message");
        assert!(
            message.contains("GroupsPerAccount") && message.contains(&QUOTA.to_string()),
            "case {attempt:?}: expected the refusal to name the quota and its value, \
             got {message:?}"
        );
        assert!(
            !names_under(&router, path)
                .await
                .contains(&attempt.to_string()),
            "case {attempt:?}: the refused create stored the group anyway"
        );
    }
}

/// RULE: a create whose name falls outside the published constraints of its member is
/// refused before the operation runs, so it never reaches the store; a name on either
/// published bound is accepted.
#[tokio::test]
async fn a_group_name_outside_the_published_constraints_is_refused() {
    let router = router();

    // `groupNameType` is len 1..128, pattern ^[\w+=,.@-]+$ (the CreateGroup$GroupName
    // constraints in this service's model): each bound, one past the upper bound, and
    // characters the alphabet admits and excludes.
    let at_min = "g";
    let at_max = "n".repeat(128);
    let past_max = "n".repeat(129);
    let whole_alphabet = "a_B9+c=d,e.f@g-h";
    let accepted: &[&str] = &[at_min, at_max.as_str(), whole_alphabet];
    let refused: &[&str] = &["", past_max.as_str(), "has space", "has/slash", "has#hash"];

    for name in accepted {
        let (status, body) = create_group(&router, name, "/ok/").await;
        assert_eq!(
            status, 200,
            "case {name:?} (len {}): expected 200, got {status}: {body}",
            name.chars().count()
        );
    }
    for name in refused {
        let (status, body) = create_group(&router, name, "/ok/").await;
        assert_eq!(
            status, 400,
            "case {name:?} (len {}): expected 400, got {status}: {body}",
            name.chars().count()
        );
        // IAM's query-protocol code for input it refuses as invalid, so a refusal
        // that happened for another reason (an authorization or serialization
        // failure) fails here.
        assert_eq!(
            error_code(&body),
            "ValidationError",
            "case {name:?}: expected the input to be refused as invalid, got {:?}",
            error_code(&body)
        );
    }

    let mut listed = names_under(&router, "/ok/").await;
    listed.sort();
    let mut expected: Vec<String> = accepted.iter().map(|name| (*name).to_string()).collect();
    expected.sort();
    assert_eq!(
        listed, expected,
        "expected exactly the accepted names to be persisted, got {listed:?}"
    );
}

/// RULE: a create whose `Path` falls outside the published constraints of its member
/// is refused before the operation runs; an accepted path is stored verbatim and is
/// the path segment of the resource's ARN, at any depth or length the constraint
/// admits.
#[tokio::test]
async fn a_group_path_outside_the_published_constraints_is_refused() {
    let router = router();

    // `pathType` is len 1..512, pattern ^(\u002F)|(\u002F[\u0021-\u007E]+\u002F)$ (the
    // CreateGroup$Path constraints in this service's model): the account root, one and
    // two segments, a segment spanning the ends of the admitted character range, and a
    // path on the 512-character bound.
    let at_max_len = format!("/{}/", "d".repeat(510));
    let past_max_len = format!("/{}/", "d".repeat(511));
    let accepted: &[(&str, &str)] = &[
        ("rooted", "/"),
        ("one_segment", "/eng/"),
        ("two_segments", "/eng/tools/"),
        ("punctuated", "/a!b~c/"),
        // The admitted range spans the XML metacharacters too, so these rows also
        // pin that a caller-supplied value is ESCAPED into the response envelope
        // and round trips unchanged rather than altering the document.
        ("ampersand", "/a&b/"),
        ("angles", "/x<y>/"),
        ("quotes", "/q\"z'/"),
        ("longest", at_max_len.as_str()),
    ];
    // Rows that satisfy NEITHER branch of the pattern, plus one past the length bound.
    let refused: &[(&str, &str)] = &[
        ("empty", ""),
        ("no_leading_slash", "eng/"),
        ("no_slash_at_all", "eng"),
        ("too_long", past_max_len.as_str()),
    ];

    for (name, path) in accepted {
        let (status, body) = create_group(&router, name, path).await;
        assert_eq!(
            status, 200,
            "case {name:?} (path len {}): expected 200, got {status}: {body}",
            path.chars().count()
        );
        assert_eq!(
            element(&body, "Path"),
            *path,
            "case {name:?}: expected the requested path to be stored verbatim, got {:?}",
            element(&body, "Path")
        );
        assert_eq!(
            element(&body, "Arn"),
            format!("arn:aws:iam::{}:group{path}{name}", account_id()),
            "case {name:?}: expected the path to be the ARN's resource path, got {:?}",
            element(&body, "Arn")
        );
        assert!(
            names_under(&router, path).await.contains(&name.to_string()),
            "case {name:?}: expected the created group to be listed under {path}"
        );
    }

    for (case, path) in refused {
        let (status, body) = create_group(&router, case, path).await;
        assert_eq!(
            status, 400,
            "case {case:?} ({path:?}): expected 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationError",
            "case {case:?}: expected the input to be refused as invalid, got {:?}",
            error_code(&body)
        );
        assert!(
            !names_under(&router, "/").await.contains(&case.to_string()),
            "case {case:?}: the refused create stored the group anyway"
        );
    }
}
