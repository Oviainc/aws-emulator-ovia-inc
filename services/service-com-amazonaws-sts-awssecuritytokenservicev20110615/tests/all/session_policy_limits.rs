// SPDX-License-Identifier: CC0-1.0

//! The three rules a mint's session policy, its packed measurement and its refusal follow.
//!
//! * A mint's inline session policy has a PLAINTEXT limit of its own — "The plaintext that
//!   you use for both inline and managed session policies can't exceed 2,048 characters"
//!   (`AssumeRoleRequest$Policy`) — counted in characters and answered BEFORE anything is
//!   packed: a document over it is a validation error naming the member and the bound,
//!   never the packed-size refusal, so "your policy text is too long" and "your session
//!   state does not fit" stay two distinct refusals a client can branch on.
//! * `PackedPolicySize` is a MEASUREMENT of the mint, so every mint whose response models
//!   it reports one: the percentage of the token budget the request's session policies and
//!   session tags occupy, which is zero for a request that packs none of them.
//! * A constraint refusal is worded from the REQUEST: the value between the quotes is the
//!   one the caller submitted — up to the ceiling one clause may carry, past which a prefix
//!   of it plus the marker naming its whole byte count, so no message is sized by the
//!   request — and a `@pattern` requirement is quoted as the model states the expression.
//!
//! Each rule is asserted over a table whose rows differ along what it generalises over —
//! which operation mints, which of the three packed inputs carries the bytes, where the
//! value sits relative to the bound, whether the document's characters are one byte each,
//! and which class of constraint was broken. The recorded cases (an over-length inline
//! policy on `AssumeRole`, an `AssumeRole` carrying managed policy ARNs and no inline
//! policy, a 257-character session tag value, a `GetFederationToken` name outside its
//! pattern) are rows among them.

use super::malformed_session_policy::decode_xml_text;
use super::packed_policy_budget::{packed_token_bytes, policy_of};
use super::sts_sessions::{call, element, emulator, member, seed_demo_role, LONG_TERM_KEY};

/// The role every row mints against, seeded in the IAM store of the shared engine.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/demo";

/// The most characters an inline session policy's plaintext may carry, as the operation
/// documents it (`AssumeRoleRequest$Policy` / `GetFederationTokenRequest$Policy`, and the
/// `@length` maximum the model puts on `sessionPolicyDocumentType`).
const MAX_PLAINTEXT_CHARS: usize = 2048;

/// The packed token budget: the model's `minimumSessionTokenSizeType` range maximum.
const SESSION_TOKEN_SIZE_LIMIT: usize = 4096;

/// The clause AWS states for one broken constraint: the value as submitted, the member
/// path it sits at, and the requirement it broke.
fn clause(value: &str, path: &str, requirement: &str) -> String {
    format!(
        "1 validation error detected: Value '{value}' at '{path}' failed to satisfy \
         constraint: {requirement}"
    )
}

/// The requirement a `@length` maximum states.
fn at_most(max: usize) -> String {
    format!("Member must have length less than or equal to {max}")
}

/// A JSON session policy whose plaintext is exactly `chars` CHARACTERS and whose
/// characters are two bytes each, so its character count and its byte count differ.
fn multibyte_policy_of(chars: usize) -> String {
    let skeleton = r#"{"Version":"2012-10-17","Comment":""}"#;
    let document = format!(
        r#"{{"Version":"2012-10-17","Comment":"{}"}}"#,
        // U+00E9, inside the `\u0020-\u00FF` class the member's `@pattern` admits and two
        // bytes in UTF-8.
        "é".repeat(chars - skeleton.chars().count())
    );
    assert_eq!(
        document.chars().count(),
        chars,
        "case multibyte_policy_of({chars}): built a document of {} characters",
        document.chars().count()
    );
    document
}

/// The percentage of the budget `bytes` take up, rounded up as the service reports it.
fn percent_of_budget(bytes: usize) -> i32 {
    i32::try_from(bytes.saturating_mul(100).div_ceil(SESSION_TOKEN_SIZE_LIMIT))
        .expect("a percentage fits in an i32")
}

/// The form parameters `action` needs beside the ones a row names.
fn mint_params(action: &str) -> Vec<(&'static str, String)> {
    match action {
        "AssumeRole" => vec![
            ("RoleArn", ROLE_ARN.to_string()),
            ("RoleSessionName", "probe".to_string()),
        ],
        _ => vec![("Name", "bob".to_string())],
    }
}

