// SPDX-License-Identifier: CC0-1.0

//! What this service REFUSES, and with which code — driven over the real wire path, so
//! every call is deserialized, constraint-checked, handled and serialized exactly as a
//! client's would be.
//!
//! The rules under test, each asserted over a SET of inputs rather than one request:
//!
//! * `Product` admits only the values its own documentation lists, and a value outside
//!   that set is a `ValidationException` NAMING the value — on every operation that takes
//!   one;
//! * an `ActiveDirectoryIdentityProvider.DirectoryId` must name a directory of this
//!   account, in the `Active` stage, and one that does not is a
//!   `ValidationException` naming the requested id;
//! * an identity provider that is addressed but not registered is an INVALID PARAMETER
//!   ("Identity provider is not valid") to the operations that manage or report the
//!   registration, and `UnauthorizedException` — failing closed — to the operations that
//!   start or end a user's product subscription;
//! * a request that addresses a resource by none of the members that identify it is
//!   refused by LISTING those members ("<members> must be provided");
//! * a `ResourceArn` the service cannot use is a `BadRequestException`, whether the ARN is
//!   malformed or merely names no resource, and whichever layer catches it;
//! * `AssociateUser` is authorized BEFORE its request shape is validated, so an
//!   unauthorized call is refused identically whatever else the request gets wrong.
//!
//! The harness (router + engine + the account's directory fleet the registrations resolve
//! against) is the one in [`super::identity_provider_subscriptions`]; only the rules are
//! new here.

use serde_json::{json, Value};

use service_com_amazonaws_licensemanagerusersubscriptions_licensemanagerusersubscriptions::state as service_state;
use service_com_amazonaws_licensemanagerusersubscriptions_licensemanagerusersubscriptions::ComAmazonawsLicensemanagerusersubscriptionsLicensemanagerusersubscriptionsState as ServiceState;

use super::identity_provider_subscriptions as fixture;

/// A directory this suite registers in the account's fleet, `Active` and resolvable.
const RESOLVABLE_DIRECTORY: &str = "d-0123456789";
/// A second one, so a rule that names "the" directory is exercised on more than one id.
const SECOND_RESOLVABLE_DIRECTORY: &str = "d-00000000ff";

/// Put `directory_id` in the account's directory fleet in `stage`, so a row can name a
/// directory the account has in a stage other than `Active`.
fn directory_stage(h: &fixture::Harness, directory_id: &str, stage: &str) {
    let store = h.engine.state::<ServiceState>();
    store.lock().register_account_directory(directory_id, stage);
}

/// Take `directory_id` out of the account's directory fleet, as deleting the directory
/// does.
fn remove_directory(h: &fixture::Harness, directory_id: &str) {
    let store = h.engine.state::<ServiceState>();
    store.lock().account_directories.remove(directory_id);
}

/// The sentence this service fails closed with, as recorded.
const UNAUTHORIZED_MESSAGE: &str =
    "Unauthorized. Please check the request and access policy and try again.";

/// One EC2 instance of the account's seeded fleet (`state::seed_account_instances`), so a
/// request can be AUTHORIZED; every other instance id below is deliberately not in it.
const AUTHORIZED_INSTANCE: &str = "i-0f1e2d3c4b5a69788";

/// The status this service reports a refusal it answers with a non-modelled code with
/// (`scaffolded::SERVICE_REFUSAL_STATUS`).
const REFUSAL_STATUS: u16 = 400;

fn settings() -> Value {
    json!({ "Subnets": ["subnet-0123456789abcdef0"], "SecurityGroupId": "sg-0123abcd" })
}

fn registration(directory_id: &str, product: &str) -> Value {
    json!({
        "IdentityProvider": fixture::provider_union(directory_id),
        "Product": product,
        "Settings": settings(),
    })
}

async fn register(h: &fixture::Harness, body: &Value) -> fixture::Reply {
    fixture::call(h, "/identity-provider/RegisterIdentityProvider", body).await
}

