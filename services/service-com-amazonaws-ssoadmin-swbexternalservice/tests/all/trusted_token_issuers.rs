// SPDX-License-Identifier: CC0-1.0

//! The trusted-token-issuer slice end to end over the `awsJson1_1` wire.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than over the one
//! example the issue cited:
//!
//! 1. `CreateTrustedTokenIssuer` PERSISTS the request under a freshly minted
//!    `TrustedTokenIssuerArn` of the form the model's `TrustedTokenIssuerArn` pattern
//!    requires, and `DescribeTrustedTokenIssuer` REPORTS exactly the `Name`,
//!    `TrustedTokenIssuerType` and `TrustedTokenIssuerConfiguration` that create stored
//!    — for every value of those members the model admits;
//! 2. `ClientToken` is the operation's `@idempotencyToken`: the same token with the same
//!    parameters answers with the ARN the first call minted and creates nothing, the
//!    same token with any different parameter is refused `ConflictException`, and
//!    a request carrying no token is never a retry of another;
//! 3. an `InstanceArn` this account holds no instance for is refused
//!    `AccessDeniedException` (403) — the only not-found channel the operation's model
//!    declares — and a `TrustedTokenIssuerArn` no create minted is refused
//!    `ResourceNotFoundException` by the describe, never answered with a 200;
//! 4. a describe is a REPORT: repeating it answers identically and changes nothing a
//!    later caller sees.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, so the modelled constraints,
//! the `awsJson1_1` envelope and the `X-Amz-Target` routing are exercised rather than
//! bypassed. The harness is the one `ssoadmin_permission_sets.rs` owns.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ssoadmin_permission_sets::{
    call, error_code, router, seeded_instance_arn, UNKNOWN_INSTANCE_ARN,
};

/// The only value the model's `TrustedTokenIssuerType` enum admits
/// (`smithy_query(query="shape",shape="TrustedTokenIssuerType")` → `["OIDC_JWT"]`).
const ISSUER_TYPE: &str = "OIDC_JWT";

/// The only value the model's `JwksRetrievalOption` enum admits
/// (`smithy_query(query="shape",shape="OidcJwtConfiguration")` → member
/// `JwksRetrievalOption` → `["OPEN_ID_DISCOVERY"]`).
const JWKS_RETRIEVAL_OPTION: &str = "OPEN_ID_DISCOVERY";

/// One trusted token issuer to create: the parameters, as a row of the table.
#[derive(Debug, Clone)]
struct Issuer {
    label: String,
    name: String,
    issuer_url: String,
    claim_attribute_path: String,
    identity_store_attribute_path: String,
    tags: Vec<(String, String)>,
    client_token: Option<String>,
}

impl Issuer {
    fn new(label: &str, name: &str, issuer_url: &str, claim: &str, identity: &str) -> Self {
        Self {
            label: label.to_string(),
            name: name.to_string(),
            issuer_url: issuer_url.to_string(),
            claim_attribute_path: claim.to_string(),
            identity_store_attribute_path: identity.to_string(),
            tags: Vec::new(),
            client_token: None,
        }
    }

    fn with_tags(mut self, tags: &[(&str, &str)]) -> Self {
        self.tags = tags
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        self
    }

    fn with_client_token(mut self, token: &str) -> Self {
        self.client_token = Some(token.to_string());
        self
    }

    /// The `CreateTrustedTokenIssuer` request body for this row.
    fn request(&self, instance_arn: &str) -> Value {
        let mut body = json!({
            "InstanceArn": instance_arn,
            "Name": self.name,
            "TrustedTokenIssuerType": ISSUER_TYPE,
            "TrustedTokenIssuerConfiguration": self.configuration(),
        });
        if !self.tags.is_empty() {
            body["Tags"] = Value::Array(
                self.tags
                    .iter()
                    .map(|(key, value)| json!({"Key": key, "Value": value}))
                    .collect(),
            );
        }
        if let Some(token) = &self.client_token {
            body["ClientToken"] = json!(token);
        }
        body
    }

