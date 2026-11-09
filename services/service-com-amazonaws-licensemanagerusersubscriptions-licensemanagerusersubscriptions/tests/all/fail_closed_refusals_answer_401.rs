// SPDX-License-Identifier: CC0-1.0

//! What a fail-closed refusal of this service looks like ON THE WIRE, and which requests
//! reach that decision — driven over the real wire path through the router's `dispatch`, so
//! every call is deserialized, constraint-checked, handled and serialized exactly as a
//! client's would be.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one request:
//!
//! * **A refusal is reported with the status its ERROR CODE carries.** Every refusal this
//!   service answers with `UnauthorizedException` — the caller's authority over the resource
//!   the request named could not be established — is HTTP 401 with the fixed sentence,
//!   whichever surface it was asked on and whichever kind of identifier it was asked about;
//!   and the tagging surface's `BadRequestException`, which is about a value in the request,
//!   is still HTTP 400. The status is not one constant for every non-modelled refusal: it
//!   travels with the code, because the two are different outcomes to a client.
//! * **An operation that acts on a user association is authorized before its request shape
//!   is validated**, whichever way the request NAMES what it acts on: by `InstanceId`, or by
//!   the `InstanceUserArn` that identifies an association on an instance. So an ARN this
//!   account's store holds no association for is refused identically whether or not it
//!   satisfies that member's modelled `@pattern` — a client that handles the denial is not
//!   handed a constraint violation instead. The SCOPE the ARN is read in is the request's
//!   own: an ARN naming another account, or another Region than the request is signed for,
//!   addresses nothing this store holds here and is refused the same way.
//! * **Neither decision is a blanket refusal**: an association that IS held, addressed by
//!   the `InstanceUserArn` alone, is served and observable afterwards; and a request that
//!   names an instance the account holds is decided on that instance, so its malformed ARN
//!   is still refused by the boundary validator.
//!
//! The harness (router + engine + the account's directories and network) is the one in
//! [`super::identity_provider_subscriptions`]; only the rules are new here.

use serde_json::{json, Value};

// ONE wire harness per service: the dispatch plumbing and the fixtures come from the
// sibling suites rather than a second copy.
use super::identity_provider_subscriptions::{
    arn_prefix, as_str, call, harness, provider_union, register, Harness, Reply, ACCOUNT, PRODUCTS,
    REGION, SIGNING_NAME,
};
use super::resource_resolution_refusals::{
    assert_fails_closed, assert_invalid_resource_arn, associate, disassociate, held_instance,
    list_tags,
};

/// The code, the fixed sentence and the status this service fails a refusal closed with
/// (`scaffolded::UNAUTHORIZED_EXCEPTION_CODE`, `UNAUTHORIZED_EXCEPTION_MESSAGE` and
/// `UNAUTHORIZED_EXCEPTION_STATUS`). Spelled out here rather than read from the production
/// constants, so a change to them fails a test instead of silently changing what this suite
/// accepts.
const UNAUTHORIZED_MESSAGE: &str =
    "Unauthorized. Please check the request and access policy and try again.";

/// A directory of the account's fleet (`Active`), so a registration resolves for it.
const REGISTERED_DIRECTORY: &str = "d-0123456789";
/// A directory of the account's fleet that this suite registers nothing for.
const UNREGISTERED_DIRECTORY: &str = "d-00000000ff";
/// A subnet of the account's own Amazon EC2 store, which a registration's `Settings` name.
const SUBNET: &str = "subnet-0123456789abcdef0";

async fn deregister(h: &Harness, body: &Value) -> Reply {
    call(h, "/identity-provider/DeregisterIdentityProvider", body).await
}

async fn update_settings(h: &Harness, body: &Value) -> Reply {
    call(h, "/identity-provider/UpdateIdentityProviderSettings", body).await
}

async fn list_subscriptions(h: &Harness, body: &Value) -> Reply {
    call(h, "/user/ListProductSubscriptions", body).await
}

async fn start_subscription(h: &Harness, body: &Value) -> Reply {
    call(h, "/user/StartProductSubscription", body).await
}

