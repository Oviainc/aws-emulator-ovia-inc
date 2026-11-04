// SPDX-License-Identifier: CC0-1.0

//! The sentence EVERY operation addressed by an `OpenIDConnectProviderArn` refuses an ARN
//! the account holds no OIDC identity provider under with, end to end over the awsQuery
//! wire.
//!
//! The rule under test: an operation addressed by an `OpenIDConnectProviderArn` that
//! resolves no provider answers `404` `NoSuchEntity` whose message is its FAMILY's fixed
//! text with the ARN the request supplied echoed into it byte for byte — the provider-record
//! operations word that absence `OpenIDConnect Provider not found for arn <arn>`, and the
//! calls that change the provider's TAGS word it `OpenId connect Provider <arn> cannot be
//! found.` The sentence belongs to the FAMILY, not to the individual handler, so no
//! operation may compose one of its own and none may normalise, re-order or punctuate the
//! ARN on the way into it.
//!
//! This is the one place the whole `OpenIDConnectProviderArn`-addressed surface is held to
//! those wordings at once, and one of the two places a template is spelled out as a literal
//! rather than read from the helpers the per-operation suites share
//! (`tests/all/open_id_connect_providers.rs`; the other is
//! `tests/all/open_id_connect_provider_tag_absence_wording.rs`): a suite that asks a helper
//! what the sentence is cannot notice the helper changing, so the texts themselves are
//! pinned here.
//!
//! `DeleteOpenIDConnectProvider` is here too, wording the absence the tag-mutation way,
//! against its own doc's claim that "this operation is idempotent; it does not fail or
//! return an error if you call the operation for a provider that does not exist": the
//! measured answer to a delete of an ARN nothing is held under is that refusal, so the
//! delete has an absence sentence like every other call addressed by this member.
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does, so what is under test is the
//! HTTP behaviour (status, awsQuery error code, `Message`) of a refusal reached through the
//! wire, never a handler called directly. The awsQuery harness is the one
//! `tests/all/user_lifecycle.rs` owns and the other suites in this crate share.
//!
//! The rows differ along what the rule generalises over — which operation is called, and
//! the form of the ARN that resolves to nothing (a host nothing was created under, the held
//! provider's host with a path appended and with its path stripped, the held host in another
//! case, in another account, in another partition, under another IAM resource type, a deeply
//! nested issuer path, and a host spelling punctuation a hostname admits) — so no single
//! request is the specification.

use smithy_server_runtime::MultiServiceRouter;

use crate::open_id_connect_providers::{expected_arn, list_members, thumbprint};
use crate::role_lifecycle::{borrowed, param};
use crate::user_lifecycle::{account_id, call, element, elements, error_code, router};

/// The provider URL the account DOES hold: the control that keeps every refusal below about
/// the ARN asked for rather than about an empty store.
const HELD_URL: &str = "https://held.example.org/realms/main";

/// The client ID the held provider is created with, so the removal row names a value the
/// HELD provider carries and only the missing ARN can refuse it.
const HELD_CLIENT_ID: &str = "audience-held";

/// The client ID the registration row ADDS — distinct from the created one, so the add and
/// the removal are independently observable in the control pass.
const ADDED_CLIENT_ID: &str = "audience-added";

/// The tag key the held provider is created with, so the untag row names a key the HELD
/// provider carries.
const HELD_TAG_KEY: &str = "env";

/// The value of that tag.
const HELD_TAG_VALUE: &str = "prod";

/// The tag key the `TagOpenIDConnectProvider` row adds.
const ADDED_TAG_KEY: &str = "team";

/// The value of that tag.
const ADDED_TAG_VALUE: &str = "platform";

/// The thumbprint the `UpdateOpenIDConnectProviderThumbprint` row replaces the held
/// provider's list with. Exactly 40 characters, the length `thumbprintType` declares
/// (`smithy_query(query="shape", shape="thumbprintType")`).
const REPLACEMENT_THUMBPRINT: &str = "00000000000000000000000000000000000000ff";

/// Which of IAM's two `NoSuchEntityException` sentences an operation words a provider's
/// absence with.
#[derive(Clone, Copy, Debug)]
enum AbsenceWording {
    /// The operations whose subject is the provider RECORD: reporting it, changing its
    /// client IDs or its thumbprints, reporting its tags.
    ProviderRecord,
    /// The calls that CHANGE the provider's tags.
    TagMutation,
}

