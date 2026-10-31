// SPDX-License-Identifier: CC0-1.0

//! What an input-validation refusal of this service SAYS, over the wire, driven exactly as
//! an SDK drives it (`POST /` with `X-Amz-Target:
//! AWSCognitoIdentityProviderService.<Op>`).
//!
//! Seven rules, each asserted over a SET of inputs rather than over one request:
//!
//! * a constraint clause quotes a `@pattern` requirement as the expression a value has to
//!   satisfy — the model's text WITHOUT the `^`/`$` it wraps a whole-value match in;
//! * a constraint clause quotes the value the request carried WHOLE, however long, up to
//!   the ceiling this service states from its own model (twice the largest `@length`
//!   maximum any member of it declares), past which the framework's elided rendering
//!   stands;
//! * the domain name a request asks a pool to TAKE is refused with Amazon Cognito's own
//!   prose, naming the characters a domain may carry and the format a name must follow —
//!   while the reader, which merely addresses a name, SERVES any name and reports the
//!   domain it found;
//! * a write that submits user attributes the pool's schema will not take is refused with
//!   the schema validation's own report: one line per offending attribute, naming it under
//!   the `user.` prefix, ending in a newline;
//! * a deletion, which names attributes without values, is refused for a name whose type
//!   the schema does not declare;
//! * a pagination token that names no page of a listing is refused in the fixed text the
//!   answering API words it with;
//! * a `Filter` the filter language cannot be read out of is refused as a PARSE failure,
//!   while one it reads over an attribute no server-side search covers is refused as input
//!   the operation will not accept.
//!
//! The pools, groups, users and tokens are real: they come from `CreateUserPool`,
//! `AdminCreateUser` and the listings themselves. The MFA-preference refusal of the same
//! class is asserted where its fixture lives (`set_user_mfa_preference.rs`).
//!
//! Each assertion names its case and reports the value it saw.

use serde_json::{json, Value};

use super::log_delivery_configuration::{call, error_code, error_message, ok, router};

/// The envelope and clause a refusal of ONE broken constraint states.
fn one_validation_error(value: &str, path: &str, requirement: &str) -> String {
    format!(
        "1 validation error detected: Value '{value}' at '{path}' failed to satisfy \
         constraint: {requirement}"
    )
}

/// The requirement half a `@pattern` violation states, over the expression the clause
/// quotes.
fn must_satisfy(pattern: &str) -> String {
    format!("Member must satisfy regular expression pattern: {pattern}")
}

/// The requirement half a `@length` maximum states.
fn at_most(maximum: usize) -> String {
    format!("Member must have length less than or equal to {maximum}")
}

/// A pool this suite can address, and its minted id.
async fn pool(router: &smithy_server_runtime::MultiServiceRouter, name: &str) -> String {
    let created = ok(router, "CreateUserPool", &json!({ "PoolName": name })).await;
    created["UserPool"]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: a minted pool id: {created}"))
        .to_string()
}

/// One row of the `@pattern` table: the operation, the request, the member the clause
/// names, the value it quotes and the expression it states.
struct PatternRow {
    case: &'static str,
    operation: &'static str,
    body: Value,
    member: &'static str,
    value: &'static str,
    /// The expression the clause quotes: the model's `@pattern` for that member with the
    /// whole-value `^`/`$` dropped. Each is the declared text from
    /// `smithy_query(query="operation",operation="<the row's operation>")`.
    stated: &'static str,
}

