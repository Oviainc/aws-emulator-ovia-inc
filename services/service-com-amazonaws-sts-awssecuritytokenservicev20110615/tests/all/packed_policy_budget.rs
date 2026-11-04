// SPDX-License-Identifier: CC0-1.0

//! The packed token budget one mint's session policies and session tags must fit in.
//!
//! RULE: a mint SERIALIZES its inline session policy, the managed policy ARNs it names and
//! its session tags into one packed session token, and the budget of 4,096 bytes applies to
//! the length of THAT token — the serialization's own framing per element included, so the
//! same total content spread across a different number of tags does not pack into the same
//! size. A request whose packed token is within the budget is served and reports its size as
//! the percentage of that budget (`PackedPolicySize`, rounded up); a request whose packed
//! token passes the budget is refused with `PackedPolicyTooLarge` / HTTP 400, and the refusal
//! names the two figures a caller needs to trim to fit — the size its own session state was
//! measured at, and the limit that size passed.
//!
//! The rows differ along what the rule generalises over: WHICH of the three packed inputs
//! carries the bytes (the inline policy, the managed policy ARNs, the session tags, or all
//! three together), how many elements those bytes are spread across, how far over or under
//! the budget the request is (the exact boundary and the smallest step past it), and which
//! operation is minting (`AssumeRole`, `GetFederationToken`). The recorded case — fifty
//! session tags of 128-character keys and 256-character values — is one row.
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

/// The bytes the packed session token of these inputs occupies, derived from the rule rather
/// than from the service: the session state — the inline policy, the managed policy ARNs and
/// the session tags — serialized as the members that carry them, then coded the way a token
/// is carried, which is four characters per three bytes of the serialization (RFC 4648 §4,
/// the final group padded). A request that carries none of the three packs NO token, so its
/// size is zero.
pub(crate) fn packed_token_bytes(
    policy: Option<&str>,
    policy_arns: &[String],
    tags: &[(String, String)],
) -> usize {
    let mut packed = serde_json::Map::new();
    if let Some(policy) = policy {
        packed.insert(
            "Policy".to_string(),
            serde_json::Value::String(policy.to_string()),
        );
    }
    if !policy_arns.is_empty() {
        packed.insert(
            "PolicyArns".to_string(),
            serde_json::Value::Array(
                policy_arns
                    .iter()
                    .map(|arn| serde_json::json!({ "arn": arn }))
                    .collect(),
            ),
        );
    }
    if !tags.is_empty() {
        packed.insert(
            "Tags".to_string(),
            serde_json::Value::Array(
                tags.iter()
                    .map(|(key, value)| serde_json::json!({ "Key": key, "Value": value }))
                    .collect(),
            ),
        );
    }
    if packed.is_empty() {
        return 0;
    }
    4 * serde_json::Value::Object(packed).to_string().len().div_ceil(3)
}

/// The session state one row packs: the three inputs the budget is measured over, which a
/// row states as values rather than as form parameters so the same state produces both the
/// request and the size it is expected to be measured at.
#[derive(Default, Clone)]
struct SessionState {
    policy: Option<String>,
    policy_arns: Vec<String>,
    tags: Vec<(String, String)>,
}

impl SessionState {
    /// `count` session tags whose keys are `key_chars` characters and whose values are
    /// `value_chars` characters. Each key is unique, because duplicate keys are their own
    /// refusal.
    fn of_tags(count: usize, key_chars: usize, value_chars: usize) -> Self {
        Self {
            tags: (1..=count)
                .map(|index| {
                    (
                        format!("{index:0>width$}", width = key_chars),
                        format!("{index:0>width$}", width = value_chars),
                    )
                })
                .collect(),
            ..Self::default()
        }
    }

    /// `count` managed session policy ARNs of `arn_chars` characters each.
    fn of_policy_arns(count: usize, arn_chars: usize) -> Self {
        Self {
            policy_arns: (1..=count)
                .map(|index| {
                    let prefix = format!("arn:aws:iam::aws:policy/Packed{index}");
                    format!(
                        "{prefix}{}",
                        "A".repeat(arn_chars.saturating_sub(prefix.len()))
                    )
                })
                .collect(),
            ..Self::default()
        }
    }