async fn deregister(h: &fixture::Harness, body: &Value) -> fixture::Reply {
    fixture::call(h, "/identity-provider/DeregisterIdentityProvider", body).await
}

async fn start_subscription(h: &fixture::Harness, body: &Value) -> fixture::Reply {
    fixture::call(h, "/user/StartProductSubscription", body).await
}

async fn stop_subscription(h: &fixture::Harness, body: &Value) -> fixture::Reply {
    fixture::call(h, "/user/StopProductSubscription", body).await
}

async fn list_subscriptions(h: &fixture::Harness, body: &Value) -> fixture::Reply {
    fixture::call(h, "/user/ListProductSubscriptions", body).await
}

async fn update_settings(h: &fixture::Harness, body: &Value) -> fixture::Reply {
    fixture::call(h, "/identity-provider/UpdateIdentityProviderSettings", body).await
}

async fn associate_user(h: &fixture::Harness, body: &Value) -> fixture::Reply {
    fixture::call(h, "/user/AssociateUser", body).await
}

/// Percent-encode one URI path segment: `ResourceArn` is an `@httpLabel` on
/// `/tags/{ResourceArn}`, and an ARN carries `:` and `/`, so a client encodes it or the
/// `/` would split the path into another segment.
///
/// The encoder is the shared fixture's ([`fixture::encoded_label`]) — ONE encoder for this
/// crate's suites, so `refusals.rs` and `resource_resolution_refusals.rs` cannot disagree
/// about what a client sends.
fn encode_segment(value: &str) -> String {
    fixture::encoded_label(value)
}

async fn list_tags(h: &fixture::Harness, resource_arn: &str) -> fixture::Reply {
    let path = format!("/tags/{}", encode_segment(resource_arn));
    fixture::send(h, "GET", &path, None).await
}

async fn tag_resource(h: &fixture::Harness, resource_arn: &str, tags: &Value) -> fixture::Reply {
    let path = format!("/tags/{}", encode_segment(resource_arn));
    fixture::send(h, "PUT", &path, Some(&json!({ "Tags": tags }))).await
}

async fn untag_resource(h: &fixture::Harness, resource_arn: &str, keys: &[&str]) -> fixture::Reply {
    let query: Vec<String> = keys
        .iter()
        .map(|key| format!("tagKeys={}", encode_segment(key)))
        .collect();
    let path = format!(
        "/tags/{}?{}",
        encode_segment(resource_arn),
        query.join("&")
    );
    fixture::send(h, "DELETE", &path, None).await
}

/// Assert one refusal: the status, the wire error code and the message, naming the case
/// and showing what was actually answered.
fn refused(reply: &fixture::Reply, case: &str, status: u16, code: &str, message: &str) {
    assert_eq!(
        reply.error_type.as_deref(),
        Some(code),
        "case {case:?}: expected {code}, got {reply:?}"
    );
    assert_eq!(
        reply.status, status,
        "case {case:?}: expected HTTP {status}, got {reply:?}"
    );
    assert_eq!(
        reply.message(),
        Some(message),
        "case {case:?}: wrong refusal message in {reply:?}"
    );
}

/// The set the handlers ENFORCE is the set the documentation publishes.
///
/// The suite carries the five published literals ([`fixture::PRODUCTS`]); this is what
/// ties them to `state::PRODUCTS`, so truncating, extending or misspelling the enforced
/// set fails here rather than silently changing which values the tests above accept.
#[test]
fn the_enforced_product_set_is_the_documented_one() {
    assert_eq!(
        service_state::PRODUCTS.as_slice(),
        fixture::PRODUCTS,
        "the enforced product set drifted from the documented one: enforced {:?}, documented {:?}",
        service_state::PRODUCTS,
        fixture::PRODUCTS
    );
}

