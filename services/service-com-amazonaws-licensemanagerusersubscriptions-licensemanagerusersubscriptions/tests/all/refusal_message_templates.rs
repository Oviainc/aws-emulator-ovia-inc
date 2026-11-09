// SPDX-License-Identifier: CC0-1.0

//! What this service's 400 refusal BODIES say, driven over the real wire path: every call
//! goes through the router's `dispatch`, so each request is deserialized,
//! constraint-checked, handled and serialized exactly as a client's would be.
//!
//! Three rules, each asserted over a TABLE whose rows differ along what the rule
//! generalises over rather than over one recorded request:
//!
//! * a `Filter` naming an attribute the list operation it was sent to does not filter on
//!   is refused with the service's template — the fixed text `Invalid Filter attribute `
//!   and the rejected name copied from the request — for every list operation of this
//!   service, and with no enumeration of what the operation does accept;
//! * a `@pattern` requirement is quoted WITHOUT the whole-value `^` / `$` anchors the
//!   model writes around the expression, for every pattern-constrained member of every
//!   operation — while an expression the model anchors at one end only keeps its text,
//!   because what the anchors express is the PAIR the model wraps a total match in;
//! * a registration naming a directory the account cannot use is refused for THAT
//!   prerequisite, whatever else the request also names, so the blocker a caller has to
//!   fix first is the one it is told about.

use serde_json::{json, Value};

use service_com_amazonaws_licensemanagerusersubscriptions_licensemanagerusersubscriptions::state::ComAmazonawsLicensemanagerusersubscriptionsLicensemanagerusersubscriptionsState;

// ONE wire harness per service: the dispatch plumbing, the signing identity, the account's
// directory fleet and the account's network come from the sibling suites rather than a
// second copy.
use crate::identity_provider_subscriptions::{
    call, harness, list_providers, provider_union, register, Harness, Reply,
    ACCOUNT_SECURITY_GROUPS, ACCOUNT_SUBNETS, PRODUCTS,
};
use crate::resource_resolution_refusals::held_instance;
use crate::settings_network_resolution::{DIRECTORY, UNHELD_SECURITY_GROUP, UNHELD_SUBNET};

/// Assert `reply` is the modelled `ValidationException` (HTTP 400) whose body says exactly
/// `message` — the whole sentence, so an extra clause or an appended list of what the
/// service accepts fails the row.
fn assert_refusal_says(reply: &Reply, message: &str, case: &str) {
    assert_eq!(
        reply.status, 400,
        "case {case:?}: ValidationException is modelled at HTTP 400, got {reply:?}"
    );
    assert_eq!(
        reply.error_type.as_deref(),
        Some("ValidationException"),
        "case {case:?}: expected ValidationException, got {reply:?}"
    );
    assert_eq!(
        reply.message(),
        Some(message),
        "case {case:?}: expected the refusal to say {message:?}, got {:?}",
        reply.message()
    );
}

// ---------------------------------------------------------------------------
// A filter attribute the operation does not filter on
// ---------------------------------------------------------------------------

/// One list operation of this service, its route, and the request members it needs beside
/// `Filters` to be addressed at all.
struct ListRoute {
    operation: &'static str,
    uri: &'static str,
    /// The members the operation's model marks `@required` beside `Filters`.
    required: fn() -> Value,
}

/// Every list operation of this service whose input carries `Filters`, except
/// `ListUserAssociations`: its `@required` `InstanceId` is resolved by the hoisted
/// authorization gate before the request shape is looked at (see
/// `instance_authorization_is_hoisted`), so its refusal is about the instance rather than
/// about the filter. Routes from `smithy_query(query="operation", operation="ListInstances,
/// ListIdentityProviders,ListProductSubscriptions,ListLicenseServerEndpoints")`.
const LIST_ROUTES: &[ListRoute] = &[
    ListRoute {
        operation: "ListInstances",
        uri: "/instance/ListInstances",
        required: || json!({}),
    },
    ListRoute {
        operation: "ListIdentityProviders",
        uri: "/identity-provider/ListIdentityProviders",
        required: || json!({}),
    },
    ListRoute {
        operation: "ListProductSubscriptions",
        uri: "/user/ListProductSubscriptions",
        required: || json!({ "IdentityProvider": provider_union(DIRECTORY) }),
    },
    ListRoute {
        operation: "ListLicenseServerEndpoints",
        uri: "/license-server/ListLicenseServerEndpoints",
        required: || json!({}),
    },
];

