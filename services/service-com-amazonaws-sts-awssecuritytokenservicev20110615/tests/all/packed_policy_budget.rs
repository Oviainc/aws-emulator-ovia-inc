// SPDX-License-Identifier: CC0-1.0

//! The packed token budget one mint's session policies and session tags must fit in.
//!
//! RULE: a mint packs its inline session policy, the managed policy ARNs it names and its
//! session tags into one serialized token, and the token has a budget of 4,096 bytes. A
//! request whose packed token is within the budget is served and reports its size as the
//! percentage of that budget (`PackedPolicySize`, rounded up); a request whose packed token
//! passes the budget is refused with `PackedPolicyTooLarge` / HTTP 400, and the refusal
//! names the two figures a caller needs to trim to fit — the size its own policies and tags
//! were measured at, and the limit that size passed.
//!
//! The rows differ along what the rule generalises over: WHICH of the three packed inputs
//! carries the bytes (the inline policy, the managed policy ARNs, the session tags, or all
//! three together), how far over or under the budget the request is (the exact boundary and
//! one byte either side of it), and which operation is minting (`AssumeRole`,
//! `GetFederationToken`). The recorded case — fifty session tags of 128-character keys and
//! 256-character values — is one row.
//!
//! The inline policy of every row is within the 2,048-character PLAINTEXT limit the
//! operation documents for it, because that is a DIFFERENT rule, answered before anything
//! is packed: a document over it is a validation error naming the member and the bound
//! (`session_policy_limits.rs`), so the budget rule is reached only by a request whose
//! plaintext fits.

use super::sts_sessions::{call, element, emulator, member, seed_demo_role, LONG_TERM_KEY};

/// The budget, as the service publishes it: the model's `minimumSessionTokenSizeType`
/// range maximum, which is the size of the largest serialized session token STS issues.
const SESSION_TOKEN_SIZE_LIMIT: usize = 4096;

/// The most characters an inline session policy's plaintext may carry ("The plaintext that
/// you use for both inline and managed session policies can't exceed 2,048 characters",
/// `AssumeRoleRequest$Policy`), which is the ceiling every row's own policy stays under.
const MAX_SESSION_POLICY_PLAINTEXT_CHARS: usize = 2048;

/// A JSON policy document of EXACTLY `bytes` bytes: the fixed skeleton a session policy
/// has, padded in one of its values.
pub(crate) fn policy_of(bytes: usize) -> String {
    let skeleton = r#"{"Version":"2012-10-17","Comment":""}"#;
    assert!(
        bytes >= skeleton.len(),
        "case policy_of({bytes}): the skeleton alone is {} bytes",
        skeleton.len()
    );
    let document = format!(
        r#"{{"Version":"2012-10-17","Comment":"{}"}}"#,
        "x".repeat(bytes - skeleton.len())
    );
    assert_eq!(
        document.len(),
        bytes,
        "case policy_of({bytes}): built a document of {} bytes",
        document.len()
    );
    document
}

/// The form parameters for `count` session tags whose keys are `key_chars` characters and
/// whose values are `value_chars` characters, plus the bytes they pack into.
fn session_tags(count: usize, key_chars: usize, value_chars: usize) -> (Vec<(String, String)>, usize) {
    let mut params = Vec::new();
    let mut packed = 0;
    for index in 1..=count {
        // Each key is unique (duplicate keys are their own refusal) and padded to the
        // requested width.
        let key = format!("{index:0>width$}", width = key_chars);
        let value = format!("{index:0>width$}", width = value_chars);
        packed += key.len() + value.len();
        params.push((format!("Tags.member.{index}.Key"), key));
        params.push((format!("Tags.member.{index}.Value"), value));
    }
    (params, packed)
}

/// The form parameters for `count` managed session policy ARNs of `arn_chars` characters
/// each, plus the bytes they pack into.
fn policy_arns(count: usize, arn_chars: usize) -> (Vec<(String, String)>, usize) {
    let mut params = Vec::new();
    let mut packed = 0;
    for index in 1..=count {
        let prefix = format!("arn:aws:iam::aws:policy/Packed{index}");
        let arn = format!("{prefix}{}", "A".repeat(arn_chars.saturating_sub(prefix.len())));
        packed += arn.len();
        params.push((format!("PolicyArns.member.{index}.arn"), arn));
    }
    (params, packed)
}

/// The message AWS words for a packed token over the budget.
fn too_large_message(packed: usize) -> String {
    format!(
        "Serialized token too large for session. Token size of {packed} bytes exceeds the \
         maximum allowed limit of {SESSION_TOKEN_SIZE_LIMIT} bytes."
    )
}

/// One row of the table: what the request packs, and what the one budget rule answers for
/// it.
struct Row {
    case: &'static str,
    action: &'static str,
    params: Vec<(String, String)>,
    /// The bytes the row's session policy, policy ARNs and session tags pack into.
    packed: usize,
}

/// The percentage of the budget `packed` bytes take up, rounded up as the service reports
/// it.
fn percent_of_budget(packed: usize) -> i32 {
    i32::try_from(packed.saturating_mul(100).div_ceil(SESSION_TOKEN_SIZE_LIMIT))
        .expect("a percentage fits in an i32")
}