/// RULE: an inline session policy whose PLAINTEXT passes the documented 2,048-character
/// limit is refused as a validation error naming the member and the bound, and quoting the
/// document as submitted — whichever operation takes the policy, and whether or not the
/// model declares that maximum on the member's own shape. A document within the limit is
/// minted, even when its characters are more than one byte each.
#[tokio::test]
async fn an_inline_policy_over_its_plaintext_limit_is_refused_before_anything_is_packed() {
    let (router, engine) = emulator();
    let (_role_arn, _role_id) = seed_demo_role(&engine);

    // One row: the case, the minting operation, the inline policy it carries, and whether
    // the plaintext limit refuses it.
    let rows: Vec<(&str, &str, String, bool)> = vec![
        (
            // `AssumeRole`'s member targets `unrestrictedSessionPolicyDocumentType`, which
            // declares no `@length` maximum, so the documented limit is the service's to
            // enforce.
            "assume-role, a policy at the plaintext limit",
            "AssumeRole",
            policy_of(MAX_PLAINTEXT_CHARS),
            false,
        ),
        (
            "assume-role, a policy one character over the plaintext limit",
            "AssumeRole",
            policy_of(MAX_PLAINTEXT_CHARS + 1),
            true,
        ),
        (
            // The recorded case: a document far past the limit, which also packs past the
            // token budget — the length refusal is the one AWS answers.
            "assume-role, a policy far past the plaintext limit",
            "AssumeRole",
            policy_of(8_840),
            true,
        ),
        (
            // Two bytes per character: the limit is on CHARACTERS, so a document of 1,500
            // of them is within it even though it occupies 2,963 bytes — more than the
            // limit would admit if it counted bytes — and its packed token still fits the
            // separate byte budget, which is the other rule.
            "assume-role, a multi-byte policy within the plaintext limit",
            "AssumeRole",
            multibyte_policy_of(1_500),
            false,
        ),
        (
            "assume-role, a multi-byte policy one character over the plaintext limit",
            "AssumeRole",
            multibyte_policy_of(MAX_PLAINTEXT_CHARS + 1),
            true,
        ),
        (
            // `GetFederationToken`'s member targets `sessionPolicyDocumentType`, whose
            // `@length` maximum IS the same limit: the framework refuses it, and the
            // wording is the same clause.
            "federation-token, a policy at the plaintext limit",
            "GetFederationToken",
            policy_of(MAX_PLAINTEXT_CHARS),
            false,
        ),
        (
            "federation-token, a policy one character over the plaintext limit",
            "GetFederationToken",
            policy_of(MAX_PLAINTEXT_CHARS + 1),
            true,
        ),
    ];

    for (case, action, policy, refused) in rows {
        let mut params = mint_params(action);
        params.push(("Policy", policy.clone()));
        let (status, body) = call(&router, LONG_TERM_KEY, action, &params).await;
        if refused {
            assert_eq!(
                status, 400,
                "case {case:?}: a policy of {} characters must be refused, got {status} {body}",
                policy.chars().count()
            );
            let code = element(&body, "Code");
            assert_eq!(
                code.as_deref(),
                Some("ValidationError"),
                "case {case:?}: expected <Code>ValidationError</Code>, got {code:?} in {body}"
            );
            let message = decode_xml_text(&member(case, &body, "Message"));
            assert_eq!(
                message,
                clause(&policy, "policy", &at_most(MAX_PLAINTEXT_CHARS)),
                "case {case:?}: expected the member and the bound, got {message:?}"
            );
            assert!(
                element(&body, "Credentials").is_none(),
                "case {case:?}: a refused mint must vend no credential, got {body}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {case:?}: a policy of {} characters is within the limit, got {status} \
                 {body}",
                policy.chars().count()
            );
            let packed = packed_token_bytes(Some(&policy), &[], &[]);
            let reported = member(case, &body, "PackedPolicySize");
            assert_eq!(
                reported,
                percent_of_budget(packed).to_string(),
                "case {case:?}: expected {packed} packed bytes as a percentage of the budget, got \
                 {reported:?} in {body}"
            );
        }
    }
}