/// Attribute names no list operation of this service filters on. The rows differ along the
/// FORM of the rejected name — a made-up name, an attribute another list operation of this
/// service does accept, a response member that is not filterable, the wrong case of a
/// documented one, a name carrying punctuation or spaces, and a long one — because the
/// template has one variable part and it is copied from the request whatever it says.
///
/// `Filter.Attribute` is a plain `String` in the model with no enum
/// (`smithy_query(query="shape",shape="Filter")`), so nothing is refused here by a modelled
/// constraint: every row reaches the operation's own filter validation.
const UNFILTERABLE_ATTRIBUTES: &[&str] = &[
    "NotAnAttribute",
    "Status",
    "InstanceId",
    "Product",
    "DirectoryId",
    "IdentityProviderArn",
    "Username",
    "OwnerAccountId",
    "instanceId",
    "INSTANCEID",
    "Products",
    "tag:Name",
    "an attribute with spaces",
    "AnAttributeNameLongEnoughToBeInterestingXXXXXXXXXXXXXXXXXXXXXXXX",
];

/// Which of the above each route DOES filter on, from that operation's own documentation
/// ("You can use the following filters to streamline results: …"), so those rows are not
/// expected to be refused there.
fn filterable(operation: &str) -> &'static [&'static str] {
    match operation {
        "ListInstances" => &["Status", "InstanceId"],
        "ListIdentityProviders" => &["Product", "DirectoryId"],
        "ListProductSubscriptions" => &["Status", "Username", "Domain"],
        "ListLicenseServerEndpoints" => &["IdentityProviderArn"],
        other => panic!("no filterable set is stated for {other}"),
    }
}

/// RULE: a `Filter.Attribute` the list operation does not filter on is refused with
/// `Invalid Filter attribute ` followed by the rejected name as the request spelled it —
/// one wording for every list operation, naming nothing else.
#[tokio::test]
async fn an_unfilterable_filter_attribute_is_refused_with_the_services_template() {
    let h = harness();
    for route in LIST_ROUTES {
        for attribute in UNFILTERABLE_ATTRIBUTES {
            if filterable(route.operation).contains(attribute) {
                continue;
            }
            let case = format!("{} with Filter.Attribute {attribute:?}", route.operation);
            let mut body = (route.required)();
            body["Filters"] = json!([{ "Attribute": attribute, "Value": "anything" }]);
            let reply = call(&h, route.uri, &body).await;
            assert_refusal_says(&reply, &format!("Invalid Filter attribute {attribute}"), &case);
        }
    }
}

/// RULE: the refusal is a fact about the REQUEST, so it does not depend on what the store
/// holds — an operation with records to filter answers it exactly as an empty one does.
#[tokio::test]
async fn the_filter_refusal_does_not_depend_on_the_store() {
    let h = harness();
    let attribute = "NotAnAttribute";
    let filters = json!([{ "Attribute": attribute, "Value": "anything" }]);
    let expected = format!("Invalid Filter attribute {attribute}");

    // An empty store.
    let empty = list_providers(&h, &json!({ "Filters": filters })).await;
    assert_refusal_says(&empty, &expected, "an empty store");

    // The same call once a registration is held.
    register(
        &h,
        &json!({
            "IdentityProvider": provider_union(DIRECTORY),
            "Product": PRODUCTS[0],
            "Settings": {
                "Subnets": [ACCOUNT_SUBNETS[0]],
                "SecurityGroupId": ACCOUNT_SECURITY_GROUPS[0],
            },
        }),
    )
    .await
    .ok("the registration a populated store needs");
    let populated = list_providers(&h, &json!({ "Filters": filters })).await;
    assert_refusal_says(&populated, &expected, "a populated store");
}