/// RULE: `Product` admits only the values its own documentation lists
/// ("Valid values: VISUAL_STUDIO_ENTERPRISE | VISUAL_STUDIO_PROFESSIONAL |
/// OFFICE_PROFESSIONAL_PLUS | OFFICE_STANDARD | REMOTE_DESKTOP_SERVICES"), and a value
/// outside that set is refused with `ValidationException` (HTTP 400) whose message names
/// the value that was refused — on every operation that takes a `Product`, which is all
/// six of them.
///
/// The accepted half of the rule is the published list, exercised on every entry; the
/// refused half is values just outside it (wrong case, a near-miss, padding, empty, and
/// one the issue did not cite).
#[tokio::test]
async fn a_product_outside_the_documented_set_is_refused_naming_the_value() {
    let h = fixture::harness();

    // Every published value is accepted. One registration per product, all on one
    // directory, which is what the (provider, product) index allows.
    for product in fixture::PRODUCTS {
        register(&h, &registration(RESOLVABLE_DIRECTORY, product))
            .await
            .ok(product);
    }

    // Values just outside the published set. `OFFICE_STANDARD ` and `office_standard` are
    // one character and one case away from a published value; the rest are further off.
    let refused_products = [
        "NOT_A_REAL_PRODUCT_XYZ",
        "office_standard",
        "OFFICE_STANDARD ",
        "VISUAL_STUDIO",
        "",
    ];
    for product in refused_products {
        let cases: [(&str, fixture::Reply); 6] = [
            (
                "RegisterIdentityProvider",
                register(&h, &registration(SECOND_RESOLVABLE_DIRECTORY, product)).await,
            ),
            (
                "StartProductSubscription",
                start_subscription(
                    &h,
                    &json!({
                        "Username": "someone",
                        "IdentityProvider": fixture::provider_union(RESOLVABLE_DIRECTORY),
                        "Product": product,
                    }),
                )
                .await,
            ),
            (
                "StopProductSubscription",
                stop_subscription(
                    &h,
                    &json!({
                        "Username": "someone",
                        "IdentityProvider": fixture::provider_union(RESOLVABLE_DIRECTORY),
                        "Product": product,
                    }),
                )
                .await,
            ),
            (
                "ListProductSubscriptions",
                list_subscriptions(
                    &h,
                    &json!({
                        "IdentityProvider": fixture::provider_union(RESOLVABLE_DIRECTORY),
                        "Product": product,
                    }),
                )
                .await,
            ),
            (
                "DeregisterIdentityProvider",
                deregister(
                    &h,
                    &json!({
                        "IdentityProvider": fixture::provider_union(RESOLVABLE_DIRECTORY),
                        "Product": product,
                    }),
                )
                .await,
            ),
            (
                "UpdateIdentityProviderSettings",
                update_settings(
                    &h,
                    &json!({
                        "IdentityProvider": fixture::provider_union(RESOLVABLE_DIRECTORY),
                        "Product": product,
                        "UpdateSettings": {
                            "AddSubnets": ["subnet-1111111111111111a"],
                            "RemoveSubnets": [],
                        },
                    }),
                )
                .await,
            ),
        ];
        for (operation, reply) in cases {
            refused(
                &reply,
                &format!("{operation} with Product {product:?}"),
                400,
                "ValidationException",
                &format!("Invalid product {product}"),
            );
        }
    }

    // And the refusals changed nothing: the registrations the published values created are
    // still the only ones.
    let state = h.engine.state::<ServiceState>();
    assert_eq!(
        state.lock().identity_providers.len(),
        fixture::PRODUCTS.len(),
        "a refused product must not have been registered"
    );
}