#[tokio::test]
async fn a_pattern_requirement_is_quoted_without_the_whole_value_anchors() {
    let router = router();
    let user_pool_id = pool(&router, "pattern-clause-pool").await;

    // The rows span the shapes of this model that declare a pattern, and with them the
    // forms a pattern takes: a character class with an escape (`UserPoolIdType`,
    // `^[\w-]+_[0-9a-zA-Z]+$`), one with a literal `+` in it (`ClientIdType`, `^[\w+]+$`),
    // one over a hexadecimal tail (`DeviceKeyType`, `^[\w-]+_[0-9a-f-]+$`), one carrying
    // punctuation (`UserPoolNameType`, `^[\w\s+=,.@-]+$`) and one over Unicode categories
    // (`GroupNameType`, `^[\p{L}\p{M}\p{S}\p{N}\p{P}]+$`).
    let rows = vec![
        PatternRow {
            case: "a pool name carrying characters the name's pattern excludes",
            operation: "CreateUserPool",
            body: json!({ "PoolName": "bad/pool*name!" }),
            member: "poolName",
            value: "bad/pool*name!",
            stated: r"[\w\s+=,.@-]+",
        },
        PatternRow {
            case: "a user pool id carrying no underscore",
            operation: "GetCSVHeader",
            body: json!({ "UserPoolId": "totally-invalid-pool-id-no-underscore" }),
            member: "userPoolId",
            value: "totally-invalid-pool-id-no-underscore",
            stated: r"[\w-]+_[0-9a-zA-Z]+",
        },
        PatternRow {
            case: "a user pool id carrying a space",
            operation: "DeleteUserPool",
            body: json!({ "UserPoolId": "not a valid pool id!!" }),
            member: "userPoolId",
            value: "not a valid pool id!!",
            stated: r"[\w-]+_[0-9a-zA-Z]+",
        },
        PatternRow {
            case: "an app client id carrying spaces",
            operation: "DescribeUserPoolClient",
            body: json!({
                "UserPoolId": user_pool_id.clone(),
                "ClientId": "not a valid client id !!",
            }),
            member: "clientId",
            value: "not a valid client id !!",
            stated: r"[\w+]+",
        },
        PatternRow {
            case: "a device key whose tail is not hexadecimal",
            operation: "AdminGetDevice",
            body: json!({
                "UserPoolId": user_pool_id.clone(),
                "Username": "ada",
                "DeviceKey": "us-west-2_ghost-device-key",
            }),
            member: "deviceKey",
            value: "us-west-2_ghost-device-key",
            stated: r"[\w-]+_[0-9a-f-]+",
        },
        PatternRow {
            case: "a group name carrying a space",
            operation: "CreateGroup",
            body: json!({ "UserPoolId": user_pool_id.clone(), "GroupName": "bad group name" }),
            member: "groupName",
            value: "bad group name",
            stated: r"[\p{L}\p{M}\p{S}\p{N}\p{P}]+",
        },
    ];

    for row in rows {
        let (status, payload) = call(&router, row.operation, &row.body).await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "InvalidParameterException"),
            "case {:?}: expected a 400 InvalidParameterException, got {status} with {payload}",
            row.case
        );
        let expected = one_validation_error(row.value, row.member, &must_satisfy(row.stated));
        let message = error_message(&payload);
        assert_eq!(
            message, expected,
            "case {:?}: expected {expected:?}, got {message:?}",
            row.case
        );
        assert!(
            !message.contains(&format!("pattern: ^{}$", row.stated)),
            "case {:?}: the anchored form is never stated, got {message:?}",
            row.case
        );
    }
}

/// One row of the quoted-value table: the member, how long a value the row submits, and
/// whether the clause quotes it whole.
struct QuotedRow {
    case: &'static str,
    operation: &'static str,
    member: &'static str,
    /// The `@length` maximum the row breaks, as the requirement states it.
    maximum: usize,
    length: usize,
    /// Whether a clause of this service quotes a value of this length whole.
    quoted_whole: bool,
}

