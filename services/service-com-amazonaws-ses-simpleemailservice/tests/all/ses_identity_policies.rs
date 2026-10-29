// SPDX-License-Identifier: CC0-1.0

//! SES sending-authorization policies, end to end over the wire.
//!
//! Every assertion goes through `MultiServiceRouter::dispatch`, so each call is a
//! real awsQuery request against the service: form-urlencoded body in, XML out,
//! one shared engine behind all of them. What is asserted is the RULE these
//! operations follow over a SET of inputs, not one recorded request:
//!
//! * a policy lives on the identity record the account holds, so
//!   `ListIdentityPolicies` reports exactly the names that were put on that
//!   identity — in one stable (lexicographic) order, whichever order they
//!   arrived in, and whichever spelling (name or ARN) the call names the identity
//!   by;
//! * a put is an UPSERT on the policy name, so repeating one never grows the
//!   list;
//! * the document is stored verbatim: `GetIdentityPolicies` answers the text
//!   that was submitted, byte for byte, and omits a name the identity does not
//!   carry;
//! * a document that is not an IAM resource-based policy, or one past the
//!   service's size ceiling, is refused with the modeled `InvalidPolicy` error
//!   and nothing is stored, and a size refusal reports the size submitted;
//! * the resource element of a sending-authorization policy names an identity by
//!   its ARN, so a value that is not an ARN is refused and the refusal names that
//!   value;
//! * an identity this account has not verified is refused by every one of these
//!   operations ("this operation is for the identity owner only").

use crate::harness::{call, element, elements, emulator, field, map_entries, xml_text, Field};
use smithy_server_runtime::MultiServiceRouter;

/// The ceiling on one policy document: `PutIdentityPolicyRequest$Policy` is "the
/// text of the policy in JSON format. The policy cannot exceed 4 KB", and the
/// service enforces that 4 KB as a round 4,000 bytes — the number its own
/// refusal names. Mirrors `crate::state::MAX_POLICY_BYTES`.
const MAX_POLICY_BYTES: usize = 4_000;

/// The documented size of an identity's policy collection:
/// `GetIdentityPoliciesRequest$PolicyNames` is "a list of the names of policies
/// to be retrieved. You can retrieve a maximum of 20 policies at a time."
const MAX_IDENTITY_POLICIES: usize = 20;

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

/// `PutIdentityPolicy`, as `(status, body)` — the refusal paths assert on both.
async fn put_policy(
    router: &MultiServiceRouter,
    identity: &str,
    name: &str,
    document: &str,
) -> (u16, String) {
    call(
        router,
        "PutIdentityPolicy",
        &[
            field("Identity", identity),
            field("PolicyName", name),
            field("Policy", document),
        ],
    )
    .await
}

/// `PutIdentityPolicy` that must be accepted.
async fn put_accepted(router: &MultiServiceRouter, identity: &str, name: &str, document: &str) {
    let (status, body) = put_policy(router, identity, name, document).await;
    assert_eq!(
        status, 200,
        "case put {name:?} on {identity}: expected 200, got {status} with {body}"
    );
}

/// `ListIdentityPolicies`, as `(status, body)`.
async fn list_policies(router: &MultiServiceRouter, identity: &str) -> (u16, String) {
    call(
        router,
        "ListIdentityPolicies",
        &[field("Identity", identity)],
    )
    .await
}

/// The policy names `ListIdentityPolicies` reports, in the order it reports them.
async fn policy_names(router: &MultiServiceRouter, identity: &str) -> Vec<String> {
    let (status, body) = list_policies(router, identity).await;
    assert_eq!(
        status, 200,
        "case ListIdentityPolicies({identity}): expected 200, got {status} with {body}"
    );
    element(&body, "PolicyNames")
        .map(|list| elements(&list, "member"))
        .unwrap_or_default()
}

/// `GetIdentityPolicies`, as `(status, body)`.
async fn get_policies(
    router: &MultiServiceRouter,
    identity: &str,
    names: &[&str],
) -> (u16, String) {
    let mut params: Vec<Field> = vec![field("Identity", identity)];
    for (index, name) in names.iter().enumerate() {
        params.push(field(format!("PolicyNames.member.{}", index + 1), *name));
    }
    call(router, "GetIdentityPolicies", &params).await
}