/// RULE, on the fifth list operation: `ListUserAssociations` is not in the table above
/// because its `@required` `InstanceId` is authorized by the hoisted gate before the
/// request's shape is looked at, so it is driven here with an instance the account's fleet
/// DOES hold — past that gate, the same template answers.
#[tokio::test]
async fn the_filter_template_answers_the_instance_scoped_list_too() {
    let h = harness();
    let instance_id = held_instance(0);
    for attribute in ["NotAnAttribute", "InstanceId", "Product", "username"] {
        let case = format!("ListUserAssociations with Filter.Attribute {attribute:?}");
        let reply = call(
            &h,
            "/user/ListUserAssociations",
            &json!({
                "InstanceId": instance_id,
                "IdentityProvider": provider_union(DIRECTORY),
                "Filters": [{ "Attribute": attribute, "Value": "anything" }],
            }),
        )
        .await;
        assert_refusal_says(&reply, &format!("Invalid Filter attribute {attribute}"), &case);
    }
}

// ---------------------------------------------------------------------------
// A @pattern requirement, quoted without the model's whole-value anchors
// ---------------------------------------------------------------------------

/// The `Arn` shape's `@pattern` as the MODEL declares it
/// (`smithy_query(query="shape",shape="Arn")`), anchors included. Every ARN-valued member
/// of this model targets that shape.
const ARN_PATTERN: &str = r"^arn:[a-z0-9-\.]{1,63}:[a-z0-9-\.]{1,63}:[a-z0-9-\.]{1,63}:[a-z0-9-\.]{1,63}:[a-zA-Z0-9-\.]{1,510}/[a-zA-Z0-9-\.]{1,510}$";

/// The `SecurityGroup` shape's `@pattern` as the model declares it
/// (`smithy_query(query="shape",shape="SecurityGroup")`) — a second expression the model
/// anchors at BOTH ends, on a member that is not an ARN.
const SECURITY_GROUP_PATTERN: &str = "^sg-(([0-9a-z]{8})|([0-9a-z]{17}))$";

/// The `Subnet` shape's `@pattern` as the model declares it
/// (`smithy_query(query="shape",shape="Subnet")`): anchored at the START only, so there is
/// no whole-value PAIR to drop and the expression is quoted exactly as declared.
const SUBNET_PATTERN: &str = "^subnet-[a-z0-9]{8,17}";

/// `pattern` without the whole-value anchor PAIR the model may write around it: what the
/// requirement clause has to quote. Spelled out here rather than read from the production
/// helper, so a change to that rule fails a test instead of silently changing what this
/// suite accepts.
///
/// It states the rule for the forms THIS model declares. The production rule has one more
/// branch — a trailing `$` that is ESCAPED is a literal dollar and is kept, so the pair is
/// not dropped — which no `@pattern` of this model carries, so no row below reaches it; a
/// model that grows one states that branch here too.
fn as_advertised(pattern: &str) -> &str {
    match pattern.strip_prefix('^').map(|body| body.strip_suffix('$')) {
        Some(Some(inner)) => inner,
        _ => pattern,
    }
}

/// An ARN of the modelled form, so a row that is about ANOTHER member's pattern is not
/// also refused for this one.
const WELL_FORMED_ARN: &str =
    "arn:aws:license-manager-user-subscriptions:us-west-2:000000000000:identity-provider/ip-0001";

/// One request whose single constraint violation is a `@pattern`, and the expression the
/// refusal must quote.
struct PatternRow {
    case: &'static str,
    uri: &'static str,
    body: fn() -> Value,
    /// The violation's member path, as a clause spells it (lowerCamel, dotted).
    member_path: &'static str,
    /// The value the clause echoes back.
    value: &'static str,
    /// The expression the member's `@pattern` declares, anchors and all.
    declared: &'static str,
}

