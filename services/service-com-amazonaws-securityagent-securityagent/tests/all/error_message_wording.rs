// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS the refusals of a request it cannot serve, end to end over the
//! wire: the pattern a validation refusal quotes, the order a refusal lists an enum's
//! value set in, and the sentence a lookup that resolves no resource states.
//!
//! Every call goes through the real `MultiServiceRouter` the emulator boots (see
//! `crate::support`), so a message asserted here is the one a client reads off the wire.
//!
//! Each case asserts a RULE over a TABLE of inputs rather than the one recorded call: the
//! rows differ along the SHAPE of the rejected value (which part of the grammar it misses,
//! which casing it carries, whether it is a near miss of a stored identifier), which the
//! rule generalises over, and the whole message is compared — the fixed text of the class
//! plus only the parts the request supplies.

use serde_json::{json, Value};
use service_com_amazonaws_securityagent_securityagent::state::{
    default_vpc_id, region_subnets, VERIFICATION_METHOD_DNS_TXT, VERIFICATION_METHOD_HTTP_ROUTE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{call, call_in, emulator, member, ok};

/// The region every case signs for, so the account's network resolves in it.
const REGION: &str = "us-east-1";

/// The path `InitiateProviderRegistration` is bound to, which is not this service's usual
/// `/<OperationName>` (source: `smithy_query(query="operation",
/// operation="InitiateProviderRegistration")` — `POST /oauth2/provider/register`), so the
/// shared harness is pointed at it rather than at the operation name.
const PROVIDER_REGISTRATION_PATH: &str = "oauth2/provider/register";

/// The pattern a refusal of a `resourceConfigurationId` quotes, as the SERVICE publishes
/// it: the union of VPC Lattice's bare-id and ARN grammars, with the twelve-digit account
/// segment spelled with the `\d` shorthand.
///
/// Source: the pattern source text the service's own refusal reports. The member carries
/// no `@pattern` in this service's model
/// (`smithy_query(query="shape", shape="ResourceConfigurationId")`), so the expression is
/// not derivable from the model and is written out here, independently of the constant the
/// handler reports it from.
const QUOTED_PATTERN: &str = concat!(
    r"((rcfg-[0-9a-z]{17})|",
    r"(arn:[a-z0-9\-]+:vpc-lattice:[a-zA-Z0-9\-]+:\d{12}:",
    r"resourceconfiguration/rcfg-[0-9a-z]{17}))",
);

/// The `Provider` value set as a refusal LISTS it, which is not the order the public model
/// declares the enum in (`smithy_query(query="shape", shape="Provider")` gives
/// `[GITHUB, GITLAB, BITBUCKET, CONFLUENCE, AZURE_DEVOPS, JIRA]`). Source: the recorded
/// refusal of a `provider` outside the enum.
const PROVIDER_VALUE_SET: &str = "GITHUB, AZURE_DEVOPS, JIRA, BITBUCKET, CONFLUENCE, GITLAB";

/// Every value the `Provider` enum admits
/// (`smithy_query(query="shape", shape="Provider")`), so the rows that must NOT be refused
/// span the whole value set rather than the one the recording used.
const PROVIDERS: [&str; 6] = [
    "GITHUB",
    "GITLAB",
    "BITBUCKET",
    "CONFLUENCE",
    "AZURE_DEVOPS",
    "JIRA",
];

/// The sentence a refusal of a private connection the account does not hold states.
/// Source: the recorded 404 for an unknown `privateConnectionName`.
const PRIVATE_CONNECTION_NOT_FOUND: &str = "Private Connection not found: null";

/// The sentence a refusal of a target domain the account does not hold states.
/// Source: the recorded 404 for an unknown `targetDomainId`.
const TARGET_DOMAIN_NOT_FOUND: &str = "The specified target domain does not exist.";

/// The `message` a refusal carried, naming the case when it carried none.
fn refusal_message(body: &Value, case: &str) -> String {
    member(body, "message")
        .or_else(|| member(body, "Message"))
        .unwrap_or_else(|| panic!("case {case:?}: the refusal carried no message: {body}"))
}

/// One service-managed `mode` over the account's own network, whose create mints the
/// resource configuration a self-managed create can then name.
fn service_managed_mode() -> Value {
    let subnets: Vec<String> = region_subnets(REGION)
        .into_iter()
        .map(|subnet| subnet.subnet_id)
        .take(1)
        .collect();
    json!({"serviceManaged": {
        "hostAddress": "provider.internal.example",
        "vpcId": default_vpc_id(REGION),
        "subnetIds": subnets,
    }})
}

/// `CreatePrivateConnection` for a service-managed `name`, requiring the modelled 201.
async fn create_connection(router: &MultiServiceRouter, name: &str, case: &str) -> Value {
    let answer = call(
        router,
        "CreatePrivateConnection",
        &json!({"privateConnectionName": name, "mode": service_managed_mode()}),
    )
    .await;
    assert_eq!(
        answer.status, 201,
        "case {case:?}: expected CreatePrivateConnection({name:?}) to answer 201, got {} \
         ({}) with {}",
        answer.status, answer.error_code, answer.body
    );
    answer.body
}

/// An identifier of the same length and alphabet as `id` that differs from it in its last
/// character — a near miss of a stored id, whatever characters the service minted it from.
fn near_miss(id: &str) -> String {
    let (head, last) = id.split_at(id.len() - 1);
    let replacement = if last == "a" { "b" } else { "a" };
    format!("{head}{replacement}")
}

/// `CreateTargetDomain`, returning the minted `targetDomainId`.
async fn register_domain(
    router: &MultiServiceRouter,
    domain: &str,
    method: &str,
    case: &str,
) -> String {
    let created = ok(
        router,
        "CreateTargetDomain",
        &json!({"targetDomainName": domain, "verificationMethod": method}),
        case,
    )
    .await;
    member(&created, "targetDomainId")
        .unwrap_or_else(|| panic!("case {case:?}: CreateTargetDomain minted no id: {created}"))
}

/// RULE: a refusal that reports a regular expression quotes the expression's SOURCE text
/// as the service publishes it — never an equivalent respelling of it — inside the
/// validation envelope, whose only variable parts are the error count, the rejected value
/// and the member path, all taken from the request.
#[tokio::test]
async fn a_pattern_refusal_quotes_the_expression_the_service_publishes() {
    let router = emulator();
    // A well-formed id of the right length, so each row below misses the grammar in
    // exactly one way.
    let suffix = "abcdefghij0123456";

    // (case, the identifier the request carries) — the rows differ along WHICH part of the
    // grammar the value misses: the prefix, the suffix length, the suffix alphabet, the
    // ARN's account segment, its service, its resource type, and the whole-value match.
    let rows: Vec<(&str, String)> = vec![
        ("a value of no identifier form at all", "not-an-id".to_string()),
        ("an empty value", String::new()),
        ("the wrong id prefix", format!("rgw-{suffix}")),
        ("a suffix one character short", format!("rcfg-{}", &suffix[1..])),
        ("a suffix one character long", format!("rcfg-{suffix}0")),
        ("an upper-cased suffix", format!("rcfg-{}", suffix.to_uppercase())),
        (
            "a well-formed id inside other text",
            format!("id=rcfg-{suffix}"),
        ),
        (
            "an ARN whose account is eleven digits",
            format!("arn:aws:vpc-lattice:{REGION}:00000000000:resourceconfiguration/rcfg-{suffix}"),
        ),
        (
            "an ARN whose account is not digits",
            format!("arn:aws:vpc-lattice:{REGION}:aaaaaaaaaaaa:resourceconfiguration/rcfg-{suffix}"),
        ),
        (
            "an ARN of another service",
            format!("arn:aws:securityagent:{REGION}:000000000000:resourceconfiguration/rcfg-{suffix}"),
        ),
        (
            "an ARN of another resource type",
            format!("arn:aws:vpc-lattice:{REGION}:000000000000:resourcegateway/rcfg-{suffix}"),
        ),
    ];

    for (index, (case, identifier)) in rows.into_iter().enumerate() {
        let answer = call(
            &router,
            "CreatePrivateConnection",
            &json!({
                "privateConnectionName": format!("refused-{index}"),
                "mode": {"selfManaged": {"resourceConfigurationId": identifier}},
            }),
        )
        .await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException for {identifier:?}, got {} \
             ({}) with {}",
            answer.status,
            answer.error_code,
            answer.body
        );
        let expected = format!(
            "1 validation error detected: Value '{identifier}' at \
             'mode.selfManaged.resourceConfigurationId' failed to satisfy constraint: \
             Member must satisfy regular expression pattern: {QUOTED_PATTERN}"
        );
        let actual = refusal_message(&answer.body, case);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
    }
}