async fn stop_subscription(h: &Harness, body: &Value) -> Reply {
    call(h, "/user/StopProductSubscription", body).await
}

async fn list_associations(h: &Harness, body: &Value) -> Reply {
    call(h, "/user/ListUserAssociations", body).await
}

async fn delete_endpoint(h: &Harness, body: &Value) -> Reply {
    call(h, "/license-server/DeleteLicenseServerEndpoint", body).await
}

/// Assert `reply`'s body carries the refusal and NOTHING else: the error code and the fixed
/// sentence, so a fail-closed refusal says nothing about the identifier it was asked about
/// whatever that identifier was.
///
/// The status, the code and the sentence are asserted by the shared
/// [`assert_fails_closed`] this crate's suites already drive every fail-closed refusal
/// through; this adds the one thing that suite does not, because it is this suite's rule:
/// the body is the whole of the refusal.
fn assert_body_is_only_the_refusal(case: &str, reply: &Reply) {
    let members: Vec<&String> = reply
        .body
        .as_object()
        .map(|body| body.keys().collect())
        .unwrap_or_default();
    assert_eq!(
        members.len(),
        2,
        "case {case:?}: the refusal body carries the code and the sentence and nothing \
         else, got {members:?} in {reply:?}"
    );
    assert_eq!(
        reply.body.get("message").and_then(Value::as_str),
        Some(UNAUTHORIZED_MESSAGE),
        "case {case:?}: expected the fixed fail-closed sentence, got {reply:?}"
    );
}

/// Register `directory_id` for `product` and answer the `IdentityProviderArn` minted.
async fn registered(h: &Harness, directory_id: &str, product: &str, case: &str) -> String {
    let reply = register(
        h,
        &json!({
            "IdentityProvider": provider_union(directory_id),
            "Product": product,
            "Settings": { "Subnets": [SUBNET], "SecurityGroupId": "sg-0123abcd" },
        }),
    )
    .await;
    as_str(
        &reply.ok(case)["IdentityProviderSummary"],
        "IdentityProviderArn",
        case,
    )
    .to_string()
}

/// Associate `username` on `instance_id` through the registered directory and answer the
/// `InstanceUserArn` minted for the association.
async fn associated(h: &Harness, instance_id: &str, username: &str, case: &str) -> String {
    let reply = associate(
        h,
        &json!({
            "Username": username,
            "InstanceId": instance_id,
            "IdentityProvider": provider_union(REGISTERED_DIRECTORY),
        }),
    )
    .await;
    as_str(
        &reply.ok(case)["InstanceUserSummary"],
        "InstanceUserArn",
        case,
    )
    .to_string()
}