/// The `(name, document)` pairs `GetIdentityPolicies` reports.
async fn fetched_policies(
    router: &MultiServiceRouter,
    identity: &str,
    names: &[&str],
) -> Vec<(String, String)> {
    let (status, body) = get_policies(router, identity, names).await;
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
/// `sid` is the statement's optional id, which is also where the size rows pad.
fn policy_with_sid(sid: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Sid\":\"{sid}\",\"Effect\":\"Allow\",\
         \"Principal\":{{\"AWS\":\"arn:aws:iam::000000000000:root\"}},\
         \"Action\":\"ses:SendEmail\",\"Resource\":\"arn:aws:ses:us-east-1:\
         000000000000:identity/example.com\"}}]}}"
    )
}

/// A valid policy whose text is EXACTLY `bytes` long, padded inside its `Sid` —
/// so a row about the size ceiling differs from its neighbour only in size.
fn policy_of_size(bytes: usize) -> String {
    let padding = bytes.saturating_sub(policy_with_sid("").len());
    let document = policy_with_sid(&"a".repeat(padding));
    assert_eq!(
        document.len(),
        bytes,
        "the padded document must be exactly {bytes} bytes"
    );
    document
}

/// The policy every row that is not about the document itself starts from.
fn policy() -> String {
    policy_with_sid("AuthorizeOneSender")
}

/// The sentence the service answers for a document past the size ceiling: the
/// submitted document's size in bytes, then the ceiling it passed, with the
/// trailing space the service's text carries.
fn size_refusal(size: usize) -> String {
    format!("The policy size, {size}, is larger than the max policy size, {MAX_POLICY_BYTES}. ")
}

#[tokio::test]
async fn list_reports_exactly_the_names_put_on_the_identity_in_one_stable_order() {
    // The rows differ along what the rule generalises over: how many policies the
    // identity carries, the order they were put in, the class of identity, and
    // the spelling the listing names the identity by (its name, or its ARN).
    struct Row<'a> {
        case: &'a str,
        identity: &'a str,
        domain: bool,
        put: &'a [&'a str],
        listed_as: &'a str,
        expected: &'a [&'a str],
    }
    let rows = [
        Row {
            case: "an identity carrying no policy at all",
            identity: "nobody@example.com",
            domain: false,
            put: &[],
            listed_as: "nobody@example.com",
            expected: &[],
        },
        Row {
            case: "one policy",
            identity: "sender@example.com",
            domain: false,
            put: &["AuthorizeMarketer"],
            listed_as: "sender@example.com",
            expected: &["AuthorizeMarketer"],
        },
        Row {
            case: "three policies put out of order",
            identity: "many@example.com",
            domain: false,
            put: &["Zeta", "Alpha", "Mu"],
            listed_as: "many@example.com",
            expected: &["Alpha", "Mu", "Zeta"],
        },
        Row {
            case: "a domain identity, listed by its name",
            identity: "example.com",
            domain: true,
            put: &["Delegate_One", "Delegate-Two"],
            listed_as: "example.com",
            // ASCII order: '-' (0x2D) sorts before '_' (0x5F).
            expected: &["Delegate-Two", "Delegate_One"],
        },
        Row {
            case: "a domain identity, listed by its ARN",
            identity: "arn.example.com",
            domain: true,
            put: &["ByArn"],
            listed_as: "arn:aws:ses:us-east-1:000000000000:identity/arn.example.com",
            expected: &["ByArn"],
        },
        Row {
            case: "an email identity, listed by its ARN",
            identity: "arn-sender@example.com",
            domain: false,
            put: &["ByArn"],
            listed_as: "arn:aws:ses:us-west-2:000000000000:identity/arn-sender@example.com",
            expected: &["ByArn"],
        },
    ];

    for row in rows {
        let router = emulator();
        if row.domain {
            verify_domain(&router, row.identity).await;
        } else {
            verify_email(&router, row.identity).await;
        }
        for name in row.put {
            put_accepted(&router, row.identity, name, &policy()).await;
        }
        let listed = policy_names(&router, row.listed_as).await;
        assert_eq!(
            listed, row.expected,
            "case {:?}: expected the stored names {:?}, got {listed:?}",
            row.case, row.expected
        );
    }
}