/// IAM's `NoSuchEntityException` message for an `OpenIDConnectProviderArn` the account holds
/// no OIDC provider under, in the wording the calling operation's family uses.
///
/// Spelled out as literals, deliberately: this suite is what pins the TEMPLATES, so it may
/// not ask the implementation (or the helpers the other suites share) what they are. The
/// single variable part of each is the ARN the request supplied.
fn expected_absence_message(wording: AbsenceWording, arn: &str) -> String {
    match wording {
        AbsenceWording::ProviderRecord => format!("OpenIDConnect Provider not found for arn {arn}"),
        AbsenceWording::TagMutation => format!("OpenId connect Provider {arn} cannot be found."),
    }
}

/// One operation addressed by an `OpenIDConnectProviderArn`, the sentence it words a missing
/// provider with, plus the other members the request must carry for the ARN to be the only
/// thing that can refuse it.
struct ProviderArnCall {
    action: &'static str,
    wording: AbsenceWording,
    extra: &'static [(&'static str, &'static str)],
}

/// Every operation of this service whose input carries an `OpenIDConnectProviderArn` and
/// models `NoSuchEntityException` (`smithy_query(query="digest")`, the members named
/// `OpenIDConnectProviderArn`).
const PROVIDER_ARN_CALLS: &[ProviderArnCall] = &[
    ProviderArnCall {
        action: "GetOpenIDConnectProvider",
        wording: AbsenceWording::ProviderRecord,
        extra: &[],
    },
    ProviderArnCall {
        action: "AddClientIDToOpenIDConnectProvider",
        wording: AbsenceWording::ProviderRecord,
        extra: &[("ClientID", ADDED_CLIENT_ID)],
    },
    ProviderArnCall {
        action: "RemoveClientIDFromOpenIDConnectProvider",
        wording: AbsenceWording::ProviderRecord,
        extra: &[("ClientID", HELD_CLIENT_ID)],
    },
    ProviderArnCall {
        action: "UpdateOpenIDConnectProviderThumbprint",
        wording: AbsenceWording::ProviderRecord,
        extra: &[("ThumbprintList.member.1", REPLACEMENT_THUMBPRINT)],
    },
    ProviderArnCall {
        action: "ListOpenIDConnectProviderTags",
        wording: AbsenceWording::ProviderRecord,
        extra: &[],
    },
    ProviderArnCall {
        action: "TagOpenIDConnectProvider",
        wording: AbsenceWording::TagMutation,
        extra: &[
            ("Tags.member.1.Key", ADDED_TAG_KEY),
            ("Tags.member.1.Value", ADDED_TAG_VALUE),
        ],
    },
    ProviderArnCall {
        action: "UntagOpenIDConnectProvider",
        wording: AbsenceWording::TagMutation,
        extra: &[("TagKeys.member.1", HELD_TAG_KEY)],
    },
    ProviderArnCall {
        action: "DeleteOpenIDConnectProvider",
        wording: AbsenceWording::TagMutation,
        extra: &[],
    },
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
            ("ClientIDList.member.1", HELD_CLIENT_ID),
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

/// Drive one `ProviderArnCall` against `arn`.
async fn provider_arn_call(
    router: &MultiServiceRouter,
    call_under_test: &ProviderArnCall,
    arn: &str,
) -> (u16, String) {
    let mut params = vec![param("OpenIDConnectProviderArn", arn)];
    for (member, value) in call_under_test.extra {
        params.push(param(member, value));
    }
    call(router, call_under_test.action, &borrowed(&params)).await
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
            // A provider is held under the issuer its create derived, so the same spelling
            // in another case names no provider — and the refusal must echo the case the
            // REQUEST used.
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
        (
            "a host spelling the punctuation a hostname admits",
            format!("arn:aws:iam::{account}:oidc-provider/idp-7.example-corp.net/auth"),
        ),
    ]
}