/// RULE: every refusal this service answers with `UnauthorizedException` is reported with
/// HTTP 401 and the fixed sentence — on every surface that answers one, and about every kind
/// of identifier one can be asked about — while the tagging surface's `BadRequestException`
/// stays HTTP 400: the status travels with the error code.
#[tokio::test]
async fn every_unauthorized_refusal_is_reported_with_its_own_status() {
    let h = harness();
    // One real registration and one association, so the refusals below are about the
    // resource each request names and not about an empty store.
    registered(&h, REGISTERED_DIRECTORY, PRODUCTS[0], "the registration").await;
    let held = held_instance(0);
    associated(&h, &held, "held.user", "the association that exists").await;

    // An instance id no instance of the account has, in either place one can be reported
    // from, and ARNs of this service's own resource kinds that state holds nothing for.
    let unheld_instance = "i-0123456789abcdef0";
    let unknown_association = format!("{}iu-000000009999", arn_prefix("instance-user"));
    let unknown_endpoint = format!("{}lse-000000009999", arn_prefix("license-server-endpoint"));

    let rows: Vec<(String, String, Reply)> = vec![
        (
            "AssociateUser on an instance the account does not hold".to_string(),
            unheld_instance.to_string(),
            associate(
                &h,
                &json!({
                    "Username": "someone",
                    "InstanceId": unheld_instance,
                    "IdentityProvider": provider_union(REGISTERED_DIRECTORY),
                }),
            )
            .await,
        ),
        (
            "ListUserAssociations on an instance the account does not hold".to_string(),
            unheld_instance.to_string(),
            list_associations(
                &h,
                &json!({
                    "InstanceId": unheld_instance,
                    "IdentityProvider": provider_union(REGISTERED_DIRECTORY),
                }),
            )
            .await,
        ),
        (
            "DisassociateUser on an instance the account does not hold".to_string(),
            unheld_instance.to_string(),
            disassociate(
                &h,
                &json!({
                    "Username": "someone",
                    "InstanceId": unheld_instance,
                    "IdentityProvider": provider_union(REGISTERED_DIRECTORY),
                }),
            )
            .await,
        ),
        (
            "DisassociateUser for a user no association on a held instance names".to_string(),
            "never.associated".to_string(),
            disassociate(
                &h,
                &json!({
                    "Username": "never.associated",
                    "InstanceId": held,
                    "IdentityProvider": provider_union(REGISTERED_DIRECTORY),
                }),
            )
            .await,
        ),
        (
            "DisassociateUser by an InstanceUserArn state holds nothing for".to_string(),
            unknown_association.clone(),
            disassociate(&h, &json!({ "InstanceUserArn": unknown_association })).await,
        ),
        (
            "DeleteLicenseServerEndpoint by an ARN state holds nothing for".to_string(),
            unknown_endpoint.clone(),
            delete_endpoint(
                &h,
                &json!({
                    "LicenseServerEndpointArn": unknown_endpoint,
                    "ServerType": "RDS_SAL",
                }),
            )
            .await,
        ),
        (
            "StartProductSubscription through an unregistered directory".to_string(),
            UNREGISTERED_DIRECTORY.to_string(),
            start_subscription(
                &h,
                &json!({
                    "Username": "someone",
                    "IdentityProvider": provider_union(UNREGISTERED_DIRECTORY),
                    "Product": PRODUCTS[0],
                }),
            )
            .await,
        ),
        (
            "StopProductSubscription through an unregistered directory".to_string(),
            UNREGISTERED_DIRECTORY.to_string(),
            stop_subscription(
                &h,
                &json!({
                    "Username": "someone",
                    "IdentityProvider": provider_union(UNREGISTERED_DIRECTORY),
                    "Product": PRODUCTS[0],
                }),
            )
            .await,
        ),
        (
            "DeregisterIdentityProvider by an unregistered (provider, product) pair"
                .to_string(),
            UNREGISTERED_DIRECTORY.to_string(),
            deregister(
                &h,
                &json!({
                    "IdentityProvider": provider_union(UNREGISTERED_DIRECTORY),
                    "Product": PRODUCTS[0],
                }),
            )
            .await,
        ),
        (
            "DeregisterIdentityProvider for a product the directory is not registered for"
                .to_string(),
            PRODUCTS[3].to_string(),
            deregister(
                &h,
                &json!({
                    "IdentityProvider": provider_union(REGISTERED_DIRECTORY),
                    "Product": PRODUCTS[3],
                }),
            )
            .await,
        ),
        (
            "ListProductSubscriptions through an unregistered directory".to_string(),
            UNREGISTERED_DIRECTORY.to_string(),
            list_subscriptions(
                &h,
                &json!({
                    "IdentityProvider": provider_union(UNREGISTERED_DIRECTORY),
                    "Product": PRODUCTS[0],
                }),
            )
            .await,
        ),
        (
            "ListProductSubscriptions through an unregistered directory, no Product"
                .to_string(),
            UNREGISTERED_DIRECTORY.to_string(),
            list_subscriptions(
                &h,
                &json!({ "IdentityProvider": provider_union(UNREGISTERED_DIRECTORY) }),
            )
            .await,
        ),
        (
            "UpdateIdentityProviderSettings by an unregistered (provider, product) pair"
                .to_string(),
            UNREGISTERED_DIRECTORY.to_string(),
            update_settings(
                &h,
                &json!({
                    "IdentityProvider": provider_union(UNREGISTERED_DIRECTORY),
                    "Product": PRODUCTS[0],
                    "UpdateSettings": { "AddSubnets": [SUBNET], "RemoveSubnets": [] },
                }),
            )
            .await,
        ),
    ];
    for (case, identifier, reply) in &rows {
        // The shared assertion: the status, the code, the fixed sentence, and that the
        // identifier the request named is nowhere in the body.
        assert_fails_closed(case, identifier, reply);
        assert_body_is_only_the_refusal(case, reply);
    }

    // The contrast: a refusal this service answers with the OTHER non-modelled code it
    // sends — the tagging surface's, about the `ResourceArn` value the request carries — is
    // still HTTP 400, so the status is the code's and not one constant for both.
    assert_invalid_resource_arn(
        "ListTagsForResource by an ARN naming no resource",
        &list_tags(&h, &format!("{}iu-000000009999", arn_prefix("instance-user"))).await,
    );
}