    fn configuration(&self) -> Value {
        json!({
            "OidcJwtConfiguration": {
                "IssuerUrl": self.issuer_url,
                "ClaimAttributePath": self.claim_attribute_path,
                "IdentityStoreAttributePath": self.identity_store_attribute_path,
                "JwksRetrievalOption": JWKS_RETRIEVAL_OPTION,
            }
        })
    }
}

/// The rows differ along what rule 1 generalises over: the length and character class of
/// `Name` (`len 1..255`, `^[\w+=,.@-]+$`), the scheme, port, path and punctuation of
/// `IssuerUrl` (`^https?:\/\/[-a-zA-Z0-9+&@\/%=~_|!:,.;]*[-a-zA-Z0-9+&@\/%=~_|]$`), the
/// separator and segment count of the two attribute paths (`\p{L}+` then up to two more
/// segments, joined by `.` or `_` for the claim and by `.` for the identity store), the
/// number of `Tags`, and whether a `ClientToken` is sent at all (`len 1..64`,
/// `^[!-~]+$`). All constraints are from
/// `smithy_query(query="operation",operation="CreateTrustedTokenIssuer")` and
/// `smithy_query(query="shape",shape="OidcJwtConfiguration")`.
fn issuers() -> Vec<Issuer> {
    vec![
        Issuer::new(
            "the shortest Name the model admits, no tags, no client token",
            "a",
            "https://example.com/oidc",
            "email",
            "email",
        ),
        Issuer::new(
            "Name at the 255-character limit, spanning every allowed character class",
            &format!("Tti_0+=,.@-{}", "x".repeat(244)),
            "https://login.example.com/.well-known/openid-configuration",
            "user_email_address",
            "emails.primary.value",
            )
        .with_tags(&[("owner", "platform"), ("Env", "staging"), ("cost-centre", "42")])
        .with_client_token("tti-token-1"),
        Issuer::new(
            "an http IssuerUrl with a port and the punctuation the pattern allows",
            "http.issuer-1",
            "http://idp.example.com:8443/tenant,one;realm!two~three/jwks",
            "sub",
            "userName",
        )
        .with_tags(&[("single", "tag")]),
        Issuer::new(
            "a ClientToken at the 64-character limit, built from the ^[!-~]+$ class",
            "token-at-limit",
            "https://issuer.example.org/%7Euser/openid",
            "claims.email_address",
            "addresses.formatted",
        )
        .with_client_token(&format!("!#$%&*+-.;=?@^_|~{}", "T".repeat(47))),
        Issuer::new(
            "a one-character ClientToken and a single-segment identity path",
            "minimal-token",
            "https://a.example/b",
            "nickname",
            "displayName",
        )
        .with_client_token("!"),
    ]
}

/// Whether `arn` has the shape the model's `TrustedTokenIssuerArn` pattern requires for
/// an issuer of `instance_arn`:
/// `^arn:aws(-[a-z]{1,5}){0,3}:sso::\d{12}:trustedTokenIssuer/(sso)?ins-[a-zA-Z0-9-.]{16}/tti-<uuid>$`.
fn is_trusted_token_issuer_arn(arn: &str, instance_arn: &str) -> bool {
    let instance_id = instance_arn.rsplit('/').next().unwrap_or_default();
    let Some(rest) = arn.strip_prefix("arn:aws:sso::") else {
        return false;
    };
    let Some((account, resource)) = rest.split_once(':') else {
        return false;
    };
    if account.len() != 12 || !account.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    let Some(uuid) = resource.strip_prefix(&format!("trustedTokenIssuer/{instance_id}/tti-")) else {
        return false;
    };
    is_lowercase_uuid(uuid)
}

/// Whether `value` is a hyphenated lowercase-hex UUID, the form the `tti-` segment of
/// `TrustedTokenIssuerArn` requires: `[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}`.
fn is_lowercase_uuid(value: &str) -> bool {
    const WIDTHS: [usize; 5] = [8, 4, 4, 4, 12];
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == WIDTHS.len()
        && groups.iter().zip(WIDTHS).all(|(group, width)| {
            group.len() == width
                && group
                    .chars()
                    .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
        })
}

/// Create one issuer and return the `TrustedTokenIssuerArn` it minted.
async fn create(router: &MultiServiceRouter, instance_arn: &str, issuer: &Issuer) -> (u16, Value) {
    call(
        router,
        "CreateTrustedTokenIssuer",
        &issuer.request(instance_arn),
    )
    .await
}