/// The rows differ along which operation is called, which member carries the pattern, which
/// SHAPE's expression that is, how the value fails it, and whether the model anchors the
/// expression at both ends or only at one — the rule is about the rendering of the
/// expression, so it must hold across all of them.
const PATTERN_ROWS: &[PatternRow] = &[
    PatternRow {
        case: "DeregisterIdentityProvider: an ARN with one path segment too many",
        uri: "/identity-provider/DeregisterIdentityProvider",
        body: || json!({ "IdentityProviderArn": "arn:aws:license-manager-user-subscriptions:us-west-2:000000000000:identity-provider/d-0123456789/VISUAL_STUDIO_PROFESSIONAL" }),
        member_path: "identityProviderArn",
        value: "arn:aws:license-manager-user-subscriptions:us-west-2:000000000000:identity-provider/d-0123456789/VISUAL_STUDIO_PROFESSIONAL",
        declared: ARN_PATTERN,
    },
    PatternRow {
        case: "DeregisterIdentityProvider: a value that is not an ARN at all",
        uri: "/identity-provider/DeregisterIdentityProvider",
        body: || json!({ "IdentityProviderArn": "not-an-arn" }),
        member_path: "identityProviderArn",
        value: "not-an-arn",
        declared: ARN_PATTERN,
    },
    PatternRow {
        case: "DeregisterIdentityProvider: an ARN with no resource path",
        uri: "/identity-provider/DeregisterIdentityProvider",
        body: || json!({ "IdentityProviderArn": "arn:aws:license-manager-user-subscriptions:us-west-2:000000000000:identity-provider" }),
        member_path: "identityProviderArn",
        value: "arn:aws:license-manager-user-subscriptions:us-west-2:000000000000:identity-provider",
        declared: ARN_PATTERN,
    },
    PatternRow {
        case: "StopProductSubscription: a ProductUserArn outside the pattern",
        uri: "/user/StopProductSubscription",
        body: || json!({ "ProductUserArn": "arn:aws:iam::000000000000:user" }),
        member_path: "productUserArn",
        value: "arn:aws:iam::000000000000:user",
        declared: ARN_PATTERN,
    },
    PatternRow {
        case: "DeleteLicenseServerEndpoint: a LicenseServerEndpointArn outside the pattern",
        uri: "/license-server/DeleteLicenseServerEndpoint",
        body: || json!({ "LicenseServerEndpointArn": "license-server-endpoint/lse-0001", "ServerType": "RDS_SAL" }),
        member_path: "licenseServerEndpointArn",
        value: "license-server-endpoint/lse-0001",
        declared: ARN_PATTERN,
    },
    PatternRow {
        case: "CreateLicenseServerEndpoint: an IdentityProviderArn outside the pattern",
        uri: "/license-server/CreateLicenseServerEndpoint",
        body: || {
            json!({
                "IdentityProviderArn": "arn:aws:license-manager-user-subscriptions:us-west-2:000000000000:identity-provider",
                "LicenseServerSettings": {
                    "ServerType": "RDS_SAL",
                    "ServerSettings": {
                        "RdsSalSettings": {
                            "RdsSalCredentialsProvider": {
                                "SecretsManagerCredentialsProvider": { "SecretId": "rds/sal" },
                            },
                        },
                    },
                },
            })
        },
        member_path: "identityProviderArn",
        value: "arn:aws:license-manager-user-subscriptions:us-west-2:000000000000:identity-provider",
        declared: ARN_PATTERN,
    },
    PatternRow {
        case: "UpdateIdentityProviderSettings: a SecurityGroupId outside its own pattern",
        uri: "/identity-provider/UpdateIdentityProviderSettings",
        body: || {
            json!({
                "IdentityProviderArn": WELL_FORMED_ARN,
                "UpdateSettings": {
                    "AddSubnets": [],
                    "RemoveSubnets": [],
                    "SecurityGroupId": "sg-NOTAGROUP",
                },
            })
        },
        member_path: "updateSettings.securityGroupId",
        value: "sg-NOTAGROUP",
        declared: SECURITY_GROUP_PATTERN,
    },
    PatternRow {
        case: "UpdateIdentityProviderSettings: a subnet element, whose pattern anchors one end",
        uri: "/identity-provider/UpdateIdentityProviderSettings",
        body: || {
            json!({
                "IdentityProviderArn": WELL_FORMED_ARN,
                "UpdateSettings": {
                    "AddSubnets": ["subnet-NOTASUBNET"],
                    "RemoveSubnets": [],
                },
            })
        },
        member_path: "updateSettings.addSubnets.1.member",
        value: "subnet-NOTASUBNET",
        declared: SUBNET_PATTERN,
    },
];

/// RULE: a constraint-violation message quotes a `@pattern` as the service advertises it —
/// the expression without the whole-value `^` / `$` PAIR the model wraps a total match in,
/// and the model's own text where there is no such pair.
#[tokio::test]
async fn a_pattern_requirement_is_quoted_without_the_models_whole_value_anchors() {
    let h = harness();
    for row in PATTERN_ROWS {
        let reply = call(&h, row.uri, &(row.body)()).await;
        let expected = format!(
            "1 validation error detected: Value '{}' at '{}' failed to satisfy constraint: \
             Member must satisfy regular expression pattern: {}",
            row.value,
            row.member_path,
            as_advertised(row.declared)
        );
        assert_refusal_says(&reply, &expected, row.case);
        // And the anchored form the model declares is not what travelled: the two differ
        // exactly when the model wrote the pair.
        let message = reply.message().unwrap_or_default();
        assert!(
            !message.ends_with(&format!("pattern: {}", row.declared))
                || as_advertised(row.declared) == row.declared,
            "case {:?}: the quoted expression must not carry the model's whole-value \
             anchors, got {message:?}",
            row.case
        );
    }
}