#[tokio::test]
async fn a_constraint_clause_quotes_the_whole_submitted_value() {
    let router = router();
    let user_pool_id = pool(&router, "quoted-value-pool").await;

    // The rows differ along the LENGTH of the submitted value, across each bound that
    // decides how much of it a clause can carry: the framework's own 256-byte elision
    // point, the shared 16 KiB restoring ceiling, the member's own declared maximum
    // (`DescriptionType` 2048 and `CSSType` 131072,
    // `smithy_query(query="shape",shape="DescriptionType,CSSType")`) and the ceiling this
    // service states from the largest of them (twice 131072), one value past which the
    // framework's elided rendering stands.
    let rows = [
        QuotedRow {
            case: "a description one character past its maximum",
            operation: "CreateGroup",
            member: "description",
            maximum: 2048,
            length: 2049,
            quoted_whole: true,
        },
        QuotedRow {
            case: "a description far past the shared 16 KiB restoring ceiling",
            operation: "CreateGroup",
            member: "description",
            maximum: 2048,
            length: 20_000,
            quoted_whole: true,
        },
        QuotedRow {
            case: "a stylesheet one character past its maximum",
            operation: "SetUICustomization",
            member: "cSS",
            maximum: 131_072,
            length: 131_073,
            quoted_whole: true,
        },
        QuotedRow {
            case: "a stylesheet one character past the ceiling this service quotes up to",
            operation: "SetUICustomization",
            member: "cSS",
            maximum: 131_072,
            length: 2 * 131_072 + 1,
            quoted_whole: false,
        },
    ];

    for row in rows {
        let value = "x".repeat(row.length);
        let body = match row.operation {
            "CreateGroup" => json!({
                "UserPoolId": user_pool_id.clone(),
                "GroupName": "quoted-value-group",
                "Description": value.clone(),
            }),
            _ => json!({ "UserPoolId": user_pool_id.clone(), "CSS": value.clone() }),
        };
        let (status, payload) = call(&router, row.operation, &body).await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "InvalidParameterException"),
            "case {:?}: expected a 400 InvalidParameterException, got {status} with the body's \
             code {:?}",
            row.case,
            error_code(&payload)
        );
        let message = error_message(&payload);
        if row.quoted_whole {
            let expected = one_validation_error(&value, row.member, &at_most(row.maximum));
            assert_eq!(
                message.len(),
                expected.len(),
                "case {:?}: expected a message of {} bytes, got one of {} bytes",
                row.case,
                expected.len(),
                message.len()
            );
            assert!(
                message == expected,
                "case {:?}: expected the whole submitted value quoted at {:?}, got a message \
                 opening {:?}",
                row.case,
                row.member,
                message.chars().take(96).collect::<String>()
            );
        } else {
            // Past the ceiling the clause states a prefix of the value and the marker
            // naming the whole value's byte count, so no response is sized by the request.
            assert!(
                message.contains(&format!("... [{} bytes]", value.len()))
                    && !message.contains(value.as_str()),
                "case {:?}: expected a prefix and the byte count, got a message of {} bytes \
                 opening {:?}",
                row.case,
                message.len(),
                message.chars().take(96).collect::<String>()
            );
        }
    }
}

/// The prose Amazon Cognito answers for a domain name a pool cannot take. The format it
/// quotes is `DomainType`'s pattern as the model declares it
/// (`smithy_query(query="shape",shape="DomainType")`) — anchors included, because the prose
/// states the expression a WHOLE name must match.
const DOMAIN_PROSE: &str = "The domain name contains an invalid character. Domain names can \
                            only contain lower-case letters, numbers, and hyphens. Please \
                            enter a different name that follows this format: \
                            ^[a-z0-9](?:[a-z0-9\\-]{0,61}[a-z0-9])?$";