    /// An inline session policy of `bytes` bytes.
    fn of_policy(bytes: usize) -> Self {
        Self {
            policy: Some(policy_of(bytes)),
            ..Self::default()
        }
    }

    /// This state and `other`'s inputs together.
    fn and(mut self, other: Self) -> Self {
        self.policy = other.policy.or(self.policy);
        self.policy_arns.extend(other.policy_arns);
        self.tags.extend(other.tags);
        self
    }

    /// The form parameters a request carrying this session state sends.
    fn params(&self) -> Vec<(String, String)> {
        let mut params = Vec::new();
        if let Some(policy) = &self.policy {
            params.push(("Policy".to_string(), policy.clone()));
        }
        for (index, arn) in self.policy_arns.iter().enumerate() {
            params.push((format!("PolicyArns.member.{}.arn", index + 1), arn.clone()));
        }
        for (index, (key, value)) in self.tags.iter().enumerate() {
            params.push((format!("Tags.member.{}.Key", index + 1), key.clone()));
            params.push((format!("Tags.member.{}.Value", index + 1), value.clone()));
        }
        params
    }

    /// The bytes this session state packs into.
    fn packed(&self) -> usize {
        packed_token_bytes(self.policy.as_deref(), &self.policy_arns, &self.tags)
    }
}

/// The message AWS words for a packed token over the budget.
fn too_large_message(packed: usize) -> String {
    format!(
        "Serialized token too large for session. Token size of {packed} bytes exceeds the \
         maximum allowed limit of {SESSION_TOKEN_SIZE_LIMIT} bytes."
    )
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

    // One row: the case, the minting operation, and the session state it packs.
    let rows: Vec<(&str, &str, SessionState)> = vec![
        (
            "an inline policy at its plaintext limit",
            "AssumeRole",
            SessionState::of_policy(MAX_SESSION_POLICY_PLAINTEXT_CHARS),
        ),
        (
            "an inline policy one byte under its plaintext limit",
            "AssumeRole",
            SessionState::of_policy(MAX_SESSION_POLICY_PLAINTEXT_CHARS - 1),
        ),
        (
            // Ten tags whose serialization is exactly 4,096 bytes once coded: the boundary
            // itself, which is INSIDE the budget.
            "session tags exactly at the budget",
            "AssumeRole",
            SessionState::of_tags(10, 28, 256),
        ),
        (
            // The smallest step past that boundary the coding can take.
            "session tags one coding group over the budget",
            "AssumeRole",
            SessionState::of_tags(8, 105, 256),
        ),
        (
            "seventeen session tags well over the budget",
            "AssumeRole",
            SessionState::of_tags(17, 100, 141),
        ),
        (
            // The recorded case.
            "fifty session tags of 128-character keys and 256-character values",
            "AssumeRole",
            SessionState::of_tags(50, 128, 256),
        ),
        (
            "three small session tags",
            "AssumeRole",
            SessionState::of_tags(3, 16, 32),
        ),
        (
            "ten managed policy ARNs of 450 characters",
            "AssumeRole",
            SessionState::of_policy_arns(10, 450),
        ),
        (
            "one managed policy ARN",
            "AssumeRole",
            SessionState::of_policy_arns(1, 60),
        ),
        (
            "a policy, an ARN and thirteen tags together",
            "AssumeRole",
            SessionState::of_policy(MAX_SESSION_POLICY_PLAINTEXT_CHARS)
                .and(SessionState::of_policy_arns(1, 99))
                .and(SessionState::of_tags(13, 50, 100)),
        ),
        (
            // `GetFederationToken`'s inline policy is modelled at 1..2048 characters, so
            // its own budget is reached with the session tags it packs beside the policy.
            "a federation token whose session tags are over the budget",
            "GetFederationToken",
            SessionState::of_policy(500).and(SessionState::of_tags(20, 128, 256)),
        ),
        (
            "a federation token whose inline policy fits",
            "GetFederationToken",
            SessionState::of_policy(1_000),
        ),
    ];

    for (case, action, state) in rows {
        let packed = state.packed();
        let mut params: Vec<(String, String)> = match action {
            "AssumeRole" => vec![
                ("RoleArn".to_string(), role_arn.clone()),
                ("RoleSessionName".to_string(), "packed".to_string()),
            ],
            _ => vec![("Name".to_string(), "packed".to_string())],
        };
        params.extend(state.params());
        let params: Vec<(&str, String)> = params
            .iter()
            .map(|(name, value)| (name.as_str(), value.clone()))
            .collect();
        let (status, body) = call(&router, LONG_TERM_KEY, action, &params).await;
        let over_budget = packed > SESSION_TOKEN_SIZE_LIMIT;
        if over_budget {
            assert_eq!(
                status, 400,
                "case {case:?}: {packed} packed bytes must be refused, got {status} {body}"
            );
            assert_eq!(
                element(&body, "Code").as_deref(),
                Some("PackedPolicyTooLarge"),
                "case {case:?}: expected <Code>PackedPolicyTooLarge</Code>, got {body}"
            );
            assert_eq!(
                member(case, &body, "Message"),
                too_large_message(packed),
                "case {case:?}: the refusal must name the measured size {packed} and the limit, \
                 got {body}"
            );
            assert!(
                element(&body, "Credentials").is_none(),
                "case {case:?}: a refused mint must vend no credential, got {body}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {case:?}: {packed} packed bytes are within the budget, got {status} {body}"
            );
            let reported = member(case, &body, "PackedPolicySize");
            assert_eq!(
                reported,
                percent_of_budget(packed).to_string(),
                "case {case:?}: expected {packed} packed bytes as {}% of the budget, got \
                 {reported:?} in {body}",
                percent_of_budget(packed)
            );
            assert!(
                element(&body, "AccessKeyId").is_some(),
                "case {case:?}: a mint within the budget must vend a credential, got {body}"
            );
        }
    }
}