/// RULE: every operation addressed by an `OpenIDConnectProviderArn` that resolves no OIDC
/// provider refuses the request with `404` `NoSuchEntity` whose message is its family's fixed
/// text carrying the ARN exactly as supplied, and writes nothing.
#[tokio::test]
async fn an_unresolvable_provider_arn_is_refused_with_aws_wording() {
    let (router, held) = fixture().await;
    let account = account_id();
    // Bound once, so the refusal pass and the no-write pass below are provably over the
    // same rows.
    let unresolvable = arns_the_account_holds_nothing_under(account);

    for (what, arn) in &unresolvable {
        for call_under_test in PROVIDER_ARN_CALLS {
            let action = call_under_test.action;
            let case = format!("{action} on {what}");
            let (status, body) = provider_arn_call(&router, call_under_test, arn).await;
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
            // this equality is exact, so it also pins each sentence's punctuation and that
            // neither the case nor the path of the ARN is normalised.
            assert_eq!(
                message,
                expected_absence_message(call_under_test.wording, arn),
                "case {case:?}: expected AWS's NoSuchEntity wording, got {message:?}"
            );
        }
    }

    // None of the refused calls above wrote anything: the ARNs that resolved to nothing
    // still resolve to nothing, worded the same way.
    for (what, arn) in &unresolvable {
        let (status, body) = call(
            &router,
            "GetOpenIDConnectProvider",
            &[("OpenIDConnectProviderArn", arn)],
        )
        .await;
        assert_eq!(
            status, 404,
            "case {what:?}: a refused call must create no provider, got {status}: {body}"
        );
        let message = element(&body, "Message");
        assert_eq!(
            message,
            expected_absence_message(AbsenceWording::ProviderRecord, arn),
            "case {what:?}: the read-back must word the absence the same way, got {message:?}"
        );
    }

    // And the provider the account DOES hold is untouched by the refusals: it still carries
    // the client ID, thumbprint and tag its create gave it.
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
    let client_ids = list_members(&body, "ClientIDList");
    assert_eq!(
        client_ids,
        vec![HELD_CLIENT_ID.to_string()],
        "case held provider: no refused call may have changed its client IDs, got \
         {client_ids:?}"
    );
    let thumbprints = list_members(&body, "ThumbprintList");
    assert_eq!(
        thumbprints,
        vec![thumbprint(1)],
        "case held provider: no refused call may have changed its thumbprints, got \
         {thumbprints:?}"
    );
    let keys = elements(&body, "Key");
    assert_eq!(
        keys,
        vec![HELD_TAG_KEY.to_string()],
        "case held provider: no refused call may have changed its tags, got {keys:?}"
    );
}

/// RULE (the control): the SAME operations serve the ARN the account does hold, so the
/// refusals above are about the provider the ARN names and not about the operation being
/// called at all.
#[tokio::test]
async fn the_held_provider_arn_is_served_by_every_operation() {
    let (router, held) = fixture().await;

    for call_under_test in PROVIDER_ARN_CALLS
        .iter()
        // The delete is driven last, below: it FORGETS the provider, so running it among
        // the rest would leave the read-back nothing to report.
        .filter(|candidate| candidate.action != "DeleteOpenIDConnectProvider")
    {
        let action = call_under_test.action;
        let (status, body) = provider_arn_call(&router, call_under_test, &held).await;
        assert_eq!(
            status, 200,
            "case {action:?}: the held provider must be served, got {status}: {body}"
        );
    }

    // The writes among those rows landed on the one record the reporting half reports: the
    // registration row's client ID is held and the removal row's — the one the create
    // registered — is gone, the thumbprint list was replaced wholesale, and the tag the tag
    // row added is held while the one the untag row named is not.
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
    let client_ids = list_members(&body, "ClientIDList");
    assert_eq!(
        client_ids,
        vec![ADDED_CLIENT_ID.to_string()],
        "case held provider: the registration row's client ID must be held and the created \
         one the removal row named gone, got {client_ids:?}"
    );
    let thumbprints = list_members(&body, "ThumbprintList");
    assert_eq!(
        thumbprints,
        vec![REPLACEMENT_THUMBPRINT.to_string()],
        "case held provider: the thumbprint row must have replaced the created list \
         wholesale, got {thumbprints:?}"
    );
    let keys = elements(&body, "Key");
    assert_eq!(
        keys,
        vec![ADDED_TAG_KEY.to_string()],
        "case held provider: the tag row's key must be held and the untag row's gone, got \
         {keys:?}"
    );
    let values = elements(&body, "Value");
    assert_eq!(
        values,
        vec![ADDED_TAG_VALUE.to_string()],
        "case held provider: the tag row's value must be reported with its key, got \
         {values:?}"
    );

    // The last row: the delete is served for the ARN the account DOES hold, and the ARN it
    // forgets then answers the refusal every row of the table above asserts.
    let (status, body) = call(
        &router,
        "DeleteOpenIDConnectProvider",
        &[("OpenIDConnectProviderArn", &held)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case held provider: the delete must be served, got {status}: {body}"
    );
    let (status, body) = call(
        &router,
        "DeleteOpenIDConnectProvider",
        &[("OpenIDConnectProviderArn", &held)],
    )
    .await;
    assert_eq!(
        status, 404,
        "case held provider: a second delete addresses an ARN nothing is held under, got \
         {status}: {body}"
    );
    let message = element(&body, "Message");
    assert_eq!(
        message,
        expected_absence_message(AbsenceWording::TagMutation, &held),
        "case held provider: expected AWS's NoSuchEntity wording, got {message:?}"
    );
}