/// RULE: an `ActiveDirectoryIdentityProvider.DirectoryId` names a directory of this
/// account in AWS Directory Service; a registration is minted only for one the account
/// holds that is `Active`, and any other is refused with `ValidationException` (HTTP 400)
/// whose message names the requested id.
///
/// The rows differ along what the rule generalises over: the id, whether the directory
/// exists, and which stage it is in.
#[tokio::test]
async fn a_directory_that_does_not_resolve_as_active_is_refused_naming_the_id() {
    let h = fixture::harness();

    // A directory the account has that is NOT `Active`: `CreateDirectory` leaves a
    // directory `Requested` until the transition its clock owes falls due, so one in that
    // stage is in the fleet and still not registrable. `Requested` is one of the stages
    // AWS Directory Service's `DirectoryStage` publishes.
    let pending = "d-aaaaaaaaaa";
    directory_stage(&h, pending, "Requested");

    // (case, directory id, is a registration expected to succeed)
    let cases: [(&str, &str, bool); 5] = [
        ("an Active directory", RESOLVABLE_DIRECTORY, true),
        ("a second Active directory", SECOND_RESOLVABLE_DIRECTORY, true),
        ("a directory that was never created", "d-ffffffffff", false),
        ("a different one that was never created", "d-123abc4567", false),
        ("a directory that is not Active yet", pending, false),
    ];
    for (case, directory_id, expected_ok) in cases {
        let reply = register(
            &h,
            &json!({
                "IdentityProvider": {
                    "ActiveDirectoryIdentityProvider": { "DirectoryId": directory_id }
                },
                "Product": fixture::PRODUCTS[0],
                "Settings": settings(),
            }),
        )
        .await;
        if expected_ok {
            let summary = reply.ok(case)["IdentityProviderSummary"].clone();
            assert_eq!(
                summary["IdentityProvider"],
                json!({
                    "ActiveDirectoryIdentityProvider": { "DirectoryId": directory_id }
                }),
                "case {case:?}: the summary must echo the directory that was registered, \
                 got {summary:?}"
            );
            continue;
        }
        refused(
            &reply,
            case,
            400,
            "ValidationException",
            &format!(
                "{directory_id} directory either cannot be found or is not in active state"
            ),
        );
    }

    // A directory that is deleted stops resolving: what was registrable before is not
    // afterwards, because the answer is read from the account's directories at each call
    // and not from a copy taken at registration time.
    remove_directory(&h, SECOND_RESOLVABLE_DIRECTORY);
    let reply = register(
        &h,
        &registration(SECOND_RESOLVABLE_DIRECTORY, fixture::PRODUCTS[1]),
    )
    .await;
    refused(
        &reply,
        "a directory that has been deleted",
        400,
        "ValidationException",
        &format!(
            "{SECOND_RESOLVABLE_DIRECTORY} directory either cannot be found or is not in \
             active state"
        ),
    );

    // The forms the account's own directory fleet does not hold are not refused on
    // Directory Service's behalf,
    // and each is recognised the way the MODEL expresses it: `IsSharedActiveDirectory`, or
    // the `sd-` form the member's `@pattern` admits beside `d-`, both name a directory
    // SHARED into this account (reported by `ds:DescribeSharedDirectories`, still a
    // scaffolded 501 there), and a provider naming no `DirectoryId` is a self-managed
    // Active Directory, not a Directory Service resource at all. The first two rows show
    // the shared carve-out does not depend on the id's shape: one wears a `sd-` id and one
    // a `d-` id no directory of this account holds.
    let unresolved_forms = [
        (
            "a shared directory with a sd- id",
            fixture::provider_union("sd-abcdef0123"),
        ),
        (
            "a shared directory with a d- id this account does not hold",
            json!({
                "ActiveDirectoryIdentityProvider": {
                    "DirectoryId": "d-7777777777",
                    "IsSharedActiveDirectory": true,
                }
            }),
        ),
        (
            "a self-managed Active Directory",
            json!({
                "ActiveDirectoryIdentityProvider": {
                    "ActiveDirectorySettings": { "DomainName": "corp.example.com" }
                }
            }),
        ),
    ];
    for (case, provider) in unresolved_forms {
        let reply = register(
            &h,
            &json!({
                "IdentityProvider": provider,
                "Product": fixture::PRODUCTS[2],
                "Settings": settings(),
            }),
        )
        .await;
        reply.ok(case);
    }
}