/// RULE: a refusal that reports an enum's value set lists the values in the order the
/// SERVICE declares that enum in, inside the same validation envelope, whose variable
/// parts are the rejected value and the member path; a value the enum does admit is not
/// refused by that constraint at all.
#[tokio::test]
async fn an_enum_refusal_lists_the_value_set_in_the_services_order() {
    let router = emulator();

    // (case, the provider the request carries) — the rows differ along HOW the value
    // misses the set: an unrelated token, the right value in the wrong case, a value with
    // surrounding whitespace, a near miss of a member's spelling, and no value at all.
    let rows: Vec<(&str, String)> = vec![
        ("an unmodelled provider", "NOT_A_REAL_PROVIDER".to_string()),
        ("a lower-cased member", "github".to_string()),
        ("a mixed-case member", "GitHub".to_string()),
        ("an empty value", String::new()),
        ("a member with a trailing space", "GITHUB ".to_string()),
        ("a member spelled with a hyphen", "AZURE-DEVOPS".to_string()),
    ];

    for (case, provider) in rows {
        let answer = call_in(
            &router,
            REGION,
            PROVIDER_REGISTRATION_PATH,
            &json!({"provider": provider}),
        )
        .await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException for {provider:?}, got {} \
             ({}) with {}",
            answer.status,
            answer.error_code,
            answer.body
        );
        let expected = format!(
            "1 validation error detected: Value '{provider}' at 'provider' failed to \
             satisfy constraint: Member must satisfy enum value set: [{PROVIDER_VALUE_SET}]"
        );
        let actual = refusal_message(&answer.body, case);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
    }

    // Every value the enum DOES admit passes that constraint, so the refusals above are
    // about the value set and nothing else.
    for provider in PROVIDERS {
        let answer = call_in(
            &router,
            REGION,
            PROVIDER_REGISTRATION_PATH,
            &json!({"provider": provider, "targetUrl": "https://provider.example"}),
        )
        .await;
        assert_ne!(
            answer.status, 400,
            "case {provider:?}: a value the enum admits must not be refused as a \
             constraint failure, got {} ({}) with {}",
            answer.status, answer.error_code, answer.body
        );
    }
}