/// RULE: the clause a refusal states quotes the submitted value up to the ceiling ONE
/// clause may carry, and past that a prefix of it plus the marker naming the whole
/// document's byte count — so the refusal of a member the model puts no maximum on is
/// bounded by that ceiling rather than by the request.
#[tokio::test]
async fn a_policy_too_long_to_quote_is_refused_with_a_bounded_message() {
    let (router, engine) = emulator();
    let (_role_arn, _role_id) = seed_demo_role(&engine);
    // Far past any ceiling a clause carries, and far past the packed token budget: the
    // plaintext limit is still the rule that answers.
    let policy = policy_of(1024 * 1024);
    let mut params = mint_params("AssumeRole");
    params.push(("Policy", policy.clone()));
    let case = "an inline policy of a mebibyte";

    let (status, body) = call(&router, LONG_TERM_KEY, "AssumeRole", &params).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected a refusal, got {status} with {} bytes of body",
        body.len()
    );
    assert_eq!(
        element(&body, "Code").as_deref(),
        Some("ValidationError"),
        "case {case:?}: expected <Code>ValidationError</Code>, got {:?}",
        element(&body, "Code")
    );
    let message = decode_xml_text(&member(case, &body, "Message"));
    assert!(
        message.len() < policy.len() / 2,
        "case {case:?}: a {} byte document must not be quoted whole, got a {} byte message",
        policy.len(),
        message.len()
    );
    assert!(
        message.starts_with(&format!(
            "1 validation error detected: Value '{}",
            &policy[..64]
        )),
        "case {case:?}: the clause must quote the document it refused, got {:?}",
        &message[..message.len().min(96)]
    );
    assert!(
        message.ends_with(&format!(
            "... [{} bytes]' at 'policy' failed to satisfy constraint: {}",
            policy.len(),
            at_most(MAX_PLAINTEXT_CHARS)
        )),
        "case {case:?}: the clause must name the whole document's size and the bound, got the \
         tail {:?}",
        &message[message.len().saturating_sub(96)..]
    );
    assert!(
        element(&body, "Credentials").is_none(),
        "case {case:?}: a refused mint must vend no credential"
    );
}

