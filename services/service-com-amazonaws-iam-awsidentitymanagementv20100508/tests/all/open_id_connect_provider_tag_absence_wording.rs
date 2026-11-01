// SPDX-License-Identifier: CC0-1.0

//! The sentence a call that CHANGES an OIDC identity provider's tags refuses an
//! `OpenIDConnectProviderArn` the account holds no provider under with, end to end over the
//! awsQuery wire.
//!
//! The rule under test: an operation that CHANGES an OIDC provider's tags
//! (`TagOpenIDConnectProvider`, `UntagOpenIDConnectProvider` — the two operations of this
//! service that write a provider's tags, `smithy_query(query="digest")`) and resolves no
//! provider under the ARN it was addressed by answers `404` `NoSuchEntity` whose message is
//! the fixed text `OpenId connect Provider ` and ` cannot be found.` around the ARN the
//! request supplied, echoed byte for byte. The operations whose subject is the provider
//! RECORD word the same absence their own way (`OpenIDConnect Provider not found for arn
//! <arn>`), so this suite drives BOTH families against the same ARNs: the sentence follows
//! the family the call belongs to, not the ARN and not the store.
//!
//! Both templates are spelled out here as literals rather than read from the helpers the
//! per-operation suites share (`tests/all/open_id_connect_providers.rs`): a suite that asks a
//! helper what a sentence is cannot notice the helper changing.
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does, so what is under test is the HTTP
//! behaviour (status, awsQuery error code, `Message`) of a refusal reached through the wire,
//! never a handler called directly. The awsQuery harness is the one
//! `tests/all/user_lifecycle.rs` owns and the other suites in this crate share.
//!
//! The rows differ along what the rule generalises over — which tag-mutation operation is
//! called, how many tags or keys the request carries and what they are named, and the form of
//! the ARN that resolves to nothing (a host nothing was created under, the held issuer with a
//! segment appended and with its path stripped, the held issuer in another case, in another
//! account, in another partition, under another IAM resource type, and a deeply nested issuer
//! path) — so no single request is the specification.

use smithy_server_runtime::MultiServiceRouter;

use crate::open_id_connect_providers::{expected_arn, thumbprint};
use crate::role_lifecycle::{borrowed, param};
use crate::user_lifecycle::{account_id, call, element, elements, error_code, router};

/// The provider URL the account DOES hold: the control that keeps every refusal below about
/// the ARN asked for rather than about an empty store.
const HELD_URL: &str = "https://tag-absence.example.org/realms/main";

/// The tag the held provider is created with, so a row may name a key the HELD provider
/// carries and still be refused when it addresses an ARN nothing is held under.
const HELD_TAG_KEY: &str = "env";

/// The value of that tag.
const HELD_TAG_VALUE: &str = "prod";

/// IAM's `NoSuchEntityException` message for a TAG-MUTATION call naming an
/// `OpenIDConnectProviderArn` the account holds no OIDC provider under. The single variable
/// part is the ARN the request supplied.
fn expected_tag_absence_message(arn: &str) -> String {
    format!("OpenId connect Provider {arn} cannot be found.")
}

/// IAM's `NoSuchEntityException` message for a call on the provider RECORD naming an
/// `OpenIDConnectProviderArn` the account holds no OIDC provider under — the OTHER template,
/// asserted here so the two cannot silently collapse into one.
fn expected_record_absence_message(arn: &str) -> String {
    format!("OpenIDConnect Provider not found for arn {arn}")
}

/// `Tags.member.<n>.Key` / `.Value` for one `TagOpenIDConnectProvider` request.
fn tag_params(tags: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut params = Vec::new();
    for (index, (key, value)) in tags.iter().enumerate() {
        let position = index + 1;
        params.push(param(&format!("Tags.member.{position}.Key"), key));
        params.push(param(&format!("Tags.member.{position}.Value"), value));
    }
    params
}

/// `TagKeys.member.<n>` for one `UntagOpenIDConnectProvider` request.
fn untag_params(keys: &[&str]) -> Vec<(String, String)> {
    keys.iter()
        .enumerate()
        .map(|(index, key)| param(&format!("TagKeys.member.{}", index + 1), key))
        .collect()
}

/// One tag-mutation call: what the row is, the operation, and the members besides the ARN.
struct TagMutation {
    what: &'static str,
    action: &'static str,
    members: Vec<(String, String)>,
}