/// RULE: a lookup that resolves no private connection is refused with the service's own
/// sentence for that resource class — fixed text that carries nothing the request sent —
/// while a connection the account does hold is still answered from its stored record.
#[tokio::test]
async fn an_unresolved_private_connection_states_the_services_own_sentence() {
    let router = emulator();
    let stored = "stored-connection";
    create_connection(&router, stored, "the connection the account holds").await;

    // (case, the name the request carries) — the rows differ along how the name misses the
    // store: never created, a near miss of a stored name (case, suffix, truncation), and a
    // name no create would have accepted.
    let rows: Vec<(&str, String)> = vec![
        ("a name never created", "no-such-connection".to_string()),
        ("the stored name upper-cased", stored.to_uppercase()),
        ("the stored name with a suffix", format!("{stored}-2")),
        (
            "the stored name with its last character dropped",
            stored[..stored.len() - 1].to_string(),
        ),
        ("a name carrying a space", "stored connection".to_string()),
        ("a name carrying non-ASCII", "stored-connectión".to_string()),
    ];

    for (case, name) in rows {
        let answer = call(
            &router,
            "DescribePrivateConnection",
            &json!({"privateConnectionName": name}),
        )
        .await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected 404 ResourceNotFoundException for {name:?}, got {} \
             ({}) with {}",
            answer.status,
            answer.error_code,
            answer.body
        );
        let actual = refusal_message(&answer.body, case);
        assert_eq!(
            actual, PRIVATE_CONNECTION_NOT_FOUND,
            "case {case:?}: expected {PRIVATE_CONNECTION_NOT_FOUND:?}, got {actual:?}"
        );
        assert!(
            !actual.contains(&name),
            "case {case:?}: the refusal must carry nothing the request sent, got {actual:?}"
        );
    }

    // The name the account DOES hold still reads back out of the store.
    let read = call(
        &router,
        "DescribePrivateConnection",
        &json!({"privateConnectionName": stored}),
    )
    .await;
    assert_eq!(
        (read.status, member(&read.body, "name").as_deref()),
        (200, Some(stored)),
        "case \"the stored connection\": expected 200 naming {stored:?}, got {} with {}",
        read.status,
        read.body
    );
}