#[tokio::test]
async fn the_name_a_pool_is_asked_to_take_is_refused_with_the_domain_prose() {
    let router = router();
    let user_pool_id = pool(&router, "domain-prose-pool").await;

    // The rows differ along WHICH character the name carries that a domain may not: an
    // underscore, an upper-case letter, a dot, and a hyphen in a position the format
    // excludes.
    let names = [
        ("an underscore", "Invalid_Domain_Name"),
        ("upper-case letters", "AuthPrefix"),
        ("a dot", "auth.example.com"),
        ("a trailing hyphen", "auth-"),
    ];

    for (case, domain) in names {
        // Both operations that ask a pool FOR a name answer the prose.
        for operation in ["CreateUserPoolDomain", "UpdateUserPoolDomain"] {
            let (status, payload) = call(
                &router,
                operation,
                &json!({ "UserPoolId": user_pool_id.clone(), "Domain": domain }),
            )
            .await;
            assert_eq!(
                (status, error_code(&payload), error_message(&payload)),
                (400, "InvalidParameterException", DOMAIN_PROSE),
                "case {case:?} ({operation} of {domain:?}): expected the domain prose, got \
                 {status} with {payload}"
            );
        }

        // The reader addresses a name rather than asking for one, so it answers the domain
        // it found: none, since no create could have taken this name.
        let payload = ok(
            &router,
            "DescribeUserPoolDomain",
            &json!({ "Domain": domain }),
        )
        .await;
        assert_eq!(
            payload["DomainDescription"],
            json!({}),
            "case {case:?}: the reader reports no domain for {domain:?}, got {payload}"
        );
    }

    // A name the format DOES admit is taken, so the refusals above are about the name's
    // form and not about every create.
    let created = ok(
        &router,
        "CreateUserPoolDomain",
        &json!({ "UserPoolId": user_pool_id, "Domain": "auth-prose-ok" }),
    )
    .await;
    assert!(
        created.get("ManagedLoginVersion").is_some(),
        "case \"a name the format admits\": the create answers its branding version, got \
         {created}"
    );
}

/// One row of the attribute table: what the request names, and the report it earns.
struct AttributeRow {
    case: &'static str,
    operation: &'static str,
    body: Value,
    expected: String,
}