#[tokio::test]
async fn a_policy_document_is_stored_verbatim_and_read_back_by_name() {
    // The rows differ along the FORM of the document — its whitespace, whether
    // `Statement` is one object or a list, how many statements it carries, the
    // effect, and the form each element is written in — because the rule is that
    // the text is stored as submitted, whatever shape it has.
    let one_statement = "{\"Effect\":\"Allow\",\"Principal\":{\"AWS\":\"*\"},\
                         \"Action\":[\"ses:SendEmail\",\"ses:SendRawEmail\"],\
                         \"Resource\":\"arn:aws:ses:us-east-1:000000000000:identity/example.com\"}";
    let rows: [(&str, String); 5] = [
        ("a compact one-statement policy", policy()),
        (
            "a statement written as the object itself",
            format!("{{\"Version\":\"2012-10-17\",\"Statement\":{one_statement}}}"),
        ),
        (
            "two statements, one of them a Deny",
            format!(
                "{{\"Version\":\"2012-10-17\",\"Statement\":[{one_statement},\
                 {{\"Effect\":\"Deny\",\"Principal\":\"*\",\"NotAction\":\"ses:SendBounce\"}}]}}"
            ),
        ),
        (
            "a document pretty-printed with newlines and indentation",
            format!(
                "{{\n    \"Version\": \"2012-10-17\",\n    \"Statement\": [\n        \
                 {one_statement}\n    ]\n}}\n"
            ),
        ),
        (
            "a document at the size ceiling",
            policy_of_size(MAX_POLICY_BYTES),
        ),
    ];

    for (case, document) in rows {
        let router = emulator();
        verify_email(&router, "sender@example.com").await;
        put_accepted(&router, "sender@example.com", "Authorize", &document).await;

        let listed = policy_names(&router, "sender@example.com").await;
        assert_eq!(
            listed,
            vec!["Authorize".to_string()],
            "case {case:?}: the stored policy must be listed, got {listed:?}"
        );

        // Read back by name, through the identity's ARN this time: the two
        // spellings reach one record.
        let fetched = fetched_policies(
            &router,
            "arn:aws:ses:us-east-1:000000000000:identity/sender@example.com",
            &["Authorize", "NeverPut"],
        )
        .await;
        assert_eq!(
            fetched,
            vec![("Authorize".to_string(), document.clone())],
            "case {case:?}: expected the document as submitted and nothing for a name the \
             identity does not carry, got {fetched:?}"
        );
    }
}

#[tokio::test]
async fn a_repeated_put_upserts_the_policy_of_that_name() {
    let router = emulator();
    verify_email(&router, "sender@example.com").await;
    let first = policy_with_sid("First");
    let second = policy_with_sid("Second");
    put_accepted(&router, "sender@example.com", "Authorize", &first).await;
    put_accepted(&router, "sender@example.com", "Authorize", &second).await;

    let listed = policy_names(&router, "sender@example.com").await;
    assert_eq!(
        listed,
        vec!["Authorize".to_string()],
        "case repeated put: one name must stay one entry, got {listed:?}"
    );
    let fetched = fetched_policies(&router, "sender@example.com", &["Authorize"]).await;
    assert_eq!(
        fetched,
        vec![("Authorize".to_string(), second)],
        "case repeated put: the later document must win, got {fetched:?}"
    );
}