/// Every `InstanceUserArn` form a client can send that this account's store holds no
/// association for, as `(class, value)`.
///
/// `InstanceUserArn` targets `Arn`, whose `@pattern` admits ONE slash in the resource part
/// (`smithy_query(query="operation", operation="DisassociateUser")`:
/// `^arn:[a-z0-9-\.]{1,63}:[a-z0-9-\.]{1,63}:[a-z0-9-\.]{1,63}:[a-z0-9-\.]{1,63}:[a-zA-Z0-9-\.]{1,510}/[a-zA-Z0-9-\.]{1,510}$`),
/// so the rows differ along the one thing the rule generalises over: whether the value
/// satisfies that pattern. A user-association ARN of the real service carries TWO slashes —
/// the instance and the user — so that form is outside the pattern, and so are an uppercase
/// service segment and a value that is not an ARN at all.
fn unheld_association_arns() -> Vec<(&'static str, String)> {
    vec![
        (
            "the form this service mints, never associated",
            format!("{}iu-000000009999", arn_prefix("instance-user")),
        ),
        (
            "a well-formed ARN in another account",
            format!("arn:aws:{SIGNING_NAME}:{REGION}:111111111111:instance-user/iu-000000000001"),
        ),
        (
            "a well-formed ARN in another Region than the request is signed for",
            format!("arn:aws:{SIGNING_NAME}:eu-central-1:{ACCOUNT}:instance-user/iu-000000000001"),
        ),
        (
            "a user-association ARN: two slashes in the resource part",
            format!(
                "arn:aws:{SIGNING_NAME}:{REGION}:{ACCOUNT}:user-association/i-0123456789abcdef0\
                 /some.user"
            ),
        ),
        (
            "three slashes in the resource part",
            format!(
                "arn:aws:{SIGNING_NAME}:{REGION}:{ACCOUNT}:user-association/i-0123456789abcdef0\
                 /corp.example/some.user"
            ),
        ),
        (
            "no slash in the resource part at all",
            format!("arn:aws:{SIGNING_NAME}:{REGION}:{ACCOUNT}:instance-user"),
        ),
        (
            "an underscore the pattern's resource class does not admit",
            format!("{}iu_000000009999", arn_prefix("instance-user")),
        ),
        (
            "an uppercase service segment the pattern does not admit",
            format!("arn:aws:License-Manager-User-Subscriptions:{REGION}:{ACCOUNT}:instance-user/iu-000000000001"),
        ),
        ("not an ARN at all", "not-an-arn".to_string()),
    ]
}