/// `DescribeTrustedTokenIssuer` for `arn`.
async fn describe(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    call(
        router,
        "DescribeTrustedTokenIssuer",
        &json!({"TrustedTokenIssuerArn": arn}),
    )
    .await
}

/// Rule 1 (and rule 4): every row round-trips through the store, under an ARN of the
/// modelled form that no other row shares.
#[tokio::test]
async fn every_created_issuer_reads_back_exactly_what_was_stored() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;
    let mut minted: Vec<String> = Vec::new();

    for issuer in issuers() {
        let case = &issuer.label;
        let (status, body) = create(&router, &instance_arn, &issuer).await;
        assert_eq!(
            status, 200,
            "case {case:?}: create must answer 200, got {status} {body}"
        );
        let arn = body["TrustedTokenIssuerArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            is_trusted_token_issuer_arn(&arn, &instance_arn),
            "case {case:?}: minted ARN is not a modelled TrustedTokenIssuerArn of this \
             instance, got {arn:?}"
        );
        assert!(
            !minted.contains(&arn),
            "case {case:?}: each create mints a fresh ARN, got the already-minted {arn:?}"
        );
        minted.push(arn.clone());

        let (status, described) = describe(&router, &arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: describe of the created issuer must answer 200, got {status} \
             {described}"
        );
        assert_eq!(
            described["TrustedTokenIssuerArn"].as_str(),
            Some(arn.as_str()),
            "case {case:?}: describe must report the ARN it was asked about, got {described}"
        );
        assert_eq!(
            described["Name"].as_str(),
            Some(issuer.name.as_str()),
            "case {case:?}: describe must report the stored Name, got {described}"
        );
        assert_eq!(
            described["TrustedTokenIssuerType"].as_str(),
            Some(ISSUER_TYPE),
            "case {case:?}: describe must report the stored type, got {described}"
        );
        assert_eq!(
            described["TrustedTokenIssuerConfiguration"],
            issuer.configuration(),
            "case {case:?}: describe must report the stored configuration, got {described}"
        );

        // Rule 4: the describe reported rather than mutated, so a second one answers
        // identically.
        let (status, again) = describe(&router, &arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a repeated describe must answer 200, got {status} {again}"
        );
        assert_eq!(
            again, described,
            "case {case:?}: a describe must change nothing a later caller sees, got {again}"
        );
    }

    // Every earlier issuer is still readable after the later creates: a create adds a
    // record, it does not replace the store.
    for (issuer, arn) in issuers().iter().zip(&minted) {
        let (status, body) = describe(&router, arn).await;
        assert_eq!(
            status, 200,
            "case {:?}: an issuer created earlier must still describe, got {status} {body}",
            issuer.label
        );
    }
}