/// Every tag-mutation call the rule covers, varied along the operation and along the tags or
/// keys the request carries.
fn tag_mutations() -> Vec<TagMutation> {
    vec![
        TagMutation {
            what: "TagOpenIDConnectProvider with one tag",
            action: "TagOpenIDConnectProvider",
            members: tag_params(&[("team", "platform")]),
        },
        TagMutation {
            what: "TagOpenIDConnectProvider with several tags",
            action: "TagOpenIDConnectProvider",
            members: tag_params(&[
                ("team", "platform"),
                ("cost-center", "41200"),
                ("project", "MyImportantProject"),
            ]),
        },
        TagMutation {
            what: "TagOpenIDConnectProvider naming the key the held provider carries",
            action: "TagOpenIDConnectProvider",
            members: tag_params(&[(HELD_TAG_KEY, "staging")]),
        },
        TagMutation {
            what: "UntagOpenIDConnectProvider with one key",
            action: "UntagOpenIDConnectProvider",
            members: untag_params(&["team"]),
        },
        TagMutation {
            what: "UntagOpenIDConnectProvider with several keys",
            action: "UntagOpenIDConnectProvider",
            members: untag_params(&["team", "cost-center", "project"]),
        },
        TagMutation {
            what: "UntagOpenIDConnectProvider naming the key the held provider carries",
            action: "UntagOpenIDConnectProvider",
            members: untag_params(&[HELD_TAG_KEY]),
        },
    ]
}

/// Every operation whose subject is the provider RECORD and which models
/// `NoSuchEntityException`: the control family, whose own sentence must be untouched by the
/// tag family having its own.
const PROVIDER_RECORD_CALLS: &[(&str, &[(&str, &str)])] = &[
    ("GetOpenIDConnectProvider", &[]),
    (
        "AddClientIDToOpenIDConnectProvider",
        &[("ClientID", "audience-added")],
    ),
    (
        "RemoveClientIDFromOpenIDConnectProvider",
        &[("ClientID", "audience-held")],
    ),
];