/// RULE: a `DisassociateUser` that names the association it acts on by `InstanceUserArn` is
/// authorized on that ARN BEFORE the request's shape is validated, so an ARN this account's
/// store holds no association for is refused with the same status, code and sentence whether
/// or not it satisfies the member's modelled `@pattern`.
#[tokio::test]
async fn an_association_arn_is_authorized_before_its_pattern_is_checked() {
    let h = harness();
    // A registration and an association, so the store is not empty and the refusals below
    // are about the ARN each request names.
    registered(&h, REGISTERED_DIRECTORY, PRODUCTS[0], "the registration").await;
    let held = held_instance(0);
    associated(&h, &held, "held.user", "the association that exists").await;

    for (class, arn) in unheld_association_arns() {
        let case = format!("DisassociateUser by {class}");
        let reply = disassociate(&h, &json!({ "InstanceUserArn": arn })).await;
        assert_fails_closed(&case, &arn, &reply);
        assert_body_is_only_the_refusal(&case, &reply);

        // The same ARN beside the other members a caller may send: the refusal does not
        // depend on what else the request carries, which is what deciding first means.
        let case = format!("DisassociateUser by {class}, with the identity it names");
        let reply = disassociate(
            &h,
            &json!({
                "InstanceUserArn": arn,
                "Username": "held.user",
                "IdentityProvider": provider_union(REGISTERED_DIRECTORY),
            }),
        )
        .await;
        assert_fails_closed(&case, &arn, &reply);
        assert_body_is_only_the_refusal(&case, &reply);
    }
}

/// RULE, the other half: the decision is about what the request NAMES, and it is not a
/// blanket refusal — an association the store HOLDS, addressed by its `InstanceUserArn`
/// alone, is served and observable afterwards, and a request that names an instance the
/// account holds is decided on that instance, so a malformed ARN it also carries is still
/// refused by the boundary validator.
#[tokio::test]
async fn a_held_association_is_served_and_a_named_instance_still_validates() {
    let h = harness();
    registered(&h, REGISTERED_DIRECTORY, PRODUCTS[0], "the registration").await;
    let held = held_instance(0);

    // A request that NAMES an instance the account holds is decided on that instance, so
    // the malformed ARN beside it is refused by the boundary validator — the gate changed
    // WHICH decision comes first, not whether the modelled constraints are enforced.
    let still_validated = disassociate(
        &h,
        &json!({
            "Username": "held.user",
            "InstanceId": held,
            "IdentityProvider": provider_union(REGISTERED_DIRECTORY),
            "InstanceUserArn": "not-an-arn",
        }),
    )
    .await;
    assert_eq!(
        still_validated.error_type.as_deref(),
        Some("ValidationException"),
        "a request naming a held instance is still constraint-checked, got \
         {still_validated:?}"
    );
    assert_eq!(
        still_validated.status, 400,
        "ValidationException is modelled at HTTP 400, got {still_validated:?}"
    );

    // And the association the store holds, addressed by the ARN alone, is served: the
    // disassociation is accepted and the association is reported in the status it
    // announced.
    for (index, username) in ["first.user", "second.user"].iter().enumerate() {
        let case = format!("the held association of {username}");
        let arn = associated(&h, &held, username, &case).await;
        let ended = disassociate(&h, &json!({ "InstanceUserArn": arn })).await;
        let summary = ended.ok(&case)["InstanceUserSummary"].clone();
        assert_eq!(
            as_str(&summary, "InstanceUserArn", &case),
            arn.as_str(),
            "case {case:?}: the answer must be the association the ARN addressed, got \
             {summary:?}"
        );
        assert_eq!(
            as_str(&summary, "Status", &case),
            "DISASSOCIATING",
            "case {case:?}: the answer carries the status the call put the association \
             in, got {summary:?}"
        );

        // Read back through the list: the association is still reported while its teardown
        // runs, which is what the non-terminal status announces.
        let listed = list_associations(
            &h,
            &json!({
                "InstanceId": held,
                "IdentityProvider": provider_union(REGISTERED_DIRECTORY),
            }),
        )
        .await;
        let summaries = listed.ok(&case)["InstanceUserSummaries"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let reported = summaries
            .iter()
            .find(|summary| summary.get("InstanceUserArn").and_then(Value::as_str) == Some(&arn))
            .unwrap_or_else(|| {
                panic!("case {case:?}: the list must still report {arn}, got {summaries:?}")
            });
        assert_eq!(
            as_str(reported, "Status", &case),
            "DISASSOCIATING",
            "case {case:?}: the list reports the stored status, got {reported:?}"
        );
        assert_eq!(
            summaries.len(),
            index + 1,
            "case {case:?}: the instance's associations are reported, got {summaries:?}"
        );
    }
}