/// RULE: an identity provider that a request addresses but that is not a registration of
/// this account is answered per PLANE — the operations that manage or report the
/// registration report an invalid parameter (`ValidationException`, HTTP 400, "Identity
/// provider is not valid"), and the operations that start or end a user's PRODUCT
/// SUBSCRIPTION fail closed (`UnauthorizedException`) rather than disclose whether it
/// exists.
///
/// `AssociateUser` is neither, and is not covered here: it resolves its registration after
/// its own instance-authorization gate has already established what its authorization is
/// about, and answers the modelled `ResourceNotFoundException` — the contract that
/// operation landed with, which `resource_resolution_refusals` pins.
///
/// The rows differ along the addressing form (an ARN, or the `(IdentityProvider, Product)`
/// pair), the directory named, and the product.
#[tokio::test]
async fn an_unregistered_identity_provider_is_answered_per_plane() {
    let h = fixture::harness();
    // One real registration, so the refusals below are about the provider the request
    // names and not about an empty store.
    register(
        &h,
        &registration(RESOLVABLE_DIRECTORY, fixture::PRODUCTS[0]),
    )
    .await
    .ok("the registration the other rows are not");

    let never_registered_arn = format!(
        "{}ip-ffffffffffff",
        fixture::arn_prefix("identity-provider")
    );

    // The registration plane: the identifier is reported as invalid input.
    let provider_plane: [(&str, fixture::Reply); 4] = [
        (
            "DeregisterIdentityProvider by a never-registered ARN",
            deregister(&h, &json!({ "IdentityProviderArn": never_registered_arn })).await,
        ),
        (
            "DeregisterIdentityProvider by an unregistered directory",
            deregister(
                &h,
                &json!({
                    "IdentityProvider": fixture::provider_union(SECOND_RESOLVABLE_DIRECTORY),
                    "Product": fixture::PRODUCTS[0],
                }),
            )
            .await,
        ),
        (
            "DeregisterIdentityProvider by a product the provider is not registered for",
            deregister(
                &h,
                &json!({
                    "IdentityProvider": fixture::provider_union(RESOLVABLE_DIRECTORY),
                    "Product": fixture::PRODUCTS[3],
                }),
            )
            .await,
        ),
        (
            "ListProductSubscriptions through an unregistered provider",
            list_subscriptions(
                &h,
                &json!({
                    "IdentityProvider": fixture::provider_union(SECOND_RESOLVABLE_DIRECTORY),
                    "Product": fixture::PRODUCTS[0],
                }),
            )
            .await,
        ),
    ];
    for (case, reply) in provider_plane {
        refused(
            &reply,
            case,
            400,
            "ValidationException",
            "Identity provider is not valid",
        );
    }

    // The user plane: the same facts, and nothing is said about them.
    let user_plane: [(&str, fixture::Reply); 3] = [
        (
            "StartProductSubscription through an unregistered provider",
            start_subscription(
                &h,
                &json!({
                    "Username": "someone",
                    "IdentityProvider": fixture::provider_union(SECOND_RESOLVABLE_DIRECTORY),
                    "Product": fixture::PRODUCTS[0],
                }),
            )
            .await,
        ),
        (
            "StartProductSubscription for a product the provider is not registered for",
            start_subscription(
                &h,
                &json!({
                    "Username": "someone",
                    "IdentityProvider": fixture::provider_union(RESOLVABLE_DIRECTORY),
                    "Product": fixture::PRODUCTS[4],
                }),
            )
            .await,
        ),
        (
            "StopProductSubscription through an unregistered provider",
            stop_subscription(
                &h,
                &json!({
                    "Username": "someone",
                    "IdentityProvider": fixture::provider_union(SECOND_RESOLVABLE_DIRECTORY),
                    "Product": fixture::PRODUCTS[0],
                }),
            )
            .await,
        ),
    ];
    for (case, reply) in user_plane {
        refused(
            &reply,
            case,
            REFUSAL_STATUS,
            "UnauthorizedException",
            UNAUTHORIZED_MESSAGE,
        );
    }
}