#[tokio::test]
async fn an_attribute_the_schema_will_not_take_is_refused_with_the_schema_report() {
    let router = router();
    // A pool whose schema declares one attribute that may change and one it freezes.
    let created = ok(
        &router,
        "CreateUserPool",
        &json!({
            "PoolName": "schema-report-pool",
            "Schema": [
                { "Name": "tier", "AttributeDataType": "String", "Mutable": true },
                { "Name": "frozen", "AttributeDataType": "String", "Mutable": false },
            ],
        }),
    )
    .await;
    let user_pool_id = created["UserPool"]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("a minted pool id: {created}"))
        .to_string();
    let username = "ada";
    ok(
        &router,
        "AdminCreateUser",
        &json!({ "UserPoolId": user_pool_id.clone(), "Username": username }),
    )
    .await;

    // The rows differ along the FINDING (an attribute the schema does not declare, one it
    // declares immutable), the NUMBER of offending attributes, the PREFIX the name carries
    // (`custom:`, `dev:`, none) and the family of the call: a write submits values and is
    // validated value by value, while a deletion names attributes alone and so has each
    // name's declared type to resolve first.
    let rows = vec![
        AttributeRow {
            case: "an update naming one custom attribute the schema does not declare",
            operation: "AdminUpdateUserAttributes",
            body: json!({
                "UserPoolId": user_pool_id.clone(),
                "Username": username,
                "UserAttributes": [{ "Name": "custom:not_in_schema", "Value": "v" }],
            }),
            expected: "user.custom:not_in_schema: Attribute does not exist in the schema.\n"
                .to_string(),
        },
        AttributeRow {
            case: "an update naming a developer-only attribute the schema does not declare",
            operation: "AdminUpdateUserAttributes",
            body: json!({
                "UserPoolId": user_pool_id.clone(),
                "Username": username,
                "UserAttributes": [{ "Name": "dev:absent", "Value": "v" }],
            }),
            expected: "user.dev:absent: Attribute does not exist in the schema.\n".to_string(),
        },
        AttributeRow {
            case: "an update naming an unprefixed attribute no pool declares",
            operation: "AdminUpdateUserAttributes",
            body: json!({
                "UserPoolId": user_pool_id.clone(),
                "Username": username,
                "UserAttributes": [{ "Name": "not_a_standard_attribute", "Value": "v" }],
            }),
            expected: "user.not_a_standard_attribute: Attribute does not exist in the schema.\n"
                .to_string(),
        },
        AttributeRow {
            case: "an update naming two attributes the schema does not declare: one line each",
            operation: "AdminUpdateUserAttributes",
            body: json!({
                "UserPoolId": user_pool_id.clone(),
                "Username": username,
                "UserAttributes": [
                    { "Name": "custom:first_absent", "Value": "v" },
                    { "Name": "custom:second_absent", "Value": "w" },
                ],
            }),
            expected: "user.custom:first_absent: Attribute does not exist in the schema.\n\
                       user.custom:second_absent: Attribute does not exist in the schema.\n"
                .to_string(),
        },
        AttributeRow {
            case: "an update naming the attribute the schema freezes",
            operation: "AdminUpdateUserAttributes",
            body: json!({
                "UserPoolId": user_pool_id.clone(),
                "Username": username,
                "UserAttributes": [{ "Name": "custom:frozen", "Value": "v" }],
            }),
            expected: "user.custom:frozen: Attribute cannot be updated.\n".to_string(),
        },
        AttributeRow {
            case: "an update naming a standard attribute the schema freezes",
            operation: "AdminUpdateUserAttributes",
            body: json!({
                "UserPoolId": user_pool_id.clone(),
                "Username": username,
                "UserAttributes": [{ "Name": "sub", "Value": "v" }],
            }),
            expected: "user.sub: Attribute cannot be updated.\n".to_string(),
        },
        AttributeRow {
            case: "an update naming two attributes the schema freezes: one line each, in \
                   request order",
            operation: "AdminUpdateUserAttributes",
            body: json!({
                "UserPoolId": user_pool_id.clone(),
                "Username": username,
                "UserAttributes": [
                    { "Name": "custom:frozen", "Value": "v" },
                    { "Name": "sub", "Value": "w" },
                ],
            }),
            expected: "user.custom:frozen: Attribute cannot be updated.\n\
                       user.sub: Attribute cannot be updated.\n"
                .to_string(),
        },
        AttributeRow {
            case: "a deletion naming a custom attribute whose type the schema does not declare",
            operation: "AdminDeleteUserAttributes",
            body: json!({
                "UserPoolId": user_pool_id.clone(),
                "Username": username,
                "UserAttributeNames": ["custom:never_defined_attr"],
            }),
            expected: "Invalid user attributes: Type for attribute {custom:never_defined_attr} \
                       could not be determined"
                .to_string(),
        },
        AttributeRow {
            case: "a deletion naming an unprefixed attribute whose type the schema does not \
                   declare",
            operation: "AdminDeleteUserAttributes",
            body: json!({
                "UserPoolId": user_pool_id.clone(),
                "Username": username,
                "UserAttributeNames": ["not_a_standard_attribute"],
            }),
            expected: "Invalid user attributes: Type for attribute {not_a_standard_attribute} \
                       could not be determined"
                .to_string(),
        },
    ];

    for row in rows {
        let (status, payload) = call(&router, row.operation, &row.body).await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "InvalidParameterException"),
            "case {:?}: expected a 400 InvalidParameterException, got {status} with {payload}",
            row.case
        );
        let message = error_message(&payload);
        assert_eq!(
            message, row.expected,
            "case {:?}: expected {:?}, got {message:?}",
            row.case, row.expected
        );
        assert!(
            !message.contains(user_pool_id.as_str()) && !message.contains(username),
            "case {:?}: the report names the attribute alone, got {message:?}",
            row.case
        );
    }

    // The attribute the schema DOES let change is still written, so the reports above are
    // the schema's answer and not every write's.
    ok(
        &router,
        "AdminUpdateUserAttributes",
        &json!({
            "UserPoolId": user_pool_id.clone(),
            "Username": username,
            "UserAttributes": [{ "Name": "custom:tier", "Value": "gold" }],
        }),
    )
    .await;
    let read = ok(
        &router,
        "AdminGetUser",
        &json!({ "UserPoolId": user_pool_id, "Username": username }),
    )
    .await;
    let stored: Vec<&Value> = read["UserAttributes"]
        .as_array()
        .map(|attributes| {
            attributes
                .iter()
                .filter(|attribute| attribute["Name"] == json!("custom:tier"))
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        stored.len(),
        1,
        "case \"the attribute the schema lets change\": the write is read back, got {read}"
    );
    assert_eq!(
        stored[0]["Value"],
        json!("gold"),
        "case \"the attribute the schema lets change\": the stored value is the one submitted, \
         got {read}"
    );
}