/// RULE: the same for a target domain — a delete that resolves no record is refused with
/// the service's own fixed sentence for that class, carrying no identifier from the
/// request, while the record the account does hold is still deletable.
#[tokio::test]
async fn an_unresolved_target_domain_states_the_services_own_sentence() {
    let router = emulator();
    let stored = register_domain(
        &router,
        "stored.example.com",
        VERIFICATION_METHOD_DNS_TXT,
        "the domain the account holds",
    )
    .await;
    let deleted = register_domain(
        &router,
        "deleted.example.com",
        VERIFICATION_METHOD_HTTP_ROUTE,
        "the domain deleted up front",
    )
    .await;
    let gone = call(
        &router,
        "DeleteTargetDomain",
        &json!({"targetDomainId": deleted}),
    )
    .await;
    assert_eq!(
        gone.status, 200,
        "case \"the domain deleted up front\": expected 200, got {} ({}) with {}",
        gone.status, gone.error_code, gone.body
    );

    // (case, the id the request carries) — the rows differ along how the id misses the
    // store: already deleted, never minted, and near misses of a stored id.
    let rows: Vec<(&str, String)> = vec![
        ("an id already deleted", deleted.clone()),
        (
            "an id never minted",
            "ffffffffffffffffffffffffffffffff".to_string(),
        ),
        ("the stored id with its last character changed", near_miss(&stored)),
        ("the stored id with a suffix", format!("{stored}0")),
        (
            "the stored id with its last character dropped",
            stored[..stored.len() - 1].to_string(),
        ),
    ];

    for (case, target_domain_id) in rows {
        assert_ne!(
            target_domain_id, stored,
            "case {case:?}: the row must not name the domain that is kept"
        );
        let answer = call(
            &router,
            "DeleteTargetDomain",
            &json!({"targetDomainId": target_domain_id}),
        )
        .await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected 404 ResourceNotFoundException for \
             {target_domain_id:?}, got {} ({}) with {}",
            answer.status,
            answer.error_code,
            answer.body
        );
        let actual = refusal_message(&answer.body, case);
        assert_eq!(
            actual, TARGET_DOMAIN_NOT_FOUND,
            "case {case:?}: expected {TARGET_DOMAIN_NOT_FOUND:?}, got {actual:?}"
        );
        assert!(
            !actual.contains(&target_domain_id),
            "case {case:?}: the refusal must carry nothing the request sent, got {actual:?}"
        );
    }

    // The id the account DOES hold still names a record, so the refusals above are about
    // the lookup and nothing else.
    let answer = ok(
        &router,
        "DeleteTargetDomain",
        &json!({"targetDomainId": stored}),
        "the domain the account holds",
    )
    .await;
    assert_eq!(
        member(&answer, "targetDomainId").as_deref(),
        Some(stored.as_str()),
        "case \"the domain the account holds\": expected the delete to report {stored:?}, \
         got {answer}"
    );
}