/// RULE: a request that addresses a resource by none of the members that identify it is
/// refused with `ValidationException` whose message LISTS those members in alphabetical
/// order — the "<members> must be provided" form `StopProductSubscription` and
/// `DisassociateUser` both use.
///
/// The rows differ along which subset of the identifying members is supplied (none, or
/// some but not all) and along which operation is asked, since the two operations require
/// different members.
#[tokio::test]
async fn a_request_naming_no_complete_identifier_lists_the_members_it_needs() {
    let h = fixture::harness();
    register(
        &h,
        &registration(RESOLVABLE_DIRECTORY, fixture::PRODUCTS[0]),
    )
    .await
    .ok("a registration exists, so these refusals are about the request");

    let product_user_triple = "IdentityProvider, Product, and Username must be provided";
    let provider_pair = "IdentityProvider and Product must be provided";

    let cases: [(&str, fixture::Reply, &str); 6] = [
        (
            "StopProductSubscription naming nothing",
            stop_subscription(&h, &json!({})).await,
            product_user_triple,
        ),
        (
            "StopProductSubscription naming only Username",
            stop_subscription(&h, &json!({ "Username": "someone" })).await,
            product_user_triple,
        ),
        (
            "StopProductSubscription naming Username and Product but no IdentityProvider",
            stop_subscription(
                &h,
                &json!({ "Username": "someone", "Product": fixture::PRODUCTS[0] }),
            )
            .await,
            product_user_triple,
        ),
        (
            "StopProductSubscription naming the provider and product but no Username",
            stop_subscription(
                &h,
                &json!({
                    "IdentityProvider": fixture::provider_union(RESOLVABLE_DIRECTORY),
                    "Product": fixture::PRODUCTS[0],
                }),
            )
            .await,
            product_user_triple,
        ),
        (
            "DeregisterIdentityProvider naming nothing",
            deregister(&h, &json!({})).await,
            provider_pair,
        ),
        (
            "DeregisterIdentityProvider naming only Product",
            deregister(&h, &json!({ "Product": fixture::PRODUCTS[0] })).await,
            provider_pair,
        ),
    ];
    for (case, reply, message) in cases {
        refused(&reply, case, 400, "ValidationException", message);
    }
}