// ---------------------------------------------------------------------------
// Which prerequisite a registration is refused for
// ---------------------------------------------------------------------------

/// A directory id of the modelled `^(d|sd)-[0-9a-f]{10}$` form that the account's fleet
/// does not hold at all.
const UNHELD_DIRECTORY: &str = "d-00dead00ff";

/// A directory id the account's fleet holds in a stage a registration cannot be built on.
const INACTIVE_DIRECTORY: &str = "d-00beef00aa";

/// A stage that is not `Active`: the directory exists and still cannot be registered for.
/// One of the stages Directory Service reports while a directory is being created.
const INACTIVE_STAGE: &str = "Requested";

/// Put [`INACTIVE_DIRECTORY`] in the account's fleet in a non-`Active` stage, through the
/// same state seam the shared harness seeds the fleet with.
fn seed_inactive_directory(h: &Harness) {
    let store = h
        .engine
        .state::<ComAmazonawsLicensemanagerusersubscriptionsLicensemanagerusersubscriptionsState>();
    let mut store = store.lock();
    store.register_account_directory(INACTIVE_DIRECTORY, INACTIVE_STAGE);
}

/// What a registration row must be told.
#[derive(Debug, Clone, Copy)]
enum Blocker {
    /// The directory the request names, which is the prerequisite reported first.
    Directory(&'static str),
    /// The security group `Settings` names, reported when the directory does resolve.
    SecurityGroup(&'static str),
}

impl Blocker {
    /// The sentence the refusal has to carry, built from the row's own id: the fixed text
    /// of the error class with the one value the request supplied.
    fn message(self) -> String {
        match self {
            Blocker::Directory(directory_id) => {
                format!("{directory_id} directory either cannot be found or is not in active state")
            }
            Blocker::SecurityGroup(group_id) => {
                format!("The security group '{group_id}' does not exist")
            }
        }
    }
}

/// One registration, and the prerequisite it must be refused for. The rows differ along
/// WHY the directory cannot be used (absent, or present in another stage), which network
/// resource is also unusable (security group, subnet, both), and whether the request
/// carries `Settings` at all — plus the control rows where the directory does resolve.
struct PrerequisiteRow {
    case: &'static str,
    directory_id: &'static str,
    /// `None` leaves `Settings` out of the request.
    settings: Option<(&'static [&'static str], &'static str)>,
    expected: Blocker,
}

const PREREQUISITE_ROWS: &[PrerequisiteRow] = &[
    PrerequisiteRow {
        case: "no such directory, and a security group the account does not have",
        directory_id: UNHELD_DIRECTORY,
        settings: Some((&[ACCOUNT_SUBNETS[0]], UNHELD_SECURITY_GROUP)),
        expected: Blocker::Directory(UNHELD_DIRECTORY),
    },
    PrerequisiteRow {
        case: "no such directory, and a subnet the account does not have",
        directory_id: UNHELD_DIRECTORY,
        settings: Some((&[UNHELD_SUBNET], ACCOUNT_SECURITY_GROUPS[0])),
        expected: Blocker::Directory(UNHELD_DIRECTORY),
    },
    PrerequisiteRow {
        case: "no such directory, and neither network resource exists",
        directory_id: UNHELD_DIRECTORY,
        settings: Some((&[UNHELD_SUBNET], UNHELD_SECURITY_GROUP)),
        expected: Blocker::Directory(UNHELD_DIRECTORY),
    },
    PrerequisiteRow {
        case: "no such directory, with settings the account can serve",
        directory_id: UNHELD_DIRECTORY,
        settings: Some((&[ACCOUNT_SUBNETS[0]], ACCOUNT_SECURITY_GROUPS[0])),
        expected: Blocker::Directory(UNHELD_DIRECTORY),
    },
    PrerequisiteRow {
        case: "no such directory, and no settings at all",
        directory_id: UNHELD_DIRECTORY,
        settings: None,
        expected: Blocker::Directory(UNHELD_DIRECTORY),
    },
    PrerequisiteRow {
        case: "a directory in another stage, and a security group that does not exist",
        directory_id: INACTIVE_DIRECTORY,
        settings: Some((&[ACCOUNT_SUBNETS[1]], UNHELD_SECURITY_GROUP)),
        expected: Blocker::Directory(INACTIVE_DIRECTORY),
    },
    PrerequisiteRow {
        case: "a directory in another stage, and a subnet that does not exist",
        directory_id: INACTIVE_DIRECTORY,
        settings: Some((&[UNHELD_SUBNET], ACCOUNT_SECURITY_GROUPS[0])),
        expected: Blocker::Directory(INACTIVE_DIRECTORY),
    },
    // The control rows: with the directory resolved, the network resources are still
    // resolved and still refused — reordering the checks did not drop one.
    PrerequisiteRow {
        case: "an active directory, and a security group that does not exist",
        directory_id: DIRECTORY,
        settings: Some((&[ACCOUNT_SUBNETS[0]], UNHELD_SECURITY_GROUP)),
        expected: Blocker::SecurityGroup(UNHELD_SECURITY_GROUP),
    },
    PrerequisiteRow {
        case: "an active directory, and another security group that does not exist",
        directory_id: DIRECTORY,
        settings: Some((&[ACCOUNT_SUBNETS[2]], "sg-11111111")),
        expected: Blocker::SecurityGroup("sg-11111111"),
    },
];

/// RULE: the directory an identity provider is registered for is resolved BEFORE the
/// network resources its `Settings` name, so a request that gets both wrong is refused for
/// the directory — and a refused registration persists nothing.
#[tokio::test]
async fn the_directory_prerequisite_is_reported_before_the_settings_network() {
    for row in PREREQUISITE_ROWS {
        // A fresh emulator per row, so what either store holds is exactly what this row
        // put there.
        let h = harness();
        seed_inactive_directory(&h);

        let mut body = json!({
            "IdentityProvider": provider_union(row.directory_id),
            "Product": PRODUCTS[0],
        });
        if let Some((subnets, security_group_id)) = row.settings {
            body["Settings"] = json!({
                "Subnets": subnets,
                "SecurityGroupId": security_group_id,
            });
        }
        let reply = register(&h, &body).await;
        assert_refusal_says(&reply, &row.expected.message(), row.case);

        // Nothing was minted: the refusal came before the record was stored.
        let listed = list_providers(&h, &json!({})).await;
        let listed = listed.ok(row.case);
        assert_eq!(
            listed["IdentityProviderSummaries"].as_array().map(Vec::len),
            Some(0),
            "case {:?}: a refused registration must persist nothing, got {listed:?}",
            row.case
        );
    }
}

/// RULE: the ordering is about which prerequisite is REPORTED, not about dropping one — a
/// registration whose directory and network both resolve is still accepted and still
/// round-trips through `ListIdentityProviders`.
#[tokio::test]
async fn a_registration_whose_prerequisites_all_resolve_is_accepted() {
    let h = harness();
    let settings = json!({
        "Subnets": [ACCOUNT_SUBNETS[0], ACCOUNT_SUBNETS[1]],
        "SecurityGroupId": ACCOUNT_SECURITY_GROUPS[0],
    });
    let reply = register(
        &h,
        &json!({
            "IdentityProvider": provider_union(DIRECTORY),
            "Product": PRODUCTS[1],
            "Settings": settings,
        }),
    )
    .await;
    let summary = &reply.ok("every prerequisite resolves")["IdentityProviderSummary"];
    assert_eq!(
        summary.get("Settings"),
        Some(&settings),
        "the accepted registration must carry the settings that resolved ({settings}), got \
         {summary:?}"
    );

    let listed = list_providers(&h, &json!({})).await;
    let body = listed.ok("every prerequisite resolves");
    let summaries = body["IdentityProviderSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("no summaries in {body:?}"));
    assert_eq!(
        summaries.len(),
        1,
        "the accepted registration must be the one reported, got {summaries:?}"
    );
    assert_eq!(
        summaries[0].get("Settings"),
        Some(&settings),
        "a later read must report the stored settings ({settings}), got {:?}",
        summaries[0]
    );
}