/// RULE (the same one, stated as the invariant that tells the measurement apart from a sum
/// of field lengths): the packed size is the length of the SERIALIZED token, so it grows
/// with the number of elements the same content is spread across, and it grows as any one of
/// them grows.
#[tokio::test]
async fn the_packed_size_counts_the_serialization_and_not_just_its_contents() {
    let (router, engine) = emulator();
    let (role_arn, _role_id) = seed_demo_role(&engine);

    // One row: the case, the session state, and the state its packed size must be strictly
    // larger than — the same content in fewer elements, or the same shape with one byte
    // less.
    let rows: Vec<(&str, SessionState, SessionState)> = vec![
        (
            "sixteen tags against eight of twice the value length",
            SessionState::of_tags(16, 8, 16),
            SessionState::of_tags(8, 8, 32),
        ),
        (
            "four tags against one carrying the same total content",
            SessionState::of_tags(4, 8, 24),
            SessionState::of_tags(1, 8, 96),
        ),
        (
            "two managed policy ARNs against one of their combined length",
            SessionState::of_policy_arns(2, 50),
            SessionState::of_policy_arns(1, 100),
        ),
        (
            "a longer tag value",
            SessionState::of_tags(2, 8, 17),
            SessionState::of_tags(2, 8, 16),
        ),
        (
            // The coding advances in groups of three bytes, so the step that must be
            // visible is a group rather than a single byte.
            "a longer inline policy",
            SessionState::of_policy(203),
            SessionState::of_policy(200),
        ),
    ];

    for (case, larger, smaller) in rows {
        let (larger_packed, smaller_packed) = (larger.packed(), smaller.packed());
        assert!(
            larger_packed > smaller_packed,
            "case {case:?}: expected a packed size over {smaller_packed}, got {larger_packed}"
        );
        // And the service reports the measurement the rule predicts, for both of them.
        for state in [&larger, &smaller] {
            let mut params: Vec<(String, String)> = vec![
                ("RoleArn".to_string(), role_arn.clone()),
                ("RoleSessionName".to_string(), "packed".to_string()),
            ];
            params.extend(state.params());
            let params: Vec<(&str, String)> = params
                .iter()
                .map(|(name, value)| (name.as_str(), value.clone()))
                .collect();
            let (status, body) = call(&router, LONG_TERM_KEY, "AssumeRole", &params).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected a mint, got {status} {body}"
            );
            let reported = member(case, &body, "PackedPolicySize");
            assert_eq!(
                reported,
                percent_of_budget(state.packed()).to_string(),
                "case {case:?}: expected {} packed bytes as {}% of the budget, got {reported:?} \
                 in {body}",
                state.packed(),
                percent_of_budget(state.packed())
            );
        }
    }
}