/// RULE: a `ResourceArn` this service cannot use is refused with `BadRequestException` —
/// "Invalid input resource arn: bad syntax" when the value is not shaped like an ARN at
/// all, and "Invalid input resource arn" when it is well formed but names no resource this
/// service holds — on every tagging operation, and whichever layer catches it.
///
/// The rows differ along how the ARN is wrong: not an ARN at all, too few segments, an
/// empty segment, no resource id, another service's resource, a region with nothing in it.
/// The first group is refused by the member's modelled `@pattern` before any handler runs
/// and the second by the handler, and the rule is that a client branches on ONE code
/// either way — only the sentence says which of the two was wrong.
#[tokio::test]
async fn a_resource_arn_the_service_cannot_use_is_a_bad_request() {
    let h = fixture::harness();
    let summary = register(
        &h,
        &registration(RESOLVABLE_DIRECTORY, fixture::PRODUCTS[0]),
    )
    .await
    .ok("a registration to tag")["IdentityProviderSummary"]
        .clone();
    let provider_arn = summary["IdentityProviderArn"]
        .as_str()
        .expect("a minted IdentityProviderArn")
        .to_string();

    let malformed = [
        "not-an-arn",
        "arn:aws:license-manager-user-subscriptions",
        "arn:aws:license-manager-user-subscriptions:us-west-2:000000000000:identity-provider",
        "arn::license-manager-user-subscriptions:us-west-2:000000000000:identity-provider/ip-1",
    ];
    for resource_arn in malformed {
        let case = format!("malformed ResourceArn {resource_arn:?}");
        refused(
            &list_tags(&h, resource_arn).await,
            &format!("ListTagsForResource with a {case}"),
            400,
            "BadRequestException",
            "Invalid input resource arn: bad syntax",
        );
        refused(
            &tag_resource(&h, resource_arn, &json!({ "team": "licensing" })).await,
            &format!("TagResource with a {case}"),
            400,
            "BadRequestException",
            "Invalid input resource arn: bad syntax",
        );
        refused(
            &untag_resource(&h, resource_arn, &["team"]).await,
            &format!("UntagResource with a {case}"),
            400,
            "BadRequestException",
            "Invalid input resource arn: bad syntax",
        );
    }

    // Well formed, and naming nothing this service holds: a different sentence, the same
    // code. The third row is well formed under the member's `@pattern` but names another
    // service's resource, which this service holds no more than it holds a registration it
    // never minted.
    let unresolvable = [
        format!("{}ip-ffffffffffff", fixture::arn_prefix("identity-provider")),
        format!("{}pu-ffffffffffff", fixture::arn_prefix("user")),
        "arn:aws:s3:us-west-2:000000000000:bucket/example".to_string(),
        format!(
            "arn:aws:license-manager-user-subscriptions:eu-west-1:000000000000:identity-provider/{}",
            "ip-000000000001"
        ),
    ];
    for resource_arn in &unresolvable {
        let case = format!("unresolvable ResourceArn {resource_arn:?}");
        refused(
            &list_tags(&h, resource_arn).await,
            &format!("ListTagsForResource with an {case}"),
            400,
            "BadRequestException",
            "Invalid input resource arn",
        );
        refused(
            &tag_resource(&h, resource_arn, &json!({ "team": "licensing" })).await,
            &format!("TagResource with an {case}"),
            400,
            "BadRequestException",
            "Invalid input resource arn",
        );
        refused(
            &untag_resource(&h, resource_arn, &["team"]).await,
            &format!("UntagResource with an {case}"),
            400,
            "BadRequestException",
            "Invalid input resource arn",
        );
    }

    // And a request that is BOTH malformed as an ARN and wrong in another member is still
    // that one refusal: the ARN this service cannot use is what a tag-management client
    // branches on, so it is not hidden behind the other violation.
    let mut oversized = serde_json::Map::new();
    for index in 0..51 {
        oversized.insert(format!("k{index}"), Value::String("x".to_string()));
    }
    refused(
        &tag_resource(&h, "not-an-arn", &Value::Object(oversized)).await,
        "TagResource with a malformed ResourceArn AND an over-length Tags map",
        400,
        "BadRequestException",
        "Invalid input resource arn: bad syntax",
    );

    // And the usable ARN round-trips through the three operations: what `TagResource`
    // wrote is what `ListTagsForResource` reports, and `UntagResource` removes from the
    // same set.
    assert_eq!(
        list_tags(&h, &provider_arn).await.ok("a registration with no tags")["Tags"],
        json!({}),
        "a resource with no tags has an empty tag set"
    );
    tag_resource(
        &h,
        &provider_arn,
        &json!({ "team": "licensing", "env": "test" }),
    )
    .await
    .ok("two tags written");
    assert_eq!(
        list_tags(&h, &provider_arn).await.ok("two tags read back")["Tags"],
        json!({ "team": "licensing", "env": "test" }),
        "ListTagsForResource must report what TagResource wrote"
    );
    // Tagging MERGES: a named key takes the new value, an unnamed one is left alone.
    tag_resource(&h, &provider_arn, &json!({ "env": "prod" }))
        .await
        .ok("one tag overwritten");
    assert_eq!(
        list_tags(&h, &provider_arn).await.ok("the merged tag set")["Tags"],
        json!({ "team": "licensing", "env": "prod" }),
        "tagging must merge rather than replace the set"
    );
    // Removing a key the resource does not carry is nothing to remove, not a failure.
    untag_resource(&h, &provider_arn, &["env", "never-set"])
        .await
        .ok("one tag removed and one that was not there");
    assert_eq!(
        list_tags(&h, &provider_arn).await.ok("what is left")["Tags"],
        json!({ "team": "licensing" }),
        "UntagResource must remove exactly the keys it named"
    );
}