/// One row of the pagination-token table: the listing, the request, and the sentence that
/// API words the refusal with.
struct TokenRow {
    case: String,
    operation: &'static str,
    body: Value,
    expected: &'static str,
}

/// The sentence the group listing words this refusal with, as AWS words it.
const TOKEN_INVALID_RETRY: &str =
    "Provided pagination token is invalid. Check the token and try your request again.";

/// The sentence every other list API of this service words it with.
const TOKEN_INVALID: &str = "Invalid pagination token given.";

#[tokio::test]
async fn a_pagination_token_that_names_no_page_is_refused_in_the_apis_own_words() {
    let router = router();
    let user_pool_id = pool(&router, "token-wording-pool").await;
    let other_pool = pool(&router, "token-wording-other-pool").await;
    // A token this service DID mint, for the listing of another pool: a well-formed token
    // that still names no page of the listing it is presented to.
    ok(
        &router,
        "CreateGroup",
        &json!({ "UserPoolId": other_pool.clone(), "GroupName": "first" }),
    )
    .await;
    ok(
        &router,
        "CreateGroup",
        &json!({ "UserPoolId": other_pool.clone(), "GroupName": "second" }),
    )
    .await;
    let first_page = ok(
        &router,
        "ListGroups",
        &json!({ "UserPoolId": other_pool, "Limit": 1 }),
    )
    .await;
    let minted = first_page["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("a non-final page carries a NextToken: {first_page}"))
        .to_string();

    // The rows differ along the operation whose words are under test, the member the token
    // arrives in (`NextToken` and `PaginationToken`), and WHY the token names no page: a
    // string no listing minted, a token minted for another pool's listing, and one that is
    // not even base64.
    let rows = vec![
        TokenRow {
            case: "the group listing, a token nothing minted".to_string(),
            operation: "ListGroups",
            body: json!({ "UserPoolId": user_pool_id.clone(), "NextToken": "not-a-token" }),
            expected: TOKEN_INVALID_RETRY,
        },
        TokenRow {
            case: "the group listing, a token minted for another pool's listing".to_string(),
            operation: "ListGroups",
            body: json!({ "UserPoolId": user_pool_id.clone(), "NextToken": minted.clone() }),
            expected: TOKEN_INVALID_RETRY,
        },
        TokenRow {
            case: "the pool listing, a token nothing minted".to_string(),
            operation: "ListUserPools",
            body: json!({ "MaxResults": 10, "NextToken": "not-a-token" }),
            expected: TOKEN_INVALID,
        },
        TokenRow {
            case: "the pool listing, a token minted for a group listing".to_string(),
            operation: "ListUserPools",
            body: json!({ "MaxResults": 10, "NextToken": minted.clone() }),
            expected: TOKEN_INVALID,
        },
        TokenRow {
            case: "the app client listing, a token nothing minted".to_string(),
            operation: "ListUserPoolClients",
            body: json!({ "UserPoolId": user_pool_id.clone(), "NextToken": "not-base64-$$" }),
            expected: TOKEN_INVALID,
        },
        TokenRow {
            case: "the identity provider listing, a token nothing minted".to_string(),
            operation: "ListIdentityProviders",
            body: json!({ "UserPoolId": user_pool_id.clone(), "NextToken": "not-a-token" }),
            expected: TOKEN_INVALID,
        },
        TokenRow {
            case: "the user search, a token nothing minted".to_string(),
            operation: "ListUsers",
            body: json!({
                "UserPoolId": user_pool_id.clone(),
                "PaginationToken": "not-a-token",
            }),
            expected: TOKEN_INVALID,
        },
    ];

    for row in rows {
        let (status, payload) = call(&router, row.operation, &row.body).await;
        assert_eq!(
            (status, error_code(&payload), error_message(&payload)),
            (400, "InvalidParameterException", row.expected),
            "case {:?}: expected {:?}, got {status} with {payload}",
            row.case,
            row.expected
        );
        let message = error_message(&payload);
        assert!(
            !message.contains(user_pool_id.as_str()),
            "case {:?}: the refusal names no pool, got {message:?}",
            row.case
        );
    }
}

#[tokio::test]
async fn a_filter_the_user_search_cannot_read_is_refused_as_a_parse_failure() {
    let router = router();
    let user_pool_id = pool(&router, "filter-parse-pool").await;

    // The rows differ along WHY the text is not one comparison in the filter language
    // (`ListUsers.Filter`) at all: unbalanced punctuation, a missing comparison, a
    // comparison the language does not carry, a value that is not one quoted string, a
    // value whose quotation mark is never closed, and more than one comparison.
    let unreadable = [
        ("unbalanced punctuation", "(("),
        ("no comparison at all", "username"),
        ("a comparison the language does not carry", "email ~= \"a\""),
        ("a value that is not quoted", "email = bob@example.com"),
        (
            "a value whose quotation mark is never closed",
            "email = \"bob@example.com",
        ),
        (
            "two comparisons in one filter",
            "email = \"b@example.com\" and given_name = \"B\"",
        ),
    ];

    for (case, filter) in unreadable {
        let (status, payload) = call(
            &router,
            "ListUsers",
            &json!({ "UserPoolId": user_pool_id.clone(), "Filter": filter }),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload), error_message(&payload)),
            (
                400,
                "InvalidParameterException",
                "Error while parsing filter."
            ),
            "case {case:?} (filter {filter:?}): got {status} with {payload}"
        );
    }

    // A comparison the language DOES read, over an attribute no server-side search covers,
    // is a different refusal: the filter parsed, so what is wrong is the member's value —
    // the same refusal an `AttributesToGet` the results cannot serve earns. The rows differ
    // along which attribute: a custom one ("custom attributes aren't searchable"), a
    // standard one outside the documented catalog, and a searchable name in another case.
    let unsearchable = [
        ("a custom attribute", "custom:department = \"finance\""),
        ("a standard attribute outside the catalog", "nickname = \"a\""),
        ("a searchable name in another case", "Username = \"alice\""),
    ];

    for (case, filter) in unsearchable {
        let (status, payload) = call(
            &router,
            "ListUsers",
            &json!({ "UserPoolId": user_pool_id.clone(), "Filter": filter }),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload), error_message(&payload)),
            (
                400,
                "InvalidParameterException",
                "Input fails to satisfy the constraints."
            ),
            "case {case:?} (filter {filter:?}): got {status} with {payload}"
        );
    }

    // A filter the search CAN read is served, so the refusals are the parser's and the
    // catalog's answers rather than every filter's.
    let served = ok(
        &router,
        "ListUsers",
        &json!({ "UserPoolId": user_pool_id, "Filter": "username = \"nobody\"" }),
    )
    .await;
    assert_eq!(
        served["Users"],
        json!([]),
        "case \"a filter the search can read\": the search answers its (empty) selection, got \
         {served}"
    );
}