/// RULE: `PackedPolicySize` is the percentage of the token budget the request's session
/// policies and session tags occupy, and every mint whose response models the member
/// reports it — zero for a request that packs none of them, so two mints AWS answers alike
/// do not differ in the shape of their response.
#[tokio::test]
async fn every_mint_reports_the_packed_size_it_measured() {
    let (router, engine) = emulator();
    let (_role_arn, _role_id) = seed_demo_role(&engine);
    let policy = policy_of(600);
    let managed_arn = "arn:aws:iam::aws:policy/EmulatorProbePolicy01";
    let tag_key = "team";
    let tag_value = "platform";

    // One row: the case, the minting operation, the packed inputs it carries, and the
    // bytes those inputs pack into.
    type PackedRow = (
        &'static str,
        &'static str,
        Vec<(&'static str, String)>,
        usize,
    );
    let one_tag = [(tag_key.to_string(), tag_value.to_string())];
    let one_arn = [managed_arn.to_string()];
    let rows: Vec<PackedRow> = vec![
        ("assume-role, nothing packed at all", "AssumeRole", vec![], 0),
        (
            "assume-role, an inline policy only",
            "AssumeRole",
            vec![("Policy", policy.clone())],
            packed_token_bytes(Some(&policy), &[], &[]),
        ),
        (
            // The recorded case: managed policy ARNs and no inline policy.
            "assume-role, a managed policy ARN only",
            "AssumeRole",
            vec![("PolicyArns.member.1.arn", managed_arn.to_string())],
            packed_token_bytes(None, &one_arn, &[]),
        ),
        (
            "assume-role, a session tag only",
            "AssumeRole",
            vec![
                ("Tags.member.1.Key", tag_key.to_string()),
                ("Tags.member.1.Value", tag_value.to_string()),
            ],
            packed_token_bytes(None, &[], &one_tag),
        ),
        (
            "assume-role, a policy, an ARN and a tag together",
            "AssumeRole",
            vec![
                ("Policy", policy.clone()),
                ("PolicyArns.member.1.arn", managed_arn.to_string()),
                ("Tags.member.1.Key", tag_key.to_string()),
                ("Tags.member.1.Value", tag_value.to_string()),
            ],
            packed_token_bytes(Some(&policy), &one_arn, &one_tag),
        ),
        (
            "federation-token, nothing packed at all",
            "GetFederationToken",
            vec![],
            0,
        ),
        (
            "federation-token, an inline policy only",
            "GetFederationToken",
            vec![("Policy", policy.clone())],
            packed_token_bytes(Some(&policy), &[], &[]),
        ),
        (
            "federation-token, a managed policy ARN and a tag",
            "GetFederationToken",
            vec![
                ("PolicyArns.member.1.arn", managed_arn.to_string()),
                ("Tags.member.1.Key", tag_key.to_string()),
                ("Tags.member.1.Value", tag_value.to_string()),
            ],
            packed_token_bytes(None, &one_arn, &one_tag),
        ),
    ];

    for (case, action, packed_params, packed_bytes) in rows {
        let mut params = mint_params(action);
        params.extend(packed_params);
        let (status, body) = call(&router, LONG_TERM_KEY, action, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a mint, got {status} {body}"
        );
        let reported = element(&body, "PackedPolicySize");
        assert_eq!(
            reported.as_deref(),
            Some(percent_of_budget(packed_bytes).to_string().as_str()),
            "case {case:?}: expected {packed_bytes} packed bytes reported as \
             {}%, got {reported:?} in {body}",
            percent_of_budget(packed_bytes)
        );
    }
}

/// RULE: a constraint refusal is built from the request — the value between the quotes is
/// the one the caller submitted, whatever its length, and a `@pattern` requirement quotes
/// the expression the model states, without the anchors a whole-value pattern wraps it in.
#[tokio::test]
async fn a_constraint_refusal_echoes_the_submitted_value_and_the_modelled_pattern() {
    let (router, engine) = emulator();
    let (_role_arn, _role_id) = seed_demo_role(&engine);
    // The character class `userNameType` and `roleSessionNameType` are modelled over,
    // quoted as the model writes it (`smithy_query(query="operation",
    // operation="GetFederationToken")`: `^[\w+=,.@-]*$`).
    let name_pattern = r"Member must satisfy regular expression pattern: [\w+=,.@-]*";
    let long_name = "n".repeat(33);
    let long_tag_value = "v".repeat(257);
    let over_long_policy = policy_of(8_840);

    // One row: the case, the operation, the parameters, the member the clause names and
    // the value and requirement it must state.
    type RefusalRow = (
        &'static str,
        &'static str,
        Vec<(&'static str, String)>,
        &'static str,
        String,
        String,
    );
    let rows: Vec<RefusalRow> = vec![
        (
            // The recorded case: a federated name outside its character class.
            "a federation name carrying a character outside its class",
            "GetFederationToken",
            vec![("Name", "bob smith".to_string())],
            "name",
            "bob smith".to_string(),
            name_pattern.to_string(),
        ),
        (
            "a federation name of punctuation outside its class",
            "GetFederationToken",
            vec![("Name", "bob/smith".to_string())],
            "name",
            "bob/smith".to_string(),
            name_pattern.to_string(),
        ),
        (
            "a federation name one character over its length",
            "GetFederationToken",
            vec![("Name", long_name.clone())],
            "name",
            long_name.clone(),
            at_most(32),
        ),
        (
            "a role session name outside its character class",
            "AssumeRole",
            vec![
                ("RoleArn", ROLE_ARN.to_string()),
                ("RoleSessionName", "sess ion".to_string()),
            ],
            "roleSessionName",
            "sess ion".to_string(),
            name_pattern.to_string(),
        ),
        (
            // The recorded case: a 257-character session tag value, echoed in full rather
            // than as a byte count.
            "a session tag value one character over its length",
            "AssumeRole",
            vec![
                ("RoleArn", ROLE_ARN.to_string()),
                ("RoleSessionName", "probe".to_string()),
                ("Tags.member.1.Key", "team".to_string()),
                ("Tags.member.1.Value", long_tag_value.clone()),
            ],
            "tags.1.member.value",
            long_tag_value.clone(),
            at_most(256),
        ),
        (
            // The recorded case: a document of thousands of characters, echoed in full.
            "a federated inline policy of thousands of characters",
            "GetFederationToken",
            vec![
                ("Name", "bob".to_string()),
                ("Policy", over_long_policy.clone()),
            ],
            "policy",
            over_long_policy.clone(),
            at_most(MAX_PLAINTEXT_CHARS),
        ),
    ];

    for (case, action, params, path, value, requirement) in rows {
        let (status, body) = call(&router, LONG_TERM_KEY, action, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a refusal, got {status} {body}"
        );
        let code = element(&body, "Code");
        assert_eq!(
            code.as_deref(),
            Some("ValidationError"),
            "case {case:?}: expected <Code>ValidationError</Code>, got {code:?} in {body}"
        );
        let message = decode_xml_text(&member(case, &body, "Message"));
        assert_eq!(
            message,
            clause(&value, path, &requirement),
            "case {case:?}: expected the submitted value and the modelled requirement, got \
             {message:?}"
        );
        assert!(
            !message.contains(" bytes]"),
            "case {case:?}: a value inside the quoting ceiling must be quoted, not counted, got \
             {message:?}"
        );
        assert!(
            !message.contains('^') && !message.contains("*$"),
            "case {case:?}: a pattern must be quoted without its anchors, got {message:?}"
        );
    }
}