/// RULE: `AssociateUser` is authorized BEFORE its request shape is validated, so a call
/// this service will not authorize is refused with `UnauthorizedException` and the fixed
/// sentence whatever else the request gets wrong — an oversized `Tags` map included, even
/// though `Tags` is modelled `@length(max: 50)` and the boundary validator would otherwise
/// refuse it as a `ValidationException`.
///
/// The predicate is the handler's own — whether the account's EC2 fleet holds the
/// `InstanceId` — asked earlier, so the two gates cannot disagree.
///
/// The rows differ along what else is wrong with the request: nothing, a `Tags` map at the
/// modelled maximum, one entry past it, far past it, and an instance id of no shape AWS
/// mints.
#[tokio::test]
async fn associate_user_is_authorized_before_its_shape_is_validated() {
    let h = fixture::harness();

    fn tags(count: usize) -> Value {
        let mut map = serde_json::Map::new();
        for index in 0..count {
            map.insert(format!("k{index}"), Value::String("x".to_string()));
        }
        Value::Object(map)
    }

    // (case, instance id, how many tags)
    let cases: [(&str, &str, usize); 5] = [
        ("a compliant request", "i-0123456789abcdef0", 1),
        ("no tags at all", "i-0123456789abcdef0", 0),
        ("the modelled maximum", "i-0123456789abcdef0", 50),
        ("one tag past the modelled maximum", "i-0123456789abcdef0", 51),
        ("far past it, and an instance id of no AWS shape", "nope", 120),
    ];

    // None of these instances is in the account's fleet, so no request is authorized:
    // every row gets the SAME answer, whatever else is wrong with it.
    for (case, instance_id, tag_count) in cases {
        let reply = associate_user(
            &h,
            &json!({
                "Username": "someone",
                "InstanceId": instance_id,
                "IdentityProvider": fixture::provider_union(RESOLVABLE_DIRECTORY),
                "Tags": tags(tag_count),
            }),
        )
        .await;
        refused(
            &reply,
            case,
            REFUSAL_STATUS,
            "UnauthorizedException",
            UNAUTHORIZED_MESSAGE,
        );
    }

    // The gate is not a blanket refusal: an instance the account's fleet DOES hold, with
    // the user's identity provider registered, is associated.
    register(
        &h,
        &registration(RESOLVABLE_DIRECTORY, fixture::PRODUCTS[0]),
    )
    .await
    .ok("the registration the association resolves the user through");
    let reply = associate_user(
        &h,
        &json!({
            "Username": "someone",
            "InstanceId": AUTHORIZED_INSTANCE,
            "IdentityProvider": fixture::provider_union(RESOLVABLE_DIRECTORY),
            "Tags": tags(1),
        }),
    )
    .await;
    let summary = reply.ok("an authorized association")["InstanceUserSummary"].clone();
    assert_eq!(
        summary["InstanceId"].as_str(),
        Some(AUTHORIZED_INSTANCE),
        "the association must be for the instance the request named, got {summary:?}"
    );

    // And a request the gate lets through is still constraint-checked, so the ordering
    // changed which refusal comes first and not whether the modelled bound is enforced.
    let reply = associate_user(
        &h,
        &json!({
            "Username": "another",
            "InstanceId": AUTHORIZED_INSTANCE,
            "IdentityProvider": fixture::provider_union(RESOLVABLE_DIRECTORY),
            "Tags": tags(51),
        }),
    )
    .await;
    assert_eq!(
        reply.error_type.as_deref(),
        Some("ValidationException"),
        "an authorized request past the modelled Tags bound is still refused, got {reply:?}"
    );
}