/// Rule 2: a replayed `ClientToken` with the same parameters answers with the first
/// call's ARN, and the same token with a different parameter is a conflict.
#[tokio::test]
async fn a_replayed_client_token_mints_no_second_issuer_and_a_changed_parameter_conflicts() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;

    for issuer in issuers().into_iter().filter(|i| i.client_token.is_some()) {
        let case = issuer.label.clone();
        let (status, body) = create(&router, &instance_arn, &issuer).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the first create must answer 200, got {status} {body}"
        );
        let first = body["TrustedTokenIssuerArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        // The identical request under the identical token: a retry, not a second issuer.
        let (status, replayed) = create(&router, &instance_arn, &issuer).await;
        assert_eq!(
            status, 200,
            "case {case:?}: replaying the token must answer 200, got {status} {replayed}"
        );
        assert_eq!(
            replayed["TrustedTokenIssuerArn"].as_str(),
            Some(first.as_str()),
            "case {case:?}: a replay must answer with the first ARN {first:?}, got {replayed}"
        );

        // Tag ORDER is not a parameter difference: a tag set carries no order.
        if issuer.tags.len() > 1 {
            let mut reordered = issuer.clone();
            reordered.tags.reverse();
            let (status, body) = create(&router, &instance_arn, &reordered).await;
            assert_eq!(
                body["TrustedTokenIssuerArn"].as_str(),
                Some(first.as_str()),
                "case {case:?}: reordering Tags is the same request, expected {first:?}, \
                 got {status} {body}"
            );
        }

        // One mutated member per row: each is a parameter the token is scoped to, so
        // each is a conflict rather than a replay.
        let mut mutations: Vec<(&str, Issuer)> = Vec::new();
        let mut renamed = issuer.clone();
        // A different Name of the same length, so even the row already at the modelled
        // 255-character limit really does change this member.
        let mut characters: Vec<char> = renamed.name.chars().collect();
        if characters.len() >= 255 {
            characters.pop();
        }
        characters.push('Z');
        renamed.name = characters.into_iter().collect();
        assert_ne!(
            renamed.name, issuer.name,
            "case {case:?}: the Name mutation must actually change the Name"
        );
        mutations.push(("Name", renamed));
        let mut re_urled = issuer.clone();
        re_urled.issuer_url = "https://other.example.com/oidc".to_string();
        mutations.push(("IssuerUrl", re_urled));
        let mut re_claimed = issuer.clone();
        re_claimed.claim_attribute_path = "otherClaim".to_string();
        mutations.push(("ClaimAttributePath", re_claimed));
        let mut re_mapped = issuer.clone();
        re_mapped.identity_store_attribute_path = "otherAttribute".to_string();
        mutations.push(("IdentityStoreAttributePath", re_mapped));
        let mut re_tagged = issuer.clone();
        re_tagged
            .tags
            .push(("extra".to_string(), "tag".to_string()));
        mutations.push(("Tags", re_tagged));

        for (member, mutated) in mutations {
            let (status, body) = create(&router, &instance_arn, &mutated).await;
            assert_eq!(
                status, 400,
                "case {case:?} changed {member}: the reused token must be refused 400, \
                 got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ConflictException",
                "case {case:?} changed {member}: expected ConflictException, got {body}"
            );
        }

        // The conflicts changed nothing: the first issuer still reads back.
        let (status, described) = describe(&router, &first).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the first issuer must survive the refusals, got {status} \
             {described}"
        );
        assert_eq!(
            described["Name"].as_str(),
            Some(issuer.name.as_str()),
            "case {case:?}: a refused create must not have altered the stored Name, got \
             {described}"
        );
    }
}

/// Rule 2, the other half: a request that carries no token is never a retry, and a FRESH
/// token for identical parameters mints a separate issuer.
#[tokio::test]
async fn identical_parameters_under_no_token_or_a_fresh_token_mint_separate_issuers() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;
    let base = Issuer::new(
        "repeatable parameters",
        "repeat-me",
        "https://repeat.example.com/oidc",
        "email",
        "email",
    )
    .with_tags(&[("team", "identity")]);

    let cases: Vec<(&str, Option<&str>)> = vec![
        ("no ClientToken, first call", None),
        ("no ClientToken, second call", None),
        ("a fresh ClientToken", Some("fresh-token-a")),
        ("another fresh ClientToken", Some("fresh-token-b")),
    ];
    let mut minted: Vec<String> = Vec::new();
    for (case, token) in cases {
        let mut issuer = base.clone();
        issuer.client_token = token.map(str::to_string);
        let (status, body) = create(&router, &instance_arn, &issuer).await;
        assert_eq!(
            status, 200,
            "case {case:?}: create must answer 200, got {status} {body}"
        );
        let arn = body["TrustedTokenIssuerArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !minted.contains(&arn),
            "case {case:?}: this is a separate create, so it must mint a separate ARN, \
             got the already-minted {arn:?}"
        );
        minted.push(arn);
    }
    for arn in &minted {
        let (status, body) = describe(&router, arn).await;
        assert_eq!(
            status, 200,
            "every separately created issuer must describe: {arn:?} answered {status} {body}"
        );
    }
}