/// Create the one OIDC provider the account holds. Answers the router and that provider's
/// ARN.
async fn fixture() -> (MultiServiceRouter, String) {
    let router = router();

    let (status, body) = call(
        &router,
        "CreateOpenIDConnectProvider",
        &[
            ("Url", HELD_URL),
            ("ClientIDList.member.1", "audience-held"),
            ("ThumbprintList.member.1", &thumbprint(1)),
            ("Tags.member.1.Key", HELD_TAG_KEY),
            ("Tags.member.1.Value", HELD_TAG_VALUE),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case fixture: CreateOpenIDConnectProvider must succeed, got {status}: {body}"
    );
    let arn = element(&body, "OpenIDConnectProviderArn");
    assert_eq!(
        arn,
        expected_arn(HELD_URL),
        "case fixture: the created provider must be named by the ARN derived from its URL, \
         got {arn:?}"
    );
    (router, arn)
}

/// Drive one tag-mutation row against `arn`.
async fn tag_mutation_call(
    router: &MultiServiceRouter,
    mutation: &TagMutation,
    arn: &str,
) -> (u16, String) {
    let mut params = vec![param("OpenIDConnectProviderArn", arn)];
    params.extend(mutation.members.iter().cloned());
    call(router, mutation.action, &borrowed(&params)).await
}

/// Every well-formed ARN this account still holds no OIDC provider under:
/// `(what the row is, the ARN)`.
fn arns_the_account_holds_nothing_under(account: &str) -> Vec<(&'static str, String)> {
    let held_issuer = HELD_URL
        .strip_prefix("https://")
        .expect("the fixture URL carries the scheme the model requires");
    let held_host = held_issuer
        .split('/')
        .next()
        .expect("a split always yields a first segment");
    vec![
        (
            "a host nothing was created under",
            format!("arn:aws:iam::{account}:oidc-provider/never.example.org"),
        ),
        (
            "the held issuer with one more path segment",
            format!("arn:aws:iam::{account}:oidc-provider/{held_issuer}/extra"),
        ),
        (
            "the held host with its path stripped",
            format!("arn:aws:iam::{account}:oidc-provider/{held_host}"),
        ),
        (
            "the held issuer in another casing",
            format!(
                "arn:aws:iam::{account}:oidc-provider/{}",
                held_issuer.to_ascii_uppercase()
            ),
        ),
        (
            "the held issuer in another account",
            format!("arn:aws:iam::210987654321:oidc-provider/{held_issuer}"),
        ),
        (
            "the held issuer in another partition",
            format!("arn:aws-us-gov:iam::{account}:oidc-provider/{held_issuer}"),
        ),
        (
            "the held issuer under another IAM resource type",
            format!("arn:aws:iam::{account}:saml-provider/{held_issuer}"),
        ),
        (
            "a deeply nested issuer path",
            format!(
                "arn:aws:iam::{account}:oidc-provider/oidc.eks.us-east-1.amazonaws.com/id/\
                 EXAMPLED539D4633E53DE1B716D3041E"
            ),
        ),
    ]
}

/// RULE: a call that CHANGES an OIDC provider's tags and resolves no provider under the ARN
/// it was addressed by refuses the request with `404` `NoSuchEntity` whose message is the
/// tag-mutation fixed text carrying the ARN exactly as supplied, and writes nothing.
#[tokio::test]
async fn an_unresolvable_provider_arn_is_refused_with_the_tag_mutation_wording() {
    let (router, held) = fixture().await;
    let account = account_id();
    let unresolvable = arns_the_account_holds_nothing_under(account);
    let mutations = tag_mutations();

    for (where_, arn) in &unresolvable {
        for mutation in &mutations {
            let case = format!("{} on {where_}", mutation.what);
            let (status, body) = tag_mutation_call(&router, mutation, arn).await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected HTTP 404, got {status}: {body}"
            );
            let code = error_code(&body);
            assert_eq!(
                code, "NoSuchEntity",
                "case {case:?}: expected NoSuchEntity, got {code:?}: {body}"
            );
            let message = element(&body, "Message");
            // The one variable part is the ARN the request supplied, echoed as supplied:
            // this equality is exact, so it also pins the sentence's trailing period and
            // that neither the case nor the path of the ARN is normalised.
            assert_eq!(
                message,
                expected_tag_absence_message(arn),
                "case {case:?}: expected the tag-mutation NoSuchEntity wording, got \
                 {message:?}"
            );
        }
    }

    // None of the refused calls wrote anything: the ARNs that resolved to nothing still
    // resolve to nothing, and the provider the account DOES hold still carries exactly the
    // tag its create gave it.
    for (where_, arn) in &unresolvable {
        let (status, body) = call(
            &router,
            "GetOpenIDConnectProvider",
            &[("OpenIDConnectProviderArn", arn)],
        )
        .await;
        assert_eq!(
            status, 404,
            "case {where_:?}: a refused tag call must create no provider, got {status}: {body}"
        );
    }

    let (status, body) = call(
        &router,
        "GetOpenIDConnectProvider",
        &[("OpenIDConnectProviderArn", &held)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case held provider: it must still be served, got {status}: {body}"
    );
    let keys = elements(&body, "Key");
    assert_eq!(
        keys,
        vec![HELD_TAG_KEY.to_string()],
        "case held provider: no refused tag call may have changed its tags, got {keys:?}"
    );
    let values = elements(&body, "Value");
    assert_eq!(
        values,
        vec![HELD_TAG_VALUE.to_string()],
        "case held provider: its tag's value must be unchanged, got {values:?}"
    );
}

/// RULE (the other half of the same rule): the operations whose subject is the provider
/// RECORD word the SAME absence with their own sentence, so the two templates stay distinct
/// on the very same ARN.
#[tokio::test]
async fn the_provider_record_family_keeps_its_own_wording_on_the_same_arns() {
    let (router, _held) = fixture().await;
    let account = account_id();

    for (where_, arn) in &arns_the_account_holds_nothing_under(account) {
        for (action, extra) in PROVIDER_RECORD_CALLS {
            let case = format!("{action} on {where_}");
            let mut params = vec![param("OpenIDConnectProviderArn", arn)];
            for (member, value) in *extra {
                params.push(param(member, value));
            }
            let (status, body) = call(&router, action, &borrowed(&params)).await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected HTTP 404, got {status}: {body}"
            );
            let message = element(&body, "Message");
            assert_eq!(
                message,
                expected_record_absence_message(arn),
                "case {case:?}: expected the provider-record NoSuchEntity wording, got \
                 {message:?}"
            );
            assert_ne!(
                message,
                expected_tag_absence_message(arn),
                "case {case:?}: the provider-record family must not take the tag-mutation \
                 sentence, got {message:?}"
            );
        }
    }
}

/// RULE (the control): the SAME tag-mutation calls SERVE the ARN the account does hold and
/// their writes persist, so the refusals above are about the provider the ARN names and not
/// about the operations being called at all.
#[tokio::test]
async fn the_held_provider_arn_is_served_by_every_tag_mutation() {
    let (router, held) = fixture().await;

    // Add the tags the rows name, then remove them again: the last row's keys are the ones
    // the earlier rows wrote, so the provider ends carrying only the tag its create gave it.
    for mutation in &tag_mutations() {
        let (status, body) = tag_mutation_call(&router, mutation, &held).await;
        assert_eq!(
            status, 200,
            "case {:?}: the held provider must be served, got {status}: {body}",
            mutation.what
        );
    }

    let (status, body) = call(
        &router,
        "GetOpenIDConnectProvider",
        &[("OpenIDConnectProviderArn", &held)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case held provider: it must still be served, got {status}: {body}"
    );
    // The untag rows named every key the tag rows wrote, including the one the create
    // stored, so the provider is left carrying no tag at all.
    let keys = elements(&body, "Key");
    assert!(
        keys.is_empty(),
        "case held provider: the served tag mutations must have removed every key they \
         named, got {keys:?}"
    );
}