#[tokio::test]
async fn a_packed_token_is_measured_against_one_budget_and_the_refusal_names_both_figures() {
    let (router, engine) = emulator();
    let (role_arn, _role_id) = seed_demo_role(&engine);
    let assume_params = |extra: Vec<(String, String)>| {
        let mut params = vec![
            ("RoleArn".to_string(), role_arn.clone()),
            ("RoleSessionName".to_string(), "packed".to_string()),
        ];
        params.extend(extra);
        params
    };

    // The recorded case: fifty session tags of 128-character keys and 256-character
    // values.
    let (recorded_tags, recorded_packed) = session_tags(50, 128, 256);
    let (few_tags, few_tags_packed) = session_tags(3, 16, 32);
    let (many_arns, many_arns_packed) = policy_arns(10, 450);
    let (one_arn, one_arn_packed) = policy_arns(1, 60);
    // All three inputs together, one byte over the budget: an inline policy at its
    // plaintext limit, one managed policy ARN and thirteen session tags.
    let mixed_policy_bytes = MAX_SESSION_POLICY_PLAINTEXT_CHARS;
    let mixed_arn = policy_arns(1, 99);
    let mixed_tags = session_tags(13, 50, 100);
    // Session tags alone, one byte over the budget (4097 = 17 x 241).
    let tags_over_budget = session_tags(17, 100, 141);
    let federated_tags = session_tags(20, 128, 256);

    let rows: Vec<Row> = vec![
        Row {
            case: "an inline policy at its plaintext limit",
            action: "AssumeRole",
            params: assume_params(vec![(
                "Policy".to_string(),
                policy_of(MAX_SESSION_POLICY_PLAINTEXT_CHARS),
            )]),
            packed: MAX_SESSION_POLICY_PLAINTEXT_CHARS,
        },
        Row {
            case: "an inline policy one byte under its plaintext limit",
            action: "AssumeRole",
            params: assume_params(vec![(
                "Policy".to_string(),
                policy_of(MAX_SESSION_POLICY_PLAINTEXT_CHARS - 1),
            )]),
            packed: MAX_SESSION_POLICY_PLAINTEXT_CHARS - 1,
        },
        Row {
            case: "session tags one byte over the budget",
            action: "AssumeRole",
            params: assume_params(tags_over_budget.0),
            packed: tags_over_budget.1,
        },
        Row {
            case: "fifty session tags of 128-character keys and 256-character values",
            action: "AssumeRole",
            params: assume_params(recorded_tags),
            packed: recorded_packed,
        },
        Row {
            case: "three small session tags",
            action: "AssumeRole",
            params: assume_params(few_tags),
            packed: few_tags_packed,
        },
        Row {
            case: "ten managed policy ARNs of 450 characters",
            action: "AssumeRole",
            params: assume_params(many_arns),
            packed: many_arns_packed,
        },
        Row {
            case: "one managed policy ARN",
            action: "AssumeRole",
            params: assume_params(one_arn),
            packed: one_arn_packed,
        },
        Row {
            case: "a policy, an ARN and two tags together, one byte over the budget",
            action: "AssumeRole",
            params: assume_params(
                [
                    vec![("Policy".to_string(), policy_of(mixed_policy_bytes))],
                    mixed_arn.0.clone(),
                    mixed_tags.0.clone(),
                ]
                .concat(),
            ),
            packed: SESSION_TOKEN_SIZE_LIMIT + 1,
        },
        Row {
            // `GetFederationToken`'s inline policy is modelled at 1..2048 characters, so
            // its own budget is reached with the session tags it packs beside the policy.
            case: "a federation token whose session tags are over the budget",
            action: "GetFederationToken",
            params: [
                vec![
                    ("Name".to_string(), "packed".to_string()),
                    ("Policy".to_string(), policy_of(500)),
                ],
                federated_tags.0,
            ]
            .concat(),
            packed: 500 + federated_tags.1,
        },
        Row {
            case: "a federation token whose inline policy fits",
            action: "GetFederationToken",
            params: vec![
                ("Name".to_string(), "packed".to_string()),
                ("Policy".to_string(), policy_of(1_000)),
            ],
            packed: 1_000,
        },
    ];

    for row in rows {
        let case = row.case;
        let params: Vec<(&str, String)> = row
            .params
            .iter()
            .map(|(name, value)| (name.as_str(), value.clone()))
            .collect();
        let (status, body) = call(&router, LONG_TERM_KEY, row.action, &params).await;
        let over_budget = row.packed > SESSION_TOKEN_SIZE_LIMIT;
        if over_budget {
            assert_eq!(
                status, 400,
                "case {case:?}: {} packed bytes must be refused, got {status} {body}",
                row.packed
            );
            assert_eq!(
                element(&body, "Code").as_deref(),
                Some("PackedPolicyTooLarge"),
                "case {case:?}: expected <Code>PackedPolicyTooLarge</Code>, got {body}"
            );
            assert_eq!(
                member(case, &body, "Message"),
                too_large_message(row.packed),
                "case {case:?}: the refusal must name the measured size and the limit, got {body}"
            );
            assert!(
                element(&body, "Credentials").is_none(),
                "case {case:?}: a refused mint must vend no credential, got {body}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {case:?}: {} packed bytes are within the budget, got {status} {body}",
                row.packed
            );
            let reported = member(case, &body, "PackedPolicySize");
            assert_eq!(
                reported,
                percent_of_budget(row.packed).to_string(),
                "case {case:?}: expected {} packed bytes as a percentage of the budget, got {body}",
                row.packed
            );
            assert!(
                element(&body, "AccessKeyId").is_some(),
                "case {case:?}: a mint within the budget must vend a credential, got {body}"
            );
        }
    }
}