/// Rule 3: an `InstanceArn` this account holds no instance for is refused 403, and
/// nothing is stored for it.
#[tokio::test]
async fn creating_under_an_instance_this_account_does_not_hold_is_refused() {
    let router = router();
    let held = seeded_instance_arn(&router).await;
    let held_id = held.rsplit('/').next().unwrap_or_default().to_string();

    // The rows span the `InstanceArn` pattern: both `(sso)?ins-` spellings, the id
    // character class, and the partition suffixes `(-[a-z]{1,5}){0,3}` admits — the held
    // instance's id under another partition names a DIFFERENT instance.
    let cases: Vec<(&str, String)> = vec![
        (
            "a pattern-valid ssoins- ARN that was never created",
            UNKNOWN_INSTANCE_ARN.to_string(),
        ),
        (
            "a pattern-valid ins- ARN that was never created",
            "arn:aws:sso:::instance/ins-a1b2c3d4e5f60718".to_string(),
        ),
        (
            "an id using the full [a-zA-Z0-9-.] class",
            "arn:aws:sso:::instance/ssoins-A.b-C.d-12345678".to_string(),
        ),
        (
            "the held instance's id in another partition",
            format!("arn:aws-us-gov:sso:::instance/{held_id}"),
        ),
    ];

    for (case, instance_arn) in cases {
        let issuer = Issuer::new(
            case,
            "unheld-instance",
            "https://unheld.example.com/oidc",
            "email",
            "email",
        );
        let (status, body) = create(&router, &instance_arn, &issuer).await;
        assert_eq!(
            status, 403,
            "case {case:?}: an unheld InstanceArn must be refused 403, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "AccessDeniedException",
            "case {case:?}: expected AccessDeniedException, got {body}"
        );
        assert!(
            body["TrustedTokenIssuerArn"].is_null(),
            "case {case:?}: a refusal mints no ARN, got {body}"
        );
    }
}

/// Rule 3: a `TrustedTokenIssuerArn` no create minted is a not-found, never a 200.
#[tokio::test]
async fn describing_an_arn_no_create_minted_answers_not_found() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;
    let instance_id = instance_arn
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_string();
    // One issuer exists, so the not-found is about the ARN asked for and not about an empty
    // store.
    let (status, body) = create(
        &router,
        &instance_arn,
        &Issuer::new(
            "the one stored issuer",
            "stored",
            "https://stored.example.com/oidc",
            "email",
            "email",
        ),
    )
    .await;
    assert_eq!(status, 200, "the seed create must answer 200, got {body}");
    let stored = body["TrustedTokenIssuerArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // The parts of the stored ARN, so each row below varies exactly ONE of them.
    let account = stored.split(':').nth(4).unwrap_or_default().to_string();
    let uuid = stored.rsplit("/tti-").next().unwrap_or_default().to_string();
    assert!(
        is_lowercase_uuid(&uuid),
        "the stored ARN must end in a tti- UUID, got {stored:?}"
    );
    let tti_arn = |partition: &str, account: &str, instance: &str, uuid: &str| {
        format!("arn:{partition}:sso::{account}:trustedTokenIssuer/{instance}/tti-{uuid}")
    };

    // The rows span the parts of the ARN that identify the issuer: the `tti-` UUID, the
    // instance segment, the account segment and the partition.
    let cases: Vec<(&str, String)> = vec![
        (
            "a pattern-valid tti- UUID that was never minted",
            tti_arn(
                "aws",
                &account,
                &instance_id,
                "00000000-0000-4000-8000-000000000000",
            ),
        ),
        (
            "the stored UUID under another instance",
            tti_arn("aws", &account, "ssoins-0000000000000000", &uuid),
        ),
        (
            "the stored issuer under another account",
            tti_arn("aws", "210987654321", &instance_id, &uuid),
        ),
        (
            "the stored issuer in another partition",
            tti_arn("aws-us-gov", &account, &instance_id, &uuid),
        ),
    ];

    for (case, arn) in cases {
        assert_ne!(
            arn, stored,
            "case {case:?}: the row must name a DIFFERENT ARN than the stored one"
        );
        let (status, body) = describe(&router, &arn).await;
        assert_eq!(
            status, 400,
            "case {case:?}: an unminted ARN must be refused 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body}"
        );
    }

    // The refusals left the stored issuer readable.
    let (status, body) = describe(&router, &stored).await;
    assert_eq!(
        status, 200,
        "the stored issuer must survive the refusals, got {status} {body}"
    );
}