#[tokio::test]
async fn a_document_that_is_not_a_policy_is_refused_and_nothing_is_stored() {
    // The rows span the grammar an IAM resource-based policy document follows —
    // the JSON type of the document, a `Statement` that holds no statement, and
    // each element a statement must carry — plus the one rule about its size.
    let statement = "{\"Effect\":\"Allow\",\"Principal\":\"*\",\"Action\":\"ses:SendEmail\"}";
    // The service reports ONE phrase for the whole "not a policy" class, whether
    // the text is not JSON or is JSON that is not a policy document; the size
    // ceiling is a different rule and says so.
    let not_a_policy = "Unable to parse policy.";
    let too_large = size_refusal(MAX_POLICY_BYTES + 1);
    let rows: [(&str, String, &str); 10] = [
        (
            "text that is not JSON at all",
            "not a policy".to_string(),
            not_a_policy,
        ),
        (
            "a truncated document",
            "{\"Statement\":".to_string(),
            not_a_policy,
        ),
        ("a bare JSON number", "5".to_string(), not_a_policy),
        (
            "a bare JSON string",
            "\"let me send\"".to_string(),
            not_a_policy,
        ),
        (
            "an array of statements with no document around them",
            format!("[{statement}]"),
            not_a_policy,
        ),
        ("an empty object", "{}".to_string(), not_a_policy),
        (
            "a Statement that holds nothing",
            "{\"Version\":\"2012-10-17\",\"Statement\":[]}".to_string(),
            not_a_policy,
        ),
        (
            "a statement with no Effect",
            "{\"Statement\":[{\"Principal\":\"*\",\"Action\":\"ses:SendEmail\"}]}".to_string(),
            not_a_policy,
        ),
        (
            "a statement with no Principal, which a sending authorization names",
            "{\"Statement\":[{\"Effect\":\"Allow\",\"Action\":\"ses:SendEmail\"}]}".to_string(),
            not_a_policy,
        ),
        (
            "one byte past the size ceiling",
            policy_of_size(MAX_POLICY_BYTES + 1),
            &too_large,
        ),
    ];

    for (case, document, expected_message) in rows {
        let router = emulator();
        verify_email(&router, "sender@example.com").await;
        let (status, body) = put_policy(&router, "sender@example.com", "Authorize", &document).await;
        assert_eq!(
            status, 400,
            "case {case:?}: an invalid policy is refused with 400, got {status} with {body}"
        );
        assert!(
            body.contains("<Code>InvalidPolicy</Code>"),
            "case {case:?}: expected the modeled InvalidPolicy code, got {body}"
        );
        assert!(
            body.contains(expected_message),
            "case {case:?}: expected the message {expected_message:?}, got {body}"
        );
        let listed = policy_names(&router, "sender@example.com").await;
        assert!(
            listed.is_empty(),
            "case {case:?}: a refused put must store nothing, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn a_policy_whose_resource_is_not_an_arn_is_refused_and_nothing_is_stored() {
    // A sending authorization policy authorizes sending THROUGH an identity, and an
    // identity is named by its ARN, so the resource element has to name one. The rows
    // differ along what the rule generalises over: which element carries the value,
    // the form it is written in (one string or a list), and the kind of value that is
    // not an ARN — the wildcard the IAM language itself admits, a bare identity name,
    // a glob over a resource path, and a prefix that only looks like an ARN's.
    let own_arn = "arn:aws:ses:us-east-1:000000000000:identity/sender@example.com";
    struct Row<'a> {
        case: &'a str,
        /// The `Resource` (or `NotResource`) element, as JSON.
        element: String,
        /// The value the refusal must name, or `None` for a policy that is accepted.
        offending: Option<&'a str>,
    }
    let rows = [
        Row {
            case: "the identity's own ARN",
            element: format!("\"Resource\":\"{own_arn}\""),
            offending: None,
        },
        Row {
            case: "a list of identity ARNs",
            element: format!("\"Resource\":[\"{own_arn}\",\"{own_arn}\"]"),
            offending: None,
        },
        Row {
            case: "an ARN whose resource path ends in a wildcard",
            element: "\"Resource\":\"arn:aws:ses:us-east-1:000000000000:identity/*\"".to_string(),
            offending: None,
        },
        Row {
            case: "a bare wildcard",
            element: "\"Resource\":\"*\"".to_string(),
            offending: Some("*"),
        },
        Row {
            case: "the identity's name rather than its ARN",
            element: "\"Resource\":\"sender@example.com\"".to_string(),
            offending: Some("sender@example.com"),
        },
        Row {
            case: "a glob over a resource path",
            element: "\"Resource\":\"identity/*\"".to_string(),
            offending: Some("identity/*"),
        },
        Row {
            case: "a prefix that only looks like an ARN's",
            element: "\"Resource\":\"ARN:aws:ses:us-east-1:000000000000:identity/x\"".to_string(),
            offending: Some("ARN:aws:ses:us-east-1:000000000000:identity/x"),
        },
        Row {
            case: "the wildcard second in a list of resources",
            element: format!("\"Resource\":[\"{own_arn}\",\"*\"]"),
            offending: Some("*"),
        },
        Row {
            case: "a NotResource that is not an ARN",
            element: "\"NotResource\":\"*\"".to_string(),
            offending: Some("*"),
        },
    ];

    for row in rows {
        let router = emulator();
        verify_email(&router, "sender@example.com").await;
        let document = format!(
            "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Sid\":\"Authorize\",\
             \"Effect\":\"Allow\",\"Principal\":{{\"AWS\":\"*\"}},\
             \"Action\":\"ses:SendEmail\",{}}}]}}",
            row.element
        );
        let (status, body) = put_policy(&router, "sender@example.com", "Authorize", &document).await;
        let listed = policy_names(&router, "sender@example.com").await;

        let Some(offending) = row.offending else {
            assert_eq!(
                status, 200,
                "case {:?}: a policy naming its resource by ARN is accepted, got {status} with \
                 {body}",
                row.case
            );
            assert_eq!(
                listed,
                vec!["Authorize".to_string()],
                "case {:?}: an accepted policy is stored, got {listed:?}",
                row.case
            );
            continue;
        };

        assert_eq!(
            status, 400,
            "case {:?}: a resource that is not an ARN is refused, got {status} with {body}",
            row.case
        );
        assert_eq!(
            element(&body, "Code").as_deref(),
            Some("InvalidPolicy"),
            "case {:?}: expected the modeled InvalidPolicy code, got {body}",
            row.case
        );
        let expected = format!("Invalid ARN: ARNs must start with 'arn:': {offending}");
        let reported = element(&body, "Message").map(|text| xml_text(&text));
        assert_eq!(
            reported.as_deref(),
            Some(expected.as_str()),
            "case {:?}: expected the message {expected:?}, got {reported:?} in {body}",
            row.case
        );
        assert!(
            listed.is_empty(),
            "case {:?}: a refused put must store nothing, got {listed:?}",
            row.case
        );
    }
}

#[tokio::test]
async fn the_size_ceiling_holds_at_the_byte_and_a_refusal_reports_the_size_submitted() {
    // The rows differ ONLY in the size of an otherwise identical, valid policy
    // document: each side of the ceiling, the byte on either side of it, and
    // documents far past it. Every refusal names the size that was submitted, so
    // no two of them carry the same sentence.
    let rows: [(&str, usize); 6] = [
        ("well under the ceiling", MAX_POLICY_BYTES / 2),
        ("one byte under the ceiling", MAX_POLICY_BYTES - 1),
        ("exactly at the ceiling", MAX_POLICY_BYTES),
        ("one byte past the ceiling", MAX_POLICY_BYTES + 1),
        ("a few hundred bytes past the ceiling", MAX_POLICY_BYTES + 562),
        ("far past the ceiling", MAX_POLICY_BYTES * 4),
    ];

    for (case, size) in rows {
        let router = emulator();
        verify_email(&router, "sender@example.com").await;
        let document = policy_of_size(size);
        let (status, body) =
            put_policy(&router, "sender@example.com", "Authorize", &document).await;

        let accepted = size <= MAX_POLICY_BYTES;
        let expected_status = if accepted { 200 } else { 400 };
        assert_eq!(
            status, expected_status,
            "case {case:?} ({size} bytes): expected {expected_status}, got {status} with {body}"
        );

        let listed = policy_names(&router, "sender@example.com").await;
        if accepted {
            assert_eq!(
                listed,
                vec!["Authorize".to_string()],
                "case {case:?} ({size} bytes): an accepted document is stored, got {listed:?}"
            );
            continue;
        }

        assert!(
            body.contains("<Code>InvalidPolicy</Code>"),
            "case {case:?} ({size} bytes): expected the modeled InvalidPolicy code, got {body}"
        );
        let expected_message = size_refusal(size);
        assert!(
            body.contains(&expected_message),
            "case {case:?} ({size} bytes): expected the message {expected_message:?}, got {body}"
        );
        assert!(
            listed.is_empty(),
            "case {case:?} ({size} bytes): an oversized document is not stored, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn an_identity_this_account_does_not_hold_carries_no_policy() {
    let router = emulator();
    // One verified identity carrying a policy, so the answers below are about the
    // identity named, not about an empty store.
    verify_email(&router, "sender@example.com").await;
    put_accepted(&router, "sender@example.com", "Authorize", &policy()).await;

    for unknown in [
        "stranger@example.com",
        "not-verified.example",
        "arn:aws:ses:us-east-1:000000000000:identity/stranger.example",
    ] {
        // `ListIdentityPolicies` models no error and has to answer a required
        // list, and the recorded service answers an identity it does not hold
        // with an empty one: it is reported as carrying no policy.
        let (status, body) = list_policies(&router, unknown).await;
        assert_eq!(
            status, 200,
            "case ListIdentityPolicies({unknown}): an identity this account does not hold \
             carries no policy, got {status} with {body}"
        );
        assert!(
            !body.contains("Authorize"),
            "case ListIdentityPolicies({unknown}): expected no policy reported, got {body}"
        );

        // The other two keep the refusal their own documentation states: reading
        // a document is "for the identity owner only", and a write has no
        // identity to attach the policy to.
        let calls = [
            (
                "GetIdentityPolicies",
                get_policies(&router, unknown, &["Authorize"]).await,
            ),
            (
                "PutIdentityPolicy",
                put_policy(&router, unknown, "Authorize", &policy()).await,
            ),
        ];
        for (operation, (status, body)) in calls {
            assert_eq!(
                status, 400,
                "case {operation}({unknown}): an identity this account has not verified is \
                 refused, got {status} with {body}"
            );
            assert!(
                body.contains("<Code>ValidationException</Code>"),
                "case {operation}({unknown}): expected the refusal code, got {body}"
            );
        }
    }

    // The verified identity is untouched by any of those calls.
    let listed = policy_names(&router, "sender@example.com").await;
    assert_eq!(
        listed,
        vec!["Authorize".to_string()],
        "case after the calls above: the verified identity keeps its policy, got {listed:?}"
    );
}

#[tokio::test]
async fn an_identity_holds_at_most_the_documented_number_of_policies() {
    let router = emulator();
    verify_email(&router, "sender@example.com").await;
    // Two digits, so the names sort the same way the numbers do.
    let name = |index: usize| format!("Policy{index:02}");
    for index in 1..=MAX_IDENTITY_POLICIES {
        put_accepted(&router, "sender@example.com", &name(index), &policy()).await;
    }
    let listed = policy_names(&router, "sender@example.com").await;
    assert_eq!(
        listed.len(),
        MAX_IDENTITY_POLICIES,
        "case at the limit: expected {MAX_IDENTITY_POLICIES} policies, got {listed:?}"
    );

    // One past the limit is refused …
    let beyond = name(MAX_IDENTITY_POLICIES + 1);
    let (status, body) = put_policy(&router, "sender@example.com", &beyond, &policy()).await;
    assert_eq!(
        status, 400,
        "case one past the limit: expected a refusal, got {status} with {body}"
    );
    // … while REPLACING a policy the identity already carries still is not,
    // because an upsert adds nothing to the store.
    put_accepted(
        &router,
        "sender@example.com",
        &name(1),
        &policy_with_sid("Replaced"),
    )
    .await;
    let listed = policy_names(&router, "sender@example.com").await;
    assert_eq!(
        listed.len(),
        MAX_IDENTITY_POLICIES,
        "case after the refusal and the replacement: the store stays at its limit, got {listed:?}"
    );
    assert!(
        !listed.contains(&beyond),
        "case one past the limit: the refused name must not be stored, got {listed:?}"
    );
}

#[tokio::test]
async fn a_policy_name_outside_its_documented_character_set_is_refused() {
    let router = emulator();
    verify_email(&router, "sender@example.com").await;
    // `PolicyName` "can only include alphanumeric characters, dashes, and
    // underscores", so a row carrying anything else is refused; the accepted
    // rows span the three admitted classes.
    let rows = [
        ("letters", "Authorize", true),
        ("digits", "Policy2024", true),
        ("a dash and an underscore", "Authorize-Marketer_2", true),
        ("a dot", "authorize.marketer", false),
        ("a space", "authorize marketer", false),
        ("a slash", "authorize/marketer", false),
        ("a non-ASCII letter", "autoriser\u{e9}", false),
    ];
    for (case, name, accepted) in rows {
        let (status, body) = put_policy(&router, "sender@example.com", name, &policy()).await;
        let expected = if accepted { 200 } else { 400 };
        assert_eq!(
            status, expected,
            "case {case:?} ({name:?}): expected {expected}, got {status} with {body}"
        );
    }
    let listed = policy_names(&router, "sender@example.com").await;
    assert_eq!(
        listed,
        vec![
            "Authorize".to_string(),
            "Authorize-Marketer_2".to_string(),
            "Policy2024".to_string()
        ],
        "case accepted names: only the admitted names are stored, got {listed:?}"
    );
}
